/* The switch's types, reduced to what the extracted code touches. */
#include <netdb.h>
#include <netinet/in.h>
#include <setjmp.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/socket.h>
#define SWITCH_DECLARE(type) type
#define _In_opt_z_
#define _In_opt_
#define _In_
#define _Check_return_

/* A backslash ending a string steps over its terminator; a second NUL stops that read inside the
   copy, as every oracle buffer does, where the switch would read past its allocation. */
static char *oracle_strdup(const char *s)
{
	size_t len = strlen(s);
	char *copy = calloc(len + 2, 1);

	if (copy) {
		memcpy(copy, s, len);
	}
	return copy;
}
#undef strdup
#define strdup(s) oracle_strdup(s)

#define zstr(x) _zstr(x)
static void oracle_assert_failed(const char *expr);
#define switch_assert(expr) do { if (!(expr)) { oracle_assert_failed(#expr); } } while (0)
/* The last line the extracted code logged, its newline cut: what a harness reports as the reason a
   pass stopped, in the switch's own words. */
static _Thread_local char oracle_log_line[512];

static void oracle_log(const char *fmt, ...)
{
	va_list ap;
	size_t len;

	va_start(ap, fmt);
	vsnprintf(oracle_log_line, sizeof(oracle_log_line), fmt, ap);
	va_end(ap);
	len = strlen(oracle_log_line);
	if (len && oracle_log_line[len - 1] == '\n') {
		oracle_log_line[len - 1] = '\0';
	}
}
#define switch_log_printf(channel, level, ...) oracle_log(__VA_ARGS__)
/* A core variable lookup answers nothing, and no dial handle is passed. */
#define switch_core_get_variable(name) ((void) (name), (const char *) NULL)
#define switch_dial_handle_get_total(dh) ((void) (dh), 0)
#define switch_dial_handle_get_peers(dh, idx, names, max) ((void) (dh), 0)
#define switch_dial_handle_get_vars(dh, idx, vars, max) ((void) 0)
typedef int switch_bool_t;
#define SWITCH_FALSE 0
#define SWITCH_TRUE 1
typedef size_t switch_size_t;
typedef enum {
	SWITCH_STATUS_SUCCESS,
	SWITCH_STATUS_FALSE,
	SWITCH_STATUS_GENERR
} switch_status_t;

typedef struct switch_memory_pool {
	void *allocs[32];
	int count;
} switch_memory_pool_t;

static void *oracle_pool_alloc(switch_memory_pool_t *pool, size_t size)
{
	if (pool->count == sizeof(pool->allocs) / sizeof(pool->allocs[0])) {
		abort();
	}
	return pool->allocs[pool->count++] = calloc(1, size);
}

static char *oracle_pool_strdup(switch_memory_pool_t *pool, const char *s)
{
	return strcpy(oracle_pool_alloc(pool, strlen(s) + 1), s);
}

static void oracle_pool_release(switch_memory_pool_t *pool)
{
	while (pool->count) {
		free(pool->allocs[--pool->count]);
	}
}

/* The formatted string in a buffer the caller frees. */
static char *oracle_vformat(const char *fmt, va_list ap)
{
	va_list measure;
	int len;
	char *out;

	va_copy(measure, ap);
	len = vsnprintf(NULL, 0, fmt, measure);
	va_end(measure);
	if (len < 0 || !(out = calloc((size_t) len + 2, 1))) {
		abort();
	}
	vsnprintf(out, (size_t) len + 1, fmt, ap);
	return out;
}

#define switch_core_alloc(_pool, _mem) oracle_pool_alloc(_pool, _mem)
#define switch_core_strdup(_pool, _todup) oracle_pool_strdup(_pool, _todup)

static char *switch_core_sprintf(switch_memory_pool_t *pool, const char *fmt, ...)
{
	va_list ap;
	char *formatted, *kept;

	va_start(ap, fmt);
	formatted = oracle_vformat(fmt, ap);
	va_end(ap);
	kept = oracle_pool_strdup(pool, formatted);
	free(formatted);
	return kept;
}

typedef void (*oracle_emit_fn)(void *ctx, int tag, const char *a, const char *b);
static _Thread_local oracle_emit_fn oracle_emit;
static _Thread_local void *oracle_ctx;

static void oracle_begin(oracle_emit_fn emit, void *ctx)
{
	oracle_emit = emit;
	oracle_ctx = ctx;
}

static void oracle_record(int tag, const char *a, const char *b)
{
	oracle_emit(oracle_ctx, tag, a, b);
}

/* A harness that arms the jump reports a failed assertion and returns; any other aborts. */
static _Thread_local jmp_buf *oracle_assert_jump;

static void oracle_assert_failed(const char *expr)
{
	if (!oracle_assert_jump) {
		abort();
	}
	oracle_record(ORACLE_FAILURE, expr, NULL);
	longjmp(*oracle_assert_jump, 1);
}

/* `name` is one of the NULL-terminated `names`. */
static int oracle_known(const char *const *names, const char *name)
{
	for (; names && *names; names++) {
		if (!strcmp(*names, name)) {
			return 1;
		}
	}
	return 0;
}

/* An event stores what an extracted pass installs, and a harness reports it with
   oracle_report_headers; a destroyed event reports first. A header read answers from the
   NULL-terminated name and value pairs the harness sets. */
#define ORACLE_EVENT_HEADERS 1024
typedef struct switch_event {
	int flags;
	int count;
	char *names[ORACLE_EVENT_HEADERS];
	char *values[ORACLE_EVENT_HEADERS];
} switch_event_t;
#define EF_UNIQ_HEADERS 1
#define SWITCH_EVENT_CHANNEL_DATA 0
#define SWITCH_STACK_BOTTOM 0
static _Thread_local const char *const *oracle_headers;

/* The model of switch_event_base_add_header this oracle holds the port to: an empty value
   deletes the name, and under EF_UNIQ_HEADERS a set replaces it, both ignoring case. */
static void oracle_delete_header(switch_event_t *event, const char *name)
{
	int from, to = 0;

	for (from = 0; from < event->count; from++) {
		if (strcasecmp(event->names[from], name)) {
			event->names[to] = event->names[from];
			event->values[to++] = event->values[from];
		} else {
			free(event->names[from]);
			free(event->values[from]);
		}
	}
	event->count = to;
}

static void oracle_report_headers(const switch_event_t *event)
{
	int x;

	for (x = 0; x < event->count; x++) {
		oracle_record(ORACLE_PAIR, event->names[x], event->values[x]);
	}
}

static void oracle_clear_headers(switch_event_t *event)
{
	while (event->count) {
		event->count--;
		free(event->names[event->count]);
		free(event->values[event->count]);
	}
}

static switch_status_t switch_event_create_plain(switch_event_t **event, int id)
{
	*event = calloc(1, sizeof(**event));
	if (!*event) {
		return SWITCH_STATUS_FALSE;
	}
	if (id == SWITCH_EVENT_CHANNEL_DATA) {
		(*event)->flags |= EF_UNIQ_HEADERS;
	}
	return SWITCH_STATUS_SUCCESS;
}

static void switch_event_destroy(switch_event_t **event)
{
	oracle_report_headers(*event);
	oracle_clear_headers(*event);
	free(*event);
	*event = NULL;
}

static switch_status_t switch_event_add_header_string(switch_event_t *event, int stack, const char *name, const char *value)
{
	(void) stack;
	if (!value || !*value) {
		oracle_delete_header(event, name);
		return SWITCH_STATUS_SUCCESS;
	}
	if (event->flags & EF_UNIQ_HEADERS) {
		oracle_delete_header(event, name);
	}
	if (event->count == ORACLE_EVENT_HEADERS) {
		abort();
	}
	event->names[event->count] = strdup(name);
	event->values[event->count++] = strdup(value);
	return SWITCH_STATUS_SUCCESS;
}

static char *switch_event_get_header(switch_event_t *event, const char *name)
{
	const char *const *header;

	(void) event;
	oracle_record(ORACLE_HEADER, name, NULL);
	for (header = oracle_headers; header && *header; header += 2) {
		if (!strcasecmp(header[0], name)) {
			return (char *) header[1];
		}
	}
	return NULL;
}

static int switch_event_check_permission_list(switch_event_t *list, const char *name)
{
	(void) list;
	(void) name;
	return 1;
}

typedef struct switch_core_session {
	struct switch_channel *channel;
} switch_core_session_t;
typedef struct switch_channel {
	switch_core_session_t *session;
} switch_channel_t;
typedef struct switch_caller_extension {
	int unused;
} switch_caller_extension_t;
typedef struct switch_caller_profile {
	const char *dialplan;
	const char *rdnis;
	char *destination_number;
	const char *source;
	const char *context;
	switch_memory_pool_t *pool;
} switch_caller_profile_t;
//@ declaration src/include/switch_types.h switch_call_cause_t
#define SOF_NONE 0
#define SCF_API_EXPANSION 0

typedef struct switch_stream_handle switch_stream_handle_t;
struct switch_stream_handle {
	switch_status_t (*write_function)(switch_stream_handle_t *handle, const char *fmt, ...);
	void *data;
};
#define SWITCH_STANDARD_STREAM(s) memset(&s, 0, sizeof(s)); s.data = calloc(1, 1)

/* Variable and API lookups answer nothing, so a reference expands to an empty string. */
static const char *switch_channel_get_variable_dup(switch_channel_t *channel, const char *varname, switch_bool_t dup, int idx)
{
	(void) channel;
	(void) dup;
	(void) idx;
	oracle_record(ORACLE_LOOKUP, varname, NULL);
	return NULL;
}
#define switch_channel_get_variable(channel, varname) switch_channel_get_variable_dup(channel, varname, SWITCH_TRUE, -1)

static switch_status_t switch_channel_set_variable(switch_channel_t *channel, const char *varname, const char *value)
{
	(void) channel;
	oracle_record(ORACLE_VARIABLE, varname, value);
	return SWITCH_STATUS_SUCCESS;
}

static switch_status_t switch_channel_set_variable_printf(switch_channel_t *channel, const char *varname, const char *fmt, ...)
{
	va_list ap;
	char *value;

	(void) channel;
	va_start(ap, fmt);
	value = oracle_vformat(fmt, ap);
	va_end(ap);
	oracle_record(ORACLE_VARIABLE, varname, value);
	free(value);
	return SWITCH_STATUS_SUCCESS;
}

/* The core's default domain, reported at each call. */
static char *switch_core_get_domain(switch_bool_t dup)
{
	(void) dup;
	oracle_record(ORACLE_DOMAIN, ORACLE_DEFAULT_DOMAIN, NULL);
	return strdup(ORACLE_DEFAULT_DOMAIN);
}

static int switch_core_test_flag(int flag)
{
	(void) flag;
	return 1;
}

static switch_status_t switch_api_execute(const char *cmd, const char *arg, switch_core_session_t *session, switch_stream_handle_t *stream)
{
	(void) session;
	(void) stream;
	oracle_record(ORACLE_API, cmd, arg);
	return SWITCH_STATUS_FALSE;
}

static _Thread_local switch_core_session_t oracle_session;
static _Thread_local switch_channel_t oracle_channel;
static _Thread_local switch_caller_extension_t oracle_extension;
static _Thread_local char *oracle_session_strings[32];
static _Thread_local int oracle_session_string_count;

static switch_status_t switch_ivr_originate(switch_core_session_t *session, switch_core_session_t **bleg, switch_call_cause_t *cause,
										   const char *bridgeto, uint32_t timelimit_sec, void *table, const char *cid_name_override,
										   const char *cid_num_override, void *caller_profile_override, void *ovars, int flags,
										   void *cancel_cause, void *dh)
{
	char timeout[16];

	(void) session;
	(void) table;
	(void) caller_profile_override;
	(void) ovars;
	(void) flags;
	(void) cancel_cause;
	(void) dh;
	snprintf(timeout, sizeof(timeout), "%u", timelimit_sec);
	oracle_record(ORACLE_ORIGINATE, bridgeto, timeout);
	oracle_record(ORACLE_CALLER_ID, cid_name_override, cid_num_override);
	*cause = SWITCH_CAUSE_NORMAL_CLEARING;
	*bleg = &oracle_session;
	return SWITCH_STATUS_SUCCESS;
}

static switch_channel_t *switch_core_session_get_channel(switch_core_session_t *session)
{
	(void) session;
	return &oracle_channel;
}

static char *oracle_session_keep(char *s)
{
	if (oracle_session_string_count == sizeof(oracle_session_strings) / sizeof(oracle_session_strings[0])) {
		abort();
	}
	return oracle_session_strings[oracle_session_string_count++] = s;
}

static void oracle_session_release(void)
{
	while (oracle_session_string_count) {
		free(oracle_session_strings[--oracle_session_string_count]);
	}
}

static char *switch_core_session_strdup(switch_core_session_t *session, const char *todup)
{
	(void) session;
	return oracle_session_keep(strdup(todup));
}

static char *switch_core_session_sprintf(switch_core_session_t *session, const char *fmt, ...)
{
	va_list ap;
	char *formatted;

	(void) session;
	va_start(ap, fmt);
	formatted = oracle_vformat(fmt, ap);
	va_end(ap);
	return oracle_session_keep(formatted);
}

static switch_caller_extension_t *switch_caller_extension_new(switch_core_session_t *session, const char *name, const char *number)
{
	(void) session;
	(void) name;
	(void) number;
	return &oracle_extension;
}

static void switch_caller_extension_add_application(switch_core_session_t *session, switch_caller_extension_t *extension,
													 const char *application_name, const char *extra_data)
{
	(void) session;
	(void) extension;
	oracle_record(ORACLE_APPLICATION, application_name, extra_data);
}

static void switch_ivr_session_transfer(switch_core_session_t *session, const char *extension, const char *dialplan, const char *context)
{
	(void) session;
	oracle_record(ORACLE_TRANSFER, extension, dialplan);
	oracle_record(ORACLE_CONTEXT, context, NULL);
}

#define switch_channel_cause2str(cause) ((void) (cause), "CAUSE")
#define switch_channel_set_caller_extension(channel, extension) ((void) 0)
#define switch_channel_set_state(channel, state) ((void) 0)
#define switch_core_session_get_uuid(session) "uuid"
#define switch_core_session_rwunlock(session) ((void) 0)

/* A field of the object an extracted reader filled, `a` its name and `b` its value. */
static void oracle_field(const char *name, const char *value)
{
	oracle_record(ORACLE_FIELD, name, value);
}

/* The channel's name, reported as the field `name`. */
static switch_status_t switch_channel_set_name(switch_channel_t *channel, const char *name)
{
	(void) channel;
	oracle_field("name", name);
	return SWITCH_STATUS_SUCCESS;
}
