//! Integration tests using mock ESL server: connect/authenticate, event
//! delivery, concurrent commands, client clone, header decoding, event queue
//! sizing, connect-refused, and connection mode.

mod mock_server;

use freeswitch_esl_tokio::{
    ConnectionMode, EslClient, EslConnectOptions, EslError, EslEventType, EventHeader,
    EventOverflow, HeaderLookup, DEFAULT_ESL_PASSWORD,
};
use mock_server::{
    recv_event, setup_connected_pair, setup_connected_pair_with_options, setup_raw_pair,
};
use std::collections::HashMap;
use std::time::Duration;
use tokio::net::TcpSocket;

#[tokio::test]
async fn test_connect_and_authenticate() {
    let (_, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    assert!(client.is_connected());
}

#[tokio::test]
async fn test_recv_event_plain() {
    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    // Subscribe to events (mock just replies OK)
    let subscribe_task = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .subscribe_events(
                    freeswitch_esl_tokio::EventFormat::Plain,
                    &[EslEventType::All],
                )
                .await
                .unwrap();
        }
    });

    // Mock reads the subscribe command and replies
    let _cmd = mock
        .read_command()
        .await
        .unwrap();
    mock.reply_ok()
        .await
        .unwrap();
    subscribe_task
        .await
        .unwrap();

    // Send an event from mock
    let mut headers = HashMap::new();
    headers.insert("Unique-ID".to_string(), "test-uuid-abc".to_string());
    headers.insert("Caller-Caller-ID-Number".to_string(), "1001".to_string());
    mock.send_event_plain("CHANNEL_CREATE", &headers)
        .await
        .unwrap();

    let event = recv_event(&mut events).await;
    assert_eq!(event.event_type(), Some(EslEventType::ChannelCreate));
    assert_eq!(event.unique_id(), Some("test-uuid-abc"));
}

#[tokio::test]
async fn test_concurrent_command_and_events() {
    let (mut mock, client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    // Send an event from mock first (before any command)
    let mut headers = HashMap::new();
    headers.insert("Unique-ID".to_string(), "event-uuid".to_string());
    mock.send_event_plain("CHANNEL_CREATE", &headers)
        .await
        .unwrap();

    // Now send an api command
    let api_task = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .api("status")
                .await
                .unwrap()
        }
    });

    // Mock reads the api command and replies
    let cmd = mock
        .read_command()
        .await
        .unwrap();
    assert!(cmd.starts_with("api status"));
    mock.reply_api("UP 0 years")
        .await
        .unwrap();

    let response = api_task
        .await
        .unwrap();
    assert_eq!(response.body(), Some("UP 0 years"));

    // The event should still be available
    let event = recv_event(&mut events).await;
    assert_eq!(event.event_type(), Some(EslEventType::ChannelCreate));
}

#[tokio::test]
async fn test_concurrent_api_commands() {
    let (mut mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    // Launch two api() calls concurrently from different tasks
    let client1 = client.clone();
    let client2 = client.clone();
    let task1 = tokio::spawn(async move {
        client1
            .api("status")
            .await
    });
    let task2 = tokio::spawn(async move {
        client2
            .api("version")
            .await
    });

    // The writer mutex serializes them: read cmd1, reply, read cmd2, reply
    let cmd1 = mock
        .read_command()
        .await
        .unwrap();
    assert!(cmd1.starts_with("api "), "first command: {}", cmd1);
    mock.reply_api("response-1")
        .await
        .unwrap();

    let cmd2 = mock
        .read_command()
        .await
        .unwrap();
    assert!(cmd2.starts_with("api "), "second command: {}", cmd2);
    mock.reply_api("response-2")
        .await
        .unwrap();

    let result1 = task1
        .await
        .unwrap()
        .unwrap();
    let result2 = task2
        .await
        .unwrap()
        .unwrap();

    // Both should succeed with their respective responses
    let bodies: Vec<&str> = vec![
        result1
            .body()
            .unwrap(),
        result2
            .body()
            .unwrap(),
    ];
    assert!(bodies.contains(&"response-1"));
    assert!(bodies.contains(&"response-2"));
}

#[tokio::test]
async fn test_client_clone() {
    let (mut mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    let client2 = client.clone();

    // Send command from clone
    let task = tokio::spawn(async move {
        client2
            .api("status")
            .await
    });

    let cmd = mock
        .read_command()
        .await
        .unwrap();
    assert!(cmd.starts_with("api status"));
    mock.reply_api("OK")
        .await
        .unwrap();

    let result = task
        .await
        .unwrap()
        .expect("the clone's command should succeed");
    assert_eq!(result.body(), Some("OK"));

    // Original client should also work
    let task2 = tokio::spawn(async move {
        client
            .api("version")
            .await
    });

    let cmd2 = mock
        .read_command()
        .await
        .unwrap();
    assert!(cmd2.starts_with("api version"));
    mock.reply_api("1.0")
        .await
        .unwrap();

    let result2 = task2
        .await
        .unwrap()
        .expect("the original client's command should succeed");
    assert_eq!(result2.body(), Some("1.0"));
}

#[tokio::test]
async fn test_heartbeat_event_headers() {
    let (mut mock, _client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    mock.send_heartbeat()
        .await
        .unwrap();

    let event = recv_event(&mut events).await;

    assert_eq!(event.event_type(), Some(EslEventType::Heartbeat));
    // Values should be percent-decoded
    assert_eq!(event.header_str("Event-Info"), Some("System Ready"));
    assert_eq!(
        event.header_str("Up-Time"),
        Some("0 years, 0 days, 1 hour, 23 minutes")
    );
    assert_eq!(event.header_str("Session-Count"), Some("5"));
    assert_eq!(event.header_str("Heartbeat-Interval"), Some("20"));
}

#[tokio::test]
async fn test_url_decoded_headers() {
    let (mut mock, _client, mut events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;

    let mut headers = HashMap::new();
    headers.insert("Caller-Caller-ID-Name".to_string(), "John Doe".to_string());
    headers.insert(
        "variable_sip_from_display".to_string(),
        "Test User (123)".to_string(),
    );
    mock.send_event_plain("CHANNEL_CREATE", &headers)
        .await
        .unwrap();

    let event = recv_event(&mut events).await;

    // Percent-encoded values should be decoded
    assert_eq!(
        event.header(EventHeader::CallerCallerIdName),
        Some("John Doe")
    );
    assert_eq!(
        event.header_str("variable_sip_from_display"),
        Some("Test User (123)")
    );
}

#[tokio::test]
async fn test_event_overflow_queue_full() {
    let options = EslConnectOptions::new().with_event_queue_size(2);
    let (mut mock, client, mut events) =
        setup_connected_pair_with_options(DEFAULT_ESL_PASSWORD, options).await;

    // Fill the queue (capacity 2) then overflow it.
    for i in 0..5 {
        let mut headers = HashMap::new();
        headers.insert("Unique-ID".to_string(), format!("uuid-{}", i));
        mock.send_event_plain("CHANNEL_CREATE", &headers)
            .await
            .unwrap();
    }

    // The overflow itself is the observable end of "the reader processed all
    // five": three of them could not fit.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while client.dropped_event_count() < 3 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Drain the 2 buffered events to make room in the channel.
    for _ in 0..2 {
        let result = tokio::time::timeout(Duration::from_millis(500), events.recv()).await;
        assert!(matches!(result, Ok(Some(Ok(_)))));
    }

    // QueueFull is delivered piggy-backed on the next dispatch_event call.
    // Send one more event to trigger it.
    let mut headers = HashMap::new();
    headers.insert("Unique-ID".to_string(), "uuid-trigger".to_string());
    mock.send_event_plain("CHANNEL_CREATE", &headers)
        .await
        .unwrap();

    // Should get QueueFull followed by the trigger event.
    let mut got_queue_full = false;
    let mut event_count = 0;
    loop {
        match tokio::time::timeout(Duration::from_millis(500), events.recv()).await {
            Ok(Some(Ok(_event))) => event_count += 1,
            Ok(Some(Err(EslError::QueueFull))) => got_queue_full = true,
            Ok(Some(Err(e))) => panic!("unexpected error: {}", e),
            Ok(None) => break,
            Err(_) => break,
        }
    }

    assert!(
        got_queue_full,
        "expected QueueFull notification (got {} events)",
        event_count
    );
    assert!(
        client.dropped_event_count() > 0,
        "dropped_event_count should be > 0"
    );
}

#[tokio::test]
async fn test_event_queue_size_zero_clamped() {
    let options = EslConnectOptions::new().with_event_queue_size(0);
    let (mut mock, _client, mut events) =
        setup_connected_pair_with_options(DEFAULT_ESL_PASSWORD, options).await;

    // Should still work: size 0 is clamped to 1
    let mut headers = HashMap::new();
    headers.insert("Unique-ID".to_string(), "test-uuid".to_string());
    mock.send_event_plain("CHANNEL_CREATE", &headers)
        .await
        .unwrap();

    let event = recv_event(&mut events).await;
    assert_eq!(event.event_type(), Some(EslEventType::ChannelCreate));
}

// --- TCP connection refused ---

#[tokio::test]
async fn connect_refused_returns_connection_error() {
    // Bound and never listening: the kernel refuses every dial to the port, and
    // no other test's ephemeral bind can take it while this socket holds it.
    let socket = TcpSocket::new_v6().expect("create an IPv6 socket");
    socket
        .bind(
            "[::]:0"
                .parse()
                .expect("a socket address"),
        )
        .expect("bind an ephemeral port");
    let port = socket
        .local_addr()
        .expect("a bound socket has a local address")
        .port();

    let err = EslClient::connect("localhost", port, "pw")
        .await
        .unwrap_err();
    assert!(
        err.is_connection_error(),
        "connection refused should be a connection error, got: {err}"
    );
}

#[tokio::test]
async fn test_connection_mode_inbound() {
    let (_, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    assert_eq!(client.connection_mode(), ConnectionMode::Inbound);
}

#[tokio::test]
async fn test_connection_mode_outbound() {
    use tokio::net::TcpStream;

    let (listener, port) = setup_raw_pair().await;

    let (accept_result, _mock_stream) = tokio::join!(
        EslClient::accept_outbound(&listener),
        TcpStream::connect(("localhost", port))
    );

    let (client, _events) = accept_result.unwrap();
    assert_eq!(client.connection_mode(), ConnectionMode::Outbound);
}

/// Send `count` CHANNEL_CREATE events carrying a sequential `Unique-ID`.
async fn send_numbered_events(mock: &mut mock_server::MockConnection, count: usize) {
    for i in 0..count {
        let mut headers = HashMap::new();
        headers.insert("Unique-ID".to_string(), format!("uuid-{}", i));
        mock.send_event_plain("CHANNEL_CREATE", &headers)
            .await
            .unwrap();
    }
}

/// Poll `probe` until it holds or the deadline passes.
async fn wait_until<F: Fn() -> bool>(probe: F, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if probe() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    probe()
}

#[tokio::test]
async fn block_mode_stalls_instead_of_dropping() {
    let options = EslConnectOptions::new()
        .with_event_queue_size(1)
        .with_event_overflow(EventOverflow::BlockFor(Duration::from_secs(30)));
    let (mut mock, client, mut events) =
        setup_connected_pair_with_options(DEFAULT_ESL_PASSWORD, options).await;

    send_numbered_events(&mut mock, 3).await;

    // The second event cannot fit and parks the reader on queue capacity.
    assert!(
        wait_until(|| client.event_stall_count() > 0, Duration::from_secs(5)).await,
        "reader never stalled"
    );
    assert_eq!(client.dropped_event_count(), 0, "block mode must not drop");

    for i in 0..3 {
        let event = recv_event(&mut events).await;
        assert_eq!(
            event.header_str("Unique-ID"),
            Some(format!("uuid-{}", i).as_str()),
            "events must arrive in wire order"
        );
    }
    assert!(client.event_stall_duration() > Duration::ZERO);
}

#[tokio::test]
async fn block_mode_falls_back_to_drop_past_budget() {
    let options = EslConnectOptions::new()
        .with_event_queue_size(1)
        .with_event_overflow(EventOverflow::BlockFor(Duration::from_millis(100)));
    let (mut mock, client, mut events) =
        setup_connected_pair_with_options(DEFAULT_ESL_PASSWORD, options).await;

    send_numbered_events(&mut mock, 4).await;

    assert!(
        wait_until(|| client.dropped_event_count() > 0, Duration::from_secs(5)).await,
        "budget expiry must fall back to dropping"
    );

    // The marker rides the next dispatch, so the queue has to be empty first.
    while (tokio::time::timeout(Duration::from_millis(300), events.recv()).await).is_ok() {}

    send_numbered_events(&mut mock, 1).await;
    let mut got_queue_full = false;
    while let Ok(Some(item)) = tokio::time::timeout(Duration::from_millis(500), events.recv()).await
    {
        if matches!(item, Err(EslError::QueueFull)) {
            got_queue_full = true;
        }
    }
    assert!(got_queue_full, "expected a QueueFull notification");
}

#[tokio::test]
async fn block_mode_stall_does_not_trip_liveness() {
    let options = EslConnectOptions::new()
        .with_event_queue_size(1)
        .with_event_overflow(EventOverflow::BlockFor(Duration::from_secs(30)));
    let (mut mock, client, mut events) =
        setup_connected_pair_with_options(DEFAULT_ESL_PASSWORD, options).await;
    client.set_liveness_timeout(Duration::from_secs(3));

    send_numbered_events(&mut mock, 2).await;
    assert!(
        wait_until(|| client.event_stall_count() > 0, Duration::from_secs(5)).await,
        "reader never stalled"
    );

    // Outlast the liveness threshold while parked, then release the reader.
    tokio::time::sleep(Duration::from_secs(5)).await;
    let _ = recv_event(&mut events).await;
    let _ = recv_event(&mut events).await;

    // Long enough for an idle tick to evaluate liveness after the stall.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(
        client.is_connected(),
        "stall time must not count against liveness: {:?}",
        client.status()
    );

    send_numbered_events(&mut mock, 1).await;
    let _ = recv_event(&mut events).await;
}
