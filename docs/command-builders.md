# Command Builder Architecture

## Problem

The library has solid ESL transport but no typed command construction. Developers
pass raw strings to `api()`/`bgapi()` and manually format channel variable strings.
This is error-prone: malformed originate strings, forgotten escaping, wrong variable
scope brackets.

## Design

Pure string types with no transport coupling. They produce and parse strings,
and `EslClient` sends what they produce. A builder whose text depends on where
it goes (`Variables`, `Endpoint` and its structs, `Originate`,
`BridgeDialString`) renders through `render` and parses through `parse_for`,
each naming its target; the rest implement `Display`/`FromStr`.

```rust
let block_parse = BlockParse::for_version(&"1.10.12".parse()?)?;
let cmd = Originate::application(endpoint, app)?.with_timeout(Duration::from_secs(30));
let line = cmd.render(block_parse);
client.bgapi(&line).await?;

let parsed = Originate::parse_for(&line, block_parse)?;
assert_eq!(cmd, parsed);
```

### Why plain strings, not a trait on EslClient

- Round-trip testing without a FreeSWITCH connection
- Downstream crates can build commands without depending on tokio/transport
- Application-specific extensions (NGCS, SIP URI builders) compose naturally
- FreeSWITCH command strings are the stable interface — types are convenience

### Why not a Command trait

A `Command` trait with `fn to_command_string(&self) -> String` would add ceremony
for no benefit over `Display` and `render`. Every FreeSWITCH API command is
ultimately a string.

## Module Layout

Command builders and domain types live in the `freeswitch-types` crate (no async
deps). The ESL transport crate re-exports everything.

```
freeswitch-types/src/
├── channel.rs              # ChannelState, CallState, AnswerState, CallDirection,
│                           # HangupCause, ChannelTimetable, TimetablePrefix
├── headers.rs              # EventHeader enum — typed event header names
├── macros.rs               # define_header_enum! — generates Display/FromStr/as_str for header enums
├── commands/               # API command string builders (→ api()/bgapi())
│   ├── mod.rs              # Re-exports, originate_quote() and originate_split()
│   ├── variables/          # Variables: its render, parse and refusals
│   │   └── target.rs       # DialStringCarrier, BlockParse, DialStringTarget
│   ├── endpoint/           # Endpoint types, DialString trait
│   ├── originate/          # Originate
│   │   ├── application.rs  # Application, OriginateTarget, DialplanType
│   │   └── error.rs        # OriginateError
│   ├── bridge.rs           # BridgeDialString (multi-endpoint dial strings)
│   ├── flattened/          # FlattenedDialString, a list the switch produced
│   ├── channel.rs          # uuid_answer, uuid_bridge, uuid_kill, uuid_setvar, ...
│   └── conference.rs       # conference mute/unmute/hold/dtmf
├── switch_passes/          # Ports of the switch's string passes
│   ├── separate.rs         # switch_separate_string and its cleanup
│   ├── api_argument.rs     # originate's argument split, its escapes and quoting
│   ├── expansion.rs        # the dialplan carrier's escape handling
│   ├── brackets.rs         # switch_event_create_brackets and the event install
│   ├── originate_legs.rs   # switch_ivr_originate's thread, group and leg splits
│   ├── escape.rs           # the escapes a render writes for each pass
│   ├── execute_on.rs       # the execute_on hook's split and what reaches its application
│   ├── inline_hunt.rs      # inline_dialplan_hunt's split and render
│   ├── originate_function.rs  # originate_function's positional read
│   └── pipeline.rs         # every pass a target applies, in order
└── variables/              # Channel variable format parsers
    ├── mod.rs              # VariableName trait, re-exports
    ├── core.rs             # ChannelVariable enum — typed variable names
    ├── sofia.rs            # SofiaVariable enum — mod_sofia / SIP variables
    ├── esl_array.rs        # ARRAY::item1|:item2 format
    └── sip_multipart.rs    # SIP multipart body extraction

src/
├── command.rs              # ESL protocol: EslCommand, EslResponse
└── app/
    └── dptools.rs          # AppCommand — answer, hangup, bridge, playback, ...
```

### app/ vs commands/

- **app/** — dialplan applications executed via `sendmsg` (outbound mode). These
  produce `EslCommand::Execute` values for `client.send_command()`.
- **commands/** — API commands sent via `api()`/`bgapi()`. These produce strings.
  The distinction matches FreeSWITCH's own split: `sendmsg` targets a specific
  channel, API commands are global.

### variables/

Parsing types for FreeSWITCH's structured channel variable formats. These are not
commands — they parse values found in event headers. Separate module because they
have no relationship to command construction.

## Key Types

### Originate

Ported from a Python originate builder implementation.

**Variables** — channel variable bag with scope. FreeSWITCH uses three bracket types:

- `{k=v}` — default scope (set on all legs)
- `<k=v>` — enterprise scope (set on all endpoints in an enterprise originate)
- `[k=v]` — channel scope (set on the immediately following endpoint only)

Escaping rules (measured on a live switch): commas → `\,`, single quotes and
backslashes → one backslash level per tokenizer pass, which depends on the
carrier and on the switch's block-parser revision, values containing spaces →
wrapped in single quotes. Counts and revisions are in
[dial-string-format.md](dial-string-format.md#variable-value-escaping).

Uses `indexmap::IndexMap` to preserve insertion order — variable order matters for
readability and debugging, and round-trip parsing should produce identical output.

**Endpoint** — enum wrapping concrete structs, one per FreeSWITCH endpoint module.
Each struct implements `Serialize`, `Deserialize` and the `DialString` trait.
All but `AudioEndpoint`, which does not know its module, and `ErrorEndpoint`,
whose text reads the same at every carrier and so keeps `Display`/`FromStr`,
implement `render(target)`; all of those but `SofiaContact` and `GroupCall`
implement `parse_for`, and an expression parses through `Endpoint::parse_for`,
which refuses it where no pass expands it. The enum provides serde-compatible
polymorphism and renders every endpoint.

Real endpoints:

- `SofiaEndpoint` — `{vars}sofia/profile/destination`
- `SofiaGateway` — `{vars}sofia/gateway/[profile::]name/destination`
- `LoopbackEndpoint` — `{vars}loopback/extension/context`
- `UserEndpoint` — `{vars}user/name[@domain]`

Expression endpoints (FS runtime expressions, expanded only by a dialplan
application or the `expand` API, so parsed at the dialplan carrier alone):

- `SofiaContact` — `{vars}${sofia_contact([profile/]user@domain)}`
- `GroupCall` — `{vars}${group_call(group@domain[+order])}`
- `ErrorEndpoint` — `error/cause`

Audio device endpoints (shared `AudioEndpoint` struct):

- `PortAudio` — `{vars}portaudio[/destination]`
- `PulseAudio` — `{vars}pulseaudio[/destination]`
- `Alsa` — `{vars}alsa[/destination]`

The text after the block is escaped for the carrier's pass and the leg splits,
and parsed through the port of those passes; a field the endpoint module splits
elsewhere is refused on parse and config load
([Endpoint text](dial-string-format.md#endpoint-text)).

See [dial-string-format.md](dial-string-format.md) for full endpoint and
variable scoping documentation.

**Application** — inline (`name` or `name:args`) or XML (`&name(args)`) format.

**OriginateTarget** — the second argument to originate, one of:

- `Extension(String)` — route through the dialplan engine (e.g. `1000`)
- `Application(Application)` — single XML-format app (e.g. `&park()`)
- `InlineApplications(Vec<Application>)` — one or more inline apps (e.g. `park,hangup:NORMAL_CLEARING`)

**Originate** — full command: `originate {endpoint} {target} [dialplan] [context] [cid_name] [cid_num] [timeout]`

`originate_function` NULLs every argument reading `undef` in any case, then reads
the rest strictly by position: the third is the dialplan whatever it says, and
past seven it answers usage. Both splits parse through one reader with those
rules, refusing an eighth argument and an `undef` target, which the switch
asserts is set and aborts on. A target of `&` and more is an application
whatever the dialplan, its arguments ending at the first `)`: parse, config
load and the constructors refuse an application carrying a `)` there, and an
extension opening `&`. Any other dialplan word is kept as `DialplanType::named()`,
`inline` and `xml` in any case resolving to the typed two; the transfer looks a
dialplan module up by it, and a name no module registers hangs the channel up
with `NO_ROUTE_DESTINATION` (measured). A positional left `None` but forced
present by a later one is written `undef` on either split, and the switch falls
back to `XML`/`default`. A line opening `^^ ` parses as the blank split.

On the blank split every argument goes through `originate_quote()`: a token that
is empty or carries a space, `'`, `\`, or whitespace the API strips from its
line's edges is wrapped and escaped as
`UuidSetVar` quotes its value, since `uuid_setvar` splits on the same blank
tokenizer, so `\n`, `\\`, `\s` and `\t` arrive as written rather than read as
escapes. `originate_unquote()` runs that split's cleanup and inverts it.

With `with_argv_separator(sep)` the line is
`originate ^^<sep><endpoint><sep><target>[<sep>positional…]`, every argument
escaped once for that split and none passed through `originate_quote()`. A
positional left `None` but forced present by a later one is written `undef`.
`Some("")` is an empty token, which
`switch_ivr_session_transfer` reads as the leg's own context (measured). An empty value in the last slot is written `''`, because a trailing
separator adds no argument. An `Originate` dials one `Endpoint`; a multi-leg list
goes on the caller's own line through `FlattenedDialString::display_raw()`.

**originate_split()** — splits a command line the way the `originate` API splits its
arguments: `separate_string_blank_delim` on a space, `separate_string_char_delim` on
any other delimiter. A leading `^^X` overrides the delimiter it is given, as the
switch reads one. Tokens keep their quoting, which later parsing consumes.

**FlattenedDialString** — any dial string read through the switch's own passes
for a `DialStringTarget`, whatever wrote it: a `group_call` expansion, a config
line, a line a human typed. Each leg answers `variable()` with what its channel
receives, carries a `LegTarget` (`error/` cause, typed `Endpoint`, or the text of
a module this crate does not model) and per-pair warnings. A module outside the
typed set fails no parse: the block, the leg splits and the argv pass are core
behaviour, so the leg keeps its text and the rest of the list reads as usual.
`retain()` drops legs; `display_raw()` forwards the kept legs as written,
`render()` renders them canonically.

**DialStringTarget::with_argv_separator** — the target for a line that splits
`originate`'s arguments on a `^^X` separator. The dial string is one argument of
that split: rendered, it is escaped once for the split; parsed, the split's
cleanup runs first. `escape_argument()` applies that escape to text the caller
holds; at an API target with no separator it escapes for the blank split, every
space as `\s`. A caller that writes its own `originate ^^~<list>~&park` escapes a
switch-produced list with `escape_argument()`, reads it with
`FlattenedDialString::parse_for()` at the same target, drops legs with
`retain()`, and splices `display_raw()`, still escaped, between the separators.
Refused separators and the reason for each are in
[dial-string-format.md](dial-string-format.md#x-argument-separator).

### Channel Commands

Thin wrappers producing `uuid_*` command strings. No parsing needed — these are
write-only commands.

| Type | Output |
|---|---|
| `UuidAnswer` | `uuid_answer {uuid}` |
| `UuidBridge` | `uuid_bridge {uuid} {other}` |
| `UuidDeflect` | `uuid_deflect {uuid} {uri}` |
| `UuidHold` | `uuid_hold [off\|toggle] {uuid}` |
| `UuidKill` | `uuid_kill {uuid} [cause]` |
| `UuidGetVar` | `uuid_getvar {uuid} {key}` |
| `UuidSetVar` | `uuid_setvar {uuid} {key} '{value}'`, the value escaped for the blank split's cleanup: `\` and `'` take a backslash |
| `UuidTransfer` | `uuid_transfer {uuid} {dest} [dialplan]` |
| `UuidSendDtmf` | `uuid_send_dtmf {uuid} {dtmf}` |

### Conference Commands

| Type | Output |
|---|---|
| `ConferenceMute` | `conference {name} mute\|unmute {member_id}` |
| `ConferenceHold` | `conference {name} hold\|unhold all [stream]` |
| `ConferenceDtmf` | `conference {name} dtmf {member} {dtmf}` |

### EslArray

Parses FreeSWITCH's `ARRAY::item1|:item2|:item3` format found in channel variables
when a variable holds multiple values. `Display` reproduces the wire format.

### MultipartBody

Parses SIP multipart bodies stored in `variable_sip_multipart` channel variables.
Each element is `mime/type:body_data` within an `ARRAY::` container. Provides
`by_mime_type()` for typed extraction (e.g., getting PIDF+XML geolocation data).


### BridgeDialString

Typed builder for bridge dial strings with multiple endpoints, simultaneous
ring, and sequential failover. Implements `render`/`parse_for` at a `BlockParse`,
`Serialize` and `Deserialize`.

Structure: `Vec<Vec<Endpoint>>` — outer vec is sequential groups (`|`),
inner vec is simultaneous endpoints (`,`). A leading `{variables}` or
`<variables>` block applies to all endpoints; `with_variables` refuses a
`[variables]` one, which the switch reads as the first leg's, and keeps an
empty one as none, and config load runs the same check. Per-endpoint
`[variables]` are carried on each `Endpoint`.

```rust
let bridge = BridgeDialString::new(vec![
    vec![ep1, ep2],  // ring ep1 and ep2 simultaneously
    vec![ep3],       // if both fail, try ep3
])?
.with_variables(vars)?;
// Wire: {vars}ep1,ep2|ep3
```

See [dial-string-format.md](dial-string-format.md) for full separator
and variable scoping semantics.

### DialString trait

The per-channel variables an endpoint carries in its leading block:

```rust
pub trait DialString {
    fn variables(&self) -> Option<&Variables>;
}
```

Implemented on each concrete endpoint struct and on the `Endpoint` enum. A
block is set through `with_variables`: infallible on a struct that keeps
one, fallible on `Endpoint`, since `error/` keeps none.

## Serde Support

All command builder types implement `Serialize`/`Deserialize` for config-driven
command construction (YAML/JSON -> struct -> wire format).

Key design choices:

- **`DialplanType`** — serde uses `"xml"`/`"inline"` (lowercase, config-friendly).
  `Display` uses `"XML"`/`"inline"` (wire format). Independent representations.
  Any other name serializes as itself, kept as the switch receives it;
  `undef` in any case is refused, as the switch reads it as absent.
- **`Variables`** — flat YAML map deserializes as `VariablesType::Default` (the
  99% case). Explicit `{scope, vars}` form for Enterprise/Channel scopes.
- **`Endpoint`** — externally tagged enum with `snake_case` variant names.
- **`Originate`** — manual `Serialize`/`Deserialize` via `OriginateRaw` intermediate
  type, running on deserialize the one check parse, the constructors and the
  builders run. `BridgeDialString` uses straightforward derives.

## Dependencies

- `indexmap` — ordered map for `Variables` (preserves insertion order, O(1) lookup,
  serde support via `features = ["serde"]`)

## What This Does Not Cover

- Automatic dispatch (no `client.originate(cmd)` — just `client.bgapi(&cmd.render(block_parse))`)
- Modelling a command's output schema (`status`, `sofia status`, `show` rows)
- SIP URI type (future extension point)
