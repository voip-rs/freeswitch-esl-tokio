//! The reader's log lines against the credential rule. Alone in its binary,
//! since it installs the process-wide tracing subscriber.

mod mock_server;

use freeswitch_esl_tokio::{EslError, DEFAULT_ESL_PASSWORD};
use mock_server::setup_connected_pair;
use std::time::Duration;

/// Collects formatted log lines for one test.
#[derive(Clone, Default)]
struct CapturedLog(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for CapturedLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap()
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A late reply's text is subscriber data (a `getvar` value): the discard
/// warning names its length, never the text.
#[tokio::test]
async fn stale_reply_warning_does_not_quote_the_reply_text() {
    let log = CapturedLog::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("the only subscriber in this binary");

    let (mut mock, client, _events) = setup_connected_pair(DEFAULT_ESL_PASSWORD).await;
    client.set_command_timeout(Duration::from_millis(100));
    let late = tokio::spawn({
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
    assert!(matches!(
        late.await
            .unwrap(),
        Err(EslError::Timeout { .. })
    ));

    client.set_command_timeout(Duration::from_secs(5));
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
    mock.reply_raw_text("late-getvar-sentinel")
        .await
        .unwrap();
    mock.reply_api("1.0")
        .await
        .unwrap();
    let reply = next
        .await
        .unwrap()
        .expect("the reply after the stale one reaches its command");
    assert_eq!(reply.body(), Some("1.0"));

    let text = String::from_utf8(
        log.0
            .lock()
            .unwrap()
            .clone(),
    )
    .unwrap();
    assert!(
        text.contains("Discarded stale"),
        "no discard warning: {text}"
    );
    assert!(!text.contains("late-getvar-sentinel"), "{text}");
}
