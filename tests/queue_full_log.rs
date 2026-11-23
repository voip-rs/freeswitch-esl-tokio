//! The reader's queue-full warning, counted rather than repeated. Alone in its
//! binary, since it installs the process-wide tracing subscriber.

mod mock_server;

use std::collections::HashMap;
use std::time::Duration;

use freeswitch_esl_tokio::{EslConnectOptions, DEFAULT_ESL_PASSWORD};
use mock_server::setup_connected_pair_with_options;

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

/// An overloaded consumer drops many events in a row: one warning for the
/// run, the total on `dropped_event_count()`, not one line per event.
#[tokio::test]
async fn a_run_of_dropped_events_warns_once() {
    let log = CapturedLog::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("the only subscriber in this binary");

    let options = EslConnectOptions::new().with_event_queue_size(1);
    let (mut mock, client, _events) =
        setup_connected_pair_with_options(DEFAULT_ESL_PASSWORD, options).await;
    for i in 0..20 {
        let headers = HashMap::from([("Unique-ID".to_string(), format!("uuid-{i}"))]);
        mock.send_event_plain("CHANNEL_CREATE", &headers)
            .await
            .unwrap();
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while client.dropped_event_count() < 19 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(client.dropped_event_count(), 19);

    let text = String::from_utf8(
        log.0
            .lock()
            .unwrap()
            .clone(),
    )
    .unwrap();
    let warnings = text
        .lines()
        .filter(|line| line.contains("queue full"))
        .count();
    assert_eq!(warnings, 1, "{text}");
}
