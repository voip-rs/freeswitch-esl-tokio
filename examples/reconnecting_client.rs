//! Reconnecting ESL client -- production pattern for persistent event listeners.
//!
//! The library never reconnects on its own; it sorts every `EslError` into one
//! of three classes and the caller sets policy. This example is that policy:
//!
//! - **Connection error** (`is_connection_error()`) -- no usable session:
//!   reconnect, with exponential backoff. `AccessDenied` is one too, but the
//!   ACL refuses every reconnect, so it exits with `EX_CONFIG` (78) and systemd
//!   keeps the unit down.
//! - **Recoverable** (`is_recoverable()`) -- the session is up and one command
//!   failed: retry it, or carry on without it.
//! - **Neither** -- a refusal: a retry and a reconnect meet it again. A wrong
//!   credential or a subscription the switch cannot take is configuration, and
//!   exits with `EX_CONFIG`. A command built from call data that the library
//!   refuses is answered where it is built: that call is skipped and the
//!   session goes on. An event the switch sent that does not parse is skipped
//!   too; the reader keeps the connection.
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
    ConnectionStatus, DisconnectReason, EslClient, EslError, EslEvent, EslEventType, EslResult,
    EventFormat, EventHeader, EventSubscription, HeaderLookup, UuidSetVar,
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

/// Set on each answered channel, so the BYE carries the caller number this
/// monitor saw. mod_sofia sends a `sip_bye_h_` variable as a SIP header.
const BYE_HEADER_VARIABLE: &str = "sip_bye_h_X-Monitor-Caller";

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
            // A connection error, but the ACL answers every reconnect the same way.
            Err(EslError::AccessDenied { reason }) => {
                error!("access denied (ACL): {reason}");
                std::process::exit(EX_CONFIG);
            }
            Err(e) if e.is_connection_error() => warn!("connection lost: {}", e.display_chain()),
            // A setup command failed on a live session; a fresh session retries it.
            Err(e) if e.is_recoverable() => warn!("session setup failed: {}", e.display_chain()),
            // AuthenticationFailed, or a subscription the switch refuses as written.
            Err(e) => {
                error!("refused, fix the configuration: {}", e.display_chain());
                std::process::exit(EX_CONFIG);
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
/// the session from running at all, or a connection error during it.
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
            Ok(event) => handle_event(&client, &event).await?,
            Err(e) if e.is_connection_error() => return Err(e),
            // A body line that did not parse, or a value the reader could not
            // decode: this event is lost, the connection is not.
            Err(e) => warn!("event skipped: {}", e.display_chain()),
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

/// Handle one event. `Err` only for a connection error, which ends the session;
/// anything else concerns this event alone and is logged here.
async fn handle_event(client: &EslClient, event: &EslEvent) -> EslResult<()> {
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
            if let (Some(full_uuid), Some(number)) = (event.unique_id(), event.caller_id_number()) {
                tag_caller(client, full_uuid, number).await?;
            }
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
    Ok(())
}

/// Copy the caller number into a header of the channel's BYE.
///
/// The number is call data: a control character in it is refused before
/// anything is sent. That refusal, and a failure of the command itself, skip
/// this call only. A connection error goes back up and ends the session.
async fn tag_caller(client: &EslClient, uuid: &str, caller: &str) -> EslResult<()> {
    let command = match UuidSetVar::new(uuid, BYE_HEADER_VARIABLE, caller) {
        Ok(command) => command,
        Err(e) => {
            warn!("{uuid}: caller number not tagged: {e}");
            return Ok(());
        }
    };
    let reply = client
        .api(&command.to_string())
        .await;
    match reply.and_then(|response| {
        response
            .api_result()
            .map(drop)
    }) {
        Ok(()) => Ok(()),
        Err(e) if e.is_connection_error() => Err(e),
        Err(e) => {
            warn!("{uuid}: caller number not tagged: {}", e.display_chain());
            Ok(())
        }
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
