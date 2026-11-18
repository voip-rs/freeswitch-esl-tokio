//! Originate a loopback call described entirely by a YAML file, then read the
//! resulting channel variables back off both loopback legs.
//!
//! A `loopback/<ext>/<ctx>` originate creates two channels: the A leg, which
//! is the UUID `originate` hands back, and the B leg, which is routed into
//! `<ctx>` at extension `<ext>`. Every variable in the originate's `[...]`
//! block lands on both, so a dialplan running on the B leg can read what the
//! originator set. No bracket block can target one leg on its own; for that
//! see the "Keeping a variable off a leg" section of
//! docs/originate-loopback-yaml.md.
//!
//! Usage: FREESWITCH_VERSION=1.10.12 cargo run --example originate_loopback_yaml
//!   Configure via ESL_HOST, ESL_PORT, ESL_PASSWORD env vars; FREESWITCH_VERSION
//!   (or ESL_BLOCK_PARSE for a dev build) is the parser revision rendered for.
//!   Requires FreeSWITCH with `mod_loopback` and extension 9199 in context
//!   `test` (answer + echo).

use freeswitch_esl_tokio::commands::{UuidGetVar, UuidKill};
use freeswitch_esl_tokio::variables::LoopbackVariable;
#[path = "common/block_parse.rs"]
mod block_parse;
mod common;

use freeswitch_esl_tokio::{
    ChannelVariable, EslClient, EslEventType, EslResult, EventFormat, HeaderLookup, Originate,
    UNDEF_VALUE,
};
use std::time::Duration;

/// The originate has already answered by the time we get here, so the two
/// channels' events are in flight, not pending.
const EVENT_DRAIN: Duration = Duration::from_secs(3);

/// The originate command lives in a YAML file, not in this source.
const ORIGINATE_YAML: &str = include_str!("originate_loopback.yaml");

/// Variables the YAML sets. Each must be readable on *both* loopback legs.
const EXPECTED_VARS: &[(&str, &str)] = &[
    ("customer_id", "CUST-42"),
    ("tenant", "acme"),
    // Escaped as `'T-1001\, urgent'` on the wire; FreeSWITCH unescapes it.
    ("sip_h_X-Ticket", "T-1001, urgent"),
];

/// Read a channel variable, mapping "not set" to `Ok(None)`.
///
/// `uuid_getvar` writes the sentinel for an unset variable, so its absence
/// arrives as a successful reply. A channel that hung up first answers `-ERR No
/// such channel!` instead, which is a different thing and stays an `Err`:
/// folding the two together is how a dead channel gets reported as a variable
/// that failed to propagate.
async fn getvar(client: &EslClient, uuid: &str, name: &str) -> EslResult<Option<String>> {
    let cmd = UuidGetVar::new(uuid, name);
    let resp = client
        .api(&cmd.to_string())
        .await?;
    let value = resp.api_result()?;
    Ok((value != UNDEF_VALUE).then(|| value.to_string()))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Escaping depends on the switch's parser revision, which the crate never
    // asks the switch for, so the version is stated in the environment.
    let block_parse = block_parse::block_parse_from_env()?;
    let originate: Originate = yaml_serde::from_str(ORIGINATE_YAML)?;

    println!("=== YAML -> Originate ===");
    let line = originate.render(block_parse);
    println!("{line}\n");

    let (client, mut events) = common::connect_from_env().await?;

    client
        .subscribe_events(
            EventFormat::Plain,
            &[EslEventType::ChannelCreate, EslEventType::ChannelAnswer],
        )
        .await?;

    // `api originate` blocks until the channel answers and returns its UUID.
    // The A leg answers once the B leg's dialplan does.
    let resp = client
        .api(&line)
        .await?;
    let a_leg = resp
        .api_result()?
        .to_string();
    println!("=== Channels ===");
    println!("A leg (originate result): {}", a_leg);

    // mod_loopback cross-links the pair with this variable.
    let b_leg = getvar(
        &client,
        &a_leg,
        LoopbackVariable::OtherLoopbackLegUuid.as_str(),
    )
    .await?
    .ok_or("A leg has no other_loopback_leg_uuid -- is this really a loopback channel?")?;
    println!("B leg (other_loopback_leg_uuid): {}\n", b_leg);

    // Drain the events the originate produced, so the reader sees the two
    // channels appear rather than just trusting the UUIDs above.
    println!("=== Events ===");
    let deadline = tokio::time::Instant::now() + EVENT_DRAIN;
    loop {
        let evt = match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => evt,
            Ok(Some(Err(e))) => {
                eprintln!("event error: {}", e.display_chain());
                continue;
            }
            Ok(None) => {
                eprintln!("event stream closed early");
                break;
            }
            Err(_) => break,
        };
        let Some(uuid) = evt.unique_id() else {
            continue;
        };
        let leg = if uuid == a_leg {
            "A"
        } else if uuid == b_leg {
            "B"
        } else {
            continue;
        };
        let name = evt
            .event_type()
            .map_or_else(|| "?".to_string(), |t| t.to_string());
        println!(
            "{name:<16} leg={leg} name={}",
            evt.channel_name()
                .unwrap_or("?")
        );
    }

    println!("\n=== Variables on both legs ===");
    let mut all_present = true;
    for (name, expected) in EXPECTED_VARS {
        for (leg, uuid) in [("A", &a_leg), ("B", &b_leg)] {
            let got = getvar(&client, uuid, name).await?;
            let ok = got.as_deref() == Some(*expected);
            all_present &= ok;
            println!(
                "{} {} leg {:<16} = {:?}",
                if ok { "ok  " } else { "FAIL" },
                leg,
                name,
                got.unwrap_or_else(|| "<unset>".into())
            );
        }
    }

    // The caller ID reaches the B leg, which is where the dialplan runs.
    // `origination_caller_id_*` beats the positional cid_name/cid_num args.
    println!("\n=== Caller ID as seen by the B leg dialplan ===");
    for name in [
        ChannelVariable::CallerIdName,
        ChannelVariable::CallerIdNumber,
    ] {
        let value = getvar(&client, &b_leg, name.as_str()).await?;
        println!(
            "  {:<18} = {:?}",
            name.as_str(),
            value.unwrap_or_else(|| "<unset>".into())
        );
    }

    // Hanging up the A leg tears the pair down. A teardown that failed is worth
    // saying so: the pair outliving the run leaks a channel on the switch.
    let kill = UuidKill::new(&a_leg);
    match client
        .api(&kill.to_string())
        .await
        .and_then(|resp| {
            resp.api_result()
                .map(str::to_string)
        }) {
        Ok(_) => println!("\nhung up {a_leg}"),
        Err(e) => eprintln!("\ncould not hang up {a_leg}: {}", e.display_chain()),
    }

    client
        .disconnect()
        .await?;

    if !all_present {
        return Err("some variables did not reach both legs".into());
    }
    Ok(())
}
