//! What the switch reads of an SDP, case by case, each named for the sofia-sip function that
//! decides it. The differential property in `c_oracle.rs` covers the same ground at random.

use super::testutil::{codec_named, rtp_codec};
use super::SdpCodecs;
use crate::sdp::{SdpDirection, SdpWarning, SwitchRejection};

const HEADER: &str = "v=0\r\no=- 0 0 IN IP4 192.0.2.1\r\ns=-\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\n";

fn parse(body: &str) -> SdpCodecs {
    SdpCodecs::parse(&format!("{HEADER}{body}")).expect("the offer parses")
}

fn names(codecs: &SdpCodecs) -> Vec<String> {
    codecs
        .audio()
        .map(|c| {
            c.name()
                .to_owned()
        })
        .collect()
}

fn rejects(reason: SwitchRejection, offset: Option<usize>) -> SdpWarning {
    SdpWarning::SwitchRejects { reason, offset }
}

fn offset_of(sdp: &str, needle: &str) -> usize {
    sdp.find(needle)
        .expect("the needle is in the fixture")
}

// parse_descs skips a media-level line that is not key=value or has a two-character key.
#[test]
fn junk_inside_a_section_is_skipped_and_later_attributes_still_apply() {
    for junk in ["junk line", "ab=x", "x=y"] {
        let codecs = parse(&format!(
            "m=audio 5004 RTP/AVP 96\r\n{junk}\r\na=rtpmap:96 opus/48000/2\r\nm=audio 5006 RTP/AVP 8\r\n"
        ));
        assert_eq!(names(&codecs), ["opus", "PCMA"], "{junk}");
        assert!(
            codecs
                .warnings()
                .is_empty(),
            "{junk}: {:?}",
            codecs.warnings()
        );
    }
}

// parse_connection, which o= reaches through parse_origin, never checks for trailing data.
#[test]
fn trailing_fields_on_origin_and_connection_are_ignored() {
    let sdp = "v=0\r\no=- 0 0 IN IP4 192.0.2.1 extra\r\ns=-\r\nc=IN IP4 192.0.2.1 extra\r\nt=0 0\r\nm=audio 5004 RTP/AVP 0\r\nc=IN IP4 192.0.2.2 extra\r\n";
    let codecs = SdpCodecs::parse(sdp).unwrap();
    assert_eq!(names(&codecs), ["PCMU"]);
    assert!(
        codecs
            .warnings()
            .is_empty(),
        "{:?}",
        codecs.warnings()
    );
}

// next() ends sofia's parse at a record that is empty or one byte once leading SP/TAB is gone.
#[test]
fn a_short_line_in_a_section_is_where_the_switch_stops_and_parsing_goes_on() {
    for short in ["x", "  ", "\t", " x"] {
        let sdp = format!(
            "{HEADER}m=audio 5004 RTP/AVP 0\r\n{short}\r\na=sendonly\r\nm=audio 5006 RTP/AVP 8\r\n"
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let offset = offset_of(&sdp, &format!("\r\n{short}\r\n")) + 2;
        assert_eq!(
            codecs.warnings(),
            [SdpWarning::SwitchStopsReading { offset }],
            "{short:?}"
        );
        assert_eq!(names(&codecs), ["PCMU", "PCMA"], "{short:?}");
        assert_eq!(codecs.sections()[0].direction(), SdpDirection::SendOnly);

        let seen = SdpCodecs::parse(&sdp[..offset]).unwrap();
        assert_eq!(names(&seen), ["PCMU"], "{short:?}");
        assert_eq!(seen.sections()[0].direction(), SdpDirection::SendRecv);
    }
}

// A short record ends parse_message's loop, so the switch's session has no media.
#[test]
fn a_short_line_in_the_session_part_leaves_the_switch_no_media() {
    let sdp = format!("{HEADER} \r\nm=audio 5004 RTP/AVP 0\r\n");
    let codecs = SdpCodecs::parse(&sdp).unwrap();
    let offset = offset_of(&sdp, " \r\n");
    assert_eq!(
        codecs.warnings(),
        [SdpWarning::SwitchStopsReading { offset }]
    );
    assert_eq!(names(&codecs), ["PCMU"]);
    assert!(SdpCodecs::parse(&sdp[..offset])
        .unwrap()
        .sections()
        .is_empty());
}

// next() returning NULL ends the loop normally, so post_session's sanity check still runs.
#[test]
fn a_whitespace_line_still_leaves_the_sanity_check_to_run() {
    let sdp = "v=0\r\ns=-\r\nc=IN IP4 192.0.2.1\r\n \r\nm=audio 5004 RTP/AVP 0\r\n";
    let codecs = SdpCodecs::parse(sdp).unwrap();
    assert_eq!(
        codecs.warnings(),
        [
            SdpWarning::SwitchStopsReading {
                offset: offset_of(sdp, " \r\n")
            },
            rejects(SwitchRejection::MissingOrigin, None),
        ]
    );
}

// next() stops only on what follows; trailing whitespace and a two-character record do not.
#[test]
fn trailing_whitespace_and_a_two_character_line_are_not_a_stop() {
    let codecs = parse("m=audio 5004 RTP/AVP 0\r\nx \r\n  \r\n");
    assert!(
        codecs
            .warnings()
            .is_empty(),
        "{:?}",
        codecs.warnings()
    );
}

// FreeSWITCH hands sdp_parse strlen(sdp), so a NUL ends the input.
#[test]
fn a_nul_is_where_the_switch_stops() {
    let sdp = format!("{HEADER}m=audio 5004 RTP/AVP 0\r\n\0m=audio 5006 RTP/AVP 8\r\n");
    let codecs = SdpCodecs::parse(&sdp).unwrap();
    let offset = offset_of(&sdp, "\0");
    assert_eq!(
        codecs.warnings(),
        [SdpWarning::SwitchStopsReading { offset }]
    );
    assert_eq!(names(&codecs), ["PCMU", "PCMA"]);
}

// next() splits records on any run of CR or LF bytes, so a bare CR ends a line.
#[test]
fn a_bare_cr_ends_a_line() {
    let codecs = parse("m=audio 5004 RTP/AVP 96\r\na=rtpmap:96 opus/48000/2\ra=ptime:40\r\n");
    let rtp = rtp_codec(codecs.entries());
    assert_eq!(codec_named(&rtp, "opus").and_then(|c| c.ptime()), Some(40));

    let codecs = parse("m=audio 5004 RTP/AVP 0\r\na=sendrecv\rm=audio 5006 RTP/AVP 8\r\n");
    assert_eq!(names(&codecs), ["PCMU", "PCMA"]);
    assert!(
        codecs
            .warnings()
            .is_empty(),
        "{:?}",
        codecs.warnings()
    );
}

// parse_message refuses a session-level line that is not key=value.
#[test]
fn a_bare_cr_with_a_junk_tail_in_the_session_part_is_refused() {
    let sdp = format!("{HEADER}a=sendonly\rjunk\r\nm=audio 5004 RTP/AVP 0\r\n");
    let codecs = SdpCodecs::parse(&sdp).unwrap();
    assert_eq!(
        codecs.warnings(),
        [rejects(
            SwitchRejection::BadLine,
            Some(offset_of(&sdp, "junk"))
        )]
    );
    assert_eq!(names(&codecs), ["PCMU"]);
}

// parse_message refuses what its field switch rejects; the first refusal ends sofia's parse.
#[test]
fn session_level_refusals_ride_as_warnings() {
    let cases = [
        ("junk line\r\n", SwitchRejection::BadLine, "junk line"),
        ("ab=x\r\n", SwitchRejection::BadLine, "ab=x"),
        ("x=y\r\n", SwitchRejection::UnknownField, "x=y"),
        ("a=\r\n", SwitchRejection::InvalidAttributeName, "a=\r"),
    ];
    for (line, reason, needle) in cases {
        let sdp = format!("{HEADER}{line}x=z\r\nm=audio 5004 RTP/AVP 0\r\n");
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(
            codecs.warnings(),
            [rejects(reason, Some(offset_of(&sdp, needle)))],
            "{line:?}"
        );
        assert_eq!(names(&codecs), ["PCMU"], "{line:?}");
    }
}

// parse_message refuses r= and z= with no t= before them.
#[test]
fn repeat_and_zone_without_time_are_refused() {
    let sdp = "v=0\r\no=- 0 0 IN IP4 192.0.2.1\r\ns=-\r\nc=IN IP4 192.0.2.1\r\nr=7d 1h 0 25h\r\nt=0 0\r\nm=audio 5004 RTP/AVP 0\r\n";
    let codecs = SdpCodecs::parse(sdp).unwrap();
    assert_eq!(
        codecs.warnings(),
        [rejects(
            SwitchRejection::RepeatWithoutTime,
            Some(offset_of(sdp, "r="))
        )]
    );
    let sdp = format!("{HEADER}z=2882844526 -1h\r\nz=2882844526 -1h\r\nm=audio 5004 RTP/AVP 0\r\n");
    let codecs = SdpCodecs::parse(&sdp).unwrap();
    let second = offset_of(&sdp, "z=") + "z=2882844526 -1h\r\n".len();
    assert_eq!(
        codecs.warnings(),
        [rejects(SwitchRejection::ZoneWithoutTime, Some(second))]
    );
}

// sdp_sanity_check needs o=, and c= at session level or on every section with a nonzero port.
#[test]
fn missing_origin_and_connection_are_refused() {
    let codecs =
        SdpCodecs::parse("v=0\r\ns=-\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\nm=audio 5004 RTP/AVP 0\r\n")
            .unwrap();
    assert_eq!(
        codecs.warnings(),
        [rejects(SwitchRejection::MissingOrigin, None)]
    );

    let sdp = "v=0\r\no=- 0 0 IN IP4 192.0.2.1\r\ns=-\r\nt=0 0\r\nm=audio 0 RTP/AVP 0\r\nm=audio 5004 RTP/AVP 8\r\n";
    let codecs = SdpCodecs::parse(sdp).unwrap();
    assert_eq!(
        codecs.warnings(),
        [rejects(
            SwitchRejection::MissingConnection,
            Some(offset_of(sdp, "m=audio 5004"))
        )]
    );
}

// A one-character session record returns from parse_message before post_session, so neither a
// later line nor the missing o= is refused.
#[test]
fn nothing_after_the_stop_is_refused() {
    let sdp = "v=0\r\ns=-\r\nx\r\nx=y\r\nm=audio 5004 RTP/AVP 0\r\n";
    let codecs = SdpCodecs::parse(sdp).unwrap();
    assert_eq!(
        codecs.warnings(),
        [SdpWarning::SwitchStopsReading {
            offset: offset_of(sdp, "x\r\n")
        }]
    );
}

// next() strips leading SP/TAB from every record.
#[test]
fn leading_whitespace_on_a_line_is_ignored() {
    let codecs = parse("m=audio 5004 RTP/AVP 0\r\n  \ta=sendonly\r\n");
    assert_eq!(codecs.sections()[0].direction(), SdpDirection::SendOnly);
    assert!(SdpCodecs::parse(&format!(" {HEADER}m=audio 5004 RTP/AVP 0\r\n")).is_ok());
}

// parse_message requires the first record to be exactly v=0.
#[test]
fn a_first_line_other_than_v0_is_not_sdp() {
    assert!(SdpCodecs::parse("v=0 \r\nm=audio 5004 RTP/AVP 0\r\n").is_err());
    assert!(SdpCodecs::parse("v=1\r\nm=audio 5004 RTP/AVP 0\r\n").is_err());
    assert!(SdpCodecs::parse("").is_err());
}

// token() ends an attribute name at its first non-token byte and consumes that byte.
#[test]
fn an_attribute_name_ends_at_its_first_non_token_byte() {
    let codecs = parse("m=audio 5004 RTP/AVP 96\r\na=rtpmap 96 PCMU/8000\r\n");
    assert_eq!(names(&codecs), ["PCMU"]);
    let codecs = parse("m=audio 5004 RTP/AVP 0\r\na=:x\r\n");
    assert_eq!(
        codecs
            .warnings()
            .len(),
        1
    );
    assert!(matches!(
        codecs.warnings()[0],
        SdpWarning::SwitchRejects {
            reason: SwitchRejection::InvalidAttributeName,
            ..
        }
    ));
}

// parse_media reads an m= line with no format list, and RTP formats below 128 only.
#[test]
fn the_format_list_is_read_as_parse_media_reads_it() {
    let codecs = parse("m=audio 5004 RTP/AVP\r\n");
    assert_eq!(
        codecs
            .sections()
            .len(),
        1
    );
    assert!(
        codecs
            .warnings()
            .is_empty(),
        "{:?}",
        codecs.warnings()
    );

    let sdp = format!("{HEADER}m=audio 5004 RTP/AVP 0 128 8\r\n");
    let codecs = SdpCodecs::parse(&sdp).unwrap();
    assert_eq!(
        codecs.warnings(),
        [rejects(
            SwitchRejection::InvalidRtpFormat,
            Some(offset_of(&sdp, "m="))
        )]
    );
    assert_eq!(names(&codecs), ["PCMU", "PCMA"]);

    let codecs = parse("m=audio 5004 RTP 0\r\n");
    assert_eq!(names(&codecs), ["PCMU"], "bare RTP is RTP/AVP");
}

// parse_message overwrites a repeated o=, s= or c= rather than refusing it.
#[test]
fn a_repeated_session_field_is_not_refused() {
    let codecs =
        parse("o=- 1 1 IN IP4 192.0.2.3\r\nc=IN IP4 192.0.2.3\r\nm=audio 5004 RTP/AVP 0\r\n");
    assert!(
        codecs
            .warnings()
            .is_empty(),
        "{:?}",
        codecs.warnings()
    );
}

// parse_payload names a static payload type from sdp_rtpmap_well_known, which RFC 3551 does not
// cover alone: 1, 2 and 19 are assigned there.
#[test]
fn static_payload_types_carry_sofias_well_known_names() {
    let codecs = parse("m=audio 5004 RTP/AVP 1 2 19\r\n");
    assert_eq!(names(&codecs), ["1016", "G721"]);
    assert_eq!(
        codecs
            .unmapped()
            .count(),
        0
    );
    let cn: Vec<_> = codecs
        .non_codec_payloads()
        .map(|n| (n.kind, n.payload_type, n.clock_rate))
        .collect();
    assert_eq!(cn, [(crate::sdp::NonCodecKind::ComfortNoise, 19, 8000)]);
}

// parse_rtpmap and parse_fmtp bind to the first m= entry carrying the payload type.
#[test]
fn a_repeated_payload_type_takes_its_attributes_on_the_first_entry_only() {
    let codecs =
        parse("m=audio 5004 RTP/AVP 9 9\r\na=rtpmap:9 opus/8000\r\na=fmtp:9 useinbandfec=1\r\n");
    let named: Vec<_> = codecs
        .audio()
        .map(|c| (c.name(), c.fmtp()))
        .collect();
    assert_eq!(named, [("opus", Some("useinbandfec=1")), ("G722", None)]);

    let codecs = parse("m=audio 5004 RTP/AVP 96 96\r\na=rtpmap:96 opus/48000/2\r\n");
    assert_eq!(names(&codecs), ["opus"]);
    assert_eq!(
        codecs
            .unmapped()
            .map(|u| u.payload_type)
            .collect::<Vec<_>>(),
        [96]
    );
}

// sdp_media_type matches the media type case-insensitively.
#[test]
fn a_media_type_matches_in_any_case() {
    let codecs = parse("m=AUDIO 5004 RTP/AVP 0\r\nm=Video 5006 RTP/AVP 34\r\n");
    let types: Vec<_> = codecs
        .sections()
        .iter()
        .map(|s| {
            s.media_type()
                .clone()
        })
        .collect();
    assert_eq!(
        types,
        [
            crate::sdp::SdpMediaType::Audio,
            crate::sdp::SdpMediaType::Video
        ]
    );
    assert_eq!(names(&codecs), ["PCMU"]);
    assert_eq!(
        codecs
            .video()
            .count(),
        1
    );
}

// parse_rtpmap and parse_fmtp drop an attribute whose payload type parse_ul cannot read below 128.
#[test]
fn a_payload_attribute_with_an_unreadable_type_is_dropped_alone() {
    let sdp = format!(
        "{HEADER}m=audio 5004 RTP/AVP 0 96\r\na=rtpmap:x PCMU/8000\r\na=rtpmap:96 opus/48000/2\r\na=fmtp:128 foo\r\na=fmtp:96 useinbandfec=1\r\n"
    );
    let codecs = SdpCodecs::parse(&sdp).unwrap();
    assert_eq!(names(&codecs), ["PCMU", "opus"]);
    assert_eq!(codecs.fmtp_for("opus", 48000), Some("useinbandfec=1"));
    assert_eq!(
        codecs.warnings(),
        [
            SdpWarning::PayloadAttributeIgnored {
                offset: offset_of(&sdp, "a=rtpmap:x")
            },
            SdpWarning::PayloadAttributeIgnored {
                offset: offset_of(&sdp, "a=fmtp:128")
            },
        ]
    );
}

// parse_rtpmap refuses the offer when no clock rate follows the encoding of a listed payload type.
#[test]
fn an_rtpmap_without_a_readable_rate_is_refused() {
    for (line, needle) in [
        ("a=rtpmap:96 opus\r\n", "a=rtpmap:96"),
        ("a=rtpmap:96 opus\r/48000/2\r\n", "a=rtpmap:96"),
        ("a=rtpmap:96 bad,name/8000\r\n", "a=rtpmap:96"),
        ("a=rtpmap:96 opus/abc\r\n", "a=rtpmap:96"),
    ] {
        let sdp = format!("{HEADER}m=audio 5004 RTP/AVP 0 96\r\n{line}");
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(
            codecs.warnings(),
            [rejects(
                SwitchRejection::InvalidRtpmap,
                Some(offset_of(&sdp, needle))
            )],
            "{line:?}"
        );
        assert_eq!(names(&codecs), ["PCMU"], "{line:?}");
        assert_eq!(
            codecs
                .unmapped()
                .map(|u| u.payload_type)
                .collect::<Vec<_>>(),
            [96],
            "{line:?}"
        );
    }
}

// parse_rtpmap checks the rate only for a payload type the m= line lists, on a live section.
#[test]
fn an_unlisted_or_held_rtpmap_is_not_refused() {
    let codecs = parse("m=audio 5004 RTP/AVP 0\r\na=rtpmap:97 opus\r\n");
    assert!(
        codecs
            .warnings()
            .is_empty(),
        "{:?}",
        codecs.warnings()
    );
    let codecs = parse("m=audio 0 RTP/AVP 96\r\na=rtpmap:96 opus\r\n");
    assert!(
        !codecs
            .warnings()
            .iter()
            .any(|w| matches!(w, SdpWarning::SwitchRejects { .. })),
        "{:?}",
        codecs.warnings()
    );
}

// parse_rtpmap ignores what follows the rate, and the switch reads the channels with atoi.
#[test]
fn an_rtpmap_reads_as_far_as_its_rate_and_channels() {
    let codecs = parse(
        "m=audio 5004 RTP/AVP 96 97\r\na=rtpmap:96 opus/48000/2 extra\r\na=rtpmap:97 L16/8000/\r\n",
    );
    assert!(
        codecs
            .warnings()
            .is_empty(),
        "{:?}",
        codecs.warnings()
    );
    let channels: Vec<_> = codecs
        .audio()
        .map(|c| (c.name(), c.channels()))
        .collect();
    assert_eq!(channels, [("opus", Some(2)), ("L16", Some(0))]);
}

// parse_media_attr and parse_session_attr match attribute names with su_casematch, and each
// direction attribute overwrites the mode the one before it set.
#[test]
fn attribute_names_match_in_any_case_and_the_last_direction_wins() {
    let codecs = parse(
        "a=RECVONLY\r\nm=audio 5004 RTP/AVP 96\r\na=RTPMAP:96 opus/48000/2\r\na=FMTP:96 useinbandfec=1\r\na=PTIME:40\r\nm=audio 5006 RTP/AVP 0\r\na=SendOnly\r\na=inactive\r\na=sendrecv\r\n",
    );
    let rtp = rtp_codec(codecs.entries());
    let opus = codec_named(&rtp, "opus").expect("RTPMAP names pt 96");
    assert_eq!(opus.fmtp(), Some("useinbandfec=1"));
    assert_eq!(opus.ptime(), Some(40));
    let directions: Vec<_> = codecs
        .sections()
        .iter()
        .map(|s| s.direction())
        .collect();
    assert_eq!(directions, [SdpDirection::RecvOnly, SdpDirection::SendRecv]);
    assert!(
        codecs
            .warnings()
            .is_empty(),
        "{:?}",
        codecs.warnings()
    );
}

// parse_media_attr returns before reading any attribute of a port-0 section, and post_session
// forces it inactive.
#[test]
fn a_port_zero_section_carries_only_what_the_switch_reads_there() {
    let codecs = parse(
        "m=audio 0 RTP/AVP 0 96 101\r\na=rtpmap:96 opus/48000/2\r\na=rtpmap:101 telephone-event/8000\r\na=rtpmap:x junk\r\na=sendrecv\r\n",
    );
    let section = &codecs.sections()[0];
    let names: Vec<_> = rtp_codec(section.entries())
        .iter()
        .map(|c| {
            c.name()
                .to_owned()
        })
        .collect();
    assert_eq!(names, ["PCMU"]);
    let unmapped: Vec<_> = section
        .unmapped()
        .iter()
        .map(|u| u.payload_type)
        .collect();
    assert_eq!(unmapped, [96, 101]);
    assert!(section
        .non_codec_payloads()
        .is_empty());
    assert_eq!(section.direction(), SdpDirection::Inactive);
    assert!(
        codecs
            .warnings()
            .is_empty(),
        "{:?}",
        codecs.warnings()
    );
}
