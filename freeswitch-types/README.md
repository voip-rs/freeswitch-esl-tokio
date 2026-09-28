# freeswitch-types

[![crates.io](https://img.shields.io/crates/v/freeswitch-types)](https://crates.io/crates/freeswitch-types)
[![docs.rs](https://img.shields.io/docsrs/freeswitch-types)](https://docs.rs/freeswitch-types)

FreeSWITCH protocol types. No async runtime dependency.

Provides FreeSWITCH ESL types (channel state, events, commands, variables) for
CDR parsing, config generation, command building, or channel variable
validation without pulling in tokio. With `default-features = false`, the only
dependencies are `sip-header-catalog` and `percent-encoding`.

SIP header names and the header row trait come from
[`sip-header-catalog`](https://docs.rs/sip-header-catalog), re-exported at the
crate root. Every header store here is a `SipHeaderRows` store, so the typed SIP
accessors of [`sip-header`](https://docs.rs/sip-header) (`call_info()`,
`p_asserted_identity()`, …) reach it through that crate's blanket impl: add
sip-header to your own dependencies and `use sip_header::prelude::*`.

For async ESL transport (connecting to FreeSWITCH, sending commands, receiving
events), see [`freeswitch-esl-tokio`](https://crates.io/crates/freeswitch-esl-tokio)
which re-exports everything from this crate.

## What's included

| Module | Contents |
|--------|----------|
| `channel` | `ChannelState`, `CallState`, `AnswerState`, `CallDirection`, `HangupCause`, `ChannelTimetable` (+ `TimetableField`, one named header at a time), `channel_driver()` |
| `headers` | `EventHeader` enum (typed event header names) |
| `lookup` | `HeaderLookup` trait (typed accessors for any key-value store) |
| `lossy_values` | `LossyValues`/`LossyValue` — non-UTF-8 header-value signal *(requires `esl` feature)* |
| `prelude` | `HeaderLookup`, the catalog's `SipHeaderRows`/`SipHeaderRowsExt`, and the header/variable enums, for a single glob import |
| `sofia` | `SofiaChannelName` (borrow-based `sofia/<profile>/<user>@<host>` parser), `SofiaEventSubclass`, `GatewayRegState`, `SipUserPingStatus` |
| `variables` | `ChannelVariable`, `CoreMediaVariable` (`unit()` → `RtpStatUnit`), `SofiaVariable`, `LoopbackVariable` (+ `LoopbackResignation`, the bowout marker, and `LoopbackChannelName`/`LoopbackLeg` — the name is the one field a resignation does not copy onto the surviving channel), `ConferenceVariable`, `SipPassthroughHeader` (unified `sip_h_*`/`sip_i_*`/etc. with `extract_from()`), `EslArray`, `MultipartBody`, `CarriedHeader` (the exhaustive mapping `SofiaVariable::carried_header()` returns from a channel variable to the SIP header(s) it carries) |
| `event` | `EslEvent`, `EslEventType`, `EventFormat`, `EslEventPriority`, `LossyValues`/`LossyValue` (non-UTF-8 header-value signal) *(requires `esl` feature)* |
| `commands` | `Originate`, `BridgeDialString`, `FlattenedDialString` (a switch-produced dial string read as the switch reads it), `UuidKill`, `UuidBridge`, endpoint types, `BlockParse`, `DialStringTarget` (carrier, parser revision and `^^X` argument separator) *(requires `esl` feature)* |
| `version` | `FreeswitchVersion`, the version an application states it targets |
| `sdp` | `CodecString`/`CodecStringEntry` (the FreeSWITCH codec-string grammar, parse and emit, with `dedup`/`simplify` ported from the switch), `SdpCodecs` (SDP offer → typed codec list, plus `SdpMediaSection` for every `m=` line the offer carried — held streams included — and `NonCodecPayload` for what the switch negotiates outside the string), `CodecImplementation` (filter a codec string against what a switch has loaded) *(requires `sdp` feature)* |

`SipHeader`, `define_header_enum!`, `SipHeaderRows`, `RowError` and the
`SipHeaderField`/`SipHeaderFields` holders are re-exported from
sip-header-catalog. SIP value types (`SipHeaderAddr`, `UriInfo`,
`HistoryInfo`, …), their parsers and the RFC 4575 conference-info types live in
sip-header, which this crate does not re-export.

## Features

- **`esl`** (enabled by default) — ESL event and command types (`EslEvent`,
  `Originate`, `Variables`, etc.). Pulls in `indexmap` for ordered header
  storage. Disable if you only need channel state or variable enums.
- **`serde`** (enabled by default) — adds `Serialize`/`Deserialize` impls for
  all public types. Disable with `default-features = false` if you only need
  wire-format parsing (`Display`/`FromStr`) without pulling in serde.
- **`sdp`** — enables the `sdp` module (`CodecString`, `SdpCodecs`,
  `CodecImplementation`), pulling in `sdp-types`.

## Usage

```toml
[dependencies]
freeswitch-types = "1"
```

Typed SIP header values come from your own sip-header:

```toml
[dependencies]
freeswitch-types = "1"
sip-header = "0.4"
```

### SIP headers on ESL stores (requires `esl` feature)

`EslHeaders`, `EslEvent` and every other store here hand sip-header one row per
header occurrence, with FreeSWITCH's `ARRAY::` encoding and `[...]` wrapping
peeled for a header that may repeat. Commas are left to the accessor, so an
authentication header keeps its parameters together.

```rust
use freeswitch_types::EslHeaders;
use sip_header::prelude::*;

let mut h = EslHeaders::new();
h.insert(
    "Call-Info",
    "ARRAY::<urn:emergency:uid:callid:abc>;purpose=emergency-CallId\
     |:<https://adr.example.com/info>;purpose=EmergencyCallData.ProviderInfo",
);
h.insert("P-Asserted-Identity", "\"Alice\" <sip:+15551234567@example.com>");

let ci = h.call_info()?.unwrap();
assert_eq!(ci.entries()[0].purpose(), Some("emergency-CallId"));
assert_eq!(ci.len(), 2);

let pai = h.p_asserted_identity()?.unwrap();
assert_eq!(pai.entries()[0].display_name(), Some("Alice"));
# Ok::<(), sip_header::ParseError>(())
```

A raw value held outside a store, such as the `sip_call_info` channel variable
fetched over ESL, goes through the same decoder into a `SipHeaderField`:

```rust
use freeswitch_types::{EslHeaders, SipHeader};
use sip_header::prelude::*;

let raw = "ARRAY::<sip:a@example.com>;purpose=icon|:<sip:b@example.com>";
let field = EslHeaders::decode_value(SipHeader::CallInfo, raw)?;
assert_eq!(field.call_info()?.unwrap().len(), 2);
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Command builders (requires `esl` feature)

```rust
use std::time::Duration;
use freeswitch_types::commands::*;

let cmd = Originate::application(
    Endpoint::SofiaGateway(SofiaGateway::new("my_provider", "18005551234")),
    Application::simple("park"),
)
.cid_name("Outbound Call")
.cid_num("5551234")
.timeout(Duration::from_secs(30));

// All builders implement Display, producing the FreeSWITCH wire format
assert!(cmd.to_string().starts_with("originate sofia/gateway/"));

// All builders implement FromStr for round-trip parsing
let parsed: Originate = cmd.to_string().parse().unwrap();
assert_eq!(parsed.to_string(), cmd.to_string());
```

Variable values are escaped for the switch's bracket-block parser as measured on
one build; `BlockParse::for_version` maps the FreeSWITCH version you target to
the revision `display_with` renders for, see
[parser revisions](https://github.com/voip-rs/freeswitch-esl-tokio/blob/master/docs/dial-string-format.md#parser-revisions).

### Typed event accessors

```rust
use freeswitch_types::{HeaderLookup, RowError, SipHeaderRows, EventHeader, ChannelVariable};

// HeaderLookup works with any key-value store, not just EslEvent
struct MyHeaders(std::collections::HashMap<String, String>);

// The SIP row trait HeaderLookup requires; the map already implements it.
impl SipHeaderRows for MyHeaders {
    fn sip_header_rows_str<'a>(&'a self, name: &str) -> Result<Vec<&'a str>, RowError> {
        self.0.sip_header_rows_str(name)
    }
}

impl HeaderLookup for MyHeaders {
    fn header_str(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(|s| s.as_str())
    }
    fn variable_str(&self, name: &str) -> Option<&str> {
        self.0.get(&format!("variable_{}", name)).map(|s| s.as_str())
    }
}

// Now MyHeaders has all typed accessors:
// h.channel_state(), h.call_direction(), h.hangup_cause(),
// h.header(EventHeader::UniqueId), h.variable(ChannelVariable::ReadCodec), etc.
```

### Serde support (requires `serde` feature, enabled by default)

All builder types implement `Serialize`/`Deserialize` for config-driven usage:

```rust
use freeswitch_types::Originate;

let json = r#"{
    "endpoint": {"sofia_gateway": {"gateway": "carrier", "destination": "18005551234"}},
    "application": {"name": "park"},
    "timeout_secs": 30
}"#;
let cmd: Originate = serde_json::from_str(json).unwrap();
println!("{}", cmd);
```

### Raw SIP message header extraction

sip-header reads a raw message into a row store (`SipMessageHeaders`).
`SipPassthroughHeader::extract_from()` takes that header's rows out of any row
store as a `SipHeaderField`, which the same accessors type:

```rust
use freeswitch_types::{SipHeader, SipPassthroughHeader};
use sip_header::prelude::*;
use sip_header::SipMessageHeaders;

let raw_invite = "INVITE sip:sos@bcf.example.com SIP/2.0\r\n\
    Call-Info: <urn:emergency:uid:callid:abc>;purpose=emergency-CallId\r\n\
    P-Asserted-Identity: \"Alice\" <sip:+15551234567@example.com>\r\n\
    \r\n";
let message = SipMessageHeaders::new(raw_invite);

let ci = SipPassthroughHeader::invite(SipHeader::CallInfo).extract_from(&message)?;
assert_eq!(ci.call_info()?.unwrap().entries()[0].purpose(), Some("emergency-CallId"));

let pai = message.p_asserted_identity()?.unwrap();
assert_eq!(pai.entries()[0].display_name(), Some("Alice"));
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Variable parsers

```rust
use freeswitch_types::variables::{EslArray, MultipartBody};

let arr = EslArray::parse("ARRAY::item1|:item2|:item3").unwrap();
assert_eq!(arr.items(), &["item1", "item2", "item3"]);
```

## Relationship to freeswitch-esl-tokio

This crate contains all domain types extracted from the
[`freeswitch-esl-tokio`](https://crates.io/crates/freeswitch-esl-tokio)
workspace. The ESL crate re-exports everything, so users of `freeswitch-esl-tokio`
don't need to depend on this crate directly.

Depend on `freeswitch-types` directly when you need FreeSWITCH types without
async transport (CDR processors, config validators, CLI tools, test harnesses).

## License

MIT OR Apache-2.0
