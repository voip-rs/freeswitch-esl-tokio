# Migrating from 2.x

freeswitch-esl-tokio 3.0 and freeswitch-types 3.0 replace freeswitch-esl-tokio 2.x and freeswitch-types 1.x. "2.x" below means that pair.

## SIP headers through your own sip-header

freeswitch-types depends publicly on [sip-header-catalog](https://docs.rs/sip-header-catalog) alone. SIP value types and parsers stay in [sip-header](https://docs.rs/sip-header) 1.0, which you add to your own dependencies; its typed accessors reach every store here through the catalog's row trait.

```toml
[dependencies]
sip-header = "1"
```

```rust,ignore
use sip_header::SipHeaderLookup;

let ci = event.call_info()?;          // Result<Option<SipUriInfo>, sip_header::ParseError>
let pai = event.p_asserted_identity()?; // Result<Option<SipHeaderAddrList>, _>
```

| 2.x | 3.0 |
|---|---|
| `freeswitch_types::sip_header`, `freeswitch_types::sip_uri` (and the same on freeswitch-esl-tokio) | your own `sip_header` dependency; URI values are `sip_header::sip_uri_types`, and parsing a URI yourself takes your own `sip_uri` |
| `SipHeaderAddr`, `UriInfo`, `UriInfoEntry`, `HistoryInfo`, `HistoryInfoEntry`, `SipGeolocation` at the crate root | `sip_header::{SipHeaderAddr, SipUriInfo, SipUriInfoEntry, SipHistoryInfo, SipHistoryInfoEntry, SipGeolocation}` |
| `UriInfoError`, `HistoryInfoError`, `ParseSipHeaderAddrError` | `sip_header::ParseError` |
| `HistoryInfoReason` | `sip_header::SipReasonEntry` |
| `SipGeolocationRef` | `sip_header::SipGeolocationEntry` |
| `extract_header` at the crate root | `sip_header::extract_header`, or `SipMessageHeaders` as a row store |
| `SipHeaderLookup` at the root and in the prelude | `sip_header::SipHeaderLookup` in scope (`use sip_header::SipHeaderLookup`); this crate's root and prelude carry the catalog's `SipHeaderRows` and `SipHeaderRowsExt` |
| `impl SipHeaderLookup for Store { fn sip_header_str(..) }` beside `HeaderLookup` | `impl SipHeaderRows for Store { fn sip_header_rows_str(..) -> Result<Vec<&str>, RowError> }`; a `HashMap<String, String>` field already implements it |
| `sip_header_str()` returns `Option<&str>` | `Result<Option<&str>, RowError>`, the first decoded row; the stored value is `HeaderLookup::header_str` |
| `EslHeaders::parse_uri_info(raw)`, `parse_history_info(raw)`, `split_entries(raw)` | `EslHeaders::decode_value(SipHeader::CallInfo, raw)?` gives a `SipHeaderField`; call `.call_info()`, `.history_info()`, … on it. `decode_value_raw(name, raw)` takes a header the catalog does not register |
| an over-cap `ARRAY::` value handed whole to the parser | `RowError` with `RowErrorKind::TooManyRows`, through every accessor as `sip_header::ParseError::Row` |
| `SipPassthroughHeader::extract_from(&str) -> Vec<String>` | `extract_from(&store) -> Result<SipHeaderField, RowError>` over any row store, `sip_header::SipMessageHeaders::new(raw)` for a raw message |
| `SipHeader::is_multi_valued()` | `is_list()` (a comma list) or `may_repeat()` (may occur more than once) |
| `conference-info` feature | sip-header's own `conference-info` feature |
| `SipHeader` serialized as its variant name (`"CallId"`) | its wire name (`"Call-ID"`); deserialize accepts any spelling |
| `EventHeader` and the `*Variable` enums serialized as variant names (`"UniqueId"`) | their wire names (`"Unique-ID"`, `"sip_from_user"`); deserialize accepts any case |

- sip-header 1.0 parses through traits: `SipHeaderAddr::parse(s)` with `HeaderParse` in scope, never `s.parse::<SipHeaderAddr>()`.
- Every sip-header accessor returns `Result<Option<T>, ParseError>`; list and auth accessors that returned a bare `Vec` are wrapped too.

## Command builders

| 2.x | 3.0 |
|---|---|
| `commands::parse_originate_target(s, dialplan)` | parse the whole line with `Originate::parse_for(s, block_parse)`, whose target is read the way `originate_function` reads it |
| `DialplanType::Xml`, `DialplanType::Inline`, matched as enum variants | `DialplanType::XML`, `DialplanType::INLINE`, compared with `==`; the type is no longer `Copy` |
| `Originate::dialplan_raw(name)` | `.dialplan(DialplanType::named(name)?)`; `inline` and `xml` in any case resolve to the known two |
| `Originate::dialplan_name()` | `dialplan_type().map(DialplanType::name)`; `dialplan_type()` now returns a named dialplan too, where it returned `None` |
| `"name".parse::<DialplanType>()` refuses any name but `inline` and `xml` | accepts any dialplan name but `undef`, which `ParseDialplanTypeError` now reports alone |
| `Application::to_string_with_dialplan` | no public replacement; render the `Originate` |
| `Variables::insert(k, v)` returns `()` | returns `Result<(), OriginateError>`, refusing what parsing a block refuses |
| `Variables::with_vars(scope, pairs)` returns `Variables` | returns `Result<Variables, OriginateError>` |
| `Variables::set_scope(scope)` returns `()` | returns `Result<(), OriginateError>`, refusing a scope a held pair or the chosen separator cannot be written in |
| `Variables::iter_mut()`, `values_mut()` | removed; `insert` the new value under the same name, which keeps its position |
| a variable pair refused as `OriginateError::ParseError(String)` | `OriginateError::UndeliverableVariable { index, variable }`; `variable` is an `UndeliverableVariable` naming the `VariablePart` and the `VariableFault`, the same from parse, config load and the builder |
| `UuidSetVar::new(uuid, key, value)` returns `UuidSetVar` | returns `Result<UuidSetVar, UndeliverableVariable>` |
| `UuidSetVar { uuid, key, value }` and `UuidTransfer { uuid, destination, dialplan }` public fields | private; read them with `uuid()`, `key()`, `value()`, `destination()` and `dialplan()` |
| `AppCommand::set_var(name, value)` returns `EslCommand` | returns `Result<EslCommand, UndeliverableVariable>` |
| `UuidSetVar::new` and `AppCommand::set_var` accept any value | refuse a control character other than tab in a value set under a name mod_sofia sends as an outgoing SIP header (`sip_h_`, `sip_rh_`, `sip_ph_`, `sip_bye_h_`), as `Variables` already did |
| `DialString: fmt::Display` | `DialString` carries only the variable accessors; render an endpoint through `Endpoint` |
| `AudioEndpoint`'s `Display` (it rendered `audio`), `fmt_with_prefix` | wrap it in `Endpoint::PortAudio`, `PulseAudio` or `Alsa` and render that |
| `AudioEndpoint::parse_with_prefix(s, prefix)` | `Endpoint::parse_for(s, carrier)`, which names the module from the text |
| `"…".parse::<SofiaContact>()`, `"…".parse::<GroupCall>()`, which always failed at the API carrier | `Endpoint::parse_for(s, DialStringCarrier::Dialplan)` |
| `DialString::set_variables(vars)` returns `()` | returns `Result<(), OriginateError>`; an `ErrorEndpoint` refuses a block with `OriginateError::VariablesNotSupported` where it dropped it |
| `BridgeDialString::new(groups)` returns `BridgeDialString` | returns `Result<BridgeDialString, OriginateError>`, refusing what parse and config load refuse: more legs or groups than the switch splits off, and a group whose bracket spans legs (`BracketSpansLegs`) |
| `BridgeDialString::groups_mut()` | removed; build a new one with `new` |
| public fields on `SofiaEndpoint`, `SofiaGateway`, `SofiaContact`, `LoopbackEndpoint`, `UserEndpoint`, `GroupCall`, `AudioEndpoint` and `ErrorEndpoint` | private; read them with the same-named methods (`profile()`, `destination()`, `gateway()`, `user()`, `domain()`, `extension()`, `context()`, `dialplan()`, `name()`, `group()`, `order()`, `cause()`, `variables()`), `Option<&str>` where the field was an `Option<String>` |
| `SofiaEndpoint::new`, `SofiaGateway::new`, `SofiaContact::new`, `LoopbackEndpoint::new`, `UserEndpoint::new`, `GroupCall::new` return the endpoint | return `Result<_, OriginateError>` |
| `with_profile`, `with_context`, `with_dialplan`, `with_domain`, `with_destination` on the endpoint structs return the endpoint | return `Result<_, OriginateError>`; `with_variables` and `GroupCall::with_order` stay infallible |
| endpoint constructors and `with_*` builders accept any field | refuse what parse and config load refuse, as `OriginateError::UndeliverableEndpointField` naming the endpoint, field and `EndpointFieldFault`; a field legal only beside its qualifier goes in through `SofiaGateway::on_profile`, `SofiaContact::on_profile` or `UserEndpoint::in_domain` |
| `Variables`, `Endpoint` and the endpoint structs' `Display` (`to_string()`, `format!("{vars}")`), which rendered for `DialStringCarrier::EslApi` | `render(target)`, naming the carrier or `DialStringTarget`; `Debug` stays for logs |
| `s.parse::<Variables>()`, `s.parse::<Endpoint>()`, `s.parse::<SofiaEndpoint>()` and the other endpoint structs' `FromStr` | `Variables::parse_for(s, target)`, `Endpoint::parse_for(s, target)`, `SofiaEndpoint::parse_for(s, target)` |
| `Variables::display_for(target)`, `Endpoint::display_for(target)` and their `VariablesDisplay`, `EndpointDisplay` adapters | `render(target)`, which returns the `String` |
| `Originate` and `BridgeDialString`'s `Display` (`cmd.to_string()`, `format!("{cmd}")`), which rendered at the default revision | `cmd.render(block_parse)`; the carrier is fixed by the type, so only the `BlockParse` is named, `BlockParse::for_version(&version)` or `BlockParse::default()` |
| `s.parse::<Originate>()`, `s.parse::<BridgeDialString>()` | `Originate::parse_for(s, block_parse)`, `BridgeDialString::parse_for(s, block_parse)` |
| `Originate::display_with(block_parse)`, `BridgeDialString::display_with(block_parse)` and their `OriginateDisplay`, `BridgeDialStringDisplay` adapters | `render(block_parse)`, which returns the `String` |
| `Originate::parse_with(s, block_parse)`, `BridgeDialString::parse_with(s, block_parse)` | `parse_for(s, block_parse)` |
| `AppCommand::bridge(bridge)` with a `BridgeDialString` or `Endpoint` | `AppCommand::bridge(bridge.render(block_parse))`, or `endpoint.render(DialStringCarrier::Dialplan)` |
| `FlattenedDialString::display_for(target)` and its `FlattenedDialStringDisplay` adapter | `render(target)`, which returns the `String`; `display_raw()` stays and returns an `impl Display` |
