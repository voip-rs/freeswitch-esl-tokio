/* sdp_parse called as switch_core_media.c calls it, with the session it parsed reported field by
   field through the callback. The SDP_* tags are defined by build.rs, numbered as in Rust. */
#include <stdio.h>
#include <string.h>
#include <sofia-sip/sdp.h>

typedef void (*oracle_emit)(void *ctx, int tag, const char *a, const char *b);

static void emit_number(oracle_emit emit, void *ctx, int tag, unsigned long value)
{
	char text[32];

	snprintf(text, sizeof(text), "%lu", value);
	emit(ctx, tag, text, NULL);
}

static void emit_attributes(oracle_emit emit, void *ctx, int tag, sdp_attribute_t const *a)
{
	for (; a; a = a->a_next)
		emit(ctx, tag, a->a_name, a->a_value);
}

void oracle_sdp_parse(const char *sdp, oracle_emit emit, void *ctx)
{
	sdp_parser_t *parser = sdp_parse(NULL, sdp, (int) strlen(sdp), 0);
	sdp_session_t *session = sdp_session(parser);

	if (!session) {
		emit(ctx, SDP_ERROR, sdp_parsing_error(parser), NULL);
		sdp_parser_free(parser);
		return;
	}
	emit_attributes(emit, ctx, SDP_SESSION_ATTR, session->sdp_attributes);
	for (sdp_media_t const *m = session->sdp_media; m; m = m->m_next) {
		emit(ctx, SDP_MEDIA, m->m_type_name, m->m_proto_name);
		emit_number(emit, ctx, SDP_PORT, m->m_port);
		emit_number(emit, ctx, SDP_MODE, m->m_mode);
		emit_number(emit, ctx, SDP_REJECTED, m->m_rejected);
		for (sdp_list_t const *l = m->m_format; l; l = l->l_next)
			emit(ctx, SDP_FORMAT, l->l_text, NULL);
		for (sdp_rtpmap_t const *rm = m->m_rtpmaps; rm; rm = rm->rm_next) {
			emit_number(emit, ctx, SDP_RTPMAP, rm->rm_pt);
			emit(ctx, SDP_ENCODING, rm->rm_encoding, NULL);
			emit_number(emit, ctx, SDP_RATE, rm->rm_rate);
			emit(ctx, SDP_PARAMS, rm->rm_params, NULL);
			emit(ctx, SDP_FMTP, rm->rm_fmtp, NULL);
			emit_number(emit, ctx, SDP_PREDEF, rm->rm_predef);
		}
		emit_attributes(emit, ctx, SDP_MEDIA_ATTR, m->m_attributes);
	}
	sdp_parser_free(parser);
}
