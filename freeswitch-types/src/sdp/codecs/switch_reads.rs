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
