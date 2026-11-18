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
| `Originate::dialplan_raw(name)` | `.with_dialplan(DialplanType::named(name)?)?`; `inline` and `xml` in any case resolve to the known two |
| `Originate::dialplan_name()` | `dialplan().map(DialplanType::name)`; `dialplan()` now returns a named dialplan too, where it returned `None` |
| `"name".parse::<DialplanType>()` refuses any name but `inline` and `xml` | accepts any dialplan name but `undef`, which `ParseDialplanTypeError` now reports alone |
| `Application::to_string_with_dialplan` | no public replacement; render the `Originate` |
| `Variables::insert(k, v)` returns `()` | returns `Result<(), OriginateError>`, refusing what parsing a block refuses |
| `Variables::with_vars(scope, pairs)` returns `Variables` | returns `Result<Variables, OriginateError>` |
| `Variables::set_scope(scope)` returns `()` | returns `Result<(), OriginateError>`, refusing a scope a held pair or the chosen separator cannot be written in |
| `Variables::iter_mut()`, `values_mut()` | removed; `insert` the new value under the same name, which keeps its position |
| a variable pair refused as `OriginateError::ParseError(String)` | `OriginateError::UndeliverableVariable { index, variable }`; `variable` is an `UndeliverableVariable` naming the `VariablePart` and the `VariableFault`, the same from parse, config load and the builder |
| `UuidSetVar::new(uuid, key, value)` returns `UuidSetVar` | returns `Result<UuidSetVar, UndeliverableVariable>` |
| `UuidSetVar { uuid, key, value }` and `UuidTransfer { uuid, destination, dialplan }` public fields | private; read them with `uuid()`, `key()`, `value()`, `destination()` and `dialplan()` |
| `UuidHold { uuid, off: bool }` public fields | private; `uuid()` and `action()`, a `UuidHoldAction` (`Hold`, `Unhold`, `Toggle`); `UuidHold::toggle(uuid)` sends `uuid_hold toggle`, `UuidHold::new(uuid, action)` picks at run time |
| `AppCommand::set_var(name, value)` returns `EslCommand` | returns `Result<EslCommand, UndeliverableVariable>` |
| `UuidSetVar::new` and `AppCommand::set_var` accept any value | refuse a control character other than tab in a value set under a name mod_sofia sends as an outgoing SIP header (`sip_h_`, `sip_rh_`, `sip_ph_`, `sip_bye_h_`), as `Variables` already did |
| `DialString: fmt::Display` | `DialString` carries only the variable accessors; render an endpoint through `Endpoint` |
| `AudioEndpoint`'s `Display` (it rendered `audio`), `fmt_with_prefix` | wrap it in `Endpoint::PortAudio`, `PulseAudio` or `Alsa` and render that |
| `AudioEndpoint::parse_with_prefix(s, prefix)` | `Endpoint::parse_for(s, target)`, which names the module from the text |
| `"…".parse::<SofiaContact>()`, `"…".parse::<GroupCall>()`, which always failed at the API carrier | `Endpoint::parse_for(s, DialStringTarget::new(DialStringCarrier::Dialplan, block_parse))` |
| `DialString::set_variables(vars)` returns `()` | returns `Result<(), OriginateError>`; an `ErrorEndpoint` refuses a block with `OriginateError::VariablesNotSupported` where it dropped it |
| `BridgeDialString::new(groups)` returns `BridgeDialString` | returns `Result<BridgeDialString, OriginateError>`, refusing what parse and config load refuse: more legs or groups than the switch splits off, and a group whose bracket spans legs (`BracketSpansLegs`) at any parser revision; config load runs the same check |
| `BridgeDialString::groups_mut()` | removed; build a new one with `new` |
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
| `AppCommand::bridge(bridge)` with a `BridgeDialString` or `Endpoint` | `AppCommand::bridge(bridge.render(block_parse))`, or `endpoint.render(DialStringTarget::new(DialStringCarrier::Dialplan, block_parse))` |
| `FlattenedDialString::display_for(target)` and its `FlattenedDialStringDisplay` adapter | `render(target)`, which returns the `String`; `display_raw()` stays and returns an `impl Display` |
| `BlockParse::default()`, `BlockParse: Default` | `BlockParse::for_version(&version)` for the release you target, or a named revision such as `BlockParse::PairSplitCleans` |
| `DialStringTarget::new(carrier)` | `DialStringTarget::new(carrier, block_parse)` |
| a bare `DialStringCarrier` passed where a `DialStringTarget` is taken (`render`, `parse_for`) | `DialStringTarget::new(carrier, block_parse)`; the `From<DialStringCarrier>` impl that filled in a revision is gone |
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
| `EventSubscription::event_types_raw_mut()`, `custom_subclasses_mut()`, `filters_mut()` | removed, since a push through them skipped the wire checks; add through `event_raw`, `events_raw`, `custom_subclass`, `custom_subclasses`, `filter` or `filter_raw`, which refuse what would break the `event` or `filter` line |
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
