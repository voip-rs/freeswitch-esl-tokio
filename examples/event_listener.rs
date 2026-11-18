//! Subscribe to FreeSWITCH events and track calls as they happen.
//!
//! The event loop is the other half of ESL: `inbound_client` shows commands and
//! their replies, this shows the stream FreeSWITCH pushes at you.
//!
//! Usage: cargo run --example event_listener
//!        cargo run --example event_listener -- -d    # dump raw wire data to stdout

mod common;

use freeswitch_esl_tokio::{
    EslEvent, EslEventType, EventFormat, EventHeader, EventSubscription, HeaderLookup,
};
use std::collections::HashMap;
use std::io::Write;
use tracing::{debug, error, info, warn};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dump_raw = std::env::args().any(|a| a == "-d");

    // Direct tracing to stderr so stdout is clean for -d wire dumps
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let (client, mut events) = common::connect_from_env().await?;

    // Build an EventSubscription describing everything we want to receive.
    // apply_subscription() sends filters and the event command in one call.
    let subscription = if dump_raw {
        EventSubscription::all(EventFormat::Plain)
    } else {
        EventSubscription::new(EventFormat::Plain)
            .events(EslEventType::CHANNEL_EVENTS)
            .event(EslEventType::Dtmf)
            .event(EslEventType::Heartbeat)
    };
    client
        .apply_subscription(&subscription)
        .await?;

    let mut active_calls: HashMap<String, CallInfo> = HashMap::new();
    let mut event_count = 0u64;
    let mut stdout = std::io::stdout();

    info!("listening for events, Ctrl+C to exit");

    while let Some(result) = events
        .recv()
        .await
    {
        let event = match result {
            Ok(event) => event,
            Err(e) => {
                error!("event error: {}", e.display_chain());
                continue;
            }
        };
        event_count += 1;

        if dump_raw {
            // A closed or broken stdout is the end of the run, not something to
            // keep writing past: the dump is what this mode is for.
            stdout.write_all(
                event
                    .to_plain_format()
                    .as_bytes(),
            )?;
        }

        process_event(&event, &mut active_calls);
    }

    if dump_raw {
        stdout.flush()?;
    }
    info!("connection closed, {event_count} events seen");
    client
        .disconnect()
        .await?;

    Ok(())
}

fn process_event(event: &EslEvent, active_calls: &mut HashMap<String, CallInfo>) {
    // HEARTBEAT describes the switch, not a channel, so it is answered before
    // the UUID every other arm here needs.
    if event.event_type() == Some(EslEventType::Heartbeat) {
        if let Some(sessions) = event.header(EventHeader::SessionCount) {
            info!("heartbeat, sessions: {sessions}");
        }
        return;
    }

    let Some(uuid) = event.unique_id() else {
        return;
    };

    match event.event_type() {
        Some(EslEventType::ChannelCreate) => {
            let caller_id = event
                .caller_id_number()
                .unwrap_or("unknown");
            let destination = event
                .destination_number()
                .unwrap_or("unknown");
            // A direction that fails to parse is FreeSWITCH sending something
            // this crate does not know, which is worth seeing, not hiding.
            match event.call_direction() {
                Ok(Some(direction)) => {
                    info!("new call: {caller_id} -> {destination} ({direction})")
                }
                Ok(None) => info!("new call: {caller_id} -> {destination}"),
                Err(e) => warn!("new call: {caller_id} -> {destination}, bad direction: {e}"),
            }

            active_calls.insert(
                uuid.to_string(),
                CallInfo {
                    caller_id: caller_id.to_string(),
                    start_time: std::time::Instant::now(),
                    answered_time: None,
                },
            );
        }
        Some(EslEventType::ChannelAnswer) => {
            if let Some(call_info) = active_calls.get_mut(uuid) {
                call_info.answered_time = Some(std::time::Instant::now());
                let ring = call_info
                    .start_time
                    .elapsed();
                info!(
                    "answered: {} (ring {:.2}s)",
                    call_info.caller_id,
                    ring.as_secs_f64()
                );
            }
        }
        Some(EslEventType::ChannelHangup) => {
            let Some(call_info) = active_calls.get(uuid) else {
                return;
            };
            let cause = match event.hangup_cause() {
                Ok(Some(cause)) => cause.to_string(),
                Ok(None) => "no cause header".to_string(),
                // FreeSWITCH grew a cause this crate does not carry yet.
                Err(e) => {
                    warn!("{uuid}: unparseable hangup cause: {e}");
                    return;
                }
            };
            match call_info
                .answered_time
                .map(|t| t.elapsed())
            {
                Some(talk) => info!(
                    "ended: {} ({cause}, talk {:.2}s)",
                    call_info.caller_id,
                    talk.as_secs_f64()
                ),
                None => info!("ended: {} ({cause}, never answered)", call_info.caller_id),
            }
        }
        Some(EslEventType::ChannelHangupComplete) => {
            active_calls.remove(uuid);
        }
        Some(EslEventType::Dtmf) => {
            if let Some(digit) = event.header(EventHeader::DtmfDigit) {
                info!("{uuid}: DTMF '{digit}'");
            }
        }
        other => debug!("ignoring {other:?}"),
    }
}

#[derive(Debug)]
struct CallInfo {
    caller_id: String,
    start_time: std::time::Instant,
    answered_time: Option<std::time::Instant>,
}
