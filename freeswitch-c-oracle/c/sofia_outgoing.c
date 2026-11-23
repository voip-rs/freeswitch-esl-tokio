/* sofia_outgoing_channel from mod_sofia.c, from its destination check through attaching
   the private object to the profile, which names the channel; the session and channel are stubs. */
//@ function src/mod/endpoints/mod_sofia/sofia_glue.c sofia_glue_str2transport
//@ function src/mod/endpoints/mod_sofia/sofia_glue.c sofia_glue_strip_proto
//@ function src/switch_utils.c switch_url_decode
//@ function src/switch_utils.c switch_split_user_domain

typedef struct private_object {
	sofia_transport_t transport;
	int cid_type;
	char *gateway_name;
	char *gateway_from_str;
	char *dest;
	char *e_dest;
	char *dest_to;
	char *invite_contact;
	char *local_url;
	struct {
		char *remote_ip;
	} mparams;
	sofia_profile_t *profile;
	switch_channel_t *channel;
} private_object_t;

/* switch_snprintf is fspr_vsnprintf, which truncates and terminates as snprintf does. */
#define switch_snprintf snprintf

//@ function src/mod/endpoints/mod_sofia/sofia_glue.c sofia_glue_set_name

/* Of sofia_glue_attach_private, only what names the channel. */
static void sofia_glue_attach_private(switch_core_session_t *session, sofia_profile_t *profile, private_object_t *tech_pvt,
									  const char *channame)
{
	tech_pvt->profile = profile;
	tech_pvt->channel = switch_core_session_get_channel(session);
	if (channame) {
		sofia_glue_set_name(tech_pvt, channame);
	}
}

/* Set by the harness: a registration answers a contact on 192.0.2.2, else nothing. */
static _Thread_local int oracle_registered;

/* Resolution answers nothing, so the rewrite it guards never runs. */
static struct hostent *oracle_gethostbyname(const char *name)
{
	oracle_record(ORACLE_HOST, name, NULL);
	return NULL;
}
#define gethostbyname(name) oracle_gethostbyname(name)
#define switch_inet_ntop(af, src, dst, size) ((void) (src), (const char *) NULL)
#define switch_string_replace(string, search, replace) (abort(), (char *) NULL)
#define switch_core_session_set_ice(session) ((void) 0)
//@ define src/include/switch_utils.h switch_str_nil

static const char *sofia_reg_find_reg_url(sofia_profile_t *profile, const char *user, const char *host, char *val, switch_size_t len)
{
	(void) profile;
	oracle_record(ORACLE_REGISTRATION, user, host);
	if (!oracle_registered) {
		return NULL;
	}
	snprintf(val, len, "sip:%s@192.0.2.2", user);
	return val;
}

//@ export oracle_sofia_outgoing_channel (destination: *const c_char, headers: *const *const c_char, profiles: *const *const c_char, gateways: *const *const c_char, registered: c_int, emit: Emit, ctx: *mut c_void)
void oracle_sofia_outgoing_channel(const char *destination, const char *const *headers, const char *const *profiles,
								   const char *const *gateways, int registered, oracle_emit_fn emit, void *ctx)
{
	switch_call_cause_t cause = SWITCH_CAUSE_DESTINATION_OUT_OF_ORDER;
	switch_core_session_t *session = NULL;
	switch_core_session_t *nsession = &oracle_session;
	switch_memory_pool_t pool = { { 0 }, 0 };
	switch_caller_profile_t outbound = { 0 };
	switch_caller_profile_t *outbound_profile = &outbound;
	switch_event_t event = { 0 };
	switch_event_t *var_event = &event;
	private_object_t pvt = { 0 };
	private_object_t *tech_pvt = &pvt;
	char *data = NULL, *profile_name = NULL, *dest = NULL;
	sofia_profile_t *profile = NULL;
	switch_channel_t *nchannel = &oracle_channel;
	char *host = NULL, *dest_to = NULL;
	const char *hval = NULL;
	int cid_locked = 0;
	switch_channel_t *o_channel = NULL;
	sofia_gateway_t *gateway_ptr = NULL;
	int mod = 0;
	char *copy = strdup(destination);
	char number[16];

	oracle_begin(emit, ctx);
	oracle_headers = headers;
	oracle_profiles = profiles;
	oracle_gateways = gateways;
	oracle_registered = registered;
	outbound.pool = &pool;
	outbound.destination_number = copy;

	//@ block src/mod/endpoints/mod_sofia/mod_sofia.c sofia_outgoing_channel if (!outbound_profile || zstr(outbound_profile->destination_number)) {

	//@ block src/mod/endpoints/mod_sofia/mod_sofia.c sofia_outgoing_channel if (!switch_true(switch_event_get_header(var_event, "sofia_suppress_url_encoding"))) {

	//@ block src/mod/endpoints/mod_sofia/mod_sofia.c sofia_outgoing_channel data = switch_core_session_strdup(nsession, outbound_profile->destination_number);
	//@ block src/mod/endpoints/mod_sofia/mod_sofia.c sofia_outgoing_channel if ((dest_to = strchr(data, '^'))) {
	//@ block src/mod/endpoints/mod_sofia/mod_sofia.c sofia_outgoing_channel profile_name = data;

	//@ block src/mod/endpoints/mod_sofia/mod_sofia.c sofia_outgoing_channel if ((hval = switch_event_get_header(var_event, "sip_invite_to_uri"))) {

	//@ block src/mod/endpoints/mod_sofia/mod_sofia.c sofia_outgoing_channel if (!strncasecmp(profile_name, "gateway/", 8)) {

	//@ block src/mod/endpoints/mod_sofia/mod_sofia.c sofia_outgoing_channel switch_channel_set_variable_printf(nchannel, "sip_local_network_addr", "%s", profile->extsipip ? profile->extsipip : profile->sipip);
	//@ block src/mod/endpoints/mod_sofia/mod_sofia.c sofia_outgoing_channel switch_channel_set_variable(nchannel, "sip_profile_name", profile_name);

	//@ block src/mod/endpoints/mod_sofia/mod_sofia.c sofia_outgoing_channel if (switch_stristr("fs_path", tech_pvt->dest)) {

	//@ block src/mod/endpoints/mod_sofia/mod_sofia.c sofia_outgoing_channel if (zstr(tech_pvt->mparams.remote_ip)) {

	//@ block src/mod/endpoints/mod_sofia/mod_sofia.c sofia_outgoing_channel if (dest_to) {

	//@ block src/mod/endpoints/mod_sofia/mod_sofia.c sofia_outgoing_channel if (!tech_pvt->dest_to) {

	//@ block src/mod/endpoints/mod_sofia/mod_sofia.c sofia_outgoing_channel if (!zstr(tech_pvt->dest) && switch_stristr("transport=ws", tech_pvt->dest)) {

	//@ block src/mod/endpoints/mod_sofia/mod_sofia.c sofia_outgoing_channel sofia_glue_attach_private(nsession, profile, tech_pvt, dest);

	goto done;

  error:
	snprintf(number, sizeof(number), "%d", (int) cause);
	oracle_record(ORACLE_CAUSE, number, NULL);

  done:
	snprintf(number, sizeof(number), "%d", (int) tech_pvt->transport);
	oracle_field("destination_number", outbound_profile->destination_number);
	oracle_field("transport", number);
	oracle_field("gateway_name", tech_pvt->gateway_name);
	oracle_field("gateway_from_str", tech_pvt->gateway_from_str);
	oracle_field("dest", tech_pvt->dest);
	oracle_field("e_dest", tech_pvt->e_dest);
	oracle_field("dest_to", tech_pvt->dest_to);
	oracle_field("invite_contact", tech_pvt->invite_contact);
	oracle_field("local_url", tech_pvt->local_url);
	oracle_field("remote_ip", tech_pvt->mparams.remote_ip);
	oracle_headers = NULL;
	oracle_profiles = NULL;
	oracle_gateways = NULL;
	free(copy);
	oracle_pool_release(&pool);
	oracle_session_release();
}
