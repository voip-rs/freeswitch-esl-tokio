//! The published `mock` module, driven through its public path the way a
//! downstream crate would.

use freeswitch_esl_tokio::mock::{MockBackgroundJob, MockEslServer};
use freeswitch_esl_tokio::{BgJobTracker, EslClient, EslError, LogLevel, DEFAULT_ESL_PASSWORD};
use std::io::ErrorKind;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

async fn recv(events: &mut freeswitch_esl_tokio::EslEventStream) -> freeswitch_esl_tokio::EslEvent {
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timeout")
        .expect("channel closed")
        .expect("event error")
}

#[tokio::test]
async fn wrong_password_is_rejected_on_both_ends() {
    let server = MockEslServer::start("correct_password")
        .await
        .unwrap();

    let (accepted, connected) = tokio::join!(
        server.accept(),
        EslClient::connect("localhost", server.port(), "wrong_password")
    );

    assert_eq!(
        accepted
            .unwrap_err()
            .kind(),
        ErrorKind::PermissionDenied
    );
    assert!(matches!(
        connected,
        Err(EslError::AuthenticationFailed { .. })
    ));
}

#[tokio::test]
async fn explicit_bind_address() {
    let server = MockEslServer::bind("127.0.0.1:0", DEFAULT_ESL_PASSWORD)
        .await
        .unwrap();
    assert!(server
        .addr()
        .ip()
        .is_ipv4());

    let (accepted, connected) = tokio::join!(
        server.accept(),
        EslClient::connect("127.0.0.1", server.port(), DEFAULT_ESL_PASSWORD)
    );
    accepted.unwrap();
    connected.unwrap();
}

#[tokio::test]
async fn accepts_again_after_a_dropped_connection() {
    let server = MockEslServer::start(DEFAULT_ESL_PASSWORD)
        .await
        .unwrap();

    for _ in 0..2 {
        let (accepted, connected) = tokio::join!(
            server.accept(),
            EslClient::connect("localhost", server.port(), DEFAULT_ESL_PASSWORD)
        );
        let (_client, mut events) = connected.unwrap();
        accepted
            .unwrap()
            .drop_connection()
            .await;
        assert!(tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("timeout")
            .is_none());
    }
}

#[tokio::test]
async fn read_command_reports_hangup() {
    let server = MockEslServer::start(DEFAULT_ESL_PASSWORD)
        .await
        .unwrap();
    let (accepted, connected) = tokio::join!(
        server.accept(),
        EslClient::connect("localhost", server.port(), DEFAULT_ESL_PASSWORD)
    );
    let mut conn = accepted.unwrap();
    drop(connected.unwrap());

    assert_eq!(
        conn.read_command()
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::UnexpectedEof
    );
}

#[tokio::test]
async fn pipelined_commands_are_read_in_turn() {
    let server = MockEslServer::start(DEFAULT_ESL_PASSWORD)
        .await
        .unwrap();
    // One write carries the handshake and both commands, so they share a segment.
    let (accepted, _raw) = tokio::join!(server.accept(), async {
        let mut raw = tokio::net::TcpStream::connect(("localhost", server.port()))
            .await
            .unwrap();
        raw.write_all(
            format!("auth {DEFAULT_ESL_PASSWORD}\n\napi status\n\napi version\n\n").as_bytes(),
        )
        .await
        .unwrap();
        raw
    });
    let mut conn = accepted.unwrap();

    assert_eq!(
        conn.read_command()
            .await
            .unwrap(),
        "api status\n\n"
    );
    assert_eq!(
        conn.read_command()
            .await
            .unwrap(),
        "api version\n\n"
    );
}

#[tokio::test]
async fn bgapi_completes_through_the_tracker() {
    let server = MockEslServer::start(DEFAULT_ESL_PASSWORD)
        .await
        .unwrap();
    let (accepted, connected) = tokio::join!(
        server.accept(),
        EslClient::connect("localhost", server.port(), DEFAULT_ESL_PASSWORD)
    );
    let mut conn = accepted.unwrap();
    let (client, mut events) = connected.unwrap();

    let job_uuid = "7f4db78a-17d7-11dd-b7a0-db4edd065621";
    let mut tracker = BgJobTracker::new();
    let (sent, cmd) = tokio::join!(tracker.bgapi(&client, "status", "ctx"), async {
        let cmd = conn
            .read_command()
            .await
            .unwrap();
        conn.reply_bgapi(job_uuid)
            .await
            .unwrap();
        cmd
    });
    assert_eq!(cmd, "bgapi status\n\n");
    assert_eq!(sent.unwrap(), job_uuid);

    conn.send_background_job(&MockBackgroundJob::new(job_uuid, "status"), "+OK up\n")
        .await
        .unwrap();
    let event = recv(&mut events).await;
    assert_eq!(event.header_str("Job-Command"), Some("status"));
    let (ctx, result) = tracker
        .try_complete(&event)
        .expect("tracked job");
    assert_eq!(ctx, "ctx");
    assert_eq!(result.body(), Some("+OK up\n"));
}

#[tokio::test]
async fn log_data_arrives_with_level_and_file() {
    let server = MockEslServer::start(DEFAULT_ESL_PASSWORD)
        .await
        .unwrap();
    let (accepted, connected) = tokio::join!(
        server.accept(),
        EslClient::connect("localhost", server.port(), DEFAULT_ESL_PASSWORD)
    );
    let mut conn = accepted.unwrap();
    let (_client, mut events) = connected.unwrap();

    conn.send_log(LogLevel::Warning, "mod_sofia.c", "registration failed\n")
        .await
        .unwrap();
    let event = recv(&mut events).await;
    assert_eq!(event.header_str("Log-Level"), Some("4"));
    assert_eq!(event.header_str("Log-File"), Some("mod_sofia.c"));
    assert_eq!(event.body(), Some("registration failed\n"));
}
