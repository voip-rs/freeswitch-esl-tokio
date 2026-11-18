//! sofia's `sdp_parse` reports what it parsed, and refuses or stops where the C says it does.

use freeswitch_c_oracle::{sofia_sdp, SdpParse, SdpSession, SofiaSdp};

const HEADER: &str = "v=0\r\no=- 0 0 IN IP4 192.0.2.1\r\ns=-\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\n";

fn sofia() -> Option<SofiaSdp> {
    sofia_sdp()
        .map_err(|missing| eprintln!("skipped, {missing}"))
        .ok()
}

fn parsed(sofia: SofiaSdp, body: &str) -> SdpSession {
    match sofia.parse(format!("{HEADER}{body}").as_bytes()) {
        SdpParse::Parsed(session) => session,
        SdpParse::Refused(why) => panic!("refused: {}", String::from_utf8_lossy(&why)),
    }
}

#[test]
fn rtpmaps_carry_static_and_named_payloads_and_consume_their_attributes() {
    let Some(sofia) = sofia() else { return };
    let session = parsed(
        sofia,
        "m=audio 5004 RTP/AVP 0 96\r\na=rtpmap:96 opus/48000/2\r\na=fmtp:96 useinbandfec=1\r\na=sendonly\r\na=ptime:20\r\n",
    );
    let [media] = session
        .media
        .as_slice()
    else {
        panic!("one section: {session:?}");
    };
    assert_eq!(media.type_name, b"audio");
    assert_eq!(media.port, 5004);
    assert_eq!(media.mode, 1, "sendonly");
    let pts: Vec<_> = media
        .rtpmaps
        .iter()
        .map(|rm| {
            (
                rm.pt,
                rm.encoding
                    .as_slice(),
                rm.rate,
                rm.predef,
            )
        })
        .collect();
    assert_eq!(
        pts,
        [(0, &b"PCMU"[..], 8000, true), (96, b"opus", 48000, false)]
    );
    assert_eq!(
        media.rtpmaps[1]
            .params
            .as_deref(),
        Some(&b"2"[..])
    );
    assert_eq!(
        media.rtpmaps[1]
            .fmtp
            .as_deref(),
        Some(&b"useinbandfec=1"[..])
    );
    let names: Vec<_> = media
        .attributes
        .iter()
        .map(|a| {
            a.name
                .as_slice()
        })
        .collect();
    assert_eq!(names, [&b"ptime"[..]]);
}

#[test]
fn missing_origin_is_refused() {
    let Some(sofia) = sofia() else { return };
    let SdpParse::Refused(why) =
        sofia.parse(b"v=0\r\ns=-\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\nm=audio 5004 RTP/AVP 0\r\n")
    else {
        panic!("accepted");
    };
    assert_eq!(why, b"No o= present");
}

#[test]
fn a_one_character_line_ends_the_parse_and_drops_later_sections() {
    let Some(sofia) = sofia() else { return };
    let session = parsed(
        sofia,
        "m=audio 5004 RTP/AVP 0\r\nx\r\nm=audio 5006 RTP/AVP 8\r\n",
    );
    assert_eq!(
        session
            .media
            .len(),
        1
    );
}

#[test]
fn a_nul_ends_the_input() {
    let Some(sofia) = sofia() else { return };
    let session = parsed(
        sofia,
        "m=audio 5004 RTP/AVP 0\r\n\0m=audio 5006 RTP/AVP 8\r\n",
    );
    assert_eq!(
        session
            .media
            .len(),
        1
    );
}

#[test]
fn a_port_zero_section_keeps_no_attributes_and_is_inactive() {
    let Some(sofia) = sofia() else { return };
    let session = parsed(
        sofia,
        "m=audio 0 RTP/AVP 96\r\na=rtpmap:96 opus/48000/2\r\na=sendrecv\r\n",
    );
    let media = &session.media[0];
    assert!(media.rejected);
    assert_eq!(media.mode, 0);
    assert!(media
        .attributes
        .is_empty());
    assert_eq!(media.rtpmaps[0].encoding, b"");
}

#[test]
fn a_section_that_is_not_rtp_lists_its_formats() {
    let Some(sofia) = sofia() else { return };
    let session = parsed(sofia, "m=image 5008 udptl t38\r\n");
    assert_eq!(session.media[0].proto_name, b"udptl");
    assert_eq!(session.media[0].formats, [b"t38".to_vec()]);
    assert!(session.media[0]
        .rtpmaps
        .is_empty());
}
