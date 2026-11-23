//! `a=rtpmap`/`a=fmtp`/`a=ptime`/direction attribute parsing.
//!
//! Attribute names match in any case, as `su_casematch` in sofia's `parse_media_attr` and
//! `strcasecmp` in the switch's own attribute walk (`switch_core_media.c:13658`) match them.
//!
//! Line numbers in this module index FreeSWITCH `v1.11.1`
//! (`c2c59645f6911a76589e5008c4d73349ded44b65`).

use std::collections::HashMap;

use crate::sdp::error::SdpWarning;
use crate::sdp::num::atoi_prefix;
use crate::sdp::session::{self, payload_type, rtpmap};
use crate::sdp::SdpDirection;

/// An `a=rtpmap` value past its payload type as `(name, clock_rate, channels)`, `None` where the
/// switch keeps no name for it: `parse_rtpmap` refused the offer or stored no encoding.
///
/// The name is `parse_rtpmap`'s token run, so a byte outside sofia's `TOKEN` set ends it. The
/// channels are the switch's `atoi` of `rm_params`.
fn rtpmap_entry(rest: &str) -> Option<(String, u32, Option<u32>)> {
    let rtpmap = rtpmap(rest)?;
    // No implementation runs above u32::MAX Hz, so a saturated rate matches nothing either.
    let rate = u32::try_from(rtpmap.rate).unwrap_or(u32::MAX);
    let channels = rtpmap
        .params
        .map(|params| {
            atoi_prefix(params.trim_start_matches([' ', '\t']))
                .0
                .unwrap_or(0)
        });
    Some((rtpmap.encoding?, rate, channels))
}

/// One section's `a=rtpmap` and `a=fmtp` values, keyed by payload type.
///
/// An `a=fmtp` value is what follows the payload type and the whitespace after it, verbatim:
/// trailing whitespace is opaque `byte-string` content per RFC 8866, as it is to `parse_fmtp`.
pub(super) struct AttrTables {
    pub(super) rtpmap: HashMap<u8, (String, u32, Option<u32>)>,
    pub(super) fmtp: HashMap<u8, String>,
}

/// Collect a section's rtpmap and fmtp attributes into payload-type lookup tables.
///
/// An attribute whose payload type is unreadable is dropped with a
/// [`SdpWarning::PayloadAttributeIgnored`], the rest of the section kept, as sofia drops it. An
/// `a=rtpmap` the switch keeps no name for is dropped too; where the switch refuses the offer
/// over it, the session layer reports that.
pub(super) fn attribute_tables(
    attributes: &[session::Attribute],
    warnings: &mut Vec<SdpWarning>,
) -> AttrTables {
    let mut tables = AttrTables {
        rtpmap: HashMap::new(),
        fmtp: HashMap::new(),
    };

    for attr in attributes {
        let name = attr
            .attribute
            .as_str();
        let fmtp = name.eq_ignore_ascii_case("fmtp");
        if !fmtp && !name.eq_ignore_ascii_case("rtpmap") {
            continue;
        }
        let value = attr
            .value
            .as_deref()
            .unwrap_or_default();
        let Some((pt, rest)) = payload_type(value) else {
            warnings.push(SdpWarning::PayloadAttributeIgnored {
                offset: attr.offset,
            });
            continue;
        };
        if fmtp {
            tables
                .fmtp
                .insert(pt, rest.to_string());
        } else if let Some(entry) = rtpmap_entry(rest) {
            tables
                .rtpmap
                .insert(pt, entry);
        }
    }

    tables
}

/// Return ptime from the first matching attribute, recording a warning if unparseable.
pub(super) fn ptime_from_attrs(
    attrs: &[session::Attribute],
    name: &str,
    warnings: &mut Vec<SdpWarning>,
) -> Option<u32> {
    attrs
        .iter()
        .find(|a| {
            a.attribute
                .eq_ignore_ascii_case(name)
        })
        .and_then(|a| {
            a.value
                .as_deref()
        })
        .and_then(|v| parse_ptime_value(v, warnings, name))
}

/// Parse a ptime/mode/interval string with FreeSWITCH-compatible tolerance.
///
/// Accepts an integer or a decimal with trailing fraction (`20.0` → 20).
/// Zero and junk values are treated as unset and a warning is recorded.
pub(super) fn parse_ptime_value(
    raw: &str,
    warnings: &mut Vec<SdpWarning>,
    attr: &str,
) -> Option<u32> {
    // Strip trailing fractional part — upstream uses atoi, which does the same.
    let int_str = raw
        .split_once('.')
        .map_or(raw, |(prefix, _)| prefix);
    match atoi_prefix(int_str) {
        (Some(n), "") if n != 0 => Some(n),
        _ => {
            warnings.push(SdpWarning::unparseable_numeric_attribute(attr, raw));
            None
        }
    }
}

/// The direction the last direction attribute in the list sets, if any.
///
/// `parse_session_attr` and `parse_media_attr` match the name in any case, whatever value
/// follows it, and each one overwrites the mode the one before it set.
pub(super) fn direction_from_attrs(attrs: &[session::Attribute]) -> Option<SdpDirection> {
    const DIRECTIONS: [SdpDirection; 4] = [
        SdpDirection::SendRecv,
        SdpDirection::SendOnly,
        SdpDirection::RecvOnly,
        SdpDirection::Inactive,
    ];
    attrs
        .iter()
        .filter_map(|attr| {
            DIRECTIONS
                .into_iter()
                .find(|direction| {
                    direction
                        .to_string()
                        .eq_ignore_ascii_case(&attr.attribute)
                })
        })
        .next_back()
}

/// Extract a named parameter value from a semicolon-delimited fmtp string.
pub(super) fn fmtp_param<'a>(fmtp: &'a str, key: &str) -> Option<&'a str> {
    fmtp.split(';')
        .find_map(|part| {
            let part = part.trim();
            let (k, v) = part.split_once('=')?;
            if k.trim()
                .eq_ignore_ascii_case(key)
            {
                Some(v.trim())
            } else {
                None
            }
        })
}

#[cfg(test)]
mod tests {
    use super::super::testutil::{codec_named, rtp_codec, sdp_header};
    use crate::sdp::{SdpCodecs, SdpWarning};

    // --- RFC 3551 quirk ---

    #[test]
    fn g722_rtpmap_clock_rate_is_8000() {
        // RFC 3551 quirk: G.722 is advertised at 8000 Hz in SDP even though it
        // runs at 16 kHz. The clock rate from a=rtpmap must be preserved as-is.
        let sdp = format!(
            "{}m=audio 5004 RTP/AVP 9\r\na=rtpmap:9 G722/8000\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let rtp = rtp_codec(codecs.entries());
        let g722 = codec_named(&rtp, "G722").expect("G722 must be present");
        assert_eq!(g722.clock_rate(), 8000);
        assert!(g722.has_rtpmap());
    }

    // --- opus stereo ---

    #[test]
    fn opus_stereo_channels_2() {
        let sdp = format!(
            "{}m=audio 5004 RTP/AVP 111\r\na=rtpmap:111 opus/48000/2\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let rtp = rtp_codec(codecs.entries());
        let opus = codec_named(&rtp, "opus").expect("opus must be present");
        assert_eq!(opus.channels(), Some(2));
        assert_eq!(opus.clock_rate(), 48000);
    }

    // --- media-level vs session-level ptime ---

    #[test]
    fn media_ptime_overrides_session_ptime() {
        let sdp = format!(
            "{}a=ptime:10\r\nm=audio 5004 RTP/AVP 0\r\na=ptime:30\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let rtp = rtp_codec(codecs.entries());
        let pcmu = codec_named(&rtp, "PCMU").expect("PCMU must be present");
        assert_eq!(
            pcmu.ptime(),
            Some(30),
            "media-level a=ptime must override session-level"
        );
    }

    #[test]
    fn session_ptime_applies_when_media_has_none() {
        let sdp = format!("{}a=ptime:10\r\nm=audio 5004 RTP/AVP 8\r\n", sdp_header());
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let rtp = rtp_codec(codecs.entries());
        let pcma = codec_named(&rtp, "PCMA").expect("PCMA must be present");
        assert_eq!(
            pcma.ptime(),
            Some(10),
            "session-level a=ptime must apply when media has none"
        );
    }

    // --- ptime numeric tolerance ---

    #[test]
    fn ptime_fractional_value_is_truncated() {
        let sdp = format!("{}m=audio 5004 RTP/AVP 0\r\na=ptime:20.0\r\n", sdp_header());
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert!(codecs
            .warnings()
            .is_empty());
        let rtp = rtp_codec(codecs.entries());
        let pcmu = codec_named(&rtp, "PCMU").expect("PCMU must be present");
        assert_eq!(pcmu.ptime(), Some(20));
    }

    #[test]
    fn ptime_junk_is_unset_with_warning() {
        let sdp = format!("{}m=audio 5004 RTP/AVP 0\r\na=ptime:abc\r\n", sdp_header());
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(
            codecs
                .warnings()
                .len(),
            1,
            "junk ptime must produce exactly one warning"
        );
        let rtp = rtp_codec(codecs.entries());
        let pcmu = codec_named(&rtp, "PCMU").expect("PCMU must be present");
        // Falls back to the codec default (20 ms for PCMU)
        assert_eq!(pcmu.ptime(), Some(20));
    }

    #[test]
    fn ptime_zero_is_unset_with_warning() {
        let sdp = format!("{}m=audio 5004 RTP/AVP 0\r\na=ptime:0\r\n", sdp_header());
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(
            codecs
                .warnings()
                .len(),
            1,
            "ptime=0 must produce exactly one warning"
        );
        let rtp = rtp_codec(codecs.entries());
        let pcmu = codec_named(&rtp, "PCMU").expect("PCMU must be present");
        assert_eq!(pcmu.ptime(), Some(20));
    }

    // --- direction attributes ---

    #[test]
    fn canonical_direction_attribute_is_read() {
        let sdp = format!("{}m=audio 5004 RTP/AVP 0\r\na=recvonly\r\n", sdp_header());
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert!(codecs
            .warnings()
            .is_empty());
        assert_eq!(
            codecs.sections()[0].direction(),
            crate::sdp::SdpDirection::RecvOnly
        );
    }

    #[test]
    fn an_unrelated_value_less_attribute_is_not_a_direction() {
        let sdp = format!("{}m=audio 5004 RTP/AVP 0\r\na=rtcp-mux\r\n", sdp_header());
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert!(codecs
            .warnings()
            .is_empty());
    }

    // --- G7221 malformed bitrate records a warning ---

    #[test]
    fn g7221_malformed_bitrate_records_warning() {
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 5004 RTP/AVP 100\r\n",
                "a=rtpmap:100 G7221/16000\r\n",
                "a=fmtp:100 bitrate=abc\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(
            codecs
                .warnings()
                .len(),
            1,
            "malformed g7221 bitrate must produce exactly one warning"
        );
        let rtp = rtp_codec(codecs.entries());
        let g7221 = codec_named(&rtp, "G7221").expect("G7221 must be present");
        assert_eq!(
            g7221.bitrate(),
            None,
            "bitrate must be None after parse error"
        );
    }

    #[test]
    fn g7221_valid_bitrate_is_set() {
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 5004 RTP/AVP 100\r\n",
                "a=rtpmap:100 G7221/16000\r\n",
                "a=fmtp:100 bitrate=24000\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert!(codecs
            .warnings()
            .is_empty());
        let rtp = rtp_codec(codecs.entries());
        let g7221 = codec_named(&rtp, "G7221").expect("G7221 must be present");
        assert_eq!(g7221.bitrate(), Some(24000));
    }

    // --- rtpmap whitespace is a tokenizer concern, not a per-field trim ---

    #[test]
    fn rtpmap_double_space_before_name_is_parsed() {
        // Two spaces between the payload type and the encoding name must not leak
        // into the name (FreeSWITCH/sofia's parse_ul skips any run of SP/HTAB).
        let sdp = format!(
            "{}m=audio 5004 RTP/AVP 111\r\na=rtpmap:111  opus/48000/2\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let rtp = rtp_codec(codecs.entries());
        let opus = codec_named(&rtp, "opus").expect("opus must be present, not \" opus\"");
        assert_eq!(opus.clock_rate(), 48000);
        assert_eq!(opus.channels(), Some(2));
    }

    #[test]
    fn rtpmap_tab_separator_is_parsed() {
        // A HTAB between payload type and encoding name is legal SDP whitespace, so
        // it is a separator and not a reason to drop the section.
        let sdp = format!(
            "{}m=audio 5004 RTP/AVP 0\r\na=rtpmap:0\tPCMU/8000\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let rtp = rtp_codec(codecs.entries());
        let pcmu = codec_named(&rtp, "PCMU").expect("PCMU must be present");
        assert_eq!(pcmu.clock_rate(), 8000);
        assert_eq!(
            pcmu.channels(),
            Some(1),
            "no channel field normalizes to mono"
        );
    }

    /// The switch's `atoi` gives an overflowing count whatever its C library decides, so the
    /// count reads 0 with a warning naming the line; the largest `u32` reads as itself.
    #[test]
    fn rtpmap_channel_count_past_u32_warns() {
        let past_u32 = (u64::from(u32::MAX) + 1).to_string();
        let sdp = format!(
            "{}m=audio 5004 RTP/AVP 96 97\r\na=rtpmap:96 L16/8000/{}\r\n\
             a=rtpmap:97 L16/8000/{past_u32}\r\n",
            sdp_header(),
            u32::MAX
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let rtp = rtp_codec(codecs.entries());
        let channels: Vec<_> = rtp
            .iter()
            .map(|c| (c.payload_type(), c.channels()))
            .collect();
        assert_eq!(channels, [(96, Some(u32::MAX)), (97, Some(0))]);
        let overflow = sdp
            .find("a=rtpmap:97")
            .unwrap();
        assert_eq!(
            codecs.warnings(),
            [SdpWarning::RtpmapChannelsOverflow { offset: overflow }]
        );
        let shown = codecs.warnings()[0].to_string();
        assert!(shown.contains(&overflow.to_string()), "{shown}");
        assert!(!shown.contains(&past_u32), "{shown}");
    }

    #[test]
    fn rtpmap_interior_whitespace_in_name_is_refused_and_the_rtpmap_ignored() {
        // The name ends at the space and parse_rtpmap reads no rate after it, refusing the
        // offer; extraction ignores the rtpmap and keeps the static name.
        let sdp = format!(
            "{}m=audio 5004 RTP/AVP 0\r\na=rtpmap:0 foo bar/8000\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let rtp = rtp_codec(codecs.entries());
        assert!(codec_named(&rtp, "PCMU").is_some());
        assert_eq!(
            codecs.warnings(),
            [SdpWarning::SwitchRejects {
                reason: crate::sdp::SwitchRejection::InvalidRtpmap,
                offset: sdp.find("a=rtpmap"),
            }]
        );
    }
}
