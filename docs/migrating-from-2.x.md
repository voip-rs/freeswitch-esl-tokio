# Migrating from 2.x

freeswitch-esl-tokio 3.0 and freeswitch-types 3.0 replace freeswitch-esl-tokio 2.x and freeswitch-types 1.x. "2.x" below means that pair.

## Log an `EslError` with its cause

`EslError`'s `Display` now states only the error's own message: `{e}` prints `I/O error`, not `I/O error: connection refused`. The cause is reachable only through `source()`. Every place that logs an `EslError` with `{e}` loses the detail that makes the log actionable, so change each one when upgrading:

```rust,ignore
error!("ESL connection failed: {}", e.display_chain()); // I/O error: connection refused
```

anyhow's `{:#}` prints the same chain for an `EslError` wrapped in an `anyhow::Error`.

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
| `impl SipHeaderLookup for Store { fn sip_header_str(..) }` beside `HeaderLookup` | `impl SipHeaderRows for Store { fn sip_header_rows_str(..) -> Result<Vec<&str>, RowError> }`; an `EslHeaders` field already implements it |
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

## Header stores and header names

| 2.x | 3.0 |
|---|---|
| `HeaderLookup` on a bare `HashMap<String, String>`, matching names exactly | `EslHeaders::from_iter(map)` at the boundary, then read through it; every lookup resolves a dash key in any casing |
| a store of your own reading a `HashMap` by exact key | wrap an `EslHeaders` and delegate `header_str` / `variable_str` to it |
| `EslEvent::headers()`, `EslResponse::headers()` return `&IndexMap<String, String>` | they return `&EslHeaders`; `.iter()` yields `(&str, &str)` in insertion order where iterating the map yielded `(&String, &String)`, `.as_map()` gives the map, `prefixed("X-")` one family of headers in any casing |
| `header(EventHeader)` on `HeaderLookup` and `EslEvent`; `EslResponse::header(impl AsRef<str>)` | `header(impl EventHeaderName)` on all three; a string name goes through `header_str` |
| `HeaderLookup::first_header(names: [EventHeader; N])` | takes `[H; N]` for any `H: EventHeaderName`, as `header()` does; a turbofish `first_header::<N>` becomes `first_header::<_, N>`. `EventHeaderName` is in the prelude |
| an unknown dash key stored Title-Cased (`C911P-Action` as `C911p-Action`) | stored as first spelled; a later write in another casing replaces the value under that key. A store or spool written by 2.x still answers every casing |
| `DisconnectReason::ServerNotice` with `controlled_session_uuid: None` on a session-bound notice | carries the UUID the switch sent |

- A `define_header_enum!` enum of your own headers reads through `header()` once it implements `EventHeaderName` (one `as_str`), as a variable enum does with `VariableName`.
- A hand-built `sendmsg` or `sendevent` goes out with the header spellings you set rather than Title-Cased ones; the switch reads them case-insensitively.

## Command builders

| 2.x | 3.0 |
|---|---|
| `commands::parse_originate_target(s, dialplan)` | parse the whole line with `Originate::parse_for(s, block_parse)`, whose target is read the way `originate_function` reads it |
| `DialplanType::Xml`, `DialplanType::Inline`, matched as enum variants | `DialplanType::XML`, `DialplanType::INLINE`, compared with `==`; the type is no longer `Copy` |
| `Originate::dialplan_raw(name)` | `.with_dialplan(DialplanType::named(name)?)?`; `inline` and `xml` in any case resolve to the known two |
| `Originate::dialplan_name()` | `dialplan().map(DialplanType::name)`; `dialplan()` now returns a named dialplan too, where it returned `None` |
| `"name".parse::<DialplanType>()` refuses any name but `inline` and `xml` | accepts any dialplan name but `undef`, which `ParseDialplanTypeError` now reports alone |
| `Application::to_string_with_dialplan` | no public replacement; render the `Originate` |
| `Variables::insert(k, v)` returns `()` | returns `Result<(), OriginateError>`, refusing what parsing a block refuses |
| `Variables::with_vars(scope, pairs)` returns `Variables` | `Variables::from_pairs(scope, pairs)`, returning `Result<Variables, OriginateError>` |
| `Variables::set_scope(scope)` returns `()` | returns `Result<(), OriginateError>`, refusing a scope a held pair or the chosen separator cannot be written in |
| `Variables::iter_mut()`, `values_mut()` | removed; `insert` the new value under the same name, which keeps its position |
| a variable pair refused as `OriginateError::ParseError(String)` | `OriginateError::UndeliverableVariable { index, variable }`; `variable` is an `UndeliverableVariable` naming the `VariablePart` and the `VariableFault`, the same from parse, config load and the builder |
| `UuidSetVar::new(uuid, key, value)` returns `UuidSetVar` | returns `Result<UuidSetVar, UndeliverableVariable>` |
| `UuidSetVar { uuid, key, value }` and `UuidTransfer { uuid, destination, dialplan }` public fields | private; read them with `uuid()`, `key()`, `value()`, `destination()` and `dialplan()` |
| `UuidHold { uuid, off: bool }` public fields | private; `uuid()` and `action()`, a `UuidHoldAction` (`Hold`, `Unhold`, `Toggle`); `UuidHold::toggle(uuid)` sends `uuid_hold toggle`, `UuidHold::new(uuid, action)` picks at run time |
| `AppCommand::set_var(name, value)` returns `EslCommand` | returns `Result<EslCommand, UndeliverableVariable>` |
| `UuidSetVar::new` and `AppCommand::set_var` accept any value | refuse a control character other than tab in a value set under a name mod_sofia sends as an outgoing SIP header (`sip_h_`, `sip_rh_`, `sip_ph_`, `sip_bye_h_`), as `Variables` already did |
| `UuidSetVar`'s `Display` wrote the value as given, so a caller quoted it (`quote_for_uuid_setvar`) | it quotes and escapes the value itself; pass the value the channel should hold, since a pre-quoted one now arrives with its quotes |
| `UuidTransfer::new(uuid, destination)` and `with_dialplan(dp)` return `UuidTransfer` | return `Result<UuidTransfer, TransferArgumentError>`, refusing a destination or dialplan the switch's blank split would not read back as itself (`Unsplittable`) and a destination opening `-`, which `uuid_transfer` reads as the leg to transfer (`LegSelector`); `TransferArgumentError` and `TransferField` now live in `freeswitch_types::commands`, still re-exported where they were |
| `DialString: fmt::Display` | `DialString` carries only `variables()`; render an endpoint through `Endpoint` |
| `AudioEndpoint`'s `Display` (it rendered `audio`), `fmt_with_prefix` | wrap it in `Endpoint::PortAudio`, `PulseAudio` or `Alsa` and render that |
| `AudioEndpoint::parse_with_prefix(s, prefix)` | `Endpoint::parse_for(s, target)`, which names the module from the text |
| `"…".parse::<SofiaContact>()`, `"…".parse::<GroupCall>()`, which always failed at the API carrier | `Endpoint::parse_for(s, DialStringTarget::new(DialStringCarrier::Dialplan, block_parse))` |
| `DialString::set_variables(vars)`, `variables_mut()` | removed from the trait; set a block with `with_variables(vars)`, which on `Endpoint` returns `Result<Endpoint, OriginateError>` and refuses one on an `ErrorEndpoint` with `OriginateError::VariablesNotSupported` where 2.x dropped it |
| `BridgeDialString::new(groups)` returns `BridgeDialString` | returns `Result<BridgeDialString, OriginateError>`, refusing what parse and config load refuse: more legs or groups than the switch splits off, and a group whose bracket spans legs (`BracketSpansLegs`) at any parser revision; config load runs the same check |
| `BridgeDialString::groups_mut()` | removed; build a new one with `new` |
| `BridgeDialString::with_variables(vars)` returns `BridgeDialString`, and it and config load take a block of any scope | returns `Result<BridgeDialString, OriginateError>`, refusing a `Channel` (`[]`) block with `ChannelScopeGlobalVariables`, since the switch reads it as the first leg's; an empty block is kept as `None`. Config load runs the same check |
| `BridgeDialString::variables_mut()` | removed; call `with_variables` on the value |
| public fields on `SofiaEndpoint`, `SofiaGateway`, `SofiaContact`, `LoopbackEndpoint`, `UserEndpoint`, `GroupCall`, `AudioEndpoint` and `ErrorEndpoint` | private; read them with the same-named methods (`profile()`, `destination()`, `gateway()`, `user()`, `domain()`, `extension()`, `context()`, `dialplan()`, `name()`, `group()`, `order()`, `cause()`, `variables()`), `Option<&str>` where the field was an `Option<String>` |
| `SofiaEndpoint::new`, `SofiaGateway::new`, `SofiaContact::new`, `LoopbackEndpoint::new`, `UserEndpoint::new`, `GroupCall::new` return the endpoint | return `Result<_, OriginateError>` |
| `with_profile`, `with_context`, `with_dialplan`, `with_domain`, `with_destination` on the endpoint structs return the endpoint | return `Result<_, OriginateError>`; `with_variables` and `GroupCall::with_order` stay infallible |
| endpoint constructors and `with_*` builders accept any field | refuse what parse and config load refuse, as `OriginateError::UndeliverableEndpointField` naming the endpoint, field and `EndpointFieldFault`; a field legal only beside its qualifier goes in through `SofiaGateway::on_profile`, `SofiaContact::on_profile` or `UserEndpoint::in_domain` |
| `Variables`, `Endpoint` and the endpoint structs' `Display` (`to_string()`, `format!("{vars}")`), which rendered for `DialStringCarrier::EslApi` | `render(target)` with a `DialStringTarget`; `Debug` stays for logs |
| `s.parse::<Variables>()`, `s.parse::<Endpoint>()`, `s.parse::<SofiaEndpoint>()` and the other endpoint structs' `FromStr` | `Variables::parse_for(s, target)`, `Endpoint::parse_for(s, target)`, `SofiaEndpoint::parse_for(s, target)` |
| `Variables::display_for(target)`, `Endpoint::display_for(target)` and their `VariablesDisplay`, `EndpointDisplay` adapters | `render(target)`, which returns the `String` |
| `Originate` and `BridgeDialString`'s `Display` (`cmd.to_string()`, `format!("{cmd}")`), which rendered at the default revision | `cmd.render(block_parse)`; the carrier is fixed by the type, so only the `BlockParse` is named: `BlockParse::for_version(&version)` or a named revision |
| `s.parse::<Originate>()`, `s.parse::<BridgeDialString>()` | `Originate::parse_for(s, block_parse)`, `BridgeDialString::parse_for(s, block_parse)` |
| `Originate::display_with(block_parse)`, `BridgeDialString::display_with(block_parse)` and their `OriginateDisplay`, `BridgeDialStringDisplay` adapters | `render(block_parse)`, which returns the `String` |
| `Originate::parse_with(s, block_parse)`, `BridgeDialString::parse_with(s, block_parse)` | `parse_for(s, block_parse)` |
| `AppCommand::bridge(destination: impl Display)` with a `BridgeDialString`, an `Endpoint` or a string | `AppCommand::bridge(&bridge, block_parse)` for a `BridgeDialString`; `AppCommand::bridge_raw(&s)` sends a string verbatim, such as `endpoint.render(DialStringTarget::new(DialStringCarrier::Dialplan, block_parse))` |
| `FlattenedDialString::display_for(target)` and its `FlattenedDialStringDisplay` adapter | `render(target)`, which returns the `String`; `display_raw()` stays and returns an `impl Display` |
| `BlockParse::default()`, `BlockParse: Default` | `BlockParse::for_version(&version)` for the release you target, or a named revision such as `BlockParse::PairSplitCleans` |
| `DialStringTarget::new(carrier)` | `DialStringTarget::new(carrier, block_parse)` |
| a bare `DialStringCarrier` passed where a `DialStringTarget` is taken (`render`, `parse_for`) | `DialStringTarget::new(carrier, block_parse)`; the `From<DialStringCarrier>` impl that filled in a revision is gone, and `render` and `parse_for` take a `DialStringTarget` rather than `impl Into<DialStringTarget>` |
| `Originate` builders `dialplan(dp)`, `context(s)`, `cid_name(s)`, `cid_num(s)`, `timeout(d)` | `with_dialplan(dp)`, `with_context(s)`, `with_cid_name(s)`, `with_cid_num(s)`, `with_timeout(d)` |
| `Originate` readers `dialplan_type()`, `context_str()`, `caller_id_name()`, `caller_id_number()`, `timeout_duration()` | `dialplan()`, `context()`, `cid_name()`, `cid_num()`, `timeout()` |
| `Originate::timeout_seconds()` | `timeout().map(\|t\| t.as_secs())` |
| `Originate::set_dialplan(dp)`, `set_context(v)`, `set_cid_name(v)`, `set_cid_num(v)`, `set_timeout(v)` on a `&mut Originate` | the consuming `with_*` builder to set, `without_dialplan()`, `without_context()`, `without_cid_name()`, `without_cid_num()`, `without_timeout()` to clear; clearing needs no `None::<String>` |
| `Originate::endpoint_mut()`, `target_mut()`, `Application::name_mut()`, `args_mut()` | removed; build a new `Originate` from the endpoint and target you want, reading the rest off the old one |
| `Originate::extension(ep, ext)`, `Originate::application(ep, app)` return `Originate` | return `Result<Originate, OriginateError>` |
| `with_context`, `with_cid_name`, `with_cid_num` (2.x `context`, `cid_name`, `cid_num`) return `Originate` | return `Result<Originate, OriginateError>`; `with_timeout` and the `without_*` builders stay infallible |
| the constructors and builders accept what config load refuses | refuse it with the same `OriginateError`: `undef` in any case as the target, context or caller id (`UndefPositional`), an extension opening `&` and more (`ExtensionReadsAsApplication`), an `&name(args)` application whose name carries a parenthesis or whose arguments carry `)` (`ParenthesisInApplication`; `Application::new` still accepts it for `Originate::inline`), an extension under `inline` or inline applications under another dialplan, and a `SofiaContact` or `GroupCall` endpoint (`UnexpandedExpression`) |

## Events and headers

| 2.x | 3.0 |
|---|---|
| `SipPassthroughHeader::is_array_header()` returns `bool`, `false` for a header the catalog does not register | returns `Option<bool>`: `None` for an unregistered `sip_i_` header, which arrives as an `ARRAY::` value whenever the message repeats it; `Some(false)` for every other prefix |
| `"JSON".parse::<EventFormat>()` and any other casing of `plain`, `json`, `xml` | refused; the token is parsed in its wire case only. Serde is unchanged: it reads the variant name (`"Json"`), as in 2.x |
| `EventSubscription::event_types_raw_mut()`, `custom_subclasses_mut()`, `filters_mut()` | removed, since a push through them skipped the wire checks; add through `with_event_raw`, `with_events_raw`, `with_custom_subclass`, `with_custom_subclasses`, `with_filter` or `with_filter_raw`, which refuse what would break the `event` or `filter` line |
| `EventSubscription` builders `event`, `events`, `event_raw`, `events_raw`, `custom_subclass`, `custom_subclasses`, `sofia_event`, `sofia_events`, `filter`, `filter_raw` | `with_event`, `with_events`, `with_event_raw`, `with_events_raw`, `with_custom_subclass`, `with_custom_subclasses`, `with_sofia_event`, `with_sofia_events`, `with_filter`, `with_filter_raw`; the config field names are unchanged |
| `EventSubscription` readers `event_types()`, `event_types_raw()`, `custom_subclass_list()` | `events()`, `events_raw()`, `custom_subclasses()`, now readers where `events` and `custom_subclasses` were builders |
| `EventSubscription::format_mut()`, `event_types_mut()` | removed; rebuild with `with_format` and `with_event` / `with_events` |
| `InvalidHeaderName` | `InvalidHeaderNameError` |
| `EslArray::new(items)` returns `EslArray`; `push(v)`, `unshift(v)` return `()` | return `Result<_, EslArrayError>`, refusing with `TooManyItems` past `MAX_ARRAY_ITEMS`, as `parse` does; deserializing an `EslArray` refuses the same |
| `HeaderLookup::sip_status_code()` returns `Result<Option<u16>, std::num::ParseIntError>`; `ParseHeaderError::SipStatusCode` wraps that `ParseIntError` and is built by name | returns `Result<Option<u16>, ParseSipStatusCodeError>`, which keeps the rejected value; it converts into `ParseHeaderError` and `EslError` with `?` like every other accessor error |
| `EslArray::new`, `push`, `unshift` and deserialize accept an item holding `\|:` | refused with `EslArrayError::SeparatorInItem { index }`: the switch splits on every `\|:` with no escape, so the item would come back as two |
| `EslArray::new(vec![])`, `new(vec!["".into()])` and `EslArray::parse("ARRAY::")` succeed | refused with `EslArrayError::Empty`: both render a bare `ARRAY::`, which the switch drops on input and never writes; two or more empty items (`ARRAY::\|:`) are still accepted |
| `sip_status_code()` reads anything `u16::from_str` takes (`+200`, `0200`, `42`) | exactly three digits with a nonzero first one; anything else is `ParseSipStatusCodeError` with the raw value. 9xx stays legal: sofia-sip's nua reports its own failures as 9xx in `sofia::gateway_state` and `sofia::sip_user_state` |
| `EslEvent::push_header` / `unshift_header` onto a header holding `""` give `ARRAY::\|:v` / `ARRAY::v\|:` | set it to `v`: the switch deletes a header set to an empty value, so there is nothing to stack onto |

## SDP (`sdp` feature)

| 2.x | 3.0 |
|---|---|
| `SdpWarning::MalformedMediaSection`: a bad `a=rtpmap`, `a=fmtp` or RTP format emptied its section | removed; the section keeps everything else. An unreadable payload type in `a=rtpmap` or `a=fmtp` is `SdpWarning::PayloadAttributeIgnored { offset }`; an `a=rtpmap` with no readable rate, or an RTP format that is not a number below 128, is `SdpWarning::SwitchRejects` with `SwitchRejection::InvalidRtpmap` or `InvalidRtpFormat` |
| `SdpWarning::NonCanonicalDirectionAttribute` for `a=SENDONLY` and other casings | removed; a direction attribute is read in any case, as the switch reads it, and the last one in a section wins |
| `SdpCodecError::MalformedRtpmap`, `MalformedFmtp`, `NonNumericPayloadType` | removed; `parse` no longer fails on an attribute or a format, see the first row |
| `SdpCodecs::parse` read the session with sdp-types: it failed on a junk line followed by an attribute, an empty `m=` format list, a repeated session `o=` or `c=`, and accepted a missing `o=` or an `r=` before `t=` without a word | it reads the session as the switch's sofia-sip parser does. It fails only on input whose first line is not `v=0`, an `m=` port above 65535, or a non-UTF-8 `m=`/`a=` line; where the switch would refuse the offer, the parse goes on and reports `SdpWarning::SwitchRejects { reason, offset }`, and where the switch would stop reading (a line of at most one character after leading whitespace, or a NUL) it reports `SdpWarning::SwitchStopsReading { offset }`; parse `&sdp[..offset]` for the switch's view |
| a bare CR inside a line, or leading whitespace on one, was part of the line | a bare CR ends a line and leading spaces and tabs are stripped, as the switch reads it |
| `SdpCodecError::ParseFailure`'s source downcast to `sdp_types::ParserError` | the source is crate-private and names the fault with its byte offset in `Display`; sdp-types is no longer a dependency |
| `SdpCodecError::ParseFailure { message, source }`, `message` always `failed to parse SDP` | `ParseFailure { source }`; `Display` reads `SDP parse failure` and the fault is the source's, so match with `{ .. }` and print the chain |
| `ParseSdpDirectionError` with a private field | `ParseSdpDirectionError(pub String)`, the rejected attribute, as the other `Parse*Error` newtypes |
| a port-0 section carried the names, fmtp and direction its `a=` lines gave | by default it names only the static payload types of its format list and is inactive, as the switch reads it; `SdpCodecs::parse_with_options` with `SdpParseOptions::default().with_port_zero_attributes(true)` reads those lines as written |
| `"AUDIO".parse::<SdpMediaType>()` gave `Other("AUDIO")` | `Audio`: the media type matches in any case, as the switch reads it; `Display` stays lowercase |
| attribute names matched byte-exactly and the first direction attribute won | `rtpmap`, `fmtp`, `ptime` and the direction names match in any case, and the last direction attribute in a section wins |
| static payload types 1, 2 and 19 were unmapped | named `1016`, `G721` and `CN` (comfort noise, beside the non-codec payloads) from sofia's well-known table |
| an `m=` line repeating a payload type gave every repeat the type's `a=rtpmap` and `a=fmtp` | only the first entry takes them; a repeat keeps its static name or stays unmapped |
| an `a=rtpmap` channel count had to be a number with nothing after it | everything after the rate's `/` is read with `atoi`, as the switch reads it: `opus/48000/2 x` has 2 channels, `L16/8000/` has 0 |

## Client

| 2.x | 3.0 |
|---|---|
| `EslClient::subscribe_events_raw(format, events)` and `nixevent_raw(events)` return `EslResult<()>` | return `EslResult<SwallowedEventTypes>`, `#[must_use]`: the event types placed after `CUSTOM`, which the switch read as subclass names while answering `+OK`. `.await?.into_result()?` refuses them, `is_empty()` and `iter()` read them; an existing `.await?;` keeps compiling and warns |
| `EslCommand::FilterDelete { header: "all", .. }` and `filter_delete_raw("all", value)` send `filter delete all`, warning when a value is dropped | refused in any case with `EslError::FilterDeleteAllHeader`; send `EslCommand::FilterDeleteAll` or call `filter_delete_all()` |
| `EslClient::getvar(name)` returning `EslResult<String>`, the reply text verbatim | `getvar_raw(name)` |
| `EslClient::getvar_opt(name)` returning `EslResult<Option<String>>` | `getvar(name)`; an unset variable (`_undef_`, an empty reply, or the switch's `-ERR command not found` filler) is `None` |
| `AppCommand::transfer(extension, None, None)` | `AppCommand::transfer(extension)` |
| `AppCommand::transfer(extension, Some(dialplan), None)` | `AppCommand::transfer_to_dialplan(extension, dialplan)` |
| `AppCommand::transfer(extension, dialplan, Some(context))`, which wrote `XML` for a `None` dialplan | `AppCommand::transfer_to_context(extension, dialplan, context)`; the dialplan is named, since leaving it out lets the switch use the channel's own (or `force_transfer_dialplan`), which `XML` overrode |
| `EslError::JsonError(String)`, `XmlError(String)` | carry the decoder's error as a boxed `#[source]` (`Box<dyn Error + Send + Sync>`); `e.source()` reaches it; see [Log an `EslError` with its cause](#log-an-eslerror-with-its-cause) |
| `EslError::ReexecFailed { reason }` | `ReexecFailed { reason, source }`, `source` an `Option` of the boxed error behind a timeout or a lost reader; match with `{ reason, .. }` |
| `impl From<std::io::Error>`, `From<std::str::Utf8Error>` and `From<serde_json::Error>` for `EslError` | removed, so `?` on a foreign error no longer turns it into an `EslError`; build `EslError::Io(e)` by name, or keep your own error type |
| `impl futures_util::Stream for EslEventStream` | removed, so futures-util's major no longer ties this crate's; loop on `try_next()`, or build a stream with your own `futures`: `futures::stream::unfold(events, \|mut events\| async move { events.try_next().await.transpose().map(\|item\| (item, events)) })`, which yields an `Err` and carries on, as the impl did |
| `EslClient::connect_with_user(host, port, user, password)`, `connect_with_user_and_options(.., options)`, deprecated since 2.5 | `EslClient::connect_with_auth(host, port, AuthMethod::user(user, password), options)`, with `EslConnectOptions::default()` for the first |
| `EslCommand::Events { format: String, .. }`, `MyEvents { format: String, .. }` | `format: EventFormat` |
| `EslCommand::Log { level: String }`, `EslClient::log(level: impl AsRef<str>)` | `Log { level: LogLevel }`, `log(LogLevel)`; a number the switch also takes is one of these levels, and any past 7 receives what `LogLevel::Debug` does |
| `EslCommand::DivertEvents { on: bool }`, `EslClient::divert_events(bool)` | `DivertEvents { mode: DivertMode }`, `divert_events(DivertMode::On)` / `DivertMode::Off` |
| `EslClient::teardown_for_reexec()` returns `(RawFd, Vec<u8>)` | returns a `#[must_use]` `ReexecHandoff`; read `fd()` and `residual()`, and pass `handoff.residual()` to `adopt_stream` |
| a line break or NUL in a caller-supplied command field refused as `EslError::ProtocolError`, which `is_connection_error()` reports as a dead session | `EslError::WireTerminatorInField { field }`, naming the field and never the value; `is_connection_error()` and `is_recoverable()` are both `false`, and nothing is sent |
| `EslCommand::Filter` and `filter_raw` accept any header; `FilterDelete` and `filter_delete_raw` any header but `all` | refuse an empty header or one holding a space, which the switch would cut, and `Filter` also `add` or `delete` in any case, which it reads as a keyword: `EslError::UnsplittableFilterHeader { header, fault }` with a `FilterHeaderFault` |
| `AppCommand::transfer*` return `EslCommand` | return `Result<EslCommand, TransferArgumentError>`; it converts into `EslError::TransferArgument` with `?` |
| `AppCommand::transfer*` accept any extension and context | refuse one the switch's blank split would not read back as itself (a space, a `'`, an escaping `\`, a leading `^^X` on the extension, an empty field) with `TransferArgumentError::Unsplittable`, and an extension of `-bleg` or `-both` in any case, which transfers a leg instead, with `LegSelector` |
| `EslError::Utf8Error(Utf8Error)`, never built by the crate, which reported a non-UTF-8 message envelope or truncated auth reply as `ProtocolError` with the decoder error dropped | `Utf8Error { context, source }`, built for those two; `e.source()` is the `std::str::Utf8Error`, and `is_connection_error()` is `true`, as it was for the `ProtocolError` |
| `EslError`'s `Display` for `Io`, `JsonError`, `XmlError`, `Originate`, `HeaderParse` (and the new `TransferArgument`) appended the underlying error (`IO error: connection refused`) | states only its own message (`I/O error`); the cause is `e.source()`, so print the chain (anyhow's `{:#}`, or walk `source()`) where the detail is wanted. A reader disconnect's `DisconnectReason::ProtocolError` text still carries the whole chain |
| a `SendEvent` whose event has no event type and no `Event-Name` refused as `EslError::ProtocolError`, a connection error | `EslError::MissingField { field }`, neither a connection error nor recoverable; nothing is sent |
| logging an `EslError` with `{e}` showed its cause | `{e}` shows only the error's own message; `e.display_chain()` writes it and every source, joined by `: ` |
| `EventSubscriptionError(pub String)`, a tuple struct | a `#[non_exhaustive]` enum: `Token(String)` for what the tuple held, `FilterHeader { header, fault }` for the case below |
| `EventSubscription::filter_raw` and config load accept any filter header without a line break or NUL | also refuse an empty header, one holding a space, and `add` or `delete` in any case, as `EventSubscriptionError::FilterHeader` with the same `FilterHeaderFault` the client's `Filter` reports; a bad entry fails at load instead of at send. `FilterHeaderFault` now lives in `freeswitch_types::event`, with `of_filter` and `of_filter_delete` |
