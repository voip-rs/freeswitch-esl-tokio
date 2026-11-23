//! Outbound ESL: FreeSWITCH connects to you, and you drive the call.
//!
//! Point a dialplan extension at this listener:
//!
//! ```xml
//! <action application="socket" data="127.0.0.1:8040 async full"/>
//! ```
//!
//! `async full` is load-bearing. Without `full`, a session is limited to
//! connect / myevents / getvar / resume / filter / sendmsg, so `linger` and the
//! `event` command are refused. See docs/outbound-esl-quirks.md.
//!
//! Usage: cargo run --example outbound_server
//!        ESL_BIND=[::]:9000 cargo run --example outbound_server

use freeswitch_esl_tokio::{
    AppCommand, EslClient, EslError, EslEventStream, EslEventType, EventFormat, EventHeader,
    HangupCause, HeaderLookup,
};
use std::time::Duration;
use tokio::net::TcpListener;
use tracing::{debug, error, info, warn};

/// A caller leaning on a key would otherwise grow the buffer without bound.
const MAX_DTMF_DIGITS: usize = 16;

/// Resource exhaustion (EMFILE) fails `accept` again immediately, which would
/// fill the log with one repeated line.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // `[::]` accepts both families on a dual-stack host; `0.0.0.0` is IPv4 only.
    let bind_addr = std::env::var("ESL_BIND").unwrap_or_else(|_| "[::]:8040".to_string());
    let listener = TcpListener::bind(&bind_addr).await?;
    info!("outbound ESL server listening on {bind_addr}");

    loop {
        match EslClient::accept_outbound(&listener).await {
            Ok((client, mut events)) => {
                tokio::spawn(async move {
                    if let Err(e) = handle_call(&client, &mut events).await {
                        error!("call failed: {}", e.display_chain());
                    }
                });
            }
            // One connection failing to come up is not a reason to stop
            // serving the rest.
            Err(e) => {
                warn!("outbound accept failed: {}", e.display_chain());
                tokio::time::sleep(ACCEPT_BACKOFF).await;
            }
        }
    }
}

async fn handle_call(client: &EslClient, events: &mut EslEventStream) -> Result<(), EslError> {
    // `connect` must be the first command on an outbound socket. Its reply
    // carries the whole channel as headers, so HeaderLookup's typed accessors
    // work on it directly.
    let channel_data = client
        .connect_session()
        .await?
        .into_result()?;
    let Some(channel) = channel_data.channel_name() else {
        // The switch always sends the channel here; a reply without it is not
        // the conversation this socket expected, so hang up on it.
        return Err(EslError::protocol_error(format!(
            "connect reply has no {}",
            EventHeader::ChannelName
        )));
    };
    info!("session established: {channel}");

    // myevents scopes delivery to this session. `subscribe_events` would send
    // the `event` command, which is switch-wide: this IVR would then answer on
    // another call's CHANNEL_ANSWER and collect another call's DTMF.
    client
        .myevents(EventFormat::Plain)
        .await?;
    // Keep the socket up past hangup so the closing events still arrive.
    client
        .linger(None)
        .await?;

    client
        .send_command(AppCommand::answer())
        .await?
        .check()?;

    if !wait_for_answer(events).await {
        info!("caller hung up before answer");
        return Ok(());
    }

    client
        .send_command(AppCommand::playback("ivr/ivr-welcome.wav"))
        .await?
        .check()?;

    collect_digits(client, events).await
}

/// `true` once the channel answered, `false` if it went away first.
async fn wait_for_answer(events: &mut EslEventStream) -> bool {
    while let Some(result) = events
        .recv()
        .await
    {
        let event = match result {
            Ok(event) => event,
            Err(e) => {
                warn!("event error: {}", e.display_chain());
                continue;
            }
        };
        match event.event_type() {
            Some(EslEventType::ChannelAnswer) => return true,
            Some(EslEventType::ChannelHangup) => return false,
            _ => {}
        }
    }
    false
}

async fn collect_digits(client: &EslClient, events: &mut EslEventStream) -> Result<(), EslError> {
    let mut digits = String::new();
    // The prompt is itself a playback, so its own PLAYBACK_STOP would re-issue
    // it forever. Only the greeting's stop opens the prompt.
    let mut prompted = false;

    while let Some(result) = events
        .recv()
        .await
    {
        let event = match result {
            Ok(event) => event,
            Err(e) => {
                warn!("event error: {}", e.display_chain());
                continue;
            }
        };

        match event.event_type() {
            Some(EslEventType::ChannelHangup) => {
                info!("caller hung up");
                return Ok(());
            }
            Some(EslEventType::PlaybackStop) if !prompted => {
                prompted = true;
                prompt_for_extension(client).await?;
            }
            Some(EslEventType::Dtmf) => {
                let Some(digit) = event.header(EventHeader::DtmfDigit) else {
                    continue;
                };
                debug!("DTMF: {digit}");
                if digit == "#" {
                    handle_dtmf_input(client, &digits).await?;
                    digits.clear();
                } else if digits.len() < MAX_DTMF_DIGITS {
                    digits.push_str(digit);
                }
            }
            _ => {}
        }
    }

    Ok(())
}

async fn prompt_for_extension(client: &EslClient) -> Result<(), EslError> {
    client
        .send_command(AppCommand::playback(
            "ivr/ivr-please_enter_extension_followed_by_pound.wav",
        ))
        .await?
        .check()
}

async fn handle_dtmf_input(client: &EslClient, input: &str) -> Result<(), EslError> {
    match input {
        "1000" | "1001" | "1002" | "1003" => {
            info!("transferring to extension {input}");
            client
                .send_command(AppCommand::playback("ivr/ivr-hold_connect_call.wav"))
                .await?
                .check()?;
            client
                .send_command(AppCommand::transfer(input)?)
                .await?
                .check()?;
        }
        "0" => {
            info!("transferring to operator");
            client
                .send_command(AppCommand::playback("ivr/ivr-hold_connect_call.wav"))
                .await?
                .check()?;
            client
                .send_command(AppCommand::transfer("operator")?)
                .await?
                .check()?;
        }
        "9" => {
            info!("hanging up at caller request");
            client
                .send_command(AppCommand::playback("voicemail/vm-goodbye.wav"))
                .await?
                .check()?;
            client
                .send_command(AppCommand::hangup(Some(HangupCause::NormalClearing)))
                .await?
                .check()?;
        }
        "" => prompt_for_extension(client).await?,
        _ => {
            info!("invalid extension: {input}");
            client
                .send_command(AppCommand::playback(
                    "ivr/ivr-that_was_an_invalid_entry.wav",
                ))
                .await?
                .check()?;
            prompt_for_extension(client).await?;
        }
    }

    Ok(())
}
