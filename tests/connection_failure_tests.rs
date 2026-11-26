//! Integration tests using mock ESL server: disconnect handling, in-flight
//! command wakeup on every disconnect path, malformed event bodies, the
//! reader-exit race, permission-denied recovery, liveness/stall detection,
//! command timeouts and late-reply delivery, and rude rejection.

mod mock_server;

use freeswitch_esl_tokio::{
    ConnectionStatus, DisconnectReason, EslClient, EslConnectOptions, EslError, EslEventType,
    EventFormat, DEFAULT_ESL_PASSWORD,
};
use mock_server::{inflight_command_woken_on, recv_event, setup_connected_pair};
use std::collections::HashMap;
use std::time::Duration;

#[tokio::test]
async fn test_disconnect_notice() {
    let (mut mock, _client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    mock.send_disconnect_notice("Disconnected, goodbye.\nSee you later.\n")
        .await
        .unwrap();

    // events.recv() should return None after disconnect
    let result = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timeout");
    assert!(result.is_none());

    assert!(!_client.is_connected());
    match _client.status() {
        ConnectionStatus::Disconnected(DisconnectReason::ServerNotice { .. }) => {}
        other => panic!("Expected ServerNotice, got: {:?}", other),
    }
}

/// The session-bound notice mod_event_socket sends; no `EventHeader` names
/// `Controlled-Session-UUID`, so only a case-insensitive read finds it.
#[tokio::test]
async fn disconnect_notice_carries_the_controlled_session_uuid() {
    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    let body = "Disconnected, goodbye.\n";
    mock.send_raw(&format!(
        "Content-Type: text/disconnect-notice\n\
         Controlled-Session-UUID: 5c4f1a2e-0000-4000-8000-000000000001\n\
         Content-Disposition: disconnect\n\
         Content-Length: {}\n\n{body}",
        body.len()
    ))
    .await
    .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timeout");
    assert!(result.is_none());
    match client.status() {
        ConnectionStatus::Disconnected(DisconnectReason::ServerNotice {
            controlled_session_uuid,
            ..
        }) => assert_eq!(
            controlled_session_uuid.as_deref(),
            Some("5c4f1a2e-0000-4000-8000-000000000001")
        ),
        other => panic!("Expected ServerNotice, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_tcp_disconnect() {
    let (mock, _client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    // Drop the mock's TCP connection
    mock.drop_connection()
        .await;

    // events.recv() should return None
    let result = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timeout");
    assert!(result.is_none());

    assert!(!_client.is_connected());
    match _client.status() {
        ConnectionStatus::Disconnected(DisconnectReason::ConnectionClosed) => {}
        other => panic!("Expected ConnectionClosed, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_command_after_disconnect() {
    let (mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    mock.drop_connection()
        .await;

    // Wait for the reader to detect the disconnect
    let _ = tokio::time::timeout(Duration::from_secs(5), events.recv()).await;

    // Commands should fail with NotConnected
    let result = client
        .api("status")
        .await;
    assert!(result.is_err());
    match result.unwrap_err() {
        EslError::NotConnected => {}
        e => panic!("Expected NotConnected, got: {}", e),
    }
}

#[tokio::test]
async fn inflight_command_woken_on_disconnect() {
    let (mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    client.set_command_timeout(Duration::from_secs(5));

    // The server closes without replying: a connection-class error well before
    // EslError::CommandTimeout would fire.
    let err = inflight_command_woken_on(&client, mock, Duration::from_secs(1), |mock| async move {
        mock.drop_connection()
            .await;
        None
    })
    .await;
    assert!(err.is_connection_error(), "got: {err}");
    match err {
        EslError::ConnectionClosed => {}
        e => panic!("Expected ConnectionClosed, got: {}", e),
    }
}

#[tokio::test]
async fn inflight_command_woken_on_disconnect_notice() {
    let (mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    client.set_command_timeout(Duration::from_secs(5));

    let err = inflight_command_woken_on(
        &client,
        mock,
        Duration::from_secs(1),
        |mut mock| async move {
            mock.send_disconnect_notice("Disconnected, goodbye.\n")
                .await
                .unwrap();
            Some(mock)
        },
    )
    .await;
    assert!(err.is_connection_error(), "got: {err}");
}

#[tokio::test]
async fn inflight_command_woken_on_rude_rejection() {
    let (mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    client.set_command_timeout(Duration::from_secs(5));

    let err = inflight_command_woken_on(
        &client,
        mock,
        Duration::from_secs(1),
        |mut mock| async move {
            mock.send_raw("Content-Type: text/rude-rejection\n\n")
                .await
                .unwrap();
            Some(mock)
        },
    )
    .await;
    assert!(err.is_connection_error(), "got: {err}");
}

#[tokio::test]
async fn inflight_command_woken_on_protocol_desync() {
    let (mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    client.set_command_timeout(Duration::from_secs(5));

    // Unrecognized Content-Type is a fatal parser error: reader exits.
    let err = inflight_command_woken_on(
        &client,
        mock,
        Duration::from_secs(1),
        |mut mock| async move {
            mock.send_raw("Content-Type: text/garbage\n\n")
                .await
                .unwrap();
            Some(mock)
        },
    )
    .await;
    assert!(err.is_connection_error(), "got: {err}");
}

#[tokio::test]
async fn inflight_command_woken_on_heartbeat_expiry() {
    let (mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    // Liveness must trip while the command is still waiting: the reader
    // checks expiry on its 2s read-timeout tick, so detection lands within
    // ~3s -- well under the 20s command timeout.
    client.set_command_timeout(Duration::from_secs(20));
    client.set_liveness_timeout(Duration::from_secs(1));

    // No reply, no traffic: liveness expires.
    let err = inflight_command_woken_on(&client, mock, Duration::from_secs(8), |mock| async move {
        Some(mock)
    })
    .await;
    assert!(err.is_connection_error(), "got: {err}");

    match client.status() {
        ConnectionStatus::Disconnected(DisconnectReason::HeartbeatExpired) => {}
        other => panic!("Expected HeartbeatExpired, got: {:?}", other),
    }
}

#[tokio::test]
async fn malformed_event_bodies_surface_as_err_and_connection_survives() {
    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    let corpus: &[(&str, &str)] = &[
        ("text/event-json", "{\"Event-Name\":\"HEART"),
        ("text/event-json", "[1,2,3]"),
        (
            "text/event-xml",
            "<event><headers><Event-Name>X</Event-Name></wrong></event>",
        ),
        (
            "text/event-xml",
            "<event><headers><Event-Name>&bogus;</Event-Name></headers></event>",
        ),
    ];

    for (content_type, body) in corpus {
        mock.send_raw(&format!(
            "Content-Length: {}\nContent-Type: {}\n\n{}",
            body.len(),
            content_type,
            body
        ))
        .await
        .unwrap();
        let item = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("timeout")
            .expect("stream must stay open after a malformed event body");
        assert!(
            item.is_err(),
            "malformed body {body:?} must surface as Err, got: {item:?}"
        );
    }

    // The reader loop survives every parse error: a valid event still flows.
    let mut headers = HashMap::new();
    headers.insert("Unique-ID".to_string(), "post-corpus-uuid".to_string());
    mock.send_event_plain("HEARTBEAT", &headers)
        .await
        .unwrap();
    let event = recv_event(&mut events).await;
    assert_eq!(event.event_type(), Some(EslEventType::Heartbeat));
    assert!(client.is_connected());
}

#[tokio::test]
async fn race_command_install_after_reader_exit() {
    let (mut mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    client.set_command_timeout(Duration::from_secs(5));

    // Command A holds the writer lock through its reply wait.
    let task_a = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .api("status")
                .await
        }
    });
    let _cmd_a = mock
        .read_command()
        .await
        .unwrap();

    // Command B passes the entry is_connected() check while the connection is
    // still up, then parks on the writer lock behind A.
    let task_b = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .api("version")
                .await
        }
    });
    // Let B reach the writer-lock await before the connection drops.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Reader wakes A (ConnectionClosed) and exits. B then acquires the writer
    // lock and installs its waiter AFTER fail_pending_reply already ran -- the
    // TOCTOU window: nothing ever wakes that waiter without a reader-dead
    // check under the same lock.
    mock.drop_connection()
        .await;

    let err_a = tokio::time::timeout(Duration::from_secs(1), task_a)
        .await
        .expect("command A still blocked after disconnect")
        .expect("A panicked")
        .expect_err("A should fail on disconnect");
    assert!(err_a.is_connection_error(), "A got: {err_a}");

    // B must fail well under command_timeout_ms with a connection-class
    // error -- not block until EslError::CommandTimeout.
    let err_b = tokio::time::timeout(Duration::from_secs(1), task_b)
        .await
        .expect("command B still blocked: waiter installed after reader exit was never woken")
        .expect("B panicked")
        .expect_err("B should fail after reader exit");
    assert!(err_b.is_connection_error(), "B got: {err_b}");
    match err_b {
        EslError::ConnectionClosed => {}
        e => panic!("Expected ConnectionClosed, got: {}", e),
    }
}

#[tokio::test]
async fn test_subscribe_permission_denied_recoverable() {
    let (mut mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    // Restricted user: FreeSWITCH rejects `event plain HEARTBEAT` with
    // -ERR permission denied. The subscribe call must surface that as a
    // recoverable error and leave the connection usable.
    let subscribe_task = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .subscribe_events(EventFormat::Plain, &[EslEventType::Heartbeat])
                .await
        }
    });

    let _cmd = mock
        .read_command()
        .await
        .unwrap();
    mock.reply_err("permission denied")
        .await
        .unwrap();

    let err = subscribe_task
        .await
        .unwrap()
        .expect_err("subscribe should fail");
    assert!(err.is_permission_denied(), "got: {err}");
    assert!(err.is_recoverable());
    assert!(!err.is_connection_error());

    // Connection stays up: a follow-up command on the same socket still works.
    assert!(client.is_connected());
    let api_task = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .api("status")
                .await
        }
    });
    let cmd = mock
        .read_command()
        .await
        .unwrap();
    assert!(cmd.starts_with("api status"));
    mock.reply_api("UP 0 years")
        .await
        .unwrap();
    let resp = api_task
        .await
        .unwrap()
        .expect("api should succeed");
    assert_eq!(resp.body(), Some("UP 0 years"));
}

#[tokio::test]
async fn test_liveness_expired() {
    let (_mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    // Set a very short liveness timeout
    client.set_liveness_timeout(Duration::from_secs(1));

    // Don't send any traffic from mock -- liveness should expire
    // The reader loop checks every 2s, so we need to wait a bit
    let result = tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("timeout waiting for heartbeat expiry");
    assert!(result.is_none());

    assert!(!client.is_connected());
    match client.status() {
        ConnectionStatus::Disconnected(DisconnectReason::HeartbeatExpired) => {}
        other => panic!("Expected HeartbeatExpired, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_liveness_reset_by_traffic() {
    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    // Set liveness timeout to 3s
    client.set_liveness_timeout(Duration::from_secs(3));

    // Send events every 2s to keep connection alive
    let mock_task = tokio::spawn(async move {
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_secs(2)).await;
            mock.send_heartbeat()
                .await
                .unwrap();
        }
        // After sending 3 heartbeats, stop -- liveness should expire
        mock
    });

    // Receive the 3 heartbeats
    let mut count = 0;
    while let Some(result) = tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("timeout")
    {
        if let Ok(event) = result {
            if event.event_type() == Some(EslEventType::Heartbeat) {
                count += 1;
                if count >= 3 {
                    break;
                }
            }
        }
    }
    assert_eq!(count, 3);

    // Now wait for liveness expiry (no more traffic)
    let _mock = mock_task
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("timeout");
    assert!(result.is_none());

    match client.status() {
        ConnectionStatus::Disconnected(DisconnectReason::HeartbeatExpired) => {}
        other => panic!("Expected HeartbeatExpired, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_stall_detected() {
    let (_mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    // Set short timeout -- auth traffic already happened, then nothing
    client.set_liveness_timeout(Duration::from_secs(1));

    let result = tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("timeout");
    assert!(result.is_none());

    match client.status() {
        ConnectionStatus::Disconnected(DisconnectReason::HeartbeatExpired) => {}
        other => panic!("Expected HeartbeatExpired, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_command_timeout() {
    let (_mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    // Set a very short command timeout
    client.set_command_timeout(Duration::from_millis(200));

    // Send a command but mock never replies -- should timeout
    let result = client
        .api("status")
        .await;

    match result {
        Err(EslError::CommandTimeout { .. }) => {}
        Err(e) => panic!("Expected Timeout, got: {}", e),
        Ok(_) => panic!("Expected timeout error, got success"),
    }
}

/// The timeout a client starts with, which every other test here replaces
/// before it sends anything. No accessor reports it; the error does.
#[tokio::test]
async fn test_command_timeout_default() {
    let (_mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    match client
        .api("status")
        .await
    {
        Err(EslError::CommandTimeout { timeout_ms, .. }) => assert_eq!(timeout_ms, 5000),
        other => panic!("expected a Timeout, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_command_timeout_cleanup() {
    let (mut mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    // Set short timeout
    client.set_command_timeout(Duration::from_millis(200));

    // First command times out (mock doesn't reply)
    let result = client
        .api("status")
        .await;
    assert!(matches!(result, Err(EslError::CommandTimeout { .. })));

    // Second command should still work -- pending_reply slot was cleaned up
    let api_task = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .api("version")
                .await
        }
    });

    // Mock reads the timed-out command then the second one. In real FreeSWITCH
    // the server eventually replies to every command it received; simulate that
    // by sending A's late reply first, then B's reply.
    let _cmd1 = mock
        .read_command()
        .await
        .unwrap();
    let _cmd2 = mock
        .read_command()
        .await
        .unwrap();
    // A's late reply goes to the event stream, not to B.
    mock.reply_api("stale")
        .await
        .unwrap();
    // B's actual reply.
    mock.reply_api("1.0")
        .await
        .unwrap();

    let result = api_task
        .await
        .unwrap();
    assert!(result.is_ok());
}

#[tokio::test]
async fn timeout_stale_reply_does_not_corrupt_next_command() {
    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    // Short timeout so A times out before the mock replies.
    client.set_command_timeout(Duration::from_millis(100));

    // Spawn A and let it time out.
    let client_a = client.clone();
    let cmd_a = tokio::spawn(async move {
        client_a
            .api("status")
            .await
    });
    let _cmd_a_str = mock
        .read_command()
        .await
        .unwrap();

    let a_result = tokio::time::timeout(Duration::from_secs(2), cmd_a)
        .await
        .expect("cmd_a join timeout")
        .expect("cmd_a panicked");
    let Err(EslError::CommandTimeout {
        ticket: ticket_a, ..
    }) = a_result
    else {
        panic!("command A should timeout, got: {a_result:?}");
    };

    // Restore normal timeout and spawn B.
    client.set_command_timeout(Duration::from_secs(5));
    let client_b = client.clone();
    let cmd_b = tokio::spawn(async move {
        client_b
            .api("version")
            .await
    });

    // Wait until mock can read B's command -- sender is installed before bytes
    // are written, so reading B confirms B's sender is in the slot.
    let _cmd_b_str = mock
        .read_command()
        .await
        .unwrap();

    // Send A's stale reply first, then B's actual reply.
    mock.reply_ok_text("reply-for-A")
        .await
        .unwrap();
    mock.reply_ok_text("reply-for-B")
        .await
        .unwrap();

    let b_result = tokio::time::timeout(Duration::from_secs(2), cmd_b)
        .await
        .expect("cmd_b join timeout")
        .expect("cmd_b panicked")
        .expect("command B should succeed");

    let reply_text = b_result
        .reply_text()
        .expect("B should have Reply-Text");
    assert!(
        reply_text.contains("reply-for-B"),
        "B must get B's reply, got: {}",
        reply_text
    );

    let (err, ticket, response) = next_late_reply(&mut events).await;
    assert_eq!(ticket, ticket_a);
    assert_eq!(response.reply_text(), Some("+OK reply-for-A"));
    assert!(!err.is_connection_error(), "{err:?}");
    assert!(!err.is_recoverable(), "{err:?}");
    let shown = err.to_string();
    assert!(shown.contains("command/reply"), "{shown}");
    assert!(!shown.contains("reply-for-A"), "{shown}");
}

/// The next stream item, which must be a `LateReply`.
async fn next_late_reply(
    events: &mut freeswitch_esl_tokio::EslEventStream,
) -> (
    EslError,
    freeswitch_esl_tokio::CommandTicket,
    freeswitch_esl_tokio::EslResponse,
) {
    let item = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timeout")
        .expect("the stream stays open");
    let err = item.expect_err("a late reply is not an event");
    let EslError::LateReply { response, ticket } = &err else {
        panic!("expected LateReply, got {err:?}");
    };
    let (ticket, response) = (*ticket, (**response).clone());
    (err, ticket, response)
}

/// Commands queued behind one the switch is still running each time out; their
/// replies arrive later, in order, each carrying its own command's ticket.
#[tokio::test]
async fn queued_timeouts_each_get_their_late_reply() {
    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    client.set_command_timeout(Duration::from_millis(100));

    let spawn_api = |cmd: &'static str| {
        let client = client.clone();
        tokio::spawn(async move {
            client
                .api(cmd)
                .await
        })
    };
    let cmd_a = spawn_api("msleep 3000");
    mock.read_command()
        .await
        .unwrap();
    let cmd_b = spawn_api("status");
    mock.read_command()
        .await
        .unwrap();

    let mut tickets = Vec::new();
    for cmd in [cmd_a, cmd_b] {
        match cmd
            .await
            .expect("panicked")
        {
            Err(EslError::CommandTimeout { ticket, .. }) => tickets.push(ticket),
            other => panic!("expected CommandTimeout, got {other:?}"),
        }
    }
    assert_ne!(tickets[0], tickets[1]);

    mock.reply_api("reply-for-A")
        .await
        .unwrap();
    mock.reply_api("reply-for-B")
        .await
        .unwrap();
    for (ticket, body) in tickets
        .iter()
        .zip(["reply-for-A", "reply-for-B"])
    {
        let (err, late_ticket, response) = next_late_reply(&mut events).await;
        assert_eq!(late_ticket, *ticket);
        assert_eq!(response.body(), Some(body));
        assert!(err
            .to_string()
            .contains("api/response"));
    }

    client.set_command_timeout(Duration::from_secs(5));
    let cmd_c = spawn_api("version");
    mock.read_command()
        .await
        .unwrap();
    mock.reply_api("1.0")
        .await
        .unwrap();
    let reply = cmd_c
        .await
        .expect("panicked")
        .expect("the session still correlates");
    assert_eq!(reply.body(), Some("1.0"));
}

/// A per-call timeout bounds that call alone: it fires under a longer shared
/// timeout, its late reply keeps its ticket, and the next plain call waits for
/// the shared timeout again.
#[tokio::test]
async fn per_call_timeout_applies_to_that_call_only() {
    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    client.set_command_timeout(Duration::from_secs(5));

    let started = tokio::time::Instant::now();
    let (short, _) = tokio::join!(
        client.api_with_timeout("console_complete show", Duration::from_millis(100)),
        mock.read_command()
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    let Err(EslError::CommandTimeout { timeout_ms, ticket }) = short else {
        panic!("expected CommandTimeout, got {short:?}");
    };
    assert_eq!(timeout_ms, 100);

    let plain = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .api("status")
                .await
        }
    });
    mock.read_command()
        .await
        .unwrap();
    // Past the per-call timeout: the plain call must still be waiting.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!plain.is_finished(), "the per-call timeout leaked");
    mock.reply_api("late")
        .await
        .unwrap();
    mock.reply_api("UP")
        .await
        .unwrap();
    let reply = plain
        .await
        .unwrap()
        .expect("the plain call gets its own reply");
    assert_eq!(reply.body(), Some("UP"));
    let (_, late_ticket, _) = next_late_reply(&mut events).await;
    assert_eq!(late_ticket, ticket);
}

/// Sends `api status`, waits until the switch has read all of it, then drops
/// the future while it waits for the reply.
async fn abandon_after_write(
    mock: &mut freeswitch_esl_tokio::mock::MockConnection,
    client: &EslClient,
) {
    let cmd = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .api("status")
                .await
        }
    });
    mock.read_command()
        .await
        .unwrap();
    cmd.abort();
    assert!(cmd
        .await
        .unwrap_err()
        .is_cancelled());
}

/// A command whose future the caller drops while it waits still has a reply
/// coming; the next command must not take that reply as its own.
#[tokio::test]
async fn cancelled_command_reply_does_not_reach_the_next_command() {
    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    abandon_after_write(&mut mock, &client).await;

    let client_b = client.clone();
    let cmd_b = tokio::spawn(async move {
        client_b
            .api("version")
            .await
    });
    mock.read_command()
        .await
        .unwrap();
    mock.reply_ok_text("reply-for-A")
        .await
        .unwrap();
    mock.reply_ok_text("reply-for-B")
        .await
        .unwrap();

    let b_result = tokio::time::timeout(Duration::from_secs(2), cmd_b)
        .await
        .expect("cmd_b join timeout")
        .expect("cmd_b panicked")
        .expect("command B should succeed");
    assert_eq!(b_result.reply_text(), Some("+OK reply-for-B"));

    let (_, _, response) = next_late_reply(&mut events).await;
    assert_eq!(response.reply_text(), Some("+OK reply-for-A"));
}

/// A reply to a dropped command that lands before any other command is sent
/// reaches the stream too, rather than failing to send into the dropped waiter.
#[tokio::test]
async fn cancelled_command_reply_before_the_next_command_is_delivered() {
    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    abandon_after_write(&mut mock, &client).await;

    mock.reply_ok_text("reply-for-dropped")
        .await
        .unwrap();
    let (_, _, response) = next_late_reply(&mut events).await;
    assert_eq!(response.reply_text(), Some("+OK reply-for-dropped"));
}

/// A command dropped before its bytes all reached the socket leaves the wire in
/// a state nothing can repair; the next command closes the connection rather
/// than write after a fragment or wait on a reply nobody owes.
#[tokio::test]
async fn command_dropped_mid_write_closes_the_connection() {
    let (_mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    client.set_command_timeout(Duration::from_millis(500));

    // The mock never reads, so a command larger than both socket buffers
    // cannot finish writing.
    let huge = "x".repeat(32 * 1024 * 1024);
    let stuck = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .api(&huge)
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!stuck.is_finished(), "the write was expected to block");
    stuck.abort();
    assert!(stuck
        .await
        .unwrap_err()
        .is_cancelled());

    // Bounded here because a write into the full buffer would never finish.
    let err = tokio::time::timeout(Duration::from_secs(5), client.api("status"))
        .await
        .expect("refused before writing")
        .expect_err("the wire state is unknown");
    assert!(err.is_connection_error(), "{err:?}");
    assert!(!client.is_connected());
}

/// A switch that stops reading must not hold a command forever: the command
/// timeout covers the write, and a write cut short closes the connection, so
/// commands queued behind it fail rather than wait.
#[tokio::test]
async fn write_that_cannot_finish_times_out_and_closes() {
    let (_mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    client.set_command_timeout(Duration::from_millis(500));

    let huge = "x".repeat(32 * 1024 * 1024);
    let queued = tokio::spawn({
        let client = client.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            client
                .api("status")
                .await
        }
    });
    // Bounded here only so a regression fails instead of hanging.
    let err = tokio::time::timeout(Duration::from_secs(30), client.api(&huge))
        .await
        .expect("the command timeout covers the write")
        .expect_err("the write cannot finish");
    assert!(err.is_connection_error(), "{err:?}");
    assert!(!client.is_connected());

    let queued = tokio::time::timeout(Duration::from_secs(30), queued)
        .await
        .expect("the queued command is released")
        .unwrap();
    assert!(
        queued.is_err_and(|e| e.is_connection_error()),
        "the queued command must fail with a connection error"
    );
}

/// A client that dropped its event stream ends its reader at the next item it
/// would deliver, here a late reply; the status must say so rather than keep
/// reporting a connection every command now fails on.
#[tokio::test]
async fn dropped_event_stream_is_reported_in_the_status() {
    let (mut mock, client, events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    drop(events);
    client.set_command_timeout(Duration::from_millis(100));

    let (timed_out, _) = tokio::join!(client.api("status"), mock.read_command());
    assert!(matches!(timed_out, Err(EslError::CommandTimeout { .. })));
    mock.reply_api("late")
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while client.is_connected() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        matches!(
            client.status(),
            ConnectionStatus::Disconnected(DisconnectReason::EventStreamDropped)
        ),
        "{:?}",
        client.status()
    );
}

#[tokio::test]
async fn timeout_normal_reply_still_routes_correctly() {
    // Regression: verify reply routing works normally when there is no timeout.
    let (mut mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    let api_task = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .api("status")
                .await
        }
    });

    let _cmd = mock
        .read_command()
        .await
        .unwrap();
    mock.reply_ok_text("normal-reply")
        .await
        .unwrap();

    let resp = api_task
        .await
        .expect("join")
        .expect("should succeed");
    let reply_text = resp
        .reply_text()
        .expect("should have Reply-Text");
    assert!(
        reply_text.contains("normal-reply"),
        "reply should route correctly, got: {}",
        reply_text
    );
}

/// The next stream item, which must be an `UnsolicitedReply`.
async fn next_unsolicited_reply(
    events: &mut freeswitch_esl_tokio::EslEventStream,
) -> (EslError, freeswitch_esl_tokio::EslResponse) {
    let item = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timeout")
        .expect("the stream stays open");
    let err = item.expect_err("a stray reply is not an event");
    let EslError::UnsolicitedReply { response } = &err else {
        panic!("expected UnsolicitedReply, got {err:?}");
    };
    let response = (**response).clone();
    (err, response)
}

/// A reply with no command waiting reaches the caller instead of the log, the
/// session stays up, and the next command still gets its own reply.
#[tokio::test]
async fn unsolicited_reply_surfaces_on_the_event_stream() {
    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    mock.reply_raw_text("+OK stray-reply-sentinel")
        .await
        .unwrap();
    let (err, response) = next_unsolicited_reply(&mut events).await;
    assert_eq!(response.reply_text(), Some("+OK stray-reply-sentinel"));
    assert!(!err.is_connection_error(), "{err:?}");
    assert!(!err.is_recoverable(), "{err:?}");
    let shown = err.to_string();
    assert!(
        shown.contains("command/reply") && shown.contains("bytes"),
        "{shown}"
    );
    assert!(!shown.contains("stray-reply-sentinel"), "{shown}");

    mock.reply_api("stray-body-sentinel")
        .await
        .unwrap();
    let (err, response) = next_unsolicited_reply(&mut events).await;
    assert_eq!(response.body(), Some("stray-body-sentinel"));
    let shown = err.to_string();
    assert!(shown.contains("api/response"), "{shown}");
    assert!(!shown.contains("stray-body-sentinel"), "{shown}");

    assert!(client.is_connected());
    let next = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .api("version")
                .await
        }
    });
    mock.read_command()
        .await
        .unwrap();
    mock.reply_api("1.0")
        .await
        .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), next)
        .await
        .expect("join timeout")
        .expect("join")
        .expect("the next command gets its own reply");
    assert_eq!(reply.body(), Some("1.0"));
}

// --- Rude rejection ---

#[tokio::test]
async fn rude_rejection_returns_access_denied() {
    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    mock.send_raw("Content-Type: text/rude-rejection\n\n")
        .await
        .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timeout");
    match result {
        Some(Err(EslError::AccessDenied { .. })) => {}
        other => panic!("expected AccessDenied error, got: {:?}", other),
    }

    let closed = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timeout");
    assert!(
        closed.is_none(),
        "stream should be closed after rude rejection"
    );

    match client.status() {
        ConnectionStatus::Disconnected(DisconnectReason::AccessDenied(_)) => {}
        other => panic!("expected Disconnected(AccessDenied), got: {:?}", other),
    }
}

#[tokio::test]
async fn parser_error_reports_protocol_error() {
    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    // An unrecognized Content-Type triggers a ProtocolError from parse_message().
    mock.send_raw("Content-Type: text/garbage\n\n")
        .await
        .unwrap();

    // Event stream should close (no valid event emitted for a protocol error).
    let closed = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timeout");
    assert!(
        closed.is_none(),
        "stream should close on parser error, got: {:?}",
        closed
    );

    match client.status() {
        ConnectionStatus::Disconnected(DisconnectReason::ProtocolError(_)) => {}
        other => panic!("expected Disconnected(ProtocolError), got: {:?}", other),
    }
}

/// A peer that accepts and then says `greeting` and nothing more, holding the
/// socket open past the client's connect timeout.
async fn connect_to_silent_peer(greeting: &'static [u8]) -> EslError {
    use tokio::io::AsyncWriteExt;
    let (listener, port) = mock_server::setup_raw_pair().await;
    let peer = tokio::spawn(async move {
        let (mut sock, _) = listener
            .accept()
            .await
            .unwrap();
        sock.write_all(greeting)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    let options = EslConnectOptions::default().with_connect_timeout(Duration::from_millis(200));
    let err = EslClient::connect_with_options("localhost", port, DEFAULT_ESL_PASSWORD, options)
        .await
        .expect_err("a silent peer must not complete the handshake");
    peer.abort();
    err
}

/// No session ever came up, so this is a connection error, not a command
/// timeout on a live connection the caller could retry on.
#[tokio::test]
async fn a_silent_handshake_is_a_connection_error() {
    for greeting in [&b""[..], b"Content-Type: auth/request\n\n"] {
        let err = connect_to_silent_peer(greeting).await;
        assert!(!matches!(err, EslError::CommandTimeout { .. }), "{err:?}");
        assert!(err.is_connection_error(), "{err:?}");
        assert!(!err.is_recoverable(), "{err:?}");
    }
}
