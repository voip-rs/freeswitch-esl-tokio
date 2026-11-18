//! One `m=` section's payload walk: RTP-transport detection and per-payload resolution.
//!
//! Line numbers in this module index FreeSWITCH `v1.11.1`
//! (`c2c59645f6911a76589e5008c4d73349ded44b65`).

use crate::sdp::error::{SdpCodecError, SdpWarning};
use crate::sdp::session;
use crate::sdp::{
    NonCodecKind, NonCodecPayload, SdpCodec, SdpCodecEntry, SdpDirection, SdpMediaType,
    UnmappedPayload,
};

use super::attrs::{attribute_tables, fmtp_param, parse_ptime_value, ptime_from_attrs};

/// What one section's walk inherits: the session-level packetization attributes and
/// the direction already resolved for the section.
pub(super) struct SessionDefaults {
    pub(super) ptime: Option<u32>,
    pub(super) maxptime: Option<u32>,
    pub(super) direction: SdpDirection,
}

/// One `m=` section's payload walk, staged locally so a mid-section parse
/// failure can be discarded wholesale instead of reaching
/// [`SdpMediaSection`](super::SdpMediaSection).
#[derive(Default)]
pub(super) struct MediaSection {
    pub(super) entries: Vec<SdpCodecEntry>,
    pub(super) unmapped: Vec<UnmappedPayload>,
    pub(super) warnings: Vec<SdpWarning>,
    pub(super) non_codec: Vec<NonCodecPayload>,
}

/// `m=image`, matched case-insensitively as FreeSWITCH's own comparison is.
pub(super) fn is_image(media_type: &SdpMediaType) -> bool {
    matches!(media_type, SdpMediaType::Other(s) if s.eq_ignore_ascii_case("image"))
}

/// Classify an encoding name the switch negotiates outside the codec string.
///
/// Matched on the canonical name the same way FreeSWITCH matches it, by
/// case-insensitive comparison against `rm_encoding` (`switch_core_media.c:5805`,
/// `:5816`). No loadable codec carries either name.
fn non_codec_kind(canonical_name: &str) -> Option<NonCodecKind> {
    if canonical_name.eq_ignore_ascii_case("telephone-event") {
        Some(NonCodecKind::TelephoneEvent)
    } else if canonical_name.eq_ignore_ascii_case("CN") {
        Some(NonCodecKind::ComfortNoise)
    } else {
        None
    }
}

/// Walk one RTP `m=` section's format list into codec entries.
///
/// Returns `Err` only for structural breakage within this section (an
/// unparseable `a=rtpmap` or `a=fmtp`) — the
/// caller records this as [`SdpWarning::MalformedMediaSection`] and keeps the
/// section empty rather than aborting the whole parse.
pub(super) fn parse_media_section(
    media: &session::Media,
    media_type: SdpMediaType,
    defaults: &SessionDefaults,
) -> Result<MediaSection, SdpCodecError> {
    let mut section = MediaSection::default();

    // Media-level attributes override session-level.
    let media_ptime =
        ptime_from_attrs(&media.attributes, "ptime", &mut section.warnings).or(defaults.ptime);
    let media_maxptime = ptime_from_attrs(&media.attributes, "maxptime", &mut section.warnings)
        .or(defaults.maxptime);

    let tables = attribute_tables(media)?;

    for (at, &pt) in media
        .payloads
        .iter()
        .enumerate()
    {
        // parse_rtpmap and parse_fmtp bind to the first entry carrying the payload type.
        let first = !media.payloads[..at].contains(&pt);
        // Name, clock rate, and channel count from rtpmap or the static table.
        let (name, clock_rate, rtpmap_channels, has_rtpmap) = if let Some((n, r, c)) = tables
            .rtpmap
            .get(&pt)
            .filter(|_| first)
        {
            (n.as_str(), *r, *c, true)
        } else if let Some(st) = crate::sdp::static_payload::static_payload_type(pt) {
            (st.encoding_name, st.clock_rate, st.channels, false)
        } else {
            section
                .unmapped
                .push(UnmappedPayload::new(pt, media_type.clone()));
            continue;
        };

        let canonical = crate::sdp::static_payload::canonical_iananame(name);

        let fmtp = tables
            .fmtp
            .get(&pt)
            .filter(|_| first)
            .cloned();

        // Leaves the walk here, where FreeSWITCH does (switch_core_media.c:5447/:5456),
        // ahead of the per-codec ptime and bitrate resolution below.
        if let Some(kind) = non_codec_kind(canonical) {
            section
                .non_codec
                .push(NonCodecPayload {
                    kind,
                    payload_type: pt,
                    media_type: media_type.clone(),
                    clock_rate,
                    fmtp,
                    has_rtpmap,
                });
            continue;
        }

        // Audio rtpmap with no channel field normalizes to mono.
        let channels = match (&media_type, rtpmap_channels) {
            (SdpMediaType::Audio, None) => Some(1),
            _ => rtpmap_channels,
        };

        let resolved = resolve_ptime_bitrate(
            canonical,
            pt,
            fmtp.as_deref(),
            media_ptime,
            &mut section.warnings,
        );

        let mut codec = SdpCodec::new(media_type.clone(), pt, canonical, clock_rate)
            .with_direction(defaults.direction);
        if has_rtpmap {
            codec = codec.with_rtpmap();
        }
        *codec.channels_mut() = channels;
        *codec.fmtp_mut() = fmtp;
        *codec.ptime_mut() = resolved.ptime;
        *codec.maxptime_mut() = media_maxptime;
        *codec.bitrate_mut() = resolved.bitrate;

        section
            .entries
            .push(SdpCodecEntry::Rtp(codec));
    }

    Ok(section)
}

/// One payload's resolved packetization and bitrate.
struct PtimeBitrate {
    ptime: Option<u32>,
    bitrate: Option<u32>,
}

/// What a codec's registered fmtp parser contributes, per `switch_core_codec_parse_fmtp`.
enum FmtpRule {
    /// The module registers no parser.
    None,
    /// `ptime=` sets the packetization.
    Ptime,
    /// `mode=` sets it, and an fmtp carrying none means 30 ms.
    IlbcMode,
    /// `bitrate=` sets the bitrate.
    Bitrate,
}

/// A codec whose ptime or bitrate is not what the generic resolution yields.
struct CodecQuirk {
    name: &'static str,
    /// Forced `(ptime, bitrate)` when the offer carries no `a=fmtp`, overriding
    /// even an explicit `a=ptime`.
    no_fmtp: Option<(u32, u32)>,
    fmtp: FmtpRule,
}

const CODEC_QUIRKS: [CodecQuirk; 4] = [
    CodecQuirk {
        name: "ilbc",
        no_fmtp: Some((30, 13330)),
        fmtp: FmtpRule::IlbcMode,
    },
    CodecQuirk {
        name: "isac",
        no_fmtp: Some((30, 32000)),
        fmtp: FmtpRule::None,
    },
    CodecQuirk {
        name: "opus",
        no_fmtp: None,
        fmtp: FmtpRule::Ptime,
    },
    CodecQuirk {
        name: "g7221",
        no_fmtp: None,
        fmtp: FmtpRule::Bitrate,
    },
];

/// Resolve one payload's ptime and bitrate as `add_audio_codec` does: a sequential
/// overwrite where a later step beats an earlier one, not a first-match-wins chain.
fn resolve_ptime_bitrate(
    canonical: &str,
    payload_type: u8,
    fmtp: Option<&str>,
    media_ptime: Option<u32>,
    warnings: &mut Vec<SdpWarning>,
) -> PtimeBitrate {
    let mut out = PtimeBitrate {
        ptime: media_ptime.or_else(|| Some(crate::sdp::static_payload::default_ptime(canonical))),
        bitrate: crate::sdp::static_payload::known_bitrate(payload_type),
    };

    let Some(quirk) = CODEC_QUIRKS
        .iter()
        .find(|q| {
            q.name
                .eq_ignore_ascii_case(canonical)
        })
    else {
        return out;
    };

    let Some(fmtp) = fmtp else {
        if let Some((ptime, bitrate)) = quirk.no_fmtp {
            out.ptime = Some(ptime);
            out.bitrate = Some(bitrate);
        }
        return out;
    };

    match quirk.fmtp {
        FmtpRule::None => {}
        FmtpRule::Ptime => {
            if let Some(p) = fmtp_param(fmtp, "ptime") {
                if let Some(v) = parse_ptime_value(p, warnings, "fmtp ptime") {
                    out.ptime = Some(v);
                }
            }
        }
        FmtpRule::IlbcMode => {
            out.ptime = Some(match fmtp_param(fmtp, "mode") {
                Some(m) => parse_ptime_value(m, warnings, "fmtp mode").unwrap_or(30),
                None => 30,
            });
        }
        FmtpRule::Bitrate => {
            if let Some(raw) = fmtp_param(fmtp, "bitrate") {
                match raw.parse::<u32>() {
                    Ok(bitrate) => out.bitrate = Some(bitrate),
                    Err(_) => warnings.push(SdpWarning::unparseable_numeric_attribute(
                        "fmtp bitrate",
                        raw,
                    )),
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::super::testutil::{codec_named, rtp_codec, sdp_header};
    use super::*;
    use crate::sdp::{CodecStringOptions, SdpCodecs, SdpMediaType, SdpWarning};

    // --- retained sections ---

    #[test]
    fn port_zero_section_contributes_nothing_but_is_retained_whole() {
        // A held or declined stream: the offer still names its codecs, and that is
        // what an operator reads when the complaint is "no audio".
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 0 RTP/AVP 104 96\r\n",
                "a=rtpmap:104 AMR-WB/16000\r\n",
                "a=fmtp:104 mode-set=1; max-red=0\r\n",
                "a=rtpmap:96 telephone-event/16000\r\n",
                "a=fmtp:96 0-15\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();

        assert!(codecs.is_empty(), "port 0 contributes no codec entries");
        assert!(codecs
            .audio_codec_string(&CodecStringOptions::audio(), None)
            .unwrap()
            .is_empty());

        let sections = codecs.sections();
        assert_eq!(
            sections.len(),
            1,
            "every m= line yields exactly one section"
        );
        let held = &sections[0];
        assert_eq!(held.port(), 0);
        assert!(!held.is_negotiable());
        assert_eq!(held.media_type(), &SdpMediaType::Audio);

        let rtp = rtp_codec(held.entries());
        let amr = codec_named(&rtp, "AMR-WB").expect("the held codec must survive by name");
        assert_eq!(amr.clock_rate(), 16000);
        assert_eq!(amr.fmtp(), Some("mode-set=1; max-red=0"));

        let te = held.non_codec_payloads();
        assert_eq!(te.len(), 1);
        assert_eq!(
            te[0]
                .fmtp
                .as_deref(),
            Some("0-15")
        );
    }

    #[test]
    fn a_retained_sections_payloads_stay_out_of_the_top_level_views() {
        // sections() is the inventory; the top-level accessors stay the switch's view.
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 0 RTP/AVP 0 99 101\r\n",
                "a=rtpmap:101 telephone-event/8000\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(
            codecs
                .non_codec_payloads()
                .count(),
            0
        );
        assert_eq!(
            codecs
                .unmapped()
                .count(),
            0
        );

        let held = &codecs.sections()[0];
        assert_eq!(
            held.non_codec_payloads()
                .len(),
            1
        );
        assert_eq!(
            held.unmapped()
                .len(),
            1,
            "PT 99 has no rtpmap and no static entry, on the section it appeared in"
        );
    }

    #[test]
    fn fmtp_for_ignores_a_held_sections_parameters() {
        // fmtp_for drives rtp_force_audio_fmtp. Answering from a stream that is not
        // being negotiated pins the wrong parameters on the live call.
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 0 RTP/AVP 104\r\n",
                "a=rtpmap:104 AMR-WB/16000\r\n",
                "a=fmtp:104 octet-align=1\r\n",
                "m=audio 5004 RTP/AVP 105\r\n",
                "a=rtpmap:105 AMR-WB/16000\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(codecs.fmtp_for("AMR-WB", 16000), None);
    }

    #[test]
    fn port_zero_image_section_yields_no_t38_entry() {
        let sdp = format!("{}m=image 0 udptl t38\r\n", sdp_header());
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert!(codecs.is_empty());
        assert!(codecs
            .audio_codec_string(&CodecStringOptions::audio(), None)
            .unwrap()
            .is_empty());

        let declined = &codecs.sections()[0];
        assert!(!declined.is_negotiable());
        assert!(matches!(declined.entries()[0], SdpCodecEntry::T38));
    }

    // --- non-RTP proto ---

    #[test]
    fn non_rtp_proto_reads_no_payload_types_and_raises_no_warning() {
        // sofia fills a section's rtpmap list only for the transports it maps to an
        // RTP proto, so a datachannel format token is not a malformed payload type.
        let sdp = format!(
            "{}m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert!(codecs
            .warnings()
            .is_empty());

        let section = &codecs.sections()[0];
        assert!(section
            .entries()
            .is_empty());
        assert_eq!(section.proto(), "UDP/DTLS/SCTP");
        assert_eq!(section.formats(), "webrtc-datachannel");
    }

    #[test]
    fn non_rtp_proto_on_audio_gets_the_same_treatment() {
        // The gate is sofia's, not the media type's: an m=audio under udptl carries
        // no rtpmaps for the switch either.
        let sdp = format!("{}m=audio 5004 udptl t38\r\n", sdp_header());
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert!(codecs
            .warnings()
            .is_empty());
        assert!(codecs.is_empty());
        assert_eq!(codecs.sections()[0].formats(), "t38");
    }

    #[test]
    fn a_proto_merely_containing_rtp_is_not_an_rtp_transport() {
        // sofia matches the proto against an exact set; TCP/RTP/AVP is not in it.
        let sdp = format!("{}m=audio 5004 TCP/RTP/AVP 0\r\n", sdp_header());
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert!(codecs.is_empty());
        assert!(codecs.sections()[0]
            .entries()
            .is_empty());
    }

    // --- text section ---

    #[test]
    fn text_section_codecs_are_retained_but_never_negotiable() {
        let sdp = format!(
            "{}m=text 5000 RTP/AVP 98\r\na=rtpmap:98 t140/1000\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert!(codecs.is_empty());
        assert_eq!(
            codecs
                .audio()
                .count(),
            0
        );
        assert_eq!(
            codecs
                .video()
                .count(),
            0
        );

        let section = &codecs.sections()[0];
        assert!(!section.is_negotiable());
        let rtp = rtp_codec(section.entries());
        assert_eq!(
            codec_named(&rtp, "t140")
                .expect("the text codec must survive")
                .clock_rate(),
            1000
        );
    }

    // --- image / T38 ---

    #[test]
    fn image_m_line_yields_t38_entry() {
        let sdp = format!("{}m=image 5008 udptl t38\r\n", sdp_header());
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(codecs.len(), 1);
        assert!(matches!(
            codecs
                .entries()
                .next(),
            Some(SdpCodecEntry::T38)
        ));
    }

    #[test]
    fn image_m_line_with_other_proto_still_yields_t38() {
        // The proto and fmt are not inspected; FreeSWITCH does not check them.
        let sdp = format!(
            "{}m=image 5008 RTP/AVP 98\r\na=rtpmap:98 t38/8000\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(codecs.len(), 1);
        assert!(matches!(
            codecs
                .entries()
                .next(),
            Some(SdpCodecEntry::T38)
        ));
    }

    #[test]
    fn t38_before_image_precedes_audio_in_codec_string() {
        // C writes ",t38" inside the m-line loop (switch_core_media.c:13651), so
        // position in the offer's m-line order determines position in the codec
        // string, same as any other entry.
        let sdp = format!(
            concat!(
                "{}",
                "m=image 5008 udptl t38\r\n",
                "m=audio 5004 RTP/AVP 0\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let cs = codecs
            .audio_codec_string(&CodecStringOptions::default(), None)
            .unwrap();
        assert_eq!(cs.entries()[0].name(), "t38");
        assert_eq!(cs.entries()[1].name(), "PCMU");
    }

    // --- malformed section handling ---

    #[test]
    fn a_non_negotiable_section_still_reports_its_parse_warnings() {
        // These sections are parsed rather than skipped, so they warn where the
        // switch, which never looks at them, has no occasion to.
        let sdp = format!("{}m=audio 0 RTP/AVP 0\r\na=ptime:abc\r\n", sdp_header());
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(
            codecs
                .warnings()
                .len(),
            1
        );
    }

    #[test]
    fn malformed_rtpmap_empties_its_section_but_the_section_survives() {
        // Recovery is per-section, not per-attribute: the otherwise-fine static PCMU
        // goes with the bad a=rtpmap. The section itself is still reported.
        let sdp = format!(
            "{}m=audio 5004 RTP/AVP 0\r\na=rtpmap:x PCMU/8000\r\n",
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

        let section = &codecs.sections()[0];
        assert_eq!(section.port(), 5004);
        assert!(section
            .entries()
            .is_empty());
    }

    #[test]
    fn an_rtp_format_parse_payload_refuses_is_reported_and_the_rest_still_read() {
        // parse_payload refuses the whole offer at a format that is not a number below
        // 128; extraction keeps every section and reports the refusal.
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 9 RTP/AVP *\r\n",
                "m=audio 5004 RTP/AVP 0 8\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let rtp = rtp_codec(codecs.entries());
        assert!(codec_named(&rtp, "PCMU").is_some());
        assert!(codec_named(&rtp, "PCMA").is_some());
        assert_eq!(
            codecs.warnings(),
            [SdpWarning::SwitchRejects {
                reason: crate::sdp::SwitchRejection::InvalidRtpFormat,
                offset: sdp.find("m=audio 9"),
            }]
        );
        assert_eq!(
            codecs
                .sections()
                .len(),
            2
        );
    }
}
