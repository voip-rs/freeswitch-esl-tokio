//! Integration tests against a live FreeSWITCH instance: connection
//! lifecycle, liveness, command timeout, userauth, outbound connect-response
//! case preservation, log events, and header/codec normalization.
//!
//! These tests require a live FreeSWITCH ESL; see docs/live-test-switch.md.
//! Run with: cargo test --test 'live_*' -- --ignored

mod live_common;

use freeswitch_esl_tokio::commands::originate::{Variables, VariablesType};
use freeswitch_esl_tokio::commands::{GroupCall, LoopbackEndpoint};
use freeswitch_esl_tokio::connection::AuthMethod;
use freeswitch_esl_tokio::{
    parse_api_body, Application, ConnectionStatus, DialStringCarrier, DisconnectReason, Endpoint,
    EslClient, EslConnectOptions, EslError, EslEventType, EventFormat, EventHeader, HeaderLookup,
    Originate,
};
use live_common::{
    block_parse_under_test, connect, esl_env, kill_channel, target_under_test, wait_for_own_event,
    ChannelReaper, CONN_SEMAPHORE,
};
use std::time::Duration;
use tokio::time::Instant;

/// The reader loop notices a shutdown on its own task, so the status arrives
/// after `disconnect()` returns rather than with it.
async fn wait_disconnected(client: &EslClient) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while client.is_connected() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !client.is_connected(),
        "still connected after disconnect(): {:?}",
        client.status()
    );
}

/// `originate` over the API expands nothing, so the expression `GroupCall` renders dials as an
/// unknown endpoint type, while `eval` expands the same text.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_api_originate_leaves_an_expression_unexpanded() {
    let (client, _events, _permit) = connect().await;
    let group = GroupCall::new("nosuchgroup", "example.com").unwrap();
    let originate = Originate::application(
        group
            .clone()
            .into(),
        Application::simple("park"),
    );
    let line = originate.render(block_parse_under_test());
    assert_eq!(
        line,
        "originate ${group_call(nosuchgroup@example.com)} &park()"
    );
    let dialed = client
        .api(&line)
        .await
        .unwrap();
    let body = dialed
        .body()
        .unwrap_or_default();
    assert!(
        body.contains("CHAN_NOT_IMPLEMENTED"),
        "port {}: originate answered {body:?}",
        esl_env().port
    );
    let expanded = client
        .api(&format!(
            "eval {}",
            group.render(target_under_test(DialStringCarrier::EslApi))
        ))
        .await
        .unwrap();
    assert_eq!(
        expanded.body(),
        Some("error/NO_ROUTE_DESTINATION"),
        "port {}",
        esl_env().port
    );
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_connect_and_status() {
    let (client, _events, _permit) = connect().await;
    assert!(client.is_connected());

    let resp = client
        .api("status")
        .await
        .unwrap();
    let body = resp
        .body()
        .expect("status should have body");
    assert!(body.contains("UP"), "expected UP in status: {}", body);
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_liveness_heartbeat_resets_timer() {
    let (client, mut events, _permit) = connect().await;

    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::Heartbeat])
        .await
        .unwrap();

    // Set liveness timeout to 30s, well above heartbeat interval (~20s)
    client.set_liveness_timeout(Duration::from_secs(30));

    // Wait for two heartbeats to confirm the timer resets
    let mut heartbeat_count = 0;
    let deadline = Instant::now() + Duration::from_secs(45);
    while Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => {
                if evt.event_type() == Some(EslEventType::Heartbeat) {
                    heartbeat_count += 1;
                    if heartbeat_count >= 2 {
                        break;
                    }
                }
            }
            Ok(Some(Err(e))) => panic!("event error: {}", e),
            Ok(None) => panic!("connection closed unexpectedly"),
            Err(_) => break,
        }
    }

    assert!(client.is_connected(), "connection should still be alive");
    assert!(
        heartbeat_count >= 2,
        "expected at least 2 heartbeats, got {}",
        heartbeat_count
    );
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_command_timeout_msleep() {
    let (client, _events, _permit) = connect().await;

    // Set a short command timeout, then send a blocking api call
    client.set_command_timeout(Duration::from_secs(1));

    let result = client
        .api("msleep 5000")
        .await;

    assert!(
        matches!(result, Err(EslError::Timeout { .. })),
        "expected Timeout error, got: {:?}",
        result
    );

    // Verify the connection is still usable after timeout.
    // Increase timeout for the recovery command.
    client.set_command_timeout(Duration::from_secs(10));

    // msleep result may arrive late and consume the next reply slot.
    // Wait for the blocked msleep to complete on the server side.
    tokio::time::sleep(Duration::from_secs(5)).await;

    let resp = client
        .api("status")
        .await;
    assert!(
        resp.is_ok(),
        "command after timeout should succeed: {:?}",
        resp
    );
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_disconnect_status() {
    let (client, _events, _permit) = connect().await;
    assert!(client.is_connected());

    client
        .disconnect()
        .await
        .unwrap();

    wait_disconnected(&client).await;

    // The final status may be ClientRequested or ServerNotice depending on
    // timing: we set ClientRequested before shutdown, but the reader loop
    // may process the server's goodbye message and overwrite with ServerNotice.
    assert!(
        matches!(
            client.status(),
            ConnectionStatus::Disconnected(
                DisconnectReason::ClientRequested | DisconnectReason::ServerNotice { .. }
            )
        ),
        "status should be ClientRequested or ServerNotice, got: {:?}",
        client.status()
    );
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_reconnect_clean_state() {
    // Connect, disconnect, then reconnect and verify clean state
    let (client1, _events1, _permit1) = connect().await;
    assert!(client1.is_connected());

    let resp1 = client1
        .api("hostname")
        .await
        .unwrap();
    let hostname = resp1
        .body()
        .unwrap()
        .trim()
        .to_string();

    client1
        .disconnect()
        .await
        .unwrap();
    wait_disconnected(&client1).await;

    // Reconnect
    let (client2, _events2, _permit2) = connect().await;
    assert!(client2.is_connected());

    let resp2 = client2
        .api("hostname")
        .await
        .unwrap();
    assert_eq!(
        resp2
            .body()
            .unwrap()
            .trim(),
        hostname,
        "hostname should match after reconnect"
    );
}

/// Verify that userauth with a long Allowed-Events list survives
/// FreeSWITCH's reply[512] truncation in mod_event_socket.c.
///
/// Requires the `many-events@default` user configured in FreeSWITCH with
/// a long esl-allowed-events list that triggers the 512-byte overflow.
/// The user has `esl-allowed-api = "show uuid_dump originate uuid_kill"`
/// and `esl-allowed-log = false`, but the 512-byte reply buffer overflow
/// truncates these headers (Allowed-LOG is always fully lost).
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_connect_userauth_truncated_response() {
    let permit = CONN_SEMAPHORE
        .acquire()
        .await
        .expect("semaphore closed");
    let opts = EslConnectOptions::new().with_connect_timeout(Duration::from_secs(5));
    let env = esl_env();
    let (client, _events) = EslClient::connect_with_auth(
        &env.host,
        env.port,
        AuthMethod::user(
            "many-events@default",
            env.password
                .as_str(),
        ),
        opts,
    )
    .await
    .expect("userauth with truncated response should succeed");

    assert!(client.is_connected());

    let auth = client
        .auth_response()
        .expect("auth_response should be present for inbound connection");
    assert_eq!(auth.reply_text(), Some("+OK accepted"));
    assert!(
        auth.header("Allowed-Events")
            .is_some(),
        "Allowed-Events header should be present (possibly truncated)"
    );

    // Allowed-LOG is the last header in the reply — with the 512-byte
    // overflow it's always fully truncated. Its absence proves the
    // salvage path was triggered (a non-truncated response would have it).
    assert!(
        auth.header("Allowed-Log")
            .is_none(),
        "Allowed-Log should be missing due to reply[512] truncation. \
         If this fails, FreeSWITCH may have been patched — \
         the workaround is no longer needed for this configuration."
    );

    // Allowed-API should be present but truncated (fewer commands than
    // the configured "show uuid_dump originate uuid_kill").
    if let Some(api) = auth.header("Allowed-Api") {
        let commands: Vec<&str> = api
            .split_whitespace()
            .collect();
        assert!(
            commands.len() < 4,
            "Allowed-API has all 4 commands ({api}), expected truncation"
        );
    }

    drop(permit);
}

/// The outbound `connect` reply carries every channel variable as a
/// `variable_<name>` header. SIP-passthrough variables (`variable_sip_h_*`)
/// must preserve the original SIP wire casing — `EslResponse::header()`
/// must not collapse `variable_sip_h_X-MixedCase` into a lowercase bucket.
///
/// Drives a real outbound socket against FS to verify the wire path.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_outbound_connect_response_preserves_underscored_case() {
    use tokio::net::TcpListener;

    let (inbound, mut events, permit) = connect().await;

    // The originate's outcome is the only thing that explains a listener that
    // never gets dialed, so watch for it rather than inferring from a timeout.
    inbound
        .subscribe_events(EventFormat::Plain, &[EslEventType::BackgroundJob])
        .await
        .expect("subscribe BACKGROUND_JOB");

    let listener = TcpListener::bind("[::]:0")
        .await
        .expect("bind outbound listener");
    let port = listener
        .local_addr()
        .expect("a bound listener has a local address")
        .port();

    // Inject a channel variable with mixed-case underscored name.
    // FS preserves the variable name verbatim and emits it as
    // `variable_<name>` in the connect reply.
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("MyMixed_Case_Var", "preserved")
        .unwrap();

    let endpoint = LoopbackEndpoint::new("9199")
        .unwrap()
        .with_context("test")
        .unwrap()
        .with_variables(vars);

    // The switch dials this literal, so it names the family the switch itself
    // listens on; the listener above takes either.
    let cmd = Originate::application(
        Endpoint::Loopback(endpoint),
        Application::new("socket", Some(format!("127.0.0.1:{} async full", port))),
    );

    // bgapi returns immediately; the call proceeds asynchronously and
    // FS dials our listener.
    let job = inbound
        .bgapi(&cmd.render(block_parse_under_test()))
        .await
        .expect("bgapi originate")
        .job_uuid()
        .expect("bgapi must return a Job-UUID")
        .to_string();

    // Race the accept against the job result. A failed originate means FS never
    // dials, so without this the only symptom is an opaque accept timeout that
    // says nothing about why.
    let accept = tokio::time::timeout(
        Duration::from_secs(10),
        EslClient::accept_outbound(&listener),
    );
    tokio::pin!(accept);

    let (outbound, _outbound_events) = loop {
        tokio::select! {
            accepted = &mut accept => {
                break accepted
                    .expect("timed out waiting for outbound connection")
                    .expect("accept_outbound failed");
            }
            event = events.recv() => match event {
                Some(Ok(evt)) => {
                    if evt.event_type() == Some(EslEventType::BackgroundJob)
                        && evt.job_uuid() == Some(job.as_str())
                    {
                        if let Some(body) = evt.body() {
                            if let Err(e) = parse_api_body(body) {
                                panic!("originate failed, so FS never dialed us: {}", e);
                            }
                        }
                    }
                }
                Some(Err(e)) => panic!("event error: {}", e),
                None => panic!("event stream closed"),
            },
        }
    };

    let resp = outbound
        .connect_session()
        .await
        .expect("connect_session failed");

    // Case-sensitive: exact-cased variable name must hit.
    assert_eq!(
        resp.header("variable_MyMixed_Case_Var"),
        Some("preserved"),
        "exact-case variable lookup must succeed"
    );

    // Case-sensitive: wrong-cased variants must NOT match. If the
    // case_index were unfiltered, lowercased lookups would resolve
    // through the alias and return the wrong value (or worse, collapse
    // distinct headers like X-Foo and X-foo).
    assert_eq!(
        resp.header("variable_mymixed_case_var"),
        None,
        "lowercased variable_* must not match — SIP wire casing is preserved"
    );
    assert_eq!(
        resp.header("VARIABLE_MYMIXED_CASE_VAR"),
        None,
        "uppercased variable_* must not match"
    );

    // Case-insensitive: framing headers (no underscore) resolve in any case.
    let canonical = resp
        .header("Reply-Text")
        .expect("Reply-Text must be present on connect reply");
    assert_eq!(resp.header("reply-text"), Some(canonical));
    assert_eq!(resp.header("REPLY-TEXT"), Some(canonical));

    // Cleanup.
    if let Some(uuid) = resp
        .header("Channel-Unique-ID")
        .or_else(|| resp.header("Unique-ID"))
        .map(String::from)
    {
        kill_channel(&inbound, &uuid).await;
    }

    drop(permit);
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_log_events_have_log_type() {
    let (client, mut events, _permit) = connect().await;

    // Enable log forwarding at DEBUG level to generate log traffic
    let resp = client
        .log("DEBUG")
        .await
        .unwrap();
    assert!(
        resp.is_success(),
        "log command failed: {:?}",
        resp.reply_text()
    );

    // Trigger log output by running an API command
    client
        .api("status")
        .await
        .unwrap()
        .api_result()
        .expect("status rejected");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => {
                if evt.event_type() == Some(EslEventType::Log) {
                    // Verify log-specific headers are present
                    assert!(
                        evt.header(EventHeader::LogLevel)
                            .is_some(),
                        "log event should have Log-Level header"
                    );
                    assert!(
                        evt.header_str("Log-File")
                            .is_some(),
                        "log event should have Log-File header"
                    );
                    assert!(
                        evt.body()
                            .is_some(),
                        "log event should have a body with the log text"
                    );

                    // Connection drops on return, so a failed nolog changes nothing.
                    let _ = client
                        .nolog()
                        .await;
                    return;
                }
            }
            Ok(Some(Err(e))) => panic!("event error: {}", e),
            Ok(None) => panic!("event stream closed"),
            Err(_) => break,
        }
    }

    // Connection drops on panic, so a failed nolog changes nothing.
    let _ = client
        .nolog()
        .await;
    panic!("did not receive any log event with EslEventType::Log");
}

/// Park a loopback channel and hand back the first `event_type` it emits,
/// reaping the channel before the caller asserts on the event.
///
/// The originate goes over `api`, which returns once the channel answers;
/// events queue while it blocks.
async fn park_and_await(event_type: EslEventType) -> freeswitch_esl_tokio::EslEvent {
    let (client, mut events, _permit) = connect().await;

    client
        .subscribe_events(EventFormat::Plain, &[event_type])
        .await
        .expect("subscribe failed");

    let originate = Originate::application(
        Endpoint::from(LoopbackEndpoint::new("9199").unwrap()),
        Application::park(),
    );
    let uuid = client
        .api(&originate.render(block_parse_under_test()))
        .await
        .expect("originate: transport error")
        .api_result()
        .expect("originate rejected")
        .to_string();

    let mut reaper = ChannelReaper::new(&client);
    reaper.track(&uuid);

    let deadline = Instant::now() + Duration::from_secs(10);
    let event = wait_for_own_event(&mut events, &uuid, event_type, deadline).await;

    reaper
        .reap()
        .await;

    event.unwrap_or_else(|| panic!("no {event_type} for {uuid}"))
}

/// The keys of an event, lowercased: as many as the event has, or the switch
/// sent one header under two spellings and normalization kept both.
fn distinct_lowercased_keys(evt: &freeswitch_esl_tokio::EslEvent) -> Vec<String> {
    let mut keys: Vec<String> = evt
        .headers()
        .keys()
        .map(|k| k.to_ascii_lowercase())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// FreeSWITCH emits the same header Title-Cased from `switch_channel.c` and
/// lowercase from `switch_event.c`, so a live event is the only proof that
/// both spellings land on one key.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_header_normalization() {
    let evt = park_and_await(EslEventType::ChannelCreate).await;

    for header in [
        EventHeader::UniqueId,
        EventHeader::ChannelState,
        EventHeader::EventName,
    ] {
        assert!(
            evt.header(header)
                .is_some(),
            "{header} must resolve on a CHANNEL_CREATE"
        );
    }
    assert_eq!(
        evt.headers()
            .len(),
        distinct_lowercased_keys(&evt).len(),
        "duplicate keys with different casing found in headers"
    );

    // Channel variables keep their underscore keys.
    let direction = evt
        .variable_str("direction")
        .expect("CHANNEL_CREATE carries variable_direction");
    assert!(!direction.is_empty());
}

/// The same, for the event `switch_core_codec.c` writes with a casing all its
/// own -- read codec lowercase, write codec mixed.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_codec_header_normalization() {
    let evt = park_and_await(EslEventType::Codec).await;

    let codec = evt
        .header(EventHeader::ChannelReadCodecName)
        .expect("a CODEC event names the read codec");
    assert!(!codec.is_empty());

    assert_eq!(
        evt.headers()
            .len(),
        distinct_lowercased_keys(&evt).len(),
        "CODEC event has duplicate keys with different casing: {:?}",
        evt.headers()
            .keys()
            .collect::<Vec<_>>()
    );
}
