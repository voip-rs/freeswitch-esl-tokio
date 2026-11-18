//! sofia-sip's `sdp_parse`, compiled from the commit `hooks/source-refs.yaml` pins under
//! `sofia_sip` and called as `switch_core_media.c` calls it: flags 0, the length taken with
//! `strlen`, so the parse reads its input up to the first NUL.
//!
//! The report is the parsed session as sofia leaves it, not as the switch then reads it: an
//! `a=rtpmap`, `a=fmtp` or direction attribute sofia consumed is on the media's rtpmaps or mode
//! and absent from its attributes.

use std::ffi::c_int;
use std::io::Write as _;

use proptest::prelude::ProptestConfig;
use proptest::strategy::Strategy;
use proptest::test_runner::{TestCaseResult, TestRunner};

use super::{buffer, config, number, recorded, text, Record};

include!(concat!(env!("OUT_DIR"), "/sdp.rs"));

#[cfg(c_oracle_sdp)]
extern "C" {
    fn oracle_sdp_parse(
        sdp: *const std::ffi::c_char,
        emit: super::Emit,
        ctx: *mut std::ffi::c_void,
    );
}

/// The compiled `sdp_parse`, or why the build found no source for it.
pub fn sofia_sdp() -> Result<SofiaSdp, &'static str> {
    SOFIA_SDP.map(|()| SofiaSdp(()))
}

/// Run `property` over `strategy` against sofia's `sdp_parse`, seeds kept beside `source`, or
/// write one skip line past the harness's output capture when it was not built.
///
/// # Panics
///
/// When the property fails, with its shrunk case.
pub fn against_sofia<S: Strategy>(
    source: &'static str,
    name: &str,
    strategy: S,
    property: impl Fn(SofiaSdp, S::Value) -> TestCaseResult,
) {
    let sofia = match sofia_sdp() {
        Ok(sofia) => sofia,
        Err(missing) => {
            writeln!(std::io::stderr(), "{name}: skipped, {missing}")
                .unwrap_or_else(|e| panic!("{name}: writing the skip line: {e}"));
            return;
        }
    };
    let config = ProptestConfig {
        source_file: Some(source),
        ..config()
    };
    if let Err(failure) = TestRunner::new(config).run(&strategy, |value| property(sofia, value)) {
        panic!("{name}: {failure}");
    }
}

/// sofia-sip's SDP parser.
#[derive(Debug, Clone, Copy)]
pub struct SofiaSdp(());

/// What `sdp_parse` made of an SDP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdpParse {
    /// `sdp_session` returned NULL; the message is `sdp_parsing_error`'s.
    Refused(Vec<u8>),
    /// The session parsed.
    Parsed(SdpSession),
}

/// An `sdp_session_t`, reduced to what codec extraction reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SdpSession {
    /// `sdp_attributes`, in order.
    pub attributes: Vec<SdpAttribute>,
    /// `sdp_media`, in order.
    pub media: Vec<SdpMedia>,
}

/// An `sdp_attribute_t`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdpAttribute {
    /// `a_name`.
    pub name: Vec<u8>,
    /// `a_value`, `None` for an attribute with no value.
    pub value: Option<Vec<u8>>,
}

/// An `sdp_media_t`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SdpMedia {
    /// `m_type_name`, canonical lowercase for a type sofia knows.
    pub type_name: Vec<u8>,
    /// `m_proto_name`, canonical for a proto sofia knows.
    pub proto_name: Vec<u8>,
    /// `m_port`.
    pub port: u64,
    /// `m_mode`: bit 1 send, bit 2 receive, as `sdp_mode_t`.
    pub mode: u8,
    /// `m_rejected`, set for port 0.
    pub rejected: bool,
    /// `m_format`, the format list of a section that is not RTP.
    pub formats: Vec<Vec<u8>>,
    /// `m_rtpmaps`, one per RTP payload type of the format list, in its order.
    pub rtpmaps: Vec<SdpRtpmap>,
    /// `m_attributes` sofia did not consume, in order.
    pub attributes: Vec<SdpAttribute>,
}

/// An `sdp_rtpmap_t`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SdpRtpmap {
    /// `rm_pt`.
    pub pt: u8,
    /// `rm_encoding`, empty for a dynamic payload type no `a=rtpmap` named.
    pub encoding: Vec<u8>,
    /// `rm_rate`.
    pub rate: u64,
    /// `rm_params`.
    pub params: Option<Vec<u8>>,
    /// `rm_fmtp`.
    pub fmtp: Option<Vec<u8>>,
    /// `rm_predef`: no `a=rtpmap` named it.
    pub predef: bool,
}

impl SofiaSdp {
    /// `sdp_parse(NULL, sdp, strlen(sdp), 0)`, then `sdp_session`.
    pub fn parse(self, sdp: &[u8]) -> SdpParse {
        let input = buffer(sdp);
        let records = recorded(|emit, ctx| {
            #[cfg(c_oracle_sdp)]
            // SAFETY: the buffer is NUL-terminated and outlives the call, which reports through
            // `emit` only while it runs.
            unsafe {
                oracle_sdp_parse(
                    input
                        .as_ptr()
                        .cast(),
                    emit,
                    ctx,
                )
            };
            #[cfg(not(c_oracle_sdp))]
            let _ = (&input, emit, ctx);
        });
        report(records)
    }
}

fn report(records: Vec<Record>) -> SdpParse {
    let mut session = SdpSession::default();
    for (tag, a, b) in records {
        let media = session
            .media
            .last_mut();
        match (tag, media) {
            (SDP_ERROR, _) => return SdpParse::Refused(text(a)),
            (SDP_SESSION_ATTR, _) => session
                .attributes
                .push(SdpAttribute {
                    name: text(a),
                    value: b,
                }),
            (SDP_MEDIA, _) => session
                .media
                .push(SdpMedia {
                    type_name: text(a),
                    proto_name: text(b),
                    ..SdpMedia::default()
                }),
            (SDP_PORT, Some(m)) => m.port = wide(a),
            (SDP_MODE, Some(m)) => m.mode = narrow(a),
            (SDP_REJECTED, Some(m)) => m.rejected = number(a) != 0,
            (SDP_FORMAT, Some(m)) => m
                .formats
                .push(text(a)),
            (SDP_RTPMAP, Some(m)) => m
                .rtpmaps
                .push(SdpRtpmap {
                    pt: narrow(a),
                    ..SdpRtpmap::default()
                }),
            (SDP_MEDIA_ATTR, Some(m)) => m
                .attributes
                .push(SdpAttribute {
                    name: text(a),
                    value: b,
                }),
            (tag, Some(m)) => {
                let rtpmap = m
                    .rtpmaps
                    .last_mut()
                    .expect("the harness reports an rtpmap's fields after the rtpmap");
                match tag {
                    SDP_ENCODING => rtpmap.encoding = text(a),
                    SDP_RATE => rtpmap.rate = wide(a),
                    SDP_PARAMS => rtpmap.params = a,
                    SDP_FMTP => rtpmap.fmtp = a,
                    SDP_PREDEF => rtpmap.predef = number(a) != 0,
                    _ => panic!("the harness reported unknown tag {tag}"),
                }
            }
            (tag, None) => panic!("the harness reported tag {tag} before any media"),
        }
    }
    SdpParse::Parsed(session)
}

fn wide(field: Option<Vec<u8>>) -> u64 {
    String::from_utf8(text(field))
        .ok()
        .and_then(|n| {
            n.parse()
                .ok()
        })
        .expect("the harness prints an unsigned long in decimal")
}

fn narrow(field: Option<Vec<u8>>) -> u8 {
    u8::try_from(wide(field)).expect("the harness prints a field that fits its bit width")
}
