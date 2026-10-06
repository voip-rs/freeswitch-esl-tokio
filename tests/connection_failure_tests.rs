//! Integration tests using mock ESL server: disconnect handling, in-flight
//! command wakeup on every disconnect path, malformed event bodies, the
//! reader-exit race, permission-denied recovery, liveness/stall detection,
//! command timeouts and stale-reply handling, and rude rejection.

mod mock_server;

use freeswitch_esl_tokio::{
    ConnectionStatus, DisconnectReason, EslError, EslEventType, EventFormat, DEFAULT_ESL_PASSWORD,
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

    // Server reads the command, then closes the socket without replying.
    // The in-flight command must fail well under command_timeout_ms, with a
    // connection-class error -- not EslError::Timeout at the timeout boundary.
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
    // error -- not block until EslError::Timeout.
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
        Err(EslError::Timeout { .. }) => {}
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
        Err(EslError::Timeout { timeout_ms }) => assert_eq!(timeout_ms, 5000),
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
    assert!(matches!(result, Err(EslError::Timeout { .. })));

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
    // A's late reply -- reader discards it via stale-reply counter.
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
    // Regression: before the stale-reply counter fix, the server's late reply
    // for command A was delivered to command B's waiter, giving B the wrong reply.
    let (mut mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

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
    assert!(
        matches!(a_result, Err(EslError::Timeout { .. })),
        "command A should timeout, got: {:?}",
        a_result
    );

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
