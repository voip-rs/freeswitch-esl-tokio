//! Drive a loopback pair through a *bowout*: mod_loopback notices that both
//! of its legs are bridged to real endpoints, splices those two endpoints
//! together with `uuid_bridge`, and removes itself from the audio path.
//!
//! Before:  null/nearend = loopback-a : loopback-b = null/farend
//! After:   null/nearend = null/farend
//!
//! Bowout needs all of: both loopback legs bridged to a non-loopback channel,
//! both answered, and neither leg setting `loopback_bowout=false`. The B leg
//! gets its bridge from the `app=bridge:null/farend` destination form; the A
//! leg gets its own from the `&bridge(null/nearend)` originate target.
//!
//! Usage: FREESWITCH_VERSION=1.10.12 cargo run --example originate_loopback_bowout
//!   Configure via ESL_HOST, ESL_PORT, ESL_PASSWORD env vars; FREESWITCH_VERSION
//!   (or ESL_BLOCK_PARSE for a dev build) is the parser revision rendered for.

#[path = "common/block_parse.rs"]
mod block_parse;
mod common;

use freeswitch_esl_tokio::commands::UuidKill;
// Loopback types live in the variables module, beside LoopbackVariable, rather
// than at the crate root.
use freeswitch_esl_tokio::variables::LoopbackChannelName;
use freeswitch_esl_tokio::{EslEventType, EventFormat, EventHeader, HeaderLookup, Originate};
use std::time::Duration;

const BOWOUT_YAML: &str = include_str!("originate_loopback_bowout.yaml");

/// The pair bows out within milliseconds of the bridge, so this bounds a run
/// where it never happened rather than one where it is still coming.
const BOWOUT_DEADLINE: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Escaping depends on the switch's parser revision, which the crate never
    // asks the switch for, so the version is stated in the environment.
    let block_parse = block_parse::block_parse_from_env()?;
    let originate: Originate = yaml_serde::from_str(BOWOUT_YAML)?;

    println!("=== YAML -> Originate ===");
    let line = originate.render(block_parse);
    println!("{line}\n");

    let (client, mut events) = common::connect_from_env().await?;

    // Subscribe before originating: the pair can bow out within milliseconds
    // of the A leg's bridge starting, and those hangups are the evidence.
    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::ChannelBridge,
                EslEventType::ChannelHangupComplete,
            ],
        )
        .await?;

    let resp = client
        .api(&line)
        .await?;
    let a_leg = resp
        .api_result()?
        .to_string();
    println!("A leg (loopback): {}\n", a_leg);

    println!("=== Waiting for bowout ===");
    // Two loopback hangups carrying loopback_hangup_cause=bridge, plus the
    // uuid_bridge that joins the two surviving real channels.
    let mut resigned = Vec::new();
    let mut spliced: Option<(String, String)> = None;

    // Distinguished from each other so a failure below names what actually
    // happened: reporting "bowout did not happen" for a dropped connection
    // sends the reader after the wrong thing.
    let mut gave_up = None;
    let deadline = tokio::time::Instant::now() + BOWOUT_DEADLINE;
    while resigned.len() < 2 || spliced.is_none() {
        let evt = match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => evt,
            Ok(Some(Err(e))) => {
                gave_up = Some(format!("event stream error: {}", e.display_chain()));
                break;
            }
            Ok(None) => {
                gave_up = Some("event stream closed".to_string());
                break;
            }
            Err(_) => {
                gave_up = Some(format!("nothing more arrived within {BOWOUT_DEADLINE:?}"));
                break;
            }
        };
        match evt.event_type() {
            Some(EslEventType::ChannelHangupComplete) => {
                // Some means the leg resigned and its call lives on elsewhere;
                // None is a real teardown. Never test the cause value to decide
                // that -- mod_loopback writes a different token depending on
                // which path resigned, so matching one silently misses the
                // other. Presence is the signal.
                let Some(resignation) = evt.loopback_resignation() else {
                    continue;
                };
                // The marker alone does not say the emitting channel is the one
                // that resigned: it gets copied onto whatever real channel
                // continues the call. Only the channel's own name answers that,
                // and never Caller-Channel-Name, which reports the leg's.
                let Some(name) = evt.channel_name() else {
                    continue;
                };
                let Some(leg) = LoopbackChannelName::parse(name) else {
                    continue;
                };
                println!(
                    "resigned {:<20} (leg {}) -> real channel {}",
                    name,
                    leg.leg(),
                    resignation
                        .other_uuid()
                        .unwrap_or("?")
                );
                resigned.push(name.to_string());
            }
            Some(EslEventType::ChannelBridge) => {
                // Three bridges happen here: loopback-a=null/nearend,
                // loopback-b=null/farend, and finally the bowout's
                // uuid_bridge. Only the last one has a real channel on
                // both sides.
                let (Some(this), Some(other)) = (
                    evt.channel_name(),
                    evt.header(EventHeader::OtherLegChannelName),
                ) else {
                    continue;
                };
                if LoopbackChannelName::parse(this).is_some()
                    || LoopbackChannelName::parse(other).is_some()
                {
                    continue;
                }
                let (Some(a), Some(b)) =
                    (evt.unique_id(), evt.header(EventHeader::OtherLegUniqueId))
                else {
                    continue;
                };
                println!("bridged  {} = {}", this, other);
                spliced = Some((a.to_string(), b.to_string()));
            }
            _ => {}
        }
    }

    if let Some(reason) = gave_up {
        return Err(format!(
            "stopped watching before the bowout completed ({reason}); \
             saw {} of 2 resignations and {} bridge",
            resigned.len(),
            if spliced.is_some() { "the" } else { "no" }
        )
        .into());
    }
    if resigned.len() != 2 {
        return Err(format!("expected 2 loopback legs to resign, saw {:?}", resigned).into());
    }
    let Some((near, far)) = spliced else {
        return Err("no uuid_bridge between the real legs -- bowout did not happen".into());
    };

    println!("\n=== After bowout ===");
    println!(
        "the loopback pair is gone; {} talks straight to {}",
        near, far
    );

    // Killing either survivor tears down what is now a plain two-party call.
    let kill = UuidKill::new(&near);
    match client
        .api(&kill.to_string())
        .await
        .and_then(|resp| {
            resp.api_result()
                .map(str::to_string)
        }) {
        Ok(_) => println!("hung up {near}"),
        Err(e) => eprintln!("could not hang up {near}: {}", e.display_chain()),
    }

    client
        .disconnect()
        .await?;
    Ok(())
}
