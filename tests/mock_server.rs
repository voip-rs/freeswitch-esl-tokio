//! Shared integration-test scaffolding around the published `mock` module.
#![allow(dead_code)]

pub use freeswitch_esl_tokio::mock::{MockConnection, MockEslServer};
use std::time::Duration;
use tokio::net::TcpListener;

/// Bind an ephemeral dual-stack listener and hand back the port to dial it on.
///
/// For the tests that drive the socket themselves -- outbound accept, re-exec
/// adoption, connection-refused -- rather than going through
/// [`MockEslServer`].
pub async fn setup_raw_pair() -> (TcpListener, u16) {
    let listener = TcpListener::bind("[::]:0")
        .await
        .expect("bind an ephemeral listener");
    let port = listener
        .local_addr()
        .expect("a bound listener has a local address")
        .port();
    (listener, port)
}

/// Create a connected mock pair (MockConnection, EslClient, EslEventStream)
pub async fn setup_connected_pair(
    password: &str,
) -> (
    MockConnection,
    freeswitch_esl_tokio::EslClient,
    freeswitch_esl_tokio::EslEventStream,
) {
    setup_connected_pair_with_options(password, freeswitch_esl_tokio::EslConnectOptions::default())
        .await
}

/// Create a connected mock pair with custom options
pub async fn setup_connected_pair_with_options(
    password: &str,
    options: freeswitch_esl_tokio::EslConnectOptions,
) -> (
    MockConnection,
    freeswitch_esl_tokio::EslClient,
    freeswitch_esl_tokio::EslEventStream,
) {
    let server = MockEslServer::start(password)
        .await
        .unwrap();
    let port = server.port();

    let (mock_client, esl_result) = tokio::join!(
        server.accept(),
        freeswitch_esl_tokio::EslClient::connect_with_options("localhost", port, password, options)
    );

    let (esl_client, esl_events) = esl_result.unwrap();
    (mock_client.unwrap(), esl_client, esl_events)
}

/// Receive the next event with a 5s timeout, unwrapping every layer down to
/// the `EslEvent`. Panics with a descriptive message on timeout, channel
/// close, or event error -- the common case in tests that expect success.
pub async fn recv_event(
    events: &mut freeswitch_esl_tokio::EslEventStream,
) -> freeswitch_esl_tokio::EslEvent {
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timeout")
        .expect("channel closed")
        .expect("event error")
}

/// Shared scaffolding for the `inflight_command_woken_on_*` tests: spawn an
/// `api("status")` call, let the mock read it, then run `action` (the
/// differing disconnect/notice/rejection/desync trigger) and return the
/// error the in-flight command failed with.
///
/// `action` takes ownership of `mock` and hands it back as `Some` unless it
/// closed the connection itself (`drop_connection` consumes it) -- the
/// returned value is held alive until the join completes, so a no-op or
/// non-closing action doesn't drop the socket out from under a test that
/// depends on the connection staying nominally open (e.g. liveness expiry).
pub async fn inflight_command_woken_on<F, Fut>(
    client: &freeswitch_esl_tokio::EslClient,
    mut mock: MockConnection,
    join_timeout: Duration,
    action: F,
) -> freeswitch_esl_tokio::EslError
where
    F: FnOnce(MockConnection) -> Fut,
    Fut: std::future::Future<Output = Option<MockConnection>>,
{
    let api_task = tokio::spawn({
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
    let _mock = action(mock).await;

    tokio::time::timeout(join_timeout, api_task)
        .await
        .expect("in-flight command still blocked after the triggering action")
        .expect("api task panicked")
        .expect_err("command should fail after the triggering action")
}
