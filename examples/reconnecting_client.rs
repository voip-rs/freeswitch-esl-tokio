//! Reconnecting ESL client -- production pattern for persistent event listeners.
//!
//! The library never reconnects on its own; it classifies the failure and the
//! caller sets policy. This example is that policy:
//!
//! - **Auth / ACL** (`AuthenticationFailed`, `AccessDenied`) -- permanent
//!   config error, exit with `EX_CONFIG` (78) so systemd keeps the unit down.
//! - **Session ended** -- anything but a disconnect we asked for is a reason to
//!   reconnect, with exponential backoff.
//! - **Recoverable** (`is_recoverable()`) -- a command failed, the connection
//!   is still good.
//!
//! What it cannot do is recover the events that fired while it was away. See
//! docs/design-rationale.md on why that makes transparent reconnection unsound
//! and re-exec the answer for a system of record.
//!
//! Usage: RUST_LOG=info cargo run --example reconnecting_client

use std::time::{Duration, Instant};

#[path = "common/env.rs"]
mod env;

use freeswitch_esl_tokio::{
    ConnectionStatus, DisconnectReason, EslClient, EslError, EslEvent, EslEventType, EventFormat,
    EventHeader, EventSubscription, HeaderLookup,
};
use tracing::{error, info, warn};

const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// A session that lasted this long counts as healthy, so the next drop starts
/// backing off from scratch instead of inheriting an old flap's delay.
const STABLE_SESSION: Duration = Duration::from_secs(60);

/// Liveness threshold. Fed by HEARTBEAT (~20s) when we are allowed to
/// subscribe; only enabled when that subscription succeeds.
const LIVENESS_TIMEOUT: Duration = Duration::from_secs(60);

/// sysexits.h EX_CONFIG -- systemd RestartPreventExitStatus=78 keeps the
/// service down on permanent config errors.
const EX_CONFIG: i32 = 78;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // A malformed port is a config error, and this binary's whole point is
    // that config errors exit rather than retry.
    let env = env::EslEnv::from_env().unwrap_or_else(|e| {
        error!("{e}");
        std::process::exit(EX_CONFIG);
    });

    // Build the subscription once, reuse on every reconnection.
    // EventSubscription is pure data -- no connection state, safe to Clone.
    // HEARTBEAT is subscribed separately in run_session (it gates the liveness
    // timer and may be permission-denied) -- keep only functional events here.
    let subscription = EventSubscription::new(EventFormat::Plain)
        .with_event(EslEventType::ChannelAnswer)
        .with_event(EslEventType::ChannelHangupComplete);

    let mut backoff = BACKOFF_INITIAL;

    loop {
        info!("connecting to {}:{}", env.host, env.port);
        let started = Instant::now();

        match run_session(&env.host, env.port, &env.password, &subscription).await {
            // The only ending this process asked for. Everything else means the
            // session went away under us and has to be rebuilt.
            Ok(reason @ DisconnectReason::ClientRequested) => {
                info!("{reason}, exiting");
                return;
            }
            Ok(reason) => warn!("session ended: {reason}"),
            Err(EslError::AuthenticationFailed { reason }) => {
                error!("authentication failed: {reason}");
                std::process::exit(EX_CONFIG);
            }
            Err(EslError::AccessDenied { reason }) => {
                error!("access denied (ACL): {reason}");
                std::process::exit(EX_CONFIG);
            }
            Err(e) if e.is_connection_error() => warn!("connection lost: {}", e.display_chain()),
            Err(e) => {
                error!("unexpected error: {}", e.display_chain());
                std::process::exit(1);
            }
        }

        if started.elapsed() >= STABLE_SESSION {
            backoff = BACKOFF_INITIAL;
        }
        info!("retrying in {backoff:?}");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// One ESL session: connect, subscribe, process events until the stream ends.
///
/// Returns why the stream ended. `Err` is reserved for a failure that stopped
/// the session from running at all, or one the event loop could not absorb.
async fn run_session(
    host: &str,
    port: u16,
    password: &str,
    subscription: &EventSubscription,
) -> Result<DisconnectReason, EslError> {
    let (client, mut events) = EslClient::connect(host, port, password).await?;
    info!("connected");

    // apply_subscription sends filters and event commands in one call.
    // The subscription object is reused on every reconnection -- no need
    // to rebuild event lists each time.
    client
        .apply_subscription(subscription)
        .await?;

    // HEARTBEAT is the idle-traffic source for the liveness timer. The library
    // never sends keepalives on its own, so without inbound traffic an idle
    // socket cannot be kept alive. Subscribe to it on its own command (bundling
    // it with the functional events above would let a denial sink the whole
    // subscription). A permission-restricted user (esl-allowed-events without
    // HEARTBEAT) gets -ERR permission denied here: recoverable, the connection
    // stays up. Warn and run without idle-liveness rather than reconnect-looping.
    match client
        .subscribe_events(EventFormat::Plain, &[EslEventType::Heartbeat])
        .await
    {
        Ok(()) => client.set_liveness_timeout(LIVENESS_TIMEOUT),
        Err(e) if e.is_permission_denied() => {
            warn!("heartbeat denied ({e}); idle-liveness disabled for this user");
        }
        Err(e) => return Err(e),
    }

    while let Some(result) = events
        .recv()
        .await
    {
        match result {
            Ok(event) => handle_event(&event),
            Err(e) if e.is_recoverable() => warn!("recoverable event error: {}", e.display_chain()),
            Err(e) => return Err(e),
        }
    }

    Ok(match events.status() {
        ConnectionStatus::Disconnected(reason) => reason,
        // The reader sets a reason before closing the channel, so this is a
        // library bug rather than a state to model.
        status => {
            warn!("event stream ended while status was {status:?}");
            DisconnectReason::ConnectionClosed
        }
    })
}

fn handle_event(event: &EslEvent) {
    match event.event_type() {
        Some(EslEventType::ChannelAnswer) => {
            let uuid = short_uuid(event);
            let caller = event
                .caller_id_number()
                .unwrap_or("?");
            let dest = event
                .destination_number()
                .unwrap_or("?");
            info!("{uuid}: {caller} -> {dest} answered");
        }
        Some(EslEventType::ChannelHangupComplete) => {
            let uuid = short_uuid(event);
            // A cause this crate does not know is a signal, not a blank: it
            // means FreeSWITCH grew one, and collapsing it into "?" is how
            // nobody ever finds out.
            match event.hangup_cause() {
                Ok(Some(cause)) => info!("{uuid}: hangup ({cause})"),
                Ok(None) => info!("{uuid}: hangup (no cause header)"),
                Err(e) => warn!("{uuid}: hangup with unparseable cause: {e}"),
            }
        }
        Some(EslEventType::Heartbeat) => {
            let sessions = event
                .header(EventHeader::SessionCount)
                .unwrap_or("?");
            info!("heartbeat, sessions: {sessions}");
        }
        _ => {}
    }
}

/// First segment of the channel UUID, which is enough to correlate log lines.
fn short_uuid(event: &EslEvent) -> &str {
    event
        .unique_id()
        .map_or("?", |uuid| {
            uuid.split('-')
                .next()
                .unwrap_or(uuid)
        })
}
