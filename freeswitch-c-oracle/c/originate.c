/* switch_event_create_brackets and the passes switch_ivr_originate and
   switch_ivr_enterprise_originate run over a dial string up to each leg's endpoint and the event
   that endpoint reads. Every statement
   of the switch is taken by directive; only the harness's loops, labels and records are its own. */
//@ define src/include/switch_types.h SWITCH_ENT_ORIGINATE_DELIM
//@ define src/switch_ivr_originate.c QUOTED_ESC_COMMA
//@ define src/switch_ivr_originate.c UNQUOTED_ESC_COMMA
//@ define src/switch_ivr_originate.c MAX_PEERS
//@ function src/switch_event.c switch_event_create_brackets

/* switch_event_create_brackets on a block opening data, as originate calls it: the offset after
   the block, or -1 where the call fails. */
//@ export oracle_brackets (data: *mut c_char, a: c_char, b: c_char, c: c_char, emit: Emit, ctx: *mut c_void) -> c_long
long oracle_brackets(char *data, char a, char b, char c, oracle_emit_fn emit, void *ctx)
{
	static _Thread_local switch_event_t event;
	switch_event_t *var_event = &event;
	char *parsed = NULL;
	long rest = -1;

	oracle_begin(emit, ctx);
	event.flags = EF_UNIQ_HEADERS;
	if (switch_event_create_brackets(data, a, b, c, &var_event, &parsed, SWITCH_FALSE) == SWITCH_STATUS_SUCCESS && parsed) {
		oracle_report_headers(var_event);
		rest = (long) (parsed - data);
	}
	oracle_clear_headers(&event);
	return rest;
}

/* switch_ivr_originate from its first space strip to each leg's endpoint, over a copy of bridgeto
   with no session, variables, dial handle or caller channel. */
static void oracle_originate(const char *bridgeto)
{
	switch_status_t status = SWITCH_STATUS_SUCCESS;
	switch_core_session_t *session = NULL;
	switch_channel_t *caller_channel = NULL;
	switch_event_t *ovars = NULL, *var_event = NULL, *local_var_event = NULL, *originate_var_event = NULL;
	void *dh = NULL;
	struct {
		switch_bool_t check_vars;
	} oglobals = { SWITCH_TRUE };
	char *pipe_names[MAX_PEERS] = { 0 };
	char *peer_names[MAX_PEERS] = { 0 };
	char *odata = strdup(bridgeto);
	char *data = odata;
	char *loop_data = NULL;
	char *chan_type = NULL;
	int or_argc = 0, and_argc = 0, r, i;

	(void) session;
	oracle_log_line[0] = '\0';
	switch_event_create_plain(&var_event, SWITCH_EVENT_CHANNEL_DATA);

	//@ block src/switch_ivr_originate.c switch_ivr_originate switch_channel_process_export(caller_channel, NULL, var_event, SWITCH_EXPORT_VARS_VARIABLE); => while (data && *data && *data == ' ') {

	//@ block src/switch_ivr_originate.c switch_ivr_originate if ((ovars && switch_true(switch_event_get_header(ovars,"origination_nested_vars"))) ||

	if (!oglobals.check_vars) {
		oracle_record(ORACLE_NESTED, NULL, NULL);
	}

	//@ block src/switch_ivr_originate.c switch_ivr_originate if (*data == '<') {

	//@ block src/switch_ivr_originate.c switch_ivr_originate while (*data == '{') {

	oracle_report_headers(var_event);

	//@ block src/switch_ivr_originate.c switch_ivr_originate while (*data == '{') { => while (data && *data && *data == ' ') {

	//@ block src/switch_ivr_originate.c switch_ivr_originate if (zstr(data) && !dh) {

	//@ block src/switch_ivr_originate.c switch_ivr_originate loop_data = strdup(data);

	//@ block src/switch_ivr_originate.c switch_ivr_originate loop_data = strdup(data); => if (dh) {

	//@ block src/switch_ivr_originate.c switch_ivr_originate if (or_argc <= 0) {

	for (r = 0; r < or_argc; ++r) {
		char *p, *end = NULL;
		int q = 0, alt = 0;

		//@ block src/switch_ivr_originate.c switch_ivr_originate p = pipe_names[r];

		//@ block src/switch_ivr_originate.c switch_ivr_originate while (p && *p) {

		//@ block src/switch_ivr_originate.c switch_ivr_originate and_argc = switch_separate_string(pipe_names[r], ',', peer_names, (sizeof(peer_names) / sizeof(peer_names[0])));
		oracle_record(ORACLE_GROUP, NULL, NULL);

		for (i = 0; i < and_argc; ++i) {
			end = NULL;
			oracle_record(ORACLE_LEG, NULL, NULL);

			//@ block src/switch_ivr_originate.c switch_ivr_originate if (!(chan_type = peer_names[i])) {

			//@ block src/switch_ivr_originate.c switch_ivr_originate if (!(chan_type = peer_names[i])) { => while (chan_type && *chan_type && *chan_type == ' ') {

			//@ block src/switch_ivr_originate.c switch_ivr_originate if (*chan_type == '[') {

			//@ block src/switch_ivr_originate.c switch_ivr_originate while (*chan_type == '[') {

			//@ block src/switch_ivr_originate.c switch_ivr_originate while (*chan_type == '[') { => while (chan_type && *chan_type && *chan_type == ' ') {

			oracle_record(ORACLE_ENDPOINT, chan_type, NULL);

			//@ block src/switch_ivr_originate.c switch_ivr_originate switch_event_dup(&originate_var_event, var_event);
			//@ block src/switch_ivr_originate.c switch_ivr_originate switch_event_dup(&originate_var_event, var_event); => if (local_var_event) {
			oracle_report_endpoint_headers(originate_var_event);
			oracle_clear_headers(originate_var_event);
			free(originate_var_event);
			originate_var_event = NULL;

			if (local_var_event) {
				switch_event_destroy(&local_var_event);
			}
		}
	}

  outer_for:
  done:
	if (status != SWITCH_STATUS_SUCCESS) {
		oracle_record(ORACLE_FAILURE, oracle_log_line, NULL);
	}
	if (local_var_event) {
		switch_event_destroy(&local_var_event);
	}
	oracle_clear_headers(var_event);
	free(var_event);
	free(loop_data);
	free(odata);
}

/* switch_ivr_enterprise_originate up to the thread split, each thread then read as
   switch_ivr_originate reads its bridgeto. */
static void oracle_enterprise_originate(const char *bridgeto)
{
	switch_status_t status = SWITCH_STATUS_FALSE;
	switch_core_session_t *session = NULL;
	switch_event_t *var_event = NULL;
	struct {
		int handle_idx;
	} *hl = NULL;
	switch_call_cause_t cause_value = SWITCH_CAUSE_NONE, *cause = &cause_value;
	int getcause = 1;
	char *x_argv[MAX_PEERS] = { 0 };
	char *odata = strdup(bridgeto);
	char *data = odata;
	int x_argc = 0, i;

	(void) session;
	(void) getcause;
	oracle_log_line[0] = '\0';
	switch_event_create_plain(&var_event, SWITCH_EVENT_CHANNEL_DATA);

	//@ block src/switch_ivr_originate.c switch_ivr_enterprise_originate /* strip leading spaces */ => while (data && *data && *data == ' ') {

	//@ block src/switch_ivr_originate.c switch_ivr_enterprise_originate while (data && *data == '<') {

	oracle_report_headers(var_event);

	//@ block src/switch_ivr_originate.c switch_ivr_enterprise_originate while (data && *data == '<') { => while (data && *data && *data == ' ') {

	//@ block src/switch_ivr_originate.c switch_ivr_enterprise_originate if (data) {

	for (i = 0; i < x_argc; ++i) {
		oracle_record(ORACLE_THREAD, NULL, NULL);
		oracle_originate(x_argv[i]);
	}
	status = SWITCH_STATUS_SUCCESS;

  end:
  done:
	if (status != SWITCH_STATUS_SUCCESS) {
		oracle_record(ORACLE_FAILURE, oracle_log_line, NULL);
	}
	oracle_clear_headers(var_event);
	free(var_event);
	free(odata);
}

//@ export oracle_dial (input: *const c_char, emit: Emit, ctx: *mut c_void)
void oracle_dial(const char *bridgeto, oracle_emit_fn emit, void *ctx)
{
	oracle_begin(emit, ctx);
	if (strstr(bridgeto, SWITCH_ENT_ORIGINATE_DELIM)) {
		oracle_enterprise_originate(bridgeto);
	} else {
		oracle_record(ORACLE_THREAD, NULL, NULL);
		oracle_originate(bridgeto);
	}
}
