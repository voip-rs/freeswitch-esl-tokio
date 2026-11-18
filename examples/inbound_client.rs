//! Inbound ESL: connect to FreeSWITCH and run commands.
//!
//! Start here. It covers the three things every ESL client does: authenticate,
//! run an `api` command and read its result, and run a `bgapi` command and
//! correlate the result that arrives later on the event stream.
//!
//! Usage: cargo run --example inbound_client
//!        ESL_HOST=pbx.example.com ESL_PASSWORD=secret cargo run --example inbound_client

mod common;

use freeswitch_esl_tokio::{BgJobTracker, EslEventType, EventFormat};
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let (client, mut events) = common::connect_from_env().await?;

    // api_result() is the whole check. A command the switch refuses (an
    // esl-allowed-api gate) is answered as a reply with no body; one that runs
    // and fails reports in the body. Both come back as Err here.
    println!(
        "{}",
        client
            .api("status")
            .await?
            .api_result()?
    );

    for var in ["hostname", "domain", "local_ip_v4", "switch_serial"] {
        // A restricted user is a normal deployment, not a reason to stop: the
        // connection is still good, so report and carry on.
        match client
            .api(&format!("global_getvar {var}"))
            .await
            .and_then(|resp| {
                resp.api_result()
                    .map(str::to_string)
            }) {
            Ok(value) => info!("{var} = {value}"),
            Err(e) if e.is_permission_denied() => warn!("{var}: not permitted for this ESL user"),
            Err(e) => warn!("{var}: {}", e.display_chain()),
        }
    }

    // bgapi returns a Job-UUID immediately and the result arrives later as a
    // BACKGROUND_JOB event. That event goes to every ESL client on the switch,
    // including an operator's fs_cli, so the Job-UUID is what makes a result
    // ours. BgJobTracker owns that bookkeeping.
    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::BackgroundJob])
        .await?;

    let mut jobs: BgJobTracker<&str> = BgJobTracker::new();
    for command in ["version", "sofia status"] {
        jobs.bgapi(&client, command, command)
            .await?;
    }

    while jobs.pending_count() > 0 {
        let Some(result) = events
            .recv()
            .await
        else {
            error!(
                "stream closed with {} job(s) outstanding",
                jobs.pending_count()
            );
            break;
        };
        let event = match result {
            Ok(event) => event,
            Err(e) => {
                warn!("event error: {}", e.display_chain());
                continue;
            }
        };
        // Anything that is not one of ours -- another client's job, an
        // unrelated event -- leaves the tracker untouched.
        if let Some((command, job)) = jobs.try_complete(&event) {
            match job.parse_body() {
                Ok(output) => println!("--- {command} ---\n{output}"),
                Err(e) => warn!("{command} failed: {}", e.display_chain()),
            }
        }
    }

    client
        .disconnect()
        .await?;
    Ok(())
}
