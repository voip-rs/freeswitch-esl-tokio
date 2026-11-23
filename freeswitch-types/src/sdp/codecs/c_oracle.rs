//! `SdpCodecs::parse` against sofia-sip's own `sdp_parse`, called as the switch calls it.
//!
//! Where sofia refuses, the parse must carry the refusal as `SwitchRejects`; where it stops
//! reading, `SwitchStopsReading`, and the prefix before the stop must parse to what sofia saw.
//! Otherwise every section must match sofia's: type, port, format list, direction, and for an RTP
//! section each payload type's name, rate, channels and fmtp. Field values sofia validates but
//! extraction never reads (`o=`, `c=`, `b=`, `t=`, `r=`, `z=`, `k=`) are always generated
//! well-formed, because a refusal on one of them alone is not reported.

use freeswitch_c_oracle::{against_sofia, SdpMedia, SdpParse, SdpSession};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::select;
use proptest::test_runner::{TestCaseError, TestCaseResult};

use super::{SdpCodecEntry, SdpCodecs, SdpMediaSection};
use crate::sdp::static_payload::canonical_iananame;
use crate::sdp::{SdpDirection, SdpWarning, SwitchRejection};

const PAYLOAD_TYPES: &[&str] = &[
    "0", "3", "8", "9", "13", "18", "34", "96", "97", "101", "127",
];
/// Static payload types sofia's well-known table names and RFC 3551 leaves unassigned.
const SOFIA_ONLY_STATIC: &[&str] = &["1", "2", "19"];
const ODD_FORMATS: &[&str] = &["128", "*", "abc", "96x"];

fn payload_type() -> impl Strategy<Value = String> {
    prop_oneof![
        8 => select(PAYLOAD_TYPES),
        1 => select(ODD_FORMATS),
        1 => select(SOFIA_ONLY_STATIC),
    ]
    .prop_map(str::to_owned)
}

/// Payload types `parse_ul` cannot read below 128.
const UNREADABLE_PAYLOAD_TYPES: &[&str] = &["128", "*", "abc", "-1", ""];

/// The payload type an `a=rtpmap` or `a=fmtp` names.
fn attribute_payload_type() -> impl Strategy<Value = String> {
    prop_oneof![
        8 => select(PAYLOAD_TYPES),
        1 => select(UNREADABLE_PAYLOAD_TYPES),
    ]
    .prop_map(str::to_owned)
}

fn rtpmap() -> impl Strategy<Value = String> {
    let name = select(
        &[
            "opus",
            "PCMU",
            "telephone-event",
            "CN",
            "AMR-WB",
            "H264",
            "speex",
        ][..],
    );
    let rate = select(&["8000", "16000", "48000", "90000"][..]);
    let past_u32 = format!("/{}", u64::from(u32::MAX) + 1);
    let channels = select(vec![String::new(), "/1".into(), "/2".into(), past_u32]);
    (attribute_payload_type(), name, rate, channels)
        .prop_map(|(pt, name, rate, ch)| format!("a=rtpmap:{pt} {name}/{rate}{ch}"))
}

fn fmtp() -> impl Strategy<Value = String> {
    let params = select(&["0-16", "useinbandfec=1", "octet-align=1; mode-set=0,2", ""][..]);
    (attribute_payload_type(), params).prop_map(|(pt, params)| format!("a=fmtp:{pt} {params}"))
}

fn direction() -> impl Strategy<Value = String> {
    select(&["a=sendrecv", "a=sendonly", "a=recvonly", "a=inactive"][..]).prop_map(str::to_owned)
}

/// Lines that are not well-formed attributes or fields, at either level.
fn noise() -> impl Strategy<Value = String> {
    const LINES: &[&str] = &[
        "junk line",
        "ab=x",
        "x=y",
        " ",
        "\t",
        "x",
        "=",
        "  a=ptime:30",
        "a=",
        "a=:x",
        "a=rtpmap 96 PCMU/8000",
        "a=ptime:20",
        "\0",
        "a=rtpmap:x opus/48000",
        "a=fmtp:y foo",
        "a=rtpmap:96 opus",
        "a=rtpmap:96 bad,name/8000",
        "a=rtpmap:96 opus/abc",
        "a=rtpmap:0 opus/48000/2 extra",
        "a=rtpmap:96 opus/48000/",
        "a=rtpmap:96x opus/8000",
        "a=rtpmap:96 +8000",
        "a=SENDONLY",
        "a=RecvOnly",
        "a=RTPMAP:96 opus/48000/2",
    ];
    select(LINES).prop_map(str::to_owned)
}

fn media_line() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => rtpmap(),
        2 => fmtp(),
        1 => direction(),
        1 => Just("c=IN IP4 192.0.2.2".to_owned()),
        1 => Just("b=AS:64".to_owned()),
        1 => Just("i=info".to_owned()),
        1 => Just("k=prompt".to_owned()),
        1 => Just("r=7d 1h 0 25h".to_owned()),
        2 => noise(),
    ]
}

fn m_line() -> impl Strategy<Value = String> {
    let media = select(
        &[
            "audio",
            "video",
            "image",
            "application",
            "AUDIO",
            "Video",
            "IMAGE",
            "Text",
        ][..],
    );
    let port = select(&["5004", "9", "65535", "0", "5004/2"][..]);
    let proto = select(
        &[
            "RTP/AVP",
            "RTP/SAVP",
            "UDP/TLS/RTP/SAVPF",
            "RTP",
            "udptl",
            "TCP/MSRP",
            "RTP/AVP/TCP",
        ][..],
    );
    (media, port, proto, vec(payload_type(), 0..4)).prop_map(|(media, port, proto, formats)| {
        format!("m={media} {port} {proto} {}", formats.join(" "))
            .trim_end()
            .to_owned()
    })
}

fn section() -> impl Strategy<Value = Vec<String>> {
    let lines = vec(media_line(), 0..6);
    let one_direction = proptest::option::of(direction());
    (m_line(), lines, one_direction).prop_map(|(m, mut lines, direction)| {
        lines.insert(0, m);
        lines.extend(direction);
        lines
    })
}

fn session_lines() -> impl Strategy<Value = Vec<String>> {
    let present = |line: &'static str| proptest::option::weighted(0.9, Just(line.to_owned()));
    let extra = prop_oneof![
        2 => Just("r=7d 1h 0 25h".to_owned()),
        1 => Just("z=2882844526 -1h".to_owned()),
        1 => Just("b=AS:64".to_owned()),
        1 => Just("k=prompt".to_owned()),
        1 => Just("a=ptime:20".to_owned()),
        1 => Just("o=- 1 1 IN IP4 192.0.2.3".to_owned()),
        1 => Just("c=IN IP4 192.0.2.3".to_owned()),
        2 => noise(),
    ];
    (
        present("o=- 0 0 IN IP4 192.0.2.1"),
        present("s=-"),
        present("c=IN IP4 192.0.2.1"),
        present("t=0 0"),
        vec((any::<prop::sample::Index>(), extra), 0..3),
        proptest::option::of(direction()),
    )
        .prop_map(|(o, s, c, t, extras, direction)| {
            let mut lines: Vec<String> = [o, s, c, t]
                .into_iter()
                .flatten()
                .collect();
            for (at, line) in extras {
                lines.insert(at.index(lines.len() + 1), line);
            }
            lines.extend(direction);
            lines
        })
}

fn offer() -> impl Strategy<Value = String> {
    let version = prop_oneof![
        20 => Just("v=0"),
        1 => Just("v=1"),
        1 => Just(" v=0"),
        1 => Just("v=0 "),
    ];
    let separator =
        prop_oneof![8 => Just("\r\n"), 2 => Just("\n"), 1 => Just("\r"), 1 => Just("\r\n\r\n")];
    (
        version,
        session_lines(),
        vec(section(), 0..4),
        vec(separator, 64),
    )
        .prop_map(|(version, session, sections, separators)| {
            let lines = std::iter::once(version.to_owned())
                .chain(session)
                .chain(
                    sections
                        .into_iter()
                        .flatten(),
                );
            lines
                .zip(
                    separators
                        .into_iter()
                        .cycle(),
                )
                .map(|(line, sep)| line + sep)
                .collect()
        })
}

/// The category the parse reports for one of sofia's refusal messages.
fn rejection(message: &str) -> SwitchRejection {
    let categories = [
        ("bad line", SwitchRejection::BadLine),
        ("unknown field", SwitchRejection::UnknownField),
        (
            "repeat field without time",
            SwitchRejection::RepeatWithoutTime,
        ),
        ("zone field without time", SwitchRejection::ZoneWithoutTime),
        (
            "invalid attribute name",
            SwitchRejection::InvalidAttributeName,
        ),
        (
            "m= invalid format for RTP",
            SwitchRejection::InvalidRtpFormat,
        ),
        ("m= ", SwitchRejection::InvalidMediaLine),
        ("a=rtpmap:", SwitchRejection::InvalidRtpmap),
        ("No o= present", SwitchRejection::MissingOrigin),
        ("No c= on either", SwitchRejection::MissingConnection),
    ];
    categories
        .iter()
        .find(|(prefix, _)| message.starts_with(prefix))
        .map(|&(_, category)| category)
        .unwrap_or_else(|| panic!("sofia refused with an unmodelled message: {message:?}"))
}

fn direction_of(mode: u8) -> SdpDirection {
    match mode {
        0 => SdpDirection::Inactive,
        1 => SdpDirection::SendOnly,
        2 => SdpDirection::RecvOnly,
        _ => SdpDirection::SendRecv,
    }
}

fn compare_section(c: &SdpMedia, ours: &SdpMediaSection) -> TestCaseResult {
    // sofia spells a type it knows in lowercase, one it does not verbatim; Other keeps the
    // spelling of a type sofia knows and SdpMediaType has no variant for.
    let ours_type = ours
        .media_type()
        .to_string();
    let c_type = String::from_utf8_lossy(&c.type_name);
    let sofia_only = ["image", "data", "control", "red"].contains(&c_type.as_ref());
    prop_assert!(
        if sofia_only {
            ours_type.eq_ignore_ascii_case(&c_type)
        } else {
            ours_type == c_type
        },
        "media type: sofia {:?}, ours {}",
        String::from_utf8_lossy(&c.type_name),
        ours.media_type()
    );
    prop_assert_eq!(u64::from(ours.port()), c.port);
    let ours_formats: Vec<&[u8]> = ours
        .formats()
        .split_ascii_whitespace()
        .map(str::as_bytes)
        .collect();
    if c.rtpmaps
        .is_empty()
    {
        let c_formats: Vec<&[u8]> = c
            .formats
            .iter()
            .map(Vec::as_slice)
            .collect();
        prop_assert_eq!(ours_formats, c_formats);
    } else {
        let c_pts: Vec<Vec<u8>> = c
            .rtpmaps
            .iter()
            .map(|rm| {
                rm.pt
                    .to_string()
                    .into_bytes()
            })
            .collect();
        let c_pts: Vec<&[u8]> = c_pts
            .iter()
            .map(Vec::as_slice)
            .collect();
        prop_assert_eq!(ours_formats, c_pts);
    }
    prop_assert_eq!(ours.direction(), direction_of(c.mode), "direction");
    // m=image is T.38 whatever its proto, as the switch reads it.
    if super::is_image(ours.media_type()) {
        return Ok(());
    }
    // Each list keeps format-list order, so sofia's entries for one payload type, taken by the
    // list their kind lands in, pair with ours in order even where the type repeats.
    let mut codecs: Vec<_> = ours
        .entries()
        .iter()
        .filter_map(|e| match e {
            SdpCodecEntry::Rtp(codec) => Some(codec),
            _ => None,
        })
        .map(Some)
        .collect();
    let mut non_codecs: Vec<_> = ours
        .non_codec_payloads()
        .iter()
        .map(Some)
        .collect();
    let mut unmapped: Vec<_> = ours
        .unmapped()
        .iter()
        .map(Some)
        .collect();
    for rm in &c.rtpmaps {
        let encoding = String::from_utf8_lossy(&rm.encoding);
        let fmtp = rm
            .fmtp
            .as_deref()
            .map(String::from_utf8_lossy);
        let negotiated_apart =
            encoding.eq_ignore_ascii_case("telephone-event") || encoding.eq_ignore_ascii_case("CN");
        let (codec, non_codec) = if rm
            .encoding
            .is_empty()
        {
            let found = take(&mut unmapped, rm.pt, |u| u.payload_type);
            prop_assert!(
                found.is_some(),
                "pt {} unnamed by sofia is not unmapped",
                rm.pt
            );
            continue;
        } else if negotiated_apart {
            (None, take(&mut non_codecs, rm.pt, |n| n.payload_type))
        } else {
            (take(&mut codecs, rm.pt, |c| c.payload_type()), None)
        };
        match (codec, non_codec) {
            (Some(codec), _) => {
                prop_assert!(
                    codec
                        .name()
                        .eq_ignore_ascii_case(canonical_iananame(&encoding)),
                    "pt {}: sofia {encoding}, ours {}",
                    rm.pt,
                    codec.name()
                );
                prop_assert_eq!(u64::from(codec.clock_rate()), rm.rate, "pt {} rate", rm.pt);
                prop_assert_eq!(codec.fmtp(), fmtp.as_deref(), "pt {} fmtp", rm.pt);
                if let Some(params) = &rm.params {
                    // The switch reads rm_params with atoi.
                    let channels = Some(
                        crate::sdp::num::atoi_prefix(&String::from_utf8_lossy(params))
                            .0
                            .unwrap_or(0),
                    );
                    prop_assert_eq!(codec.channels(), channels, "pt {} channels", rm.pt);
                }
            }
            (None, Some(non_codec)) => {
                prop_assert_eq!(
                    u64::from(non_codec.clock_rate),
                    rm.rate,
                    "pt {} rate",
                    rm.pt
                );
                prop_assert_eq!(
                    non_codec
                        .fmtp
                        .as_deref(),
                    fmtp.as_deref(),
                    "pt {} fmtp",
                    rm.pt
                );
            }
            (None, None) => prop_assert!(false, "pt {} ({encoding}) missing from ours", rm.pt),
        }
    }
    Ok(())
}

/// The first item of `list` not yet paired that carries payload type `pt`, taken out of it.
fn take<T: Copy>(list: &mut [Option<T>], pt: u8, of: impl Fn(T) -> u8) -> Option<T> {
    list.iter_mut()
        .find(|item| item.is_some_and(|item| of(item) == pt))
        .and_then(Option::take)
}

fn compare(c: &SdpSession, ours: &SdpCodecs) -> TestCaseResult {
    prop_assert_eq!(
        ours.sections()
            .len(),
        c.media
            .len(),
        "section count"
    );
    for (c, ours) in c
        .media
        .iter()
        .zip(ours.sections())
    {
        compare_section(c, ours)?;
    }
    Ok(())
}

#[test]
fn sdp_codecs_read_what_sofia_reads() {
    against_sofia(
        file!(),
        "sdp_codecs_read_what_sofia_reads",
        offer(),
        |sofia, sdp| {
            let ours = SdpCodecs::parse(&sdp);
            match sofia.parse(sdp.as_bytes()) {
                SdpParse::Refused(message) => {
                    let message = String::from_utf8_lossy(&message);
                    if message == "bad SDP message" {
                        prop_assert!(ours.is_err(), "sofia: not SDP; ours parsed it");
                        return Ok(());
                    }
                    let reason = rejection(&message);
                    let ours =
                        ours.map_err(|e| TestCaseError::fail(format!("ours failed: {e}")))?;
                    prop_assert!(
                        ours.warnings()
                            .iter()
                            .any(|w| matches!(w, SdpWarning::SwitchRejects { reason: r, .. } if *r == reason)),
                        "sofia refused ({message}); ours warned {:?}",
                        ours.warnings()
                    );
                }
                SdpParse::Parsed(session) => {
                    let ours =
                        ours.map_err(|e| TestCaseError::fail(format!("ours failed: {e}")))?;
                    let rejects: Vec<_> = ours
                        .warnings()
                        .iter()
                        .filter(|w| matches!(w, SdpWarning::SwitchRejects { .. }))
                        .collect();
                    prop_assert!(
                        rejects.is_empty(),
                        "sofia accepted; ours warned {rejects:?}"
                    );
                    let stop = ours
                        .warnings()
                        .iter()
                        .find_map(|w| match w {
                            SdpWarning::SwitchStopsReading { offset } => Some(*offset),
                            _ => None,
                        });
                    match stop {
                        Some(offset) => {
                            let prefix = SdpCodecs::parse(&sdp[..offset])
                                .map_err(|e| TestCaseError::fail(format!("prefix failed: {e}")))?;
                            compare(&session, &prefix)?;
                        }
                        None => compare(&session, &ours)?,
                    }
                }
            }
            Ok(())
        },
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        source_file: Some(file!()),
        ..freeswitch_c_oracle::config()
    })]

    // Reading port-0 attributes reaches only port-0 sections, which nothing the switch
    // negotiates from can see.
    #[test]
    fn reading_port_zero_attributes_cannot_widen_the_switch_view(sdp in offer()) {
        let as_written = crate::sdp::SdpParseOptions::default().with_port_zero_attributes(true);
        let (Ok(switch), Ok(written)) =
            (SdpCodecs::parse(&sdp), SdpCodecs::parse_with_options(&sdp, &as_written))
        else {
            return Ok(());
        };
        let debug = |c: &SdpCodecs| {
            let options = crate::sdp::CodecStringOptions::default().with_fmtp(true);
            format!(
                "{:?}{:?}{:?}{:?}{:?}",
                c.entries().collect::<Vec<_>>(),
                c.unmapped().collect::<Vec<_>>(),
                c.non_codec_payloads().collect::<Vec<_>>(),
                c.audio_codec_string(&options, Some(&mut Vec::new())).map(|s| s.to_string()),
                c.video_codec_string(&options, Some(&mut Vec::new())).map(|s| s.to_string()),
            )
        };
        prop_assert_eq!(debug(&switch), debug(&written));
        for (a, b) in switch.sections().iter().zip(written.sections()) {
            if a.port() != 0 {
                prop_assert_eq!(format!("{a:?}"), format!("{b:?}"));
            }
        }
    }
}
