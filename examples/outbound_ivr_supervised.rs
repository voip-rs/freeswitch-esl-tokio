//! A supervised per-call IVR: an inbound daemon originates the call, a spawned
//! child process is the call's application, and the child finishes the call on
//! its own if the parent dies.
//!
//! The shape this demonstrates sits between the two obvious ones. Driving the
//! caller leg over the daemon's own inbound connection means `sendmsg` with a
//! uuid, whose `+OK` says only that the private event was queued. Letting the
//! daemon itself be the outbound socket application puts a long-lived
//! supervisor in the call path, where its restart drops a live call. Here the
//! process in the call path is a child that exists for one prompt.
//!
//! Three things fall out of that split, and each is asserted below rather than
//! described:
//!
//! 1. **Static mode is a completion acknowledgement.** With neither `async` nor
//!    `full` on the socket application, `sendmsg` runs the application inline on
//!    the listener thread and answers `+OK` only once it returns. No
//!    `Event-UUID` correlation, no event subscription, and `getvar` reads the
//!    collected digit back.
//! 2. **The uuid `originate` returns is not the channel the IVR gets.** The
//!    loopback leg bows out and masquerades the socket application onto the real
//!    channel, which has a different uuid. The parent learns the real one from
//!    the `loopback::bowout` event, and the child confirms it independently from
//!    its own connect payload.
//! 3. **Parent loss is survivable.** The child holds the channel, so a dead
//!    parent means an unsupervised child, not a dropped call. It stops asking
//!    for a decision it can no longer report and sends the call to the fallback
//!    extension.
//!
//! What it does not fix: static mode never parks, so `park_timeout` is
//! unavailable and nothing switch-side bounds a child that wedges rather than
//! exits. The parent arms `sched_hangup` on the real uuid for that.
//!
//! Usage: FREESWITCH_VERSION=1.10.12 cargo run --example outbound_ivr_supervised
//!   ESL_HOST / ESL_PORT / ESL_PASSWORD select the switch.
//!   FREESWITCH_VERSION parser revision the originate renders for (required;
//!                      ESL_BLOCK_PARSE names one outright for a dev build)
//!   IVR_ENDPOINT       loopback destination (default `app=bridge:null/farend`)
//!   IVR_FALLBACK_EXT   extension the child transfers to (default `9199`)
//!   IVR_CONTEXT        context for that transfer (default `test`)
//!   IVR_BIND           child listen address (default `[::1]:0`)
//!   IVR_ABANDON=1      parent closes the supervision pipe after originating,
//!                      so the child takes the unsupervised path

#[path = "common/block_parse.rs"]
mod block_parse;
mod common;

use freeswitch_esl_tokio::commands::LoopbackEndpoint;
use freeswitch_esl_tokio::variables::LoopbackChannelName;
use freeswitch_esl_tokio::{
    AppCommand, Application, DialplanType, Endpoint, EslClient, EslEventStream, EslEventType,
    EventFormat, EventSubscription, HeaderLookup, Originate, Variables, VariablesType,
};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tracing::{info, warn};

/// Selects the child role. The child is this same binary re-spawned: a bare
/// `fork()` would leave the tokio runtime with one surviving thread and a
/// half-owned reactor.
const CHILD_ARG: &str = "--ivr-child";

/// First line the child writes: the address to hand the socket application.
const ADDR_PREFIX: &str = "listening ";

/// Last line the child writes, so the parent can report an outcome it never
/// observed on the wire itself.
const RESULT_PREFIX: &str = "result ";

/// Nothing switch-side bounds a wedged child in static mode, so the parent arms
/// this on the real leg as soon as it knows which channel that is.
const CALL_DEADLINE_SECS: u32 = 60;

/// A bowout that has not happened within this long did not happen.
const BOWOUT_DEADLINE: Duration = Duration::from_secs(15);

/// Bounds a `sendmsg` whose reply waits on an application, so it must exceed
/// the longest prompt this IVR can play rather than the protocol round trip.
const APP_TIMEOUT: Duration = Duration::from_secs(120);

/// `play_and_get_digits` positional arguments: min max tries timeout
/// terminators file invalid-file variable regexp. One short try, because
/// nothing on the far end of an automated run ever presses a key -- a
/// deployment wants a real prompt and a human's worth of timeout.
const DEFAULT_PROMPT_ARGS: &str =
    "1 1 1 1500 # tone_stream://%(500,0,800) silence_stream://250 ivr_choice \\d";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|arg| arg == CHILD_ARG) {
        // The child's stdout is a protocol, so its logs go to stderr or they
        // arrive in the middle of one.
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .init();
        child().await
    } else {
        tracing_subscriber::fmt::init();
        parent().await
    }
}

// ---------------------------------------------------------------- parent ----

async fn parent() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint =
        std::env::var("IVR_ENDPOINT").unwrap_or_else(|_| "app=bridge:null/farend".to_string());
    let abandon = std::env::var("IVR_ABANDON").is_ok();
    // Escaping depends on the switch's parser revision, which the crate never
    // asks the switch for; resolved before the child is spawned.
    let block_parse = block_parse::block_parse_from_env()?;

    let mut child = tokio::process::Command::new(std::env::current_exe()?)
        .arg(CHILD_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;

    // The child never reads this pipe's contents, only its EOF. Holding the
    // write end here is what makes the parent's death observable to it without
    // a signal handler or a getppid() poll that races with the spawn.
    let supervision = child
        .stdin
        .take()
        .ok_or("child stdin was not piped")?;

    let mut lines = BufReader::new(
        child
            .stdout
            .take()
            .ok_or("child stdout was not piped")?,
    )
    .lines();

    let addr = lines
        .next_line()
        .await?
        .and_then(|line| {
            line.strip_prefix(ADDR_PREFIX)
                .map(str::to_string)
        })
        .ok_or("child exited before reporting its address")?;
    info!("child is listening; socket application will dial {addr}");

    let (client, mut events) = common::connect_from_env().await?;

    // CUSTOM is terminal in the event grammar, so the subclass has to be added
    // as one rather than spelled into the token list.
    let sub = EventSubscription::new(EventFormat::Plain)
        .event(EslEventType::ChannelHangupComplete)
        .custom_subclass("loopback::bowout")?;
    client
        .apply_subscription(&sub)
        .await?;

    // `loopback_bowout=false` vetoes the frame-count path, leaving the
    // execute-time one as the only way this pair can resign -- which is the
    // path that masquerades the socket application onto the real channel.
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("loopback_bowout", "false")?;
    vars.insert("loopback_bowout_on_execute", "true")?;

    let originate = Originate::application(
        Endpoint::Loopback(LoopbackEndpoint::new(&endpoint)?.with_variables(vars)),
        Application::new("socket", Some(addr.as_str())),
    )?;
    let line = originate.render(block_parse);
    info!("originate: {line}");

    let loopback_leg = client
        .api(&line)
        .await?
        .api_result()?
        .to_string();
    info!("originate returned {loopback_leg} -- this is the loopback leg, not the IVR's channel");

    if abandon {
        info!("IVR_ABANDON set: closing the supervision pipe, child is on its own");
        drop(supervision);
    }

    let real_leg = wait_for_bowout(&mut events, &loopback_leg).await;

    match &real_leg {
        Some(uuid) => {
            info!("bowout acquired {uuid}");
            // Static mode gives the session no deadline of its own.
            let armed = client
                .api(&format!("sched_hangup +{CALL_DEADLINE_SECS} {uuid}"))
                .await
                .and_then(|resp| {
                    resp.api_result()
                        .map(str::to_string)
                });
            if let Err(e) = armed {
                warn!("could not arm the call deadline: {e}");
            }
        }
        None => {
            warn!("no loopback::bowout arrived; the socket application may be on the loopback leg")
        }
    }

    while let Some(line) = lines
        .next_line()
        .await?
    {
        match line.strip_prefix(RESULT_PREFIX) {
            Some(result) => info!("child reports: {result}"),
            None => info!("child: {line}"),
        }
    }

    let status = child
        .wait()
        .await?;
    info!("child exited: {status}");

    // The transfer sends the call somewhere with its own lifetime, so the
    // example does not leave it running on a shared switch.
    if let Some(uuid) = real_leg {
        if let Err(e) = client
            .api(&format!("uuid_kill {uuid}"))
            .await
        {
            warn!("could not hang up {uuid}: {e}");
        }
    }

    client
        .disconnect()
        .await?;
    Ok(())
}

/// The surviving channel's uuid, taken from the event that names it before the
/// masquerade runs. `loopback_bowout_other_uuid` carries the same value but is
/// only readable afterwards, and the originate's own uuid never carries it.
async fn wait_for_bowout(events: &mut EslEventStream, loopback_leg: &str) -> Option<String> {
    let deadline = tokio::time::Instant::now() + BOWOUT_DEADLINE;

    loop {
        let event = match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(event))) => event,
            Ok(Some(Err(e))) => {
                warn!("event error: {e}");
                continue;
            }
            Ok(None) => {
                warn!("event stream closed before the bowout");
                return None;
            }
            Err(_) => {
                warn!("no bowout within {BOWOUT_DEADLINE:?}");
                return None;
            }
        };

        if event.event_subclass() != Some("loopback::bowout") {
            continue;
        }
        // The switch is shared, so another call's bowout lands here too.
        if event.header_str("Resigning-UUID") != Some(loopback_leg) {
            continue;
        }
        return event
            .header_str("Acquired-UUID")
            .map(str::to_string);
    }
}

// ----------------------------------------------------------------- child ----

async fn child() -> Result<(), Box<dyn std::error::Error>> {
    let bind = std::env::var("IVR_BIND").unwrap_or_else(|_| "[::1]:0".to_string());
    let fallback = std::env::var("IVR_FALLBACK_EXT").unwrap_or_else(|_| "9199".to_string());
    let context = std::env::var("IVR_CONTEXT").unwrap_or_else(|_| "test".to_string());

    let listener = TcpListener::bind(&bind).await?;
    let local = listener.local_addr()?;

    // The socket application splits host from port on the *last* colon and
    // hands the rest to getaddrinfo, so an IPv6 literal must arrive unbracketed
    // -- the opposite of every other host:port this crate assembles.
    let mut stdout = tokio::io::stdout();
    stdout
        .write_all(format!("{ADDR_PREFIX}{}:{}\n", local.ip(), local.port()).as_bytes())
        .await?;
    stdout
        .flush()
        .await?;

    let orphaned = watch_for_parent_exit();

    // A parent that dies before it originates leaves nothing to connect, so the
    // wait for FreeSWITCH has to lose to the orphan signal or this process
    // becomes an idle listener nobody will ever dial.
    let mut orphan_signal = orphaned.clone();
    let (client, _events) = tokio::select! {
        accepted = EslClient::accept_outbound(&listener) => accepted?,
        _ = orphan_signal.wait_for(|gone| *gone) => {
            return Err("parent exited before FreeSWITCH connected".into());
        }
    };
    // In static mode a `sendmsg` reply waits for the application to finish, and
    // an application's duration is the caller's, not the protocol's. The default
    // 5s command timeout would expire during any real prompt.
    client.set_command_timeout(APP_TIMEOUT);
    let channel_data = client
        .connect_session()
        .await?
        .into_result()?;

    let name = channel_data
        .channel_name()
        .ok_or("connect reply carried no channel name")?;
    let uuid = channel_data
        .unique_id()
        .ok_or("connect reply carried no unique id")?;

    // Landing on the loopback leg is not an error the switch reports: when
    // `find_non_loopback_bridge` finds nothing, mod_loopback runs the socket
    // application on the leg it was about to retire.
    if LoopbackChannelName::parse(name).is_some() {
        return Err(format!("socket application ran on the loopback leg {name}; aborting").into());
    }
    info!("child has {name} ({uuid})");

    let outcome = run_ivr(&client, &orphaned, &fallback, &context).await?;

    let mut stdout = tokio::io::stdout();
    stdout
        .write_all(format!("{RESULT_PREFIX}{outcome}\n").as_bytes())
        .await?;
    stdout
        .flush()
        .await?;
    Ok(())
}

/// Runs the prompt and transfers, returning what to report upstream.
async fn run_ivr(
    client: &EslClient,
    orphaned: &tokio::sync::watch::Receiver<bool>,
    fallback: &str,
    context: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    // In static mode every one of these returns only once the application has
    // finished, so `+OK` is the completion and the ordering needs no events.
    client
        .send_command(AppCommand::answer())
        .await?
        .check()?;

    if *orphaned.borrow() {
        client
            .send_command(AppCommand::transfer_to_context(
                fallback,
                DialplanType::XML,
                context,
            )?)
            .await?
            .check()?;
        return Ok(format!(
            "orphaned before the prompt; transferred to {fallback}"
        ));
    }

    // No typed builder for this one yet, so the nine positional arguments are
    // spelled out: min max tries timeout terminators file invalid-file
    // variable regexp.
    let prompt =
        std::env::var("IVR_PROMPT_ARGS").unwrap_or_else(|_| DEFAULT_PROMPT_ARGS.to_string());

    info!("sending play_and_get_digits");
    client
        .execute("play_and_get_digits", Some(&prompt), None)
        .await?
        .check()?;

    // Logged either side because the gap between these two lines is the point:
    // the reply waited for the application.
    info!("play_and_get_digits returned");
    let choice = client
        .getvar("ivr_choice")
        .await?;

    // The parent may have died while the prompt was playing. The call is still
    // ours to finish; what is gone is anyone to report a decision to.
    if *orphaned.borrow() {
        client
            .send_command(AppCommand::transfer_to_context(
                fallback,
                DialplanType::XML,
                context,
            )?)
            .await?
            .check()?;
        return Ok(format!(
            "orphaned during the prompt (digit {:?}); transferred to {fallback}",
            choice.as_deref()
        ));
    }

    match choice.as_deref() {
        Some(digit) if !digit.is_empty() => {
            client
                .send_command(AppCommand::transfer_to_context(
                    fallback,
                    DialplanType::XML,
                    context,
                )?)
                .await?
                .check()?;
            Ok(format!("digit {digit}; transferred to {fallback}"))
        }
        _ => {
            client
                .send_command(AppCommand::hangup(None))
                .await?
                .check()?;
            Ok("no digit collected; hung up".to_string())
        }
    }
}

/// `true` once the parent is gone. Reading stdin to EOF detects that without a
/// signal: SIGCHLD travels the other way, and `getppid()` has to be polled.
fn watch_for_parent_exit() -> tokio::sync::watch::Receiver<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);

    // A plain thread, not `tokio::io::stdin()`: that reads on the blocking pool,
    // and the runtime waits for blocking tasks at shutdown, so a read still
    // parked on a pipe the parent holds open would stop this process exiting.
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut scratch = [0u8; 64];
        loop {
            match std::io::Read::read(&mut stdin, &mut scratch) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(e) => {
                    warn!("supervision pipe failed: {e}");
                    break;
                }
            }
        }
        match tx.send(true) {
            Ok(()) => info!("parent is gone"),
            Err(e) => warn!("parent is gone but nothing is watching: {e}"),
        }
    });

    rx
}
