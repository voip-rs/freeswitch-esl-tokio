//! Integration tests against a live FreeSWITCH instance: event subscription
//! and heartbeat, the sendevent family, noevents/nixevent/filter, repeating
//! SIP header round trips, multi-command api, reply status, and bgapi.
//!
//! These tests require a live FreeSWITCH ESL; see docs/live-test-switch.md.
//! Run with: cargo test --test 'live_*' -- --ignored

mod live_common;

use freeswitch_esl_tokio::{
    EslCommand, EslError, EslEvent, EslEventPriority, EslEventType, EventFormat, EventHeader,
    EventSubscription, HeaderLookup, ReplyStatus,
};
use live_common::{
    connect, custom_roundtrip, custom_roundtrip_as, percent_escape_tree, switch_version,
    PercentEscape,
};
use std::time::Duration;
use tokio::time::Instant;

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_subscribe_and_recv_heartbeat() {
    let (client, mut events, _permit) = connect().await;

    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::Heartbeat])
        .await
        .unwrap();

    let event = tokio::time::timeout(Duration::from_secs(25), events.recv())
        .await
        .expect("timeout waiting for heartbeat")
        .expect("channel closed")
        .expect("event error");

    assert_eq!(event.event_type(), Some(EslEventType::Heartbeat));
    assert!(event
        .header(EventHeader::CoreUuid)
        .is_some());
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_sendevent_with_priority() {
    let (client, _events, _permit) = connect().await;

    let mut event = EslEvent::with_type(EslEventType::Custom);
    event.set_header("Event-Name", "CUSTOM");
    event.set_header("Event-Subclass", "esl_test::priority");
    event.set_priority(EslEventPriority::High);

    let resp = client
        .sendevent(event)
        .await
        .unwrap();
    assert!(
        resp.is_success(),
        "sendevent failed: {:?}",
        resp.reply_text()
    );
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_sendevent_with_array_header() {
    let (client, _events, _permit) = connect().await;

    let mut event = EslEvent::with_type(EslEventType::Custom);
    event.set_header("Event-Name", "CUSTOM");
    event.set_header("Event-Subclass", "esl_test::array");
    event
        .push_header("X-Test-Array", "value1")
        .unwrap();
    event
        .push_header("X-Test-Array", "value2")
        .unwrap();
    event
        .push_header("X-Test-Array", "value3")
        .unwrap();

    assert_eq!(
        event.header_str("X-Test-Array"),
        Some("ARRAY::value1|:value2|:value3")
    );

    let resp = client
        .sendevent(event)
        .await
        .unwrap();
    assert!(
        resp.is_success(),
        "sendevent failed: {:?}",
        resp.reply_text()
    );
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_recv_custom_sendevent() {
    let (client, mut events, _permit) = connect().await;

    let evt = custom_roundtrip(
        &client,
        &mut events,
        &[("X-Test-Data", "hello"), ("X-Test-Data", "world")],
    )
    .await;

    assert_eq!(evt.header(EventHeader::Priority), Some("NORMAL"));
    assert_eq!(evt.header_str("X-Test-Data"), Some("ARRAY::hello|:world"));
}

/// End-to-end percent-decode against real FreeSWITCH: a header value with
/// characters FS percent-encodes (space, `@`, and a multibyte UTF-8 char)
/// must arrive decoded. Events delivered to ESL go through
/// `switch_event_serialize(SWITCH_TRUE)`, so the value is `%20`/`%40`/`%C3%A9`
/// on the wire and the parser must reconstruct the original. The lossy path
/// (genuinely non-UTF-8 bytes) can't be exercised here because a Rust `&str`
/// is always valid UTF-8 -- it stays unit-tested.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_recv_custom_sendevent_percent_decoded() {
    let (client, mut events, _permit) = connect().await;

    let value = "Jean Dupont@héllo";
    let evt = custom_roundtrip(&client, &mut events, &[("X-Decode-Test", value)]).await;

    assert_eq!(
        evt.header_str("X-Decode-Test"),
        Some(value),
        "FS percent-encodes the value on the wire; the parser must decode it"
    );
    assert!(
        evt.lossy_values()
            .is_empty(),
        "valid UTF-8 is not lossy"
    );
}

/// `switch_event_serialize` encodes each value with `switch_url_encode`. Upstream copies a `%`
/// opening a valid `%XX` through unescaped, so such a value decodes to something the switch never
/// held; the 1.10.13 fork writes every `%` as `%25` and the value survives. A JSON event
/// escapes nothing by URL and carries the value intact on either.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_event_value_with_a_valid_percent_escape_by_tree() {
    let (client, mut events, _permit) = connect().await;
    let version = switch_version(&client).await;

    let sent = "a%41b c%zz";
    let plain = custom_roundtrip(&client, &mut events, &[("X-Percent-Test", sent)]).await;
    let json = custom_roundtrip_as(
        &client,
        &mut events,
        EventFormat::Json,
        &[("X-Percent-Test", sent)],
    )
    .await;

    let want = match percent_escape_tree(version) {
        PercentEscape::KeepsValidEscapes => "aAb c%zz",
        PercentEscape::EncodesEveryPercent => sent,
    };
    assert_eq!(
        plain.header_str("X-Percent-Test"),
        Some(want),
        "plain, FreeSWITCH {version}"
    );
    assert_eq!(
        json.header_str("X-Percent-Test"),
        Some(sent),
        "json, FreeSWITCH {version}"
    );
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_api_multiple_commands() {
    let (client, _events, _permit) = connect().await;

    for command in ["version", "hostname", "global_getvar"] {
        let resp = client
            .api(command)
            .await
            .unwrap_or_else(|e| panic!("{command}: transport error: {e}"));
        assert!(
            resp.body()
                .is_some(),
            "{command} should have a body"
        );
    }
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_reply_status_ok() {
    let (client, _events, _permit) = connect().await;

    // subscribe_events uses send_command_ok → into_result(), so Ok means +OK
    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::Heartbeat])
        .await
        .expect("subscribe should return +OK");
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_reply_status_err() {
    let (client, _events, _permit) = connect().await;

    // divert_events needs a session, so inbound gets -ERR; send_command keeps
    // the raw EslResponse that divert_events() would turn into an Err.
    let resp = client
        .send_command(EslCommand::DivertEvents { on: true })
        .await
        .expect("send_command should not fail at transport level");

    assert_eq!(
        resp.reply_status(),
        ReplyStatus::Err,
        "expected -ERR reply, got: {:?}",
        resp.reply_text()
    );
    assert!(
        resp.reply_text()
            .unwrap_or("")
            .starts_with("-ERR"),
        "reply text should start with -ERR: {:?}",
        resp.reply_text()
    );

    // into_result() should convert to CommandFailed
    let err = resp
        .into_result()
        .unwrap_err();
    assert!(
        matches!(err, EslError::CommandFailed { .. }),
        "expected CommandFailed, got: {:?}",
        err
    );
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_noevents_stops_delivery() {
    let (client, mut events, _permit) = connect().await;
    let subclass = format!("esl_test::noev_{}", std::process::id());

    client
        .subscribe_events_raw(EventFormat::Plain, &format!("CUSTOM {}", subclass))
        .await
        .unwrap()
        .into_result()
        .unwrap();

    // Fire event and confirm delivery
    let mut evt1 = EslEvent::with_type(EslEventType::Custom);
    evt1.set_header("Event-Name", "CUSTOM");
    evt1.set_header("Event-Subclass", subclass.clone());
    evt1.set_header("X-Phase", "before");
    client
        .sendevent(evt1)
        .await
        .unwrap()
        .check()
        .expect("sendevent rejected");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt)))
                if evt.header(EventHeader::EventSubclass) == Some(subclass.as_str()) =>
            {
                break
            }
            Ok(Some(Ok(_))) => continue,
            other => panic!("expected custom event before noevents: {:?}", other),
        }
    }

    // Unsubscribe from all events
    client
        .noevents()
        .await
        .unwrap();

    // Fire another event — should not arrive
    let mut evt2 = EslEvent::with_type(EslEventType::Custom);
    evt2.set_header("Event-Name", "CUSTOM");
    evt2.set_header("Event-Subclass", subclass.clone());
    evt2.set_header("X-Phase", "after");
    client
        .sendevent(evt2)
        .await
        .unwrap()
        .check()
        .expect("sendevent rejected");

    match tokio::time::timeout(Duration::from_secs(2), events.recv()).await {
        Err(_) => {} // timeout — correct
        Ok(Some(Ok(evt))) => panic!(
            "received event after noevents: {:?} phase={}",
            evt.event_type(),
            evt.header_str("X-Phase")
                .unwrap_or("?")
        ),
        Ok(Some(Err(e))) => panic!("event error: {}", e),
        Ok(None) => {}
    }
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_nixevent_selective_unsubscribe() {
    let (client, mut events, _permit) = connect().await;
    let subclass = format!("esl_test::nix_{}", std::process::id());

    // Subscribe to both HEARTBEAT and CUSTOM
    client
        .subscribe_events_raw(
            EventFormat::Plain,
            &format!("HEARTBEAT CUSTOM {}", subclass),
        )
        .await
        .unwrap()
        .into_result()
        .unwrap();

    // Unsubscribe only HEARTBEAT
    client
        .nixevent(&[EslEventType::Heartbeat])
        .await
        .unwrap();

    // Send a custom event — should still arrive
    let mut event = EslEvent::with_type(EslEventType::Custom);
    event.set_header("Event-Name", "CUSTOM");
    event.set_header("Event-Subclass", subclass.clone());
    client
        .sendevent(event)
        .await
        .unwrap()
        .check()
        .expect("sendevent rejected");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => {
                assert_ne!(
                    evt.event_type(),
                    Some(EslEventType::Heartbeat),
                    "received HEARTBEAT after nixevent"
                );
                if evt.header(EventHeader::EventSubclass) == Some(subclass.as_str()) {
                    return; // custom event delivered — nixevent was selective
                }
            }
            Ok(Some(Err(e))) => panic!("event error: {}", e),
            Ok(None) => panic!("event stream closed"),
            Err(_) => break,
        }
    }
    panic!("did not receive custom event after nixevent HEARTBEAT");
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_api_err_body() {
    let (client, _events, _permit) = connect().await;

    // api with a non-existent command returns -ERR in the body
    let resp = client
        .api("nonexistent_command_xyz")
        .await
        .unwrap();
    let err = resp
        .api_result()
        .unwrap_err();
    assert!(
        matches!(err, EslError::CommandFailed { .. }),
        "expected CommandFailed, got: {}",
        err
    );
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_sendevent_comma_separated_sip_header() {
    let (client, mut events, _permit) = connect().await;

    // RFC 3325 comma-separated format: two identities in one header value.
    let evt = custom_roundtrip(
        &client,
        &mut events,
        &[(
            "variable_sip_P-Asserted-Identity",
            "<sip:alice@atlanta.example.com>, <tel:+15551234567>",
        )],
    )
    .await;

    assert_eq!(
        evt.variable_str("sip_P-Asserted-Identity"),
        Some("<sip:alice@atlanta.example.com>, <tel:+15551234567>"),
        "comma-separated P-Asserted-Identity should survive round-trip"
    );
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_sendevent_array_sip_header() {
    use freeswitch_types::EslArray;

    let (client, mut events, _permit) = connect().await;

    // ARRAY format: repeating SIP header stored as separate values.
    let evt = custom_roundtrip(
        &client,
        &mut events,
        &[
            (
                "variable_sip_P-Asserted-Identity",
                "<sip:alice@atlanta.example.com>",
            ),
            ("variable_sip_P-Asserted-Identity", "<tel:+15551234567>"),
        ],
    )
    .await;

    let raw = evt
        .variable_str("sip_P-Asserted-Identity")
        .expect("P-Asserted-Identity should be present");
    let arr = EslArray::parse(raw).expect("should parse as ARRAY");
    assert_eq!(arr.len(), 2, "expected 2 identities in ARRAY");
    assert_eq!(arr.items()[0], "<sip:alice@atlanta.example.com>");
    assert_eq!(arr.items()[1], "<tel:+15551234567>");
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_sendevent_repeated_diversion_header() {
    use freeswitch_types::EslArray;

    let (client, mut events, _permit) = connect().await;

    // SIP Diversion header (RFC 5806) with history info containing URI params.
    let evt = custom_roundtrip(
        &client,
        &mut events,
        &[
            (
                "variable_sip_h_Diversion",
                "<sip:+15551234567@gw.example.com;reason=unconditional>",
            ),
            (
                "variable_sip_h_Diversion",
                "<sip:+15559876543@proxy.example.com;reason=no-answer;counter=3>",
            ),
        ],
    )
    .await;

    let raw = evt
        .variable_str("sip_h_Diversion")
        .expect("Diversion variable should be present");
    let arr = EslArray::parse(raw).expect("should parse as ARRAY");
    assert_eq!(arr.len(), 2, "expected 2 Diversion entries");
    assert_eq!(
        arr.items()[0],
        "<sip:+15551234567@gw.example.com;reason=unconditional>"
    );
    assert_eq!(
        arr.items()[1],
        "<sip:+15559876543@proxy.example.com;reason=no-answer;counter=3>"
    );
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_filter_event_name() {
    let (client, mut events, _permit) = connect().await;

    // Filter before subscribing: the switch is shared, so any window between the
    // two queues a parallel test's BACKGROUND_JOB on this listener.
    client
        .filter(EventHeader::EventName, "HEARTBEAT")
        .await
        .unwrap();

    client
        .subscribe_events(
            EventFormat::Plain,
            &[EslEventType::Heartbeat, EslEventType::BackgroundJob],
        )
        .await
        .unwrap();

    // Fire a bgapi to generate a BACKGROUND_JOB event
    client
        .bgapi("status")
        .await
        .unwrap()
        .check()
        .expect("bgapi rejected");

    // We should only see HEARTBEAT, not BACKGROUND_JOB
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut got_heartbeat = false;
    while Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => {
                assert_ne!(
                    evt.event_type(),
                    Some(EslEventType::BackgroundJob),
                    "BACKGROUND_JOB should have been filtered out"
                );
                if evt.event_type() == Some(EslEventType::Heartbeat) {
                    got_heartbeat = true;
                    break;
                }
            }
            Ok(Some(Err(e))) => panic!("event error: {}", e),
            Ok(None) => panic!("connection closed"),
            Err(_) => break,
        }
    }
    assert!(
        got_heartbeat,
        "should have received HEARTBEAT through filter"
    );

    // Delete filter, verify BACKGROUND_JOB now arrives
    client
        .filter_delete(EventHeader::EventName, Some("HEARTBEAT"))
        .await
        .unwrap();

    client
        .bgapi("status")
        .await
        .unwrap()
        .check()
        .expect("bgapi rejected");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => {
                if evt.event_type() == Some(EslEventType::BackgroundJob) {
                    return; // filter successfully removed
                }
            }
            Ok(Some(Err(e))) => panic!("event error: {}", e),
            Ok(None) => panic!("connection closed"),
            Err(_) => break,
        }
    }
    panic!("BACKGROUND_JOB not received after filter_delete");
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_sendevent_returns_event_uuid() {
    let (client, _events, _permit) = connect().await;

    let mut event = EslEvent::with_type(EslEventType::Custom);
    event.set_header("Event-Name", "CUSTOM");
    event.set_header("Event-Subclass", "esl_test::uuid_check");

    let resp = client
        .sendevent(event)
        .await
        .unwrap();
    assert!(resp.is_success());

    let uuid = resp.event_uuid();
    assert!(
        uuid.is_some(),
        "sendevent should return event UUID in +OK reply, got: {:?}",
        resp.reply_text()
    );
    // UUID should look like a UUID (36 chars with dashes)
    let uuid = uuid.unwrap();
    assert!(
        uuid.len() >= 36,
        "event UUID should be at least 36 chars: {}",
        uuid
    );
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_bgapi_correlation() {
    let (client, mut events, _permit) = connect().await;

    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::BackgroundJob])
        .await
        .unwrap();

    // Send multiple bgapi commands and collect their Job-UUIDs
    let resp1 = client
        .bgapi("status")
        .await
        .unwrap();
    let job1 = resp1
        .job_uuid()
        .expect("bgapi should return Job-UUID")
        .to_string();

    let resp2 = client
        .bgapi("version")
        .await
        .unwrap();
    let job2 = resp2
        .job_uuid()
        .expect("bgapi should return Job-UUID")
        .to_string();

    let resp3 = client
        .bgapi("hostname")
        .await
        .unwrap();
    let job3 = resp3
        .job_uuid()
        .expect("bgapi should return Job-UUID")
        .to_string();

    assert_ne!(job1, job2, "Job-UUIDs should be unique");
    assert_ne!(job2, job3, "Job-UUIDs should be unique");

    // Collect BACKGROUND_JOB events and match them to our Job-UUIDs
    let expected: std::collections::HashSet<String> = [job1.clone(), job2.clone(), job3.clone()]
        .into_iter()
        .collect();
    let mut matched = std::collections::HashSet::new();

    let deadline = Instant::now() + Duration::from_secs(10);
    while matched.len() < 3 && Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => {
                if evt.event_type() == Some(EslEventType::BackgroundJob) {
                    if let Some(job_uuid) = evt.job_uuid() {
                        if expected.contains(job_uuid) {
                            assert!(
                                evt.body()
                                    .is_some(),
                                "BACKGROUND_JOB for {} should have body",
                                job_uuid
                            );
                            matched.insert(job_uuid.to_string());
                        }
                    }
                }
            }
            Ok(Some(Err(e))) => panic!("event error: {}", e),
            Ok(None) => panic!("connection closed"),
            Err(_) => break,
        }
    }

    assert_eq!(
        matched.len(),
        3,
        "should match all 3 bgapi jobs, matched: {:?}, expected: {:?}",
        matched,
        expected
    );
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_bgapi_single_round_trip() {
    let (client, mut events, _permit) = connect().await;

    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::BackgroundJob])
        .await
        .unwrap();

    let resp = client
        .bgapi("status")
        .await
        .unwrap();
    let job_uuid = resp
        .job_uuid()
        .expect("bgapi should return Job-UUID")
        .to_string();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => {
                if evt.event_type() == Some(EslEventType::BackgroundJob)
                    && evt.job_uuid() == Some(job_uuid.as_str())
                {
                    let body = evt
                        .body()
                        .expect("BACKGROUND_JOB should have a body");
                    assert!(!body.is_empty(), "body should contain status output");
                    return;
                }
            }
            Ok(Some(Err(e))) => panic!("event error: {}", e),
            Ok(None) => panic!("connection closed"),
            Err(_) => panic!("timeout waiting for BACKGROUND_JOB {}", job_uuid),
        }
    }
}

/// The reported ordering defect, end to end: `Custom` listed first with another
/// event type after it. Before the fix the wire read
/// `event plain CUSTOM HEARTBEAT <subclass>`, FreeSWITCH registered `HEARTBEAT`
/// as a subclass name, and the heartbeat never arrived while the `+OK` and the
/// custom event both said the subscription was healthy.
///
/// `Heartbeat` is the second type because the switch emits it unprompted -- no
/// synthetic channel event injected into a switch shared with parallel tests,
/// and nothing to reap. Both halves are asserted: the reorder must not cost the
/// `CUSTOM` subscription it is protecting.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_custom_first_does_not_swallow_later_event_types() {
    let (client, mut events, _permit) = connect().await;
    let subclass = format!("esl_test::ordering_{}", std::process::id());

    let sub = EventSubscription::new(EventFormat::Plain)
        .event(EslEventType::Custom)
        .event(EslEventType::Heartbeat)
        .custom_subclass(subclass.as_str())
        .unwrap();
    client
        .apply_subscription(&sub)
        .await
        .unwrap();

    let mut event = EslEvent::with_type(EslEventType::Custom);
    event.set_header("Event-Name", "CUSTOM");
    event.set_header("Event-Subclass", subclass.clone());
    client
        .sendevent(event)
        .await
        .unwrap()
        .check()
        .expect("sendevent rejected");

    // event-heartbeat-interval defaults to 20s; both halves land inside 25.
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut got_custom = false;
    let mut got_heartbeat = false;
    while !(got_custom && got_heartbeat) {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => {
                if evt.header(EventHeader::EventSubclass) == Some(subclass.as_str()) {
                    got_custom = true;
                } else if evt.event_type() == Some(EslEventType::Heartbeat) {
                    got_heartbeat = true;
                }
            }
            Ok(Some(Err(e))) => panic!("event error: {}", e),
            Ok(None) => panic!("event stream closed"),
            Err(_) => break,
        }
    }

    assert!(got_custom, "CUSTOM {} never arrived", subclass);
    assert!(
        got_heartbeat,
        "HEARTBEAT never arrived -- swallowed as a CUSTOM subclass name"
    );
}

/// A bare `CUSTOM` subscribes to nothing, contrary to the obvious reading.
/// mod_event_socket delivers a CUSTOM event only when its subclass is in the
/// listener's subclass hash, and only `ALL` (via `set_all_custom`) populates
/// that hash wholesale. Subscribing `CUSTOM` with no subclass leaves it empty.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_bare_custom_delivers_no_subclassed_events() {
    let (client, mut events, _permit) = connect().await;
    let subclass = format!("esl_test::bare_{}", std::process::id());

    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::Custom])
        .await
        .unwrap();

    let mut event = EslEvent::with_type(EslEventType::Custom);
    event.set_header("Event-Name", "CUSTOM");
    event.set_header("Event-Subclass", subclass.clone());
    client
        .sendevent(event)
        .await
        .unwrap()
        .check()
        .expect("sendevent rejected");

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => assert_ne!(
                evt.header(EventHeader::EventSubclass),
                Some(subclass.as_str()),
                "bare CUSTOM delivered a subclassed event"
            ),
            Ok(Some(Err(e))) => panic!("event error: {}", e),
            Ok(None) => panic!("event stream closed"),
            Err(_) => break,
        }
    }
}
