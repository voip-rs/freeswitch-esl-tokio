/* channel_outgoing_channel from mod_loopback.c, from the caller profile it clones through the
   channel name it sets, over an outbound profile with no context or dialplan. */
typedef struct loopback_private {
	unsigned int flags;
	switch_caller_profile_t *caller_profile;
} loopback_private_t;
#define TFLAG_APP (1 << 0)
#define TFLAG_OUTBOUND (1 << 1)
#define switch_set_flag(obj, flag) (obj)->flags |= (flag)
#define switch_set_flag_locked(obj, flag) (obj)->flags |= (flag)
#define switch_snprintf snprintf
#define switch_channel_set_caller_profile(channel, profile) ((void) 0)
#define switch_core_session_destroy(session) ((void) 0)
static const char modname[] = "mod_loopback";

static switch_caller_profile_t *switch_caller_profile_clone(switch_core_session_t *session, switch_caller_profile_t *tocopy)
{
	switch_caller_profile_t *profile = oracle_pool_alloc(tocopy->pool, sizeof(*profile));

	(void) session;
	*profile = *tocopy;
	profile->destination_number = oracle_pool_strdup(tocopy->pool, tocopy->destination_number);
	return profile;
}

static switch_call_cause_t oracle_loopback(switch_caller_profile_t *outbound_profile)
{
	switch_core_session_t *new_session_data = &oracle_session;
	switch_core_session_t **new_session = &new_session_data;
	switch_caller_profile_t *caller_profile = NULL;
	loopback_private_t pvt = { 0 };
	loopback_private_t *tech_pvt = &pvt;
	switch_channel_t *channel = &oracle_channel;
	switch_event_t *clone = NULL;
	char name[128];

	//@ block src/mod/endpoints/mod_loopback/mod_loopback.c channel_outgoing_channel if (outbound_profile) {

	oracle_field("destination_number", caller_profile->destination_number);
	oracle_field("context", caller_profile->context);
	oracle_field("dialplan", caller_profile->dialplan);
	oracle_field("app", (tech_pvt->flags & TFLAG_APP) ? "true" : "false");
	return SWITCH_CAUSE_SUCCESS;
}

//@ export oracle_loopback_outgoing_channel (input: *const c_char, emit: Emit, ctx: *mut c_void)
void oracle_loopback_outgoing_channel(const char *destination, oracle_emit_fn emit, void *ctx)
{
	switch_memory_pool_t pool = { { 0 }, 0 };
	switch_caller_profile_t outbound = { 0 };

	oracle_begin(emit, ctx);
	outbound.pool = &pool;
	outbound.destination_number = oracle_pool_strdup(&pool, destination);
	oracle_loopback(&outbound);
	oracle_pool_release(&pool);
	oracle_session_release();
}
