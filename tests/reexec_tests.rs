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

    let (fd, residual) = result.unwrap();
    assert!(fd >= 0, "fd should be non-negative");
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

    let (fd, _residual) = client
        .teardown_for_reexec()
        .await
        .unwrap();
    assert!(fd >= 0);

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

    let second = client
        .teardown_for_reexec()
        .await;
    assert!(second.is_err());
    match second.unwrap_err() {
        EslError::ReexecFailed { reason } => {
            assert!(
                reason.contains("already called"),
                "unexpected reason: {}",
                reason
            );
        }
        e => panic!("expected ReexecFailed, got: {}", e),
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
        EslError::ReexecFailed { reason } => {
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
    let (fd, residual) = client
        .teardown_for_reexec()
        .await
        .expect("first teardown failed");
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
    let (client2, events2) = EslClient::adopt_stream(stream, &residual).unwrap();
    assert!(client2.is_connected());

    // Second teardown
    let (_fd2, _residual2) = client2
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
    let (fd, _residual) = result.expect("teardown failed");
    assert!(fd >= 0);

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
