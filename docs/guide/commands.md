# Command builders

Typed builders for FreeSWITCH API commands, independent of `EslClient` and testable without a connection. A builder whose text depends on where it goes (`Variables`, `Endpoint`, `Originate`, `BridgeDialString`) renders only through `render` and parses through `parse_for`, each naming its target; the others implement `Display`.

See [command-builders.md](../command-builders.md) for the full builder architecture, all channel/conference command types, and escaping rules.

## Endpoint types

Each endpoint type is a concrete struct implementing the `DialString` trait. The `Endpoint` enum wraps them for polymorphic storage and serde.

- **SofiaEndpoint** -- `sofia/{profile}/{destination}`, direct SIP profile routing
- **SofiaGateway** -- `sofia/gateway/{gateway}/{destination}`, SIP gateway routing
- **LoopbackEndpoint** -- `loopback/{extension}/{context}`, internal loopback
- **UserEndpoint** -- `user/{name}@{domain}`, directory-based dial-string lookup
- **SofiaContact** -- `${sofia_contact(user@domain)}`, registered contacts resolved by the switch at runtime; dialplan carrier or `expand` only, refused for `originate` over the API
- **GroupCall** -- `${group_call(group@domain+A)}`, group members resolved by the switch at runtime; dialplan carrier or `expand` only, refused for `originate` over the API
- **ErrorEndpoint** -- `error/{cause}`, bridge to a hangup cause

```rust
use freeswitch_esl_tokio::commands::*;
use freeswitch_esl_tokio::FreeswitchVersion;

// Escaping depends on the command carrying the dial string and on the switch's
// parser revision, which the crate never asks the switch for: name both.
let version: FreeswitchVersion = "1.10.12".parse().unwrap();
let target =
    DialStringTarget::new(DialStringCarrier::EslApi, BlockParse::for_version(&version).unwrap());

// Direct SIP profile routing
let ep = Endpoint::Sofia(SofiaEndpoint::new("internal", "1000@example.com").unwrap());
assert_eq!(ep.render(target), "sofia/internal/1000@example.com");

// SIP gateway routing
let ep = Endpoint::SofiaGateway(SofiaGateway::new("my_provider", "18005551234").unwrap());
assert_eq!(ep.render(target), "sofia/gateway/my_provider/18005551234");

// Parse from wire format, as the target named reads it
let ep = Endpoint::parse_for("sofia/gateway/my_provider/18005551234", target).unwrap();

// Downstream crates can implement DialString on custom endpoint types
```

## Originate

```rust,no_run
# use freeswitch_esl_tokio::{EslClient, EslError};
use freeswitch_esl_tokio::commands::*;
use freeswitch_esl_tokio::FreeswitchVersion;
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let (client, _events) = EslClient::connect("localhost", 8021, "ClueCon").await?;

// The parser revision of the switch the commands are rendered for
let version: FreeswitchVersion = "1.10.12".parse()?;
let block_parse = BlockParse::for_version(&version)?;

let gw = || Endpoint::SofiaGateway(SofiaGateway::new("my_provider", "18005551234").unwrap());

// Inline applications
let cmd = Originate::inline(gw(), vec![
    Application::new("conference", Some("room1")),
]).unwrap();
// -> "originate sofia/gateway/my_provider/18005551234 conference:room1 inline"

// Extension target with dialplan and context
let ext_cmd = Originate::extension(gw(), "1000").unwrap()
    .with_dialplan(DialplanType::XML).unwrap()
    .with_context("default").unwrap();
// -> "originate sofia/gateway/my_provider/18005551234 1000 XML default"
let line = cmd.render(block_parse);
client.bgapi(&line).await?;

// Round-trip: render <-> parse_for at the same revision
let parsed = Originate::parse_for(&line, block_parse).unwrap();
assert_eq!(parsed, cmd);
# let _ = ext_cmd;
# Ok(())
# }
```

With an argument separator, spaces and quotes in any argument need no quoting: each is escaped once for the split on that separator, and an absent slot before a set one is written `undef`.

```rust
use freeswitch_esl_tokio::commands::*;
use freeswitch_esl_tokio::FreeswitchVersion;

let version: FreeswitchVersion = "1.10.12".parse().unwrap();
let block_parse = BlockParse::for_version(&version).unwrap();

let cmd = Originate::application(
    Endpoint::Loopback(LoopbackEndpoint::new("9199").unwrap().with_context("default").unwrap()),
    Application::new("socket", Some("127.0.0.1:8040 async full")),
).unwrap()
.with_cid_name("Front Desk").unwrap()
.with_argv_separator('~')
.unwrap(); // '~' is a usable separator

assert_eq!(
    cmd.render(block_parse),
    "originate ^^~loopback/9199/default~&socket(127.0.0.1:8040 async full)~undef~undef~Front Desk"
);
```

## Bridge dial strings

`BridgeDialString` builds multi-endpoint bridge arguments with simultaneous ring (`,`) and sequential failover (`|`):

```rust,no_run
# use freeswitch_esl_tokio::{EslClient, EslError, AppCommand};
use freeswitch_esl_tokio::commands::*;
use freeswitch_esl_tokio::FreeswitchVersion;
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let (client, _events) = EslClient::connect("localhost", 8021, "ClueCon").await?;

let version: FreeswitchVersion = "1.10.12".parse()?;
let block_parse = BlockParse::for_version(&version)?;

// Try primary and secondary simultaneously, then failover to backup
let bridge = BridgeDialString::new(vec![
    vec![
        Endpoint::SofiaGateway(SofiaGateway::new("primary", "18005551234")?),
        Endpoint::SofiaGateway(SofiaGateway::new("secondary", "18005551234")?),
    ],
    vec![Endpoint::SofiaGateway(SofiaGateway::new("backup", "18005551234")?)],
])?;
// -> "sofia/gateway/primary/18005551234,sofia/gateway/secondary/18005551234|sofia/gateway/backup/18005551234"

// Use with the bridge dptools application
client.send_command(AppCommand::bridge(bridge.render(block_parse))).await?;
# Ok(())
# }
```

See [dial-string-format.md](../dial-string-format.md) for the complete dial string reference (variable scoping, `^^:` custom delimiters, enterprise `:_:` originate).

## The switch's parser revision

How deeply a variable value is escaped depends on the switch's bracket-block parser as well as the command carrying it, so every render names a `BlockParse`. State the FreeSWITCH version you target and render with the revision it maps to. A development build or a version outside the vouched range is refused, and the application names a `BlockParse` itself:

```rust
use freeswitch_esl_tokio::commands::*;
use freeswitch_esl_tokio::FreeswitchVersion;

let cmd = Originate::application(
    Endpoint::SofiaGateway(SofiaGateway::new("my_provider", "18005551234").unwrap()),
    Application::simple("park"),
).unwrap();

let version: FreeswitchVersion = "1.10.12".parse().unwrap();
let parse = BlockParse::for_version(&version).unwrap();
let wire = cmd.render(parse);
assert!(wire.starts_with("originate sofia/gateway/my_provider/"));

let dev: FreeswitchVersion = "1.10.13-dev".parse().unwrap();
assert!(BlockParse::for_version(&dev).is_err());
```

The vouched range and what a revision does and does not cover are in [dial-string-format.md](../dial-string-format.md#parser-revisions).

## Forwarding a switch-produced list under `^^X`

A list the switch expanded, such as a `group_call` result, can carry values with spaces that the blank split of `originate` would cut. Name a separator on the target, escape the list for it, read it, keep the legs you want, and splice what is left into your own line:

```rust
use freeswitch_esl_tokio::commands::*;
use freeswitch_esl_tokio::FreeswitchVersion;

let version: FreeswitchVersion = "1.10.12".parse().unwrap();
let target = DialStringTarget::new(DialStringCarrier::EslApi, BlockParse::for_version(&version).unwrap())
    .with_argv_separator('~')
    .unwrap(); // '~' is a usable separator
let expanded = "[sip_h_X-Seat=desk one]user/1001@pbx.example.com,error/USER_NOT_REGISTERED";

// escape_argument is None only at the dialplan carrier.
let escaped = target.escape_argument(expanded).unwrap();
let mut list = FlattenedDialString::parse_for(&escaped, target).unwrap();
list.retain(|leg| !matches!(leg.target(), LegTarget::Error(_)));

let line = format!("originate ^^~{}~&park()", list.display_raw());
assert_eq!(line, "originate ^^~[sip_h_X-Seat=desk one]user/1001@pbx.example.com~&park()");
```

On a plain line, a target with no separator escapes the list for the blank split instead, every space written `\s`:

```rust
use freeswitch_esl_tokio::commands::*;
use freeswitch_esl_tokio::FreeswitchVersion;

let version: FreeswitchVersion = "1.10.12".parse().unwrap();
let target =
    DialStringTarget::new(DialStringCarrier::EslApi, BlockParse::for_version(&version).unwrap());
let expanded = "[sip_h_X-Seat=desk one]user/1001@pbx.example.com,error/USER_NOT_REGISTERED";

let escaped = target.escape_argument(expanded).unwrap(); // the API carrier escapes
let mut list = FlattenedDialString::parse_for(&escaped, target).unwrap();
list.retain(|leg| !matches!(leg.target(), LegTarget::Error(_)));

let line = format!("originate {} &park()", list.display_raw());
assert_eq!(line, r"originate [sip_h_X-Seat=desk\sone]user/1001@pbx.example.com &park()");
```

The measured behaviour of the override and the separators `with_argv_separator` refuses are in [dial-string-format.md](../dial-string-format.md#x-argument-separator).

## A value that must not cross the dial string

A large or free-text value — a PIDF-LO for `sip_multipart`, say — is set on the new channel by an `execute_on_originate` hook instead, which runs before the channel's session thread starts and so before a SIP leg builds its INVITE. `ExecuteOn` builds the hook's value and refuses the shapes the switch would misread; the block then carries paths and nothing the tokenizer can damage:

```rust
use freeswitch_esl_tokio::commands::*;
use freeswitch_esl_tokio::{ChannelVariable, FreeswitchVersion};

let version: FreeswitchVersion = "1.10.12".parse().unwrap();
let target =
    DialStringTarget::new(DialStringCarrier::EslApi, BlockParse::for_version(&version).unwrap());

let hook = ExecuteOn::lua("/run/app/load_multipart.lua", ["/run/app/call-42.xml"]).unwrap();
let mut vars = Variables::new(VariablesType::Default);
vars.insert(ChannelVariable::ExecuteOnOriginate.as_str(), hook.to_string()).unwrap();
assert_eq!(
    vars.render(target),
    "{execute_on_originate='lua /run/app/load_multipart.lua /run/app/call-42.xml'}"
);
```

`cargo run --example originate_multipart_file` drives it end to end; the "Keeping a value out of the tokenizer entirely" section of the dial string reference has the measurements and the traps.

## UUID and conference commands

```rust,no_run
# use freeswitch_esl_tokio::{EslClient, EslError};
use freeswitch_esl_tokio::commands::*;
use freeswitch_esl_tokio::HangupCause;
# #[tokio::main]
# async fn main() -> Result<(), EslError> {
# let (client, _events) = EslClient::connect("localhost", 8021, "ClueCon").await?;
# let uuid = "11111111-1111-1111-1111-111111111111";

// UUID commands
let kill = UuidKill::with_cause(uuid, HangupCause::NormalClearing);
// -> "uuid_kill <uuid> NORMAL_CLEARING"
client.api(&kill.to_string()).await?;

// Conference commands
let dtmf = ConferenceDtmf::new("room1", "all", "1");
// -> "conference room1 dtmf all 1"
client.api(&dtmf.to_string()).await?;
# Ok(())
# }
```

Output strings are verified by unit tests in [originate/](../../freeswitch-types/src/commands/originate/), [endpoint/](../../freeswitch-types/src/commands/endpoint/), [bridge.rs](../../freeswitch-types/src/commands/bridge.rs), [channel.rs](../../freeswitch-types/src/commands/channel.rs), and [conference.rs](../../freeswitch-types/src/commands/conference.rs).
