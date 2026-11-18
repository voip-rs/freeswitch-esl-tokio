//! bgapi throughput benchmark
//!
//! Sends N `bgapi status` commands as fast as possible and collects the
//! BACKGROUND_JOB results. Measures send-phase latency and full round-trip
//! (send -> BACKGROUND_JOB event arrival).
//!
//! Environment variables:
//! - `ESL_HOST` / `ESL_PORT` / `ESL_PASSWORD` -- connection parameters
//! - `BENCH_COUNT` -- number of bgapi commands (default: 1000)
//!
//! Run with: `cargo run --release --example bgapi_bench`

use std::collections::HashMap;
use std::time::{Duration, Instant};

mod common;

use freeswitch_esl_tokio::{parse_api_body, EslEvent, EslEventType, EventFormat, HeaderLookup};
use tokio::sync::oneshot;

/// How long to keep collecting after the last command was sent. Results still
/// in flight arrive within this window; anything later is counted as lost.
const DRAIN_WINDOW: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A benchmark that silently ran a different N than you asked for reports
    // numbers you cannot compare against anything.
    let n: usize = match std::env::var("BENCH_COUNT") {
        Ok(value) => value.parse()?,
        Err(_) => 1000,
    };

    let (client, mut events) = common::connect_from_env().await?;

    // Scale timeout with N so large runs don't time out
    let timeout_ms = 5000 + (n as u64 * 50);
    client.set_command_timeout(Duration::from_millis(timeout_ms));

    // Warm-up, and a check that this ESL user may run the command at all: a
    // denied `status` would otherwise show up as a benchmark of nothing.
    client
        .api("status")
        .await?
        .api_result()?;

    client
        .subscribe_events(EventFormat::Plain, [EslEventType::BackgroundJob])
        .await?;

    let (done_tx, mut done_rx) = oneshot::channel::<()>();
    let collector = tokio::spawn(async move {
        // Every BACKGROUND_JOB on the switch lands here, not just ours, so this
        // map is filtered against our own Job-UUIDs after the run rather than
        // counted up to N -- another client's results would end the run early.
        let mut arrivals: HashMap<String, (Instant, bool)> = HashMap::with_capacity(n);

        let mut record = |event: &EslEvent| {
            if let Some(uuid) = event.job_uuid() {
                let succeeded = parse_api_body(
                    event
                        .body()
                        .unwrap_or(""),
                )
                .is_ok();
                arrivals.insert(uuid.to_string(), (Instant::now(), succeeded));
            }
        };

        // While the send loop is still running.
        loop {
            tokio::select! {
                biased;
                event = events.recv() => match event {
                    Some(Ok(event)) => record(&event),
                    Some(Err(e)) => eprintln!("event error: {}", e.display_chain()),
                    None => return arrivals,
                },
                _ = &mut done_rx => break,
            }
        }

        // Bounded drain, so a result that never comes back ends the run instead
        // of hanging until FreeSWITCH closes the socket.
        let deadline = tokio::time::Instant::now() + DRAIN_WINDOW;
        loop {
            match tokio::time::timeout_at(deadline, events.recv()).await {
                Ok(Some(Ok(event))) => record(&event),
                Ok(Some(Err(e))) => eprintln!("event error: {}", e.display_chain()),
                Ok(None) | Err(_) => break,
            }
        }
        arrivals
    });

    let mut send_times: Vec<(String, Instant, Duration)> = Vec::with_capacity(n);
    let run_start = Instant::now();

    for _ in 0..n {
        let t0 = Instant::now();
        // into_result() before reading the header: a refused bgapi carries no
        // Job-UUID, and reporting that as a missing header blames the wrong thing.
        let resp = client
            .bgapi("status")
            .await?
            .into_result()?;
        let elapsed = t0.elapsed();
        let uuid = resp
            .job_uuid()
            .ok_or("bgapi reply carried no Job-UUID")?
            .to_string();
        send_times.push((uuid, t0, elapsed));
    }

    let send_phase = run_start.elapsed();
    // The only failure is a collector that already stopped, which the counts
    // below report on their own.
    let _ = done_tx.send(());

    let arrivals = collector.await?;
    let total = run_start.elapsed();

    let mut send_lats: Vec<Duration> = send_times
        .iter()
        .map(|(_, _, d)| *d)
        .collect();
    send_lats.sort();

    let mut rtts: Vec<Duration> = Vec::with_capacity(send_times.len());
    let mut failed = 0usize;
    for (uuid, sent_at, _) in &send_times {
        if let Some((arrived, succeeded)) = arrivals.get(uuid) {
            rtts.push(arrived.duration_since(*sent_at));
            if !succeeded {
                failed += 1;
            }
        }
    }
    rtts.sort();

    println!("bench=rust n={n}");
    println!("received={}", rtts.len());
    println!("failed={failed}");
    println!("send_phase_ms={}", send_phase.as_millis());
    println!(
        "send_rate_per_sec={:.1}",
        n as f64 / send_phase.as_secs_f64()
    );
    // Results arrive while commands are still going out, so the two phases
    // overlap: total is wall clock for the whole run, never their sum.
    print_latencies("send_lat", &send_lats);
    print_latencies("rtt", &rtts);
    println!("total_ms={}", total.as_millis());

    client
        .disconnect()
        .await?;
    Ok(())
}

fn print_latencies(prefix: &str, sorted: &[Duration]) {
    if sorted.is_empty() {
        return;
    }
    let n = sorted.len();
    // Nearest-rank over the sample itself, so a short run's p99 is just its max.
    let rank = |q: f64| sorted[((n as f64 * q) as usize).min(n - 1)].as_micros();
    println!("{prefix}_min_us={}", sorted[0].as_micros());
    println!("{prefix}_median_us={}", rank(0.5));
    println!("{prefix}_p95_us={}", rank(0.95));
    println!("{prefix}_p99_us={}", rank(0.99));
    println!("{prefix}_max_us={}", sorted[n - 1].as_micros());
}
