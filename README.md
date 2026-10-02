# freeswitch-esl-tokio

[![CI](https://github.com/voip-rs/freeswitch-esl-tokio/actions/workflows/ci.yml/badge.svg)][ci]
[![Tests][tests-badge]][ci]
[![crates.io](https://img.shields.io/crates/v/freeswitch-esl-tokio)](https://crates.io/crates/freeswitch-esl-tokio)
[![docs.rs](https://img.shields.io/docsrs/freeswitch-esl-tokio)][docs]

| C-verified enums | Typed API |
|---|---|
| [![EslEventType][evt-badge]][ci] [![HangupCause][hc-badge]][ci] | [![EventHeader][eh-badge]][docs] [![ChannelVariable][cv-badge]][docs] |
| [![ChannelState][cs-badge]][ci] [![CallState][ccs-badge]][ci] | [![HeaderLookup][hl-badge]][docs] |
| [![SipHeaderPrefix][sph-badge]][ci] | [![SofiaVariable][sv-badge]][docs] |
| [![CoreMediaVariable][cmv-badge]][ci] | |

[ci]: https://github.com/voip-rs/freeswitch-esl-tokio/actions/workflows/ci.yml
[docs]: https://docs.rs/freeswitch-esl-tokio
[tests-badge]: https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/ticpu/def178758b6a88effff310aca87b6b50/raw/test-count.json
[evt-badge]: https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/ticpu/def178758b6a88effff310aca87b6b50/raw/event-type-count.json
[hc-badge]: https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/ticpu/def178758b6a88effff310aca87b6b50/raw/hangup-cause-count.json
[cs-badge]: https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/ticpu/def178758b6a88effff310aca87b6b50/raw/channel-state-count.json
[ccs-badge]: https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/ticpu/def178758b6a88effff310aca87b6b50/raw/call-state-count.json
[eh-badge]: https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/ticpu/def178758b6a88effff310aca87b6b50/raw/event-header-count.json
[cv-badge]: https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/ticpu/def178758b6a88effff310aca87b6b50/raw/channel-var-count.json
[hl-badge]: https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/ticpu/def178758b6a88effff310aca87b6b50/raw/header-lookup-count.json
[sph-badge]: https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/ticpu/def178758b6a88effff310aca87b6b50/raw/sip-header-prefix-count.json
[sv-badge]: https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/ticpu/def178758b6a88effff310aca87b6b50/raw/sofia-variable-count.json
[cmv-badge]: https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/ticpu/def178758b6a88effff310aca87b6b50/raw/core-media-var-count.json

Async Rust client for FreeSWITCH [ESL](https://developer.signalwire.com/freeswitch/FreeSWITCH-Explained/Client-and-Developer-Interfaces/Event-Socket-Library/). Typed endpoints, typed events, serde support, split reader/writer, liveness detection.

## Quick start

```toml
[dependencies]
freeswitch-esl-tokio = "2"
tokio = { version = "1.0", features = ["full"] }
```

Originate through a gateway, chain a playback and a hangup inline on the answered channel, follow that channel to its last event and read the cause it hung up with.

```rust,no_run
use std::time::Duration;
use freeswitch_esl_tokio::*;
use freeswitch_esl_tokio::commands::*;

#[tokio::main]
async fn main() -> Result<(), EslError> {
    let (client, mut events) = EslClient::connect("localhost", 8021, "ClueCon").await?;

    // No CHANNEL_CREATE: it fires before the reply carries the UUID to match on.
    client.subscribe_events(EventFormat::Plain, &[
        EslEventType::BackgroundJob,
        EslEventType::ChannelState,
        EslEventType::ChannelDestroy,
    ]).await?;

    let cmd = Originate::inline(
        Endpoint::SofiaGateway(SofiaGateway::new("my_provider", "18005551234")),
        [
            Application::new(
                "playback",
                Some("/usr/share/freeswitch/sounds/en/us/callie/ivr/ivr-welcome.wav"),
            ),
            Application::new("hangup", Some(HangupCause::NormalClearing.to_string())),
        ],
    )?
    .timeout(Duration::from_secs(30));

    // BACKGROUND_JOB is a switch-wide event, so the Job-UUID is what makes a
    // result yours. BgJobTracker keeps that bookkeeping.
    let mut jobs: BgJobTracker<()> = BgJobTracker::new();
    jobs.bgapi(&client, &cmd.to_string(), ()).await?;

    let call_uuid = loop {
        let Some(event) = events.try_next().await? else {
            return Ok(());
        };
        if let Some(((), job)) = jobs.try_complete(&event) {
            break job.parse_body()?.to_string();
        }
    };

    while let Some(event) = events.try_next().await? {
        if event.unique_id() != Some(call_uuid.as_str()) {
            continue;
        }
        match event.event_type() {
            Some(EslEventType::ChannelDestroy) => {
                // The cause lands here, but CHANNEL_STATE with CS_DESTROY comes
                // after this event, so this is not where the loop ends.
                let cause = match event.hangup_cause() {
                    Ok(Some(c)) => c.to_string(),
                    Ok(None) => "no cause header".into(),
                    Err(e) => format!("unparseable: {e}"),
                };
                println!("channel destroyed: {call_uuid} ({cause})");
            }
            Some(EslEventType::ChannelState) if event.is_terminal_channel_state()? => break,
            _ => {}
        }
    }
    Ok(())
}
```

[Channel event ordering](docs/guide/events.md#channel-event-ordering) spells out why the teardown ends on the state event.

## Guides

| Guide | Covers |
|---|---|
| [Connecting](docs/guide/connecting.md) | Architecture, inbound and userauth, liveness, `bgapi` with `BgJobTracker`, outbound mode |
| [Commands](docs/guide/commands.md) | Endpoint types, originate, bridge, `execute_on_originate`, UUID and conference commands |
| [Config](docs/guide/config.md) | Originate, subscriptions and variables from YAML/JSON |
| [Events](docs/guide/events.md) | Typed accessors, timetables, custom `HeaderLookup`, variable parsers, channel event ordering |
| [SDP](docs/guide/sdp.md) | Codec strings and SDP offers (`sdp` feature) |
| [Examples](examples/README.md) | What each runnable example teaches and what it needs |
| [Migrating from 1.x](docs/migrating-from-1.x.md) | Breaking changes and upgrade steps |

Reference: [dial strings](docs/dial-string-format.md), [codec strings](docs/codec-string-format.md), [outbound quirks](docs/outbound-esl-quirks.md), [loopback bowout](docs/loopback-bowout.md), [re-exec](docs/reexec.md), [design rationale](docs/design-rationale.md).

## Features

- **Split reader/writer**: `EslClient` is `Clone + Send`, events arrive on a separate channel.
- **Typed endpoints**: every FreeSWITCH endpoint behind a `DialString` trait downstream crates can extend.
- **Typed events**: `HeaderLookup` gives typed accessors to any key-value store, not just `EslEvent`.
- **Loopback bowout detection**: `loopback_resignation()` tells a leg mod_loopback removed from a live call apart from a real teardown.
- **Failed replies read as data**: `EslError::command_failure()` hands back the text behind `-ERR` / `-USAGE`.
- **Channel dumps**: `parse_channel_dump()` decodes `uuid_dump` through the same parser as an event.
- **Command builders**: all `Display`/`FromStr`, no transport coupling.
- **Serde**: config-driven originate and bridge from YAML/JSON.
- **Connection health**: liveness detection, command timeouts, `is_connection_error()` / `is_recoverable()`.
- **Correct wire format**: two-part framing, percent-decoded headers, matches `mod_event_socket.c`.
- **Re-exec** (Unix): hand the socket to a new binary without dropping the ESL connection.

Throughput matches C ESL; see [bench/](bench/README.md).

## Requirements

- Rust 1.86+
- Tokio async runtime

## Other Rust ESL crates

- [freeswitch-esl](https://crates.io/crates/freeswitch-esl): async/tokio, JSON-only events, no split reader/writer, no liveness detection, no command builders or typed state. Stale since 2023.
- [eslrs](https://crates.io/crates/eslrs): async, still in RC. Unified stream (not split), silently discards unexpected responses, no timeouts.
- [freeswitch-esl-rs](https://crates.io/crates/freeswitch-esl-rs): synchronous/blocking, inbound only, plain events only.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for hooks and live tests.

## License

MIT OR Apache-2.0 -- see [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
