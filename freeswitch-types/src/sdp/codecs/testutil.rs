//! Fixtures the three section-parsing test modules share.

use crate::sdp::{SdpCodec, SdpCodecEntry};

/// The session lines every fixture SDP needs before its first `m=`.
pub(super) fn sdp_header() -> String {
    "v=0\r\no=- 0 0 IN IP4 192.0.2.1\r\ns=-\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\n".to_string()
}

/// The RTP codecs among a section's entries, dropping T.38.
pub(super) fn rtp_codec<'a>(
    entries: impl IntoIterator<Item = &'a SdpCodecEntry>,
) -> Vec<&'a SdpCodec> {
    entries
        .into_iter()
        .filter_map(|e| {
            if let SdpCodecEntry::Rtp(c) = e {
                Some(c)
            } else {
                None
            }
        })
        .collect()
}

/// The first codec of that encoding name, compared as FreeSWITCH compares one.
pub(super) fn codec_named<'a>(entries: &[&'a SdpCodec], name: &str) -> Option<&'a SdpCodec> {
    entries
        .iter()
        .find(|c| {
            c.name()
                .eq_ignore_ascii_case(name)
        })
        .copied()
}
