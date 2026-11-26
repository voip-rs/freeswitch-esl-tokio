//! Integration tests using mock ESL server: re-exec teardown/adopt lifecycle.
#![cfg(unix)]

mod mock_server;

use freeswitch_esl_tokio::{
    ConnectionStatus, DisconnectReason, EslClient, EslConnectOptions, EslError, EslEventType,
    EventOverflow, DEFAULT_ESL_PASSWORD,
};
use mock_server::{
    recv_event, setup_connected_pair, setup_connected_pair_with_options, setup_raw_pair,
    MockConnection,
};
use std::collections::HashMap;

#[tokio::test]
async fn teardown_returns_valid_fd_and_residual() {
    let (_mock, client, events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    let result = client
        .teardown_for_reexec()
        .await;
    assert!(result.is_ok(), "teardown failed: {:?}", result.err());

    let handoff = result.unwrap();
    assert!(handoff.fd() >= 0, "fd should be non-negative");
    let residual = handoff.residual();
    // No data was in-flight, so residual should be empty
    assert!(
        residual.is_empty(),
        "expected empty residual, got {} bytes",
        residual.len()
    );

    // Connection should be marked as disconnected
    assert!(!client.is_connected());
    assert_eq!(
        client.status(),
        ConnectionStatus::Disconnected(DisconnectReason::ReexecTeardown)
    );
    // Event stream must also see ReexecTeardown -- this is what downstream
    // consumers check when the event channel closes.
    assert_eq!(
        events.status(),
        ConnectionStatus::Disconnected(DisconnectReason::ReexecTeardown)
    );
}

/// After a successful teardown the socket belongs to the next process image;
/// a `disconnect()` on a remaining clone must not shut it down.
#[tokio::test]
async fn disconnect_after_teardown_leaves_the_handed_over_socket_open() {
    let (mut mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    let clone = client.clone();
    let _handoff = client
        .teardown_for_reexec()
        .await
        .expect("teardown succeeds");

    clone
        .disconnect()
        .await
        .expect("disconnect on an ended connection is not an error");
    assert_eq!(
        clone.status(),
        ConnectionStatus::Disconnected(DisconnectReason::ReexecTeardown)
    );
    // The switch side sees no EOF: nothing arrives, so the read times out.
    let read =
        tokio::time::timeout(std::time::Duration::from_millis(300), mock.read_command()).await;
    assert!(read.is_err(), "the switch saw the socket close: {read:?}");
}

#[tokio::test]
async fn teardown_with_buffered_event_delivers_then_stops() {
    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    // Send an event then immediately teardown.
    // The event should be delivered before teardown completes.
    let mut headers = HashMap::new();
    headers.insert("Unique-ID".to_string(), "reexec-test-uuid".to_string());
    mock.send_event_plain("HEARTBEAT", &headers)
        .await
        .unwrap();

    // Nothing reports that the reader has taken the bytes off the socket, and
    // a teardown that beats them there would drain an empty parser.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let handoff = client
        .teardown_for_reexec()
        .await
        .unwrap();
    assert!(handoff.fd() >= 0);

    // The event should have been dispatched before the drain completed
    let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv()).await;
    assert!(
        event.is_ok(),
        "event should have been dispatched before drain"
    );
}

#[tokio::test]
async fn teardown_twice_fails() {
    let (_mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    let first = client
        .teardown_for_reexec()
        .await;
    assert!(first.is_ok());

    // The first handed the socket over, so the connection is not usable.
    let second = client
        .teardown_for_reexec()
        .await;
    match second {
        Err(EslError::ReexecReaderLost { .. }) => {}
        other => panic!("expected ReexecReaderLost, got: {other:?}"),
    }
}

/// A connection already ended, here by `disconnect()`, has nothing to hand
/// over; the refusal must not read as a usable connection.
#[tokio::test]
async fn teardown_after_disconnect_is_a_lost_reader() {
    let (_mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    client
        .disconnect()
        .await
        .unwrap();
    let err = client
        .teardown_for_reexec()
        .await
        .expect_err("the connection ended");
    assert!(matches!(err, EslError::ReexecReaderLost { .. }), "{err:?}");
    assert_eq!(
        client.status(),
        ConnectionStatus::Disconnected(DisconnectReason::ClientRequested)
    );
}

/// A command dropped mid-write left the stream unframeable: teardown cannot
/// hand it over, and must say so rather than report a command in flight that
/// will never finish.
#[tokio::test]
async fn teardown_after_a_command_dropped_mid_write_closes() {
    use std::time::Duration;

    let (_mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    // The mock never reads, so this cannot finish writing.
    let huge = "x".repeat(32 * 1024 * 1024);
    let stuck = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .api_with_timeout(&huge, None)
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

    let err = tokio::time::timeout(Duration::from_secs(5), client.teardown_for_reexec())
        .await
        .expect("teardown returns")
        .expect_err("the stream is unframeable");
    assert!(matches!(err, EslError::ReexecReaderLost { .. }), "{err:?}");
    assert!(
        matches!(
            client.status(),
            ConnectionStatus::Disconnected(DisconnectReason::ProtocolError(_))
        ),
        "{:?}",
        client.status()
    );
}

/// An owed count read back after `exec()` is caller input: an absurd one is a
/// counter, not a ticket per reply, and every reply goes to the stream late.
#[tokio::test]
async fn an_absurd_owed_reply_count_neither_panics_nor_allocates() {
    use std::time::Duration;

    let (listener, port) = setup_raw_pair().await;
    let accept = tokio::spawn(async move {
        let (stream, _) = listener
            .accept()
            .await
            .unwrap();
        MockConnection::from_stream(stream)
    });
    let stream = tokio::net::TcpStream::connect(("localhost", port))
        .await
        .unwrap();
    let mut mock = accept
        .await
        .unwrap();

    let options = EslConnectOptions::new().with_owed_replies(usize::MAX);
    let (_client, mut events) = EslClient::adopt_stream_with_options(stream, &[], options).unwrap();
    mock.reply_api("owed-reply")
        .await
        .unwrap();
    match tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("the reply arrives")
    {
        Some(Err(EslError::LateReply { response, .. })) => {
            assert_eq!(response.body(), Some("owed-reply"))
        }
        other => panic!("expected LateReply, got {other:?}"),
    }
}

#[tokio::test]
async fn teardown_with_pending_command_fails() {
    let (mut mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    // Start a command but don't reply from mock
    let client2 = client.clone();
    let cmd_handle = tokio::spawn(async move {
        client2
            .api("status")
            .await
    });

    // The waiter goes into the slot before the bytes are written, so reading
    // the command is what proves teardown will find one in flight.
    let _cmd = mock
        .read_command()
        .await
        .unwrap();

    let result = client
        .teardown_for_reexec()
        .await;
    assert!(result.is_err());
    match result.unwrap_err() {
        EslError::ReexecFailed { reason, .. } => {
            assert!(
                reason.contains("in-flight"),
                "unexpected reason: {}",
                reason
            );
        }
        e => panic!("expected ReexecFailed, got: {}", e),
    }

    // Clean up: reply to the pending command so the task completes
    mock.reply_api("OK")
        .await
        .unwrap();
    let _ = cmd_handle.await;
}

#[tokio::test]
async fn adopt_stream_with_empty_residual() {
    // Set up a mock that acts as an already-authenticated server
    let (listener, port) = setup_raw_pair().await;

    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener
            .accept()
            .await
            .unwrap();
        MockConnection::from_stream(stream)
    });

    let stream = tokio::net::TcpStream::connect(("localhost", port))
        .await
        .unwrap();

    let mut mock = server_handle
        .await
        .unwrap();

    // adopt_stream bypasses auth
    let (client, mut events) = EslClient::adopt_stream(stream, &[]).unwrap();
    assert!(client.is_connected());

    // Send an event from mock, verify it arrives
    let mut headers = HashMap::new();
    headers.insert("Unique-ID".to_string(), "adopted-uuid".to_string());
    mock.send_event_plain("CHANNEL_CREATE", &headers)
        .await
        .unwrap();

    let event = recv_event(&mut events).await;
    assert_eq!(event.event_type(), Some(EslEventType::ChannelCreate));
}

#[tokio::test]
async fn adopt_stream_with_residual_bytes() {
    let (listener, port) = setup_raw_pair().await;

    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener
            .accept()
            .await
            .unwrap();
        MockConnection::from_stream(stream)
    });

    let stream = tokio::net::TcpStream::connect(("localhost", port))
        .await
        .unwrap();

    let _mock = server_handle
        .await
        .unwrap();

    // Pre-seed with a complete event in the residual buffer
    let event_body = "Event-Name: HEARTBEAT\nCore-UUID: test-core\n\n";
    let envelope = format!(
        "Content-Length: {}\nContent-Type: text/event-plain\n\n{}",
        event_body.len(),
        event_body
    );

    let (_client, mut events) = EslClient::adopt_stream(stream, envelope.as_bytes()).unwrap();

    // The pre-seeded event should be delivered from the residual buffer
    let event = recv_event(&mut events).await;
    assert_eq!(event.event_type(), Some(EslEventType::Heartbeat));
}

#[tokio::test]
async fn adopt_stream_with_options() {
    let (listener, port) = setup_raw_pair().await;

    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener
            .accept()
            .await
            .unwrap();
        MockConnection::from_stream(stream)
    });

    let stream = tokio::net::TcpStream::connect(("localhost", port))
        .await
        .unwrap();

    let _mock = server_handle
        .await
        .unwrap();

    let options = EslConnectOptions::new().with_event_queue_size(10);
    let (client, _events) = EslClient::adopt_stream_with_options(stream, &[], options).unwrap();
    assert!(client.is_connected());
}

#[tokio::test]
// qual:allow(complexity, unsafe) reason: "fd dup/close is what the re-exec handoff test exercises"
async fn sequential_teardown_adopt_teardown() {
    use std::os::unix::io::FromRawFd;

    let (_mock, client, events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    // First teardown
    let handoff = client
        .teardown_for_reexec()
        .await
        .expect("first teardown failed");
    let fd = handoff.fd();
    assert_eq!(
        events.status(),
        ConnectionStatus::Disconnected(DisconnectReason::ReexecTeardown)
    );

    // Prevent EslClient from closing the fd on drop (as documented).
    // In production the process calls exec(), destroying the old runtime.
    std::mem::forget(client);

    // dup() the fd so the new TcpStream gets a fresh reactor registration
    // (the original fd is still registered by the forgotten OwnedWriteHalf).
    let new_fd = unsafe { libc::dup(fd) };
    assert!(
        new_fd >= 0,
        "dup() failed: {}",
        std::io::Error::last_os_error()
    );
    // Close the original fd that was leaked by mem::forget
    unsafe { libc::close(fd) };

    let std_stream = unsafe { std::net::TcpStream::from_raw_fd(new_fd) };
    std_stream
        .set_nonblocking(true)
        .expect("set_nonblocking failed");
    let stream = tokio::net::TcpStream::from_std(std_stream).expect("from_std failed");
    let (client2, events2) = EslClient::adopt_stream(stream, handoff.residual()).unwrap();
    assert!(client2.is_connected());

    // Second teardown
    let _handoff2 = client2
        .teardown_for_reexec()
        .await
        .expect("second teardown failed");
    assert_eq!(
        events2.status(),
        ConnectionStatus::Disconnected(DisconnectReason::ReexecTeardown)
    );
}

#[tokio::test]
async fn teardown_wins_race_against_stalled_dispatch() {
    use std::time::Duration;

    let options = EslConnectOptions::new()
        .with_event_queue_size(1)
        .with_event_overflow(EventOverflow::BlockFor(Duration::from_secs(30)));
    let (mut mock, client, mut events) =
        setup_connected_pair_with_options(DEFAULT_ESL_PASSWORD, options).await;

    let sent = 3;
    for i in 0..sent {
        let mut headers = HashMap::new();
        headers.insert("Unique-ID".to_string(), format!("uuid-{}", i));
        mock.send_event_plain("CHANNEL_CREATE", &headers)
            .await
            .unwrap();
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while client.event_stall_count() == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(client.event_stall_count() > 0, "reader never stalled");

    // The stop signal must cut the wait short rather than let teardown run out
    // its own drain budget.
    let result = tokio::time::timeout(Duration::from_secs(3), client.teardown_for_reexec())
        .await
        .expect("teardown did not beat the stalled dispatch");
    let handoff = result.expect("teardown failed");
    assert!(handoff.fd() >= 0);

    let mut received = 0u64;
    while let Ok(Some(Ok(_))) =
        tokio::time::timeout(Duration::from_millis(200), events.recv()).await
    {
        received += 1;
    }
    assert_eq!(
        received + client.dropped_event_count(),
        sent as u64,
        "every event is either delivered or counted as dropped"
    );
}

/// A drain the switch cuts short mid-message ends the reader; the status must
/// say so rather than read `Connected` over a reader that is gone.
#[tokio::test]
async fn a_failed_drain_publishes_the_disconnect() {
    use std::time::Duration;

    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    // One write: the reader takes the partial message with the heartbeat, so
    // the drain starts mid-body.
    mock.send_raw(concat!(
        "Content-Type: text/event-plain\nContent-Length: 21\n\nEvent-Name: HEARTBEAT\n",
        "Content-Type: text/event-plain\nContent-Length: 100\n\npartial",
    ))
    .await
    .unwrap();
    let event = recv_event(&mut events).await;
    assert_eq!(event.event_type(), Some(EslEventType::Heartbeat));

    let teardown = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .teardown_for_reexec()
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    mock.drop_connection()
        .await;

    let err = tokio::time::timeout(Duration::from_secs(10), teardown)
        .await
        .expect("teardown returns")
        .unwrap()
        .expect_err("the body never completed");
    // Read before anything else runs: the reader publishes before it reports.
    let status = client.status();
    assert!(matches!(err, EslError::ReexecReaderLost { .. }), "{err:?}");
    assert!(
        err.is_connection_error() && !err.is_recoverable(),
        "{err:?}"
    );
    assert_eq!(
        status,
        ConnectionStatus::Disconnected(DisconnectReason::ConnectionClosed)
    );
    let end = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("the reader exits");
    assert!(end.is_none(), "{end:?}");
}

/// A reader that exits mid-drain without a result for teardown, here on a
/// framing fault, leaves the status set by the time teardown reports it.
#[tokio::test]
async fn a_reader_lost_mid_drain_is_published_before_teardown_returns() {
    use std::time::Duration;

    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    mock.send_raw(concat!(
        "Content-Type: text/event-plain\nContent-Length: 21\n\nEvent-Name: HEARTBEAT\n",
        "Content-Type: text/event-plain\nContent-Length: 4\n\n",
    ))
    .await
    .unwrap();
    let event = recv_event(&mut events).await;
    assert_eq!(event.event_type(), Some(EslEventType::Heartbeat));

    let teardown = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .teardown_for_reexec()
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    // Completes the body, then a message whose framing the parser refuses.
    mock.send_raw("abcdContent-Type: text/garbage\n\n")
        .await
        .unwrap();

    let err = tokio::time::timeout(Duration::from_secs(10), teardown)
        .await
        .expect("teardown returns")
        .unwrap()
        .expect_err("the reader stopped mid-drain");
    let status = client.status();
    assert!(matches!(err, EslError::ReexecReaderLost { .. }), "{err:?}");
    assert!(
        matches!(
            status,
            ConnectionStatus::Disconnected(DisconnectReason::ProtocolError(_))
        ),
        "{status:?}"
    );
}

/// A drain that outlasts teardown's own deadline is abandoned with it: the
/// caller gets `ReexecReaderLost` with the status already saying the connection
/// ended, and the reader stops rather than finish later as `ReexecTeardown`.
#[tokio::test]
async fn a_drain_past_the_teardown_deadline_is_abandoned() {
    use std::time::Duration;

    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    mock.send_raw(concat!(
        "Content-Type: text/event-plain\nContent-Length: 21\n\nEvent-Name: HEARTBEAT\n",
        "Content-Type: text/event-plain\nContent-Length: 40\n\n",
    ))
    .await
    .unwrap();
    let event = recv_event(&mut events).await;
    assert_eq!(event.event_type(), Some(EslEventType::Heartbeat));

    let teardown = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .teardown_for_reexec()
                .await
        }
    });
    // Each byte lands inside the reader's per-read drain timeout, so only
    // teardown's overall deadline ends the drain.
    let mut sent = 0;
    while !teardown.is_finished() && sent < 30 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        mock.send_raw("x")
            .await
            .unwrap();
        sent += 1;
    }
    let err = teardown
        .await
        .unwrap()
        .expect_err("the body never completed in time");
    assert!(matches!(err, EslError::ReexecReaderLost { .. }), "{err:?}");
    let status = client.status();
    assert!(
        matches!(
            status,
            ConnectionStatus::Disconnected(DisconnectReason::ProtocolError(_))
        ),
        "{status:?}"
    );

    // Well inside the reader's own drain timeout, which a reader still
    // draining would wait out.
    let end = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(item) = events
            .recv()
            .await
        {
            drop(item);
        }
    })
    .await;
    assert!(end.is_ok(), "the reader kept running");
    assert_eq!(client.status(), status);
}

/// What the next process image gets: the handed-over socket, dup'ed so it
/// registers afresh with the reactor, the old client forgotten as `exec()`
/// would.
// qual:allow(unsafe) reason: "fd dup/close is what the re-exec handoff test exercises"
fn adopt_handoff(
    client: EslClient,
    handoff: &freeswitch_esl_tokio::ReexecHandoff,
    options: EslConnectOptions,
) -> (EslClient, freeswitch_esl_tokio::EslEventStream) {
    use std::os::unix::io::FromRawFd;

    let fd = handoff.fd();
    std::mem::forget(client);
    let new_fd = unsafe { libc::dup(fd) };
    assert!(
        new_fd >= 0,
        "dup() failed: {}",
        std::io::Error::last_os_error()
    );
    unsafe { libc::close(fd) };
    let std_stream = unsafe { std::net::TcpStream::from_raw_fd(new_fd) };
    std_stream
        .set_nonblocking(true)
        .expect("set_nonblocking failed");
    let stream = tokio::net::TcpStream::from_std(std_stream).expect("from_std failed");
    EslClient::adopt_stream_with_options(stream, handoff.residual(), options).unwrap()
}

/// Tears down with one timed-out command's reply still owed, adopts the
/// socket with or without that count, and has the switch answer the owed
/// command and then the new process's first command. Returns that command's
/// result and the next item on the new event stream.
async fn owed_reply_across_reexec(
    carry_count: bool,
) -> (
    Result<freeswitch_esl_tokio::EslResponse, EslError>,
    Result<freeswitch_esl_tokio::EslEvent, EslError>,
) {
    use std::time::Duration;

    let (mut mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    client.set_command_timeout(Duration::from_millis(100));
    let (timed_out, _) = tokio::join!(client.api("msleep 3000"), mock.read_command());
    assert!(
        matches!(timed_out, Err(EslError::CommandTimeout { .. })),
        "{timed_out:?}"
    );

    let handoff = client
        .teardown_for_reexec()
        .await
        .expect("a timed-out command does not block teardown");
    assert_eq!(handoff.owed_replies(), 1);
    let options = if carry_count {
        EslConnectOptions::new().with_owed_replies(handoff.owed_replies())
    } else {
        EslConnectOptions::new()
    };
    let (client, mut events) = adopt_handoff(client, &handoff, options);

    let first = tokio::spawn({
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
    mock.reply_api("owed-reply")
        .await
        .unwrap();
    mock.reply_api("own-reply")
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), first)
        .await
        .expect("the command returns")
        .unwrap();
    let next = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("an item arrives")
        .expect("the stream stays open");
    (result, next)
}

/// With the count carried across, the owed reply comes out as a `LateReply`
/// and the new process's command gets its own.
#[tokio::test]
async fn an_owed_reply_carried_across_reexec_stays_late() {
    let (result, next) = owed_reply_across_reexec(true).await;
    assert_eq!(
        result
            .expect("the command gets a reply")
            .body(),
        Some("own-reply")
    );
    match next {
        Err(EslError::LateReply { response, .. }) => {
            assert_eq!(response.body(), Some("owed-reply"))
        }
        other => panic!("expected LateReply, got {other:?}"),
    }
}

/// Control: without the count the new process takes the owed reply as its
/// first command's, and its own reply is left over.
#[tokio::test]
async fn an_owed_reply_not_carried_across_reexec_is_misrouted() {
    let (result, next) = owed_reply_across_reexec(false).await;
    assert_eq!(
        result
            .expect("the command gets a reply")
            .body(),
        Some("owed-reply")
    );
    assert!(
        matches!(next, Err(EslError::UnsolicitedReply { .. })),
        "{next:?}"
    );
}
