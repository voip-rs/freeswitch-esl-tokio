# Config-driven commands (serde)

All command builder types implement `Serialize`/`Deserialize`, so originate and bridge commands can be driven entirely from config files:

```yaml
endpoint: !sofia_gateway
  gateway: my_provider
  destination: "18005551234"
application:
  name: park
timeout_secs: 30
```

```rust,no_run
# use freeswitch_esl_tokio::{BlockParse, EslClient, FreeswitchVersion, Originate};
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let (client, _events) = EslClient::connect("localhost", 8021, "ClueCon").await?;
# let yaml = r#"
# endpoint: !sofia_gateway
#   gateway: my_provider
#   destination: "18005551234"
# application:
#   name: park
# timeout_secs: 30
# "#;
let originate: Originate = yaml_serde::from_str(yaml)?;
// The switch's parser revision, which the crate never asks the switch for;
// see "FreeSWITCH version and parser revision" below for where it comes from.
let version: FreeswitchVersion = "1.10.12".parse()?;
let block_parse = BlockParse::for_version(&version)?;
client.bgapi(&originate.render(block_parse)).await?;
# Ok(())
# }
```

## Event subscriptions

`EventSubscription` also serializes, so subscriptions can live in config files:

```rust
# use freeswitch_esl_tokio::{EslEventType, EventSubscription};
let subscription: EventSubscription = yaml_serde::from_str(
    r#"
format: plain
events:
- CHANNEL_CREATE
- CHANNEL_ANSWER
- CHANNEL_HANGUP_COMPLETE
- HEARTBEAT
custom_subclasses:
- "sofia::register"
## Each filter is a (header, value) tuple
filters:
- [Call-Direction, inbound]
"#,
)?;
assert!(subscription.events().contains(&EslEventType::ChannelCreate));
# Ok::<(), Box<dyn std::error::Error>>(())
```

Every enum with a wire name, event types, hangup causes and header names among them, is written as the switch spells it (`CHANNEL_CREATE`, `USER_BUSY`, `Unique-ID`) and read back in any ASCII case. The Rust variant name (`ChannelCreate`), which 2.x wrote, still reads.

The order of `events` does not matter: `CUSTOM` is terminal on the wire and the serializer always emits it last. See [event-command-grammar.md](../event-command-grammar.md) for the grammar, what the raw string commands do not guarantee, and why a bare `CUSTOM` subscribes to nothing.

## Variables

`Variables` deserializes ergonomically -- a flat map defaults to `Default` scope:

```rust
# use freeswitch_esl_tokio::{Variables, VariablesType};
let flat: Variables = yaml_serde::from_str(
    r#"
originate_timeout: "600"
sip_h_X-Custom: value
"#,
)?;
assert_eq!(flat.scope(), VariablesType::Default);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Other scopes use the explicit form:

```rust
# use freeswitch_esl_tokio::{Variables, VariablesType};
let scoped: Variables = yaml_serde::from_str(
    r#"
scope: enterprise
vars:
  key: value
"#,
)?;
assert_eq!(scoped.scope(), VariablesType::Enterprise);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## FreeSWITCH version and parser revision

The version a deployment targets belongs in the application's own config, next to the connection, never inside an originate: it describes the switch, not the command. `FreeswitchVersion` and `BlockParse` both deserialize, so an application can carry the version and an explicit override for a build the crate does not vouch for:

```yaml
freeswitch_version: "1.10.13-dev"
block_parse: pair_split_cleans
```

Map it with `BlockParse::for_version` when `block_parse` is absent, and pass the result to `render`; see [commands.md](commands.md#the-switchs-parser-revision).

## How endpoint types appear in YAML

The `!sofia_gateway` prefix in the example above is a YAML tag -- it tells the deserializer which endpoint type to build from the fields that follow. Each variant of the `Endpoint` enum has its own tag:

```rust
# use freeswitch_esl_tokio::Endpoint;
let endpoints: Vec<Endpoint> = yaml_serde::from_str(
    r#"
## SIP gateway routing
- !sofia_gateway
  gateway: my_provider
  destination: "18005551234"

## Direct SIP profile routing
- !sofia
  profile: internal
  destination: "1000@example.com"

## Internal loopback
- !loopback
  extension: "9199"

## Directory-based routing
- !user
  name: "1001"
  domain: example.com

## Hang up with a cause, named as the switch names it
- !error
  cause: USER_BUSY
"#,
)?;
assert_eq!(endpoints.len(), 5);
# Ok::<(), Box<dyn std::error::Error>>(())
```

This is the format produced by `yaml_serde`. JSON libraries represent the same data differently (`{"sofia_gateway": {"gateway": ...}}` instead of a YAML tag), but both deserialize into the same Rust types.

See [originate-loopback-yaml.md](../originate-loopback-yaml.md) for a complete YAML originate covering every field, how variables reach both loopback legs, and how to make a loopback pair bow out.
