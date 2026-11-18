//! `a=rtpmap`/`a=fmtp`/`a=ptime`/direction attribute parsing.
//!
//! Line numbers in this module index FreeSWITCH `v1.11.1`
//! (`c2c59645f6911a76589e5008c4d73349ded44b65`).

use std::collections::HashMap;

use crate::sdp::error::{SdpCodecError, SdpWarning};
use crate::sdp::num::atoi_prefix;
use crate::sdp::session;
use crate::sdp::SdpDirection;

/// Cursor over an `a=rtpmap`/`a=fmtp` attribute value.
///
/// Whitespace is decided once, while consuming a field, mirroring `parse_ul` and
/// `token` in sofia-sip's `sdp_parse.c` — never per-field after a split.
struct AttrCursor<'a> {
    rest: &'a str,
}

impl<'a> AttrCursor<'a> {
    fn new(value: &'a str) -> Self {
        Self { rest: value }
    }

    /// Skip a run of SP/HTAB.
    fn skip_ws(&mut self) {
        self.rest = self
            .rest
            .trim_start_matches([' ', '\t']);
    }

    /// Skip whitespace, take the run of ASCII digits, skip trailing whitespace —
    /// mirrors `parse_ul`'s `strspn` on both sides of the number. The cursor does
    /// not advance when there is no number to take.
    fn number<T: TryFrom<u32>>(&mut self) -> Option<T> {
        self.skip_ws();
        let (value, rest) = atoi_prefix(self.rest);
        let value = value?;
        self.rest = rest;
        self.skip_ws();
        T::try_from(value).ok()
    }

    /// Skip whitespace, then take the maximal run of chars that are neither
    /// whitespace nor one of `stop`.
    fn field(&mut self, stop: &[char]) -> &'a str {
        self.skip_ws();
        let end = self
            .rest
            .find(|c: char| c == ' ' || c == '\t' || stop.contains(&c))
            .unwrap_or(
                self.rest
                    .len(),
            );
        let (field, rest) = self
            .rest
            .split_at(end);
        self.rest = rest;
        field
    }

    /// Consume an expected delimiter char if present.
    fn eat(&mut self, ch: char) -> bool {
        match self
            .rest
            .strip_prefix(ch)
        {
            Some(rest) => {
                self.rest = rest;
                true
            }
            None => false,
        }
    }

    /// The unconsumed remainder, verbatim (no whitespace stripped).
    fn remaining(&self) -> &'a str {
        self.rest
    }
}

/// Parse an `a=rtpmap` attribute value into `(pt, name, clock_rate, channels)`.
///
/// Grammar: number (pt) / field stopping at `/` (name) / required `/` / number
/// (clock rate) / optionally `/` then number (channels). The name has no
/// charset restriction beyond "not whitespace, not `/`" — sofia's stricter
/// `TOKEN` charset is enforced later, in `audio_codec_string`, on a path that
/// warns and drops one codec rather than the whole media section.
///
/// Returns `Err(MalformedRtpmap)` for any structural violation (missing
/// separator, non-numeric PT, non-numeric clock rate, non-numeric channel count).
pub(super) fn parse_rtpmap(value: &str) -> Result<(u8, String, u32, Option<u32>), SdpCodecError> {
    let mut cursor = AttrCursor::new(value);

    let pt = cursor
        .number()
        .ok_or_else(|| SdpCodecError::MalformedRtpmap(value.to_string()))?;

    let name = cursor.field(&['/']);
    if name.is_empty() {
        return Err(SdpCodecError::MalformedRtpmap(value.to_string()));
    }
    let name = name.to_string();

    if !cursor.eat('/') {
        return Err(SdpCodecError::MalformedRtpmap(value.to_string()));
    }

    let clock_rate = cursor
        .number()
        .ok_or_else(|| SdpCodecError::MalformedRtpmap(value.to_string()))?;

    let channels = if cursor.eat('/') {
        cursor.skip_ws();
        if cursor
            .remaining()
            .is_empty()
        {
            None
        } else {
            Some(
                cursor
                    .number()
                    .ok_or_else(|| SdpCodecError::MalformedRtpmap(value.to_string()))?,
            )
        }
    } else {
        None
    };

    if !cursor
        .remaining()
        .is_empty()
    {
        return Err(SdpCodecError::MalformedRtpmap(value.to_string()));
    }

    Ok((pt, name, clock_rate, channels))
}

/// Extract `(pt, params)` from an `a=fmtp` attribute value.
///
/// Only the whitespace between the payload type and the params is a
/// separator (consumed by `number`'s trailing skip); everything after that,
/// including any trailing whitespace, is opaque `byte-string` content per
/// RFC 8866 and is returned verbatim.
///
/// A non-numeric payload type is a hard error — same structural breakage as in `a=rtpmap`.
pub(super) fn parse_fmtp_pt(value: &str) -> Result<(u8, String), SdpCodecError> {
    let mut cursor = AttrCursor::new(value);
    let pt = cursor
        .number()
        .ok_or_else(|| SdpCodecError::MalformedFmtp(value.to_string()))?;
    Ok((
        pt,
        cursor
            .remaining()
            .to_string(),
    ))
}

/// One section's `a=rtpmap` and `a=fmtp` values, keyed by payload type.
pub(super) struct AttrTables {
    pub(super) rtpmap: HashMap<u8, (String, u32, Option<u32>)>,
    pub(super) fmtp: HashMap<u8, String>,
}

/// Collect a section's rtpmap and fmtp attributes into payload-type lookup tables.
pub(super) fn attribute_tables(media: &session::Media) -> Result<AttrTables, SdpCodecError> {
    let mut tables = AttrTables {
        rtpmap: HashMap::new(),
        fmtp: HashMap::new(),
    };

    // Attribute names are compared byte-exactly here and in ptime_from_attrs/
    // direction_from_attrs below, unlike FreeSWITCH's own attribute walk, which uses
    // strcasecmp (switch_core_media.c:13658). RFC 8866 makes attribute names
    // case-sensitive, so byte-exact stays the intended behaviour here; this is a
    // documented divergence, not an oversight.
    for attr in &media.attributes {
        match attr
            .attribute
            .as_str()
        {
            "rtpmap" => {
                if let Some(val) = &attr.value {
                    let (pt, name, rate, channels) = parse_rtpmap(val)?;
                    tables
                        .rtpmap
                        .insert(pt, (name, rate, channels));
                }
            }
            "fmtp" => {
                if let Some(val) = &attr.value {
                    let (pt, params) = parse_fmtp_pt(val)?;
                    tables
                        .fmtp
                        .insert(pt, params);
                }
            }
            _ => {}
        }
    }

    Ok(tables)
}

/// Return ptime from the first matching attribute, recording a warning if unparseable.
pub(super) fn ptime_from_attrs(
    attrs: &[session::Attribute],
    name: &str,
    warnings: &mut Vec<SdpWarning>,
) -> Option<u32> {
    attrs
        .iter()
        .find(|a| a.attribute == name)
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

/// The direction attribute names, which carry no value (`a=sendrecv`, never
/// `a=sendrecv:…`).
const DIRECTION_ATTRIBUTES: [&str; 4] = ["sendrecv", "sendonly", "recvonly", "inactive"];

/// Return the first direction attribute from the list, if any.
///
/// A value-less attribute naming a direction in another case is not one — RFC 8866
/// attribute names are case-sensitive — and is warned about instead of leaving the
/// caller to read the enclosing level's direction as the peer's.
pub(super) fn direction_from_attrs(
    attrs: &[session::Attribute],
    warnings: &mut Vec<SdpWarning>,
) -> Option<SdpDirection> {
    for attr in attrs
        .iter()
        .filter(|a| {
            a.value
                .is_none()
        })
    {
        if !DIRECTION_ATTRIBUTES
            .iter()
            .any(|d| d.eq_ignore_ascii_case(&attr.attribute))
        {
            continue;
        }
        match attr
            .attribute
            .parse::<SdpDirection>()
        {
            Ok(direction) => return Some(direction),
            Err(_) => warnings.push(SdpWarning::non_canonical_direction_attribute(
                &attr.attribute,
            )),
        }
    }
    None
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
    fn non_canonical_direction_attribute_warns_instead_of_inheriting_silently() {
        // RFC 8866 attribute names are case-sensitive, so `a=SENDONLY` is not a
        // direction. Inheriting the session's without a word makes a one-way call
        // read as bidirectional in the parse output.
        let sdp = format!(
            "{}a=sendrecv\r\nm=audio 5004 RTP/AVP 0\r\na=SENDONLY\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(
            codecs.sections()[0].direction(),
            crate::sdp::SdpDirection::SendRecv
        );
        assert!(matches!(
            codecs.warnings()[0],
            SdpWarning::NonCanonicalDirectionAttribute { .. }
        ));
    }

    #[test]
    fn an_unrelated_value_less_attribute_is_not_a_direction() {
        let sdp = format!("{}m=audio 5004 RTP/AVP 0\r\na=rtcp-mux\r\n", sdp_header());
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert!(codecs
            .warnings()
            .is_empty());
    }

    // --- malformed fmtp skips only its own section ---

    #[test]
    fn malformed_fmtp_non_numeric_pt_skips_section_with_warning() {
        let sdp = format!(
            "{}m=audio 5004 RTP/AVP 0\r\na=fmtp:x minptime=10\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(
            codecs
                .entries()
                .count(),
            0
        );
        assert_eq!(
            codecs
                .warnings()
                .len(),
            1
        );
        assert!(matches!(
            codecs.warnings()[0],
            SdpWarning::MalformedMediaSection { .. }
        ));
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

    #[test]
    fn rtpmap_interior_whitespace_in_name_is_malformed() {
        // Whitespace inside what would be the encoding name is not a separator
        // sofia recognises either — it still rejects this shape, via a failed
        // clock-rate parse once the name is truncated at the first space.
        let sdp = format!(
            "{}m=audio 5004 RTP/AVP 0\r\na=rtpmap:0 foo bar/8000\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(
            codecs
                .entries()
                .count(),
            0,
            "malformed rtpmap must still drop the whole section"
        );
        assert!(matches!(
            codecs.warnings()[0],
            SdpWarning::MalformedMediaSection { .. }
        ));
    }
}
