# Migrating from 1.x

2.x requires Rust 1.86. 1.x requires 1.71 with its dependencies resolved for that toolchain (`CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback cargo update`); the newest ones need 1.85.

## Originate

`Originate` and `Endpoint` no longer have public fields. Build them with constructors and chained setters:

```rust,ignore
// 1.x
let cmd = Originate {
    endpoint: Endpoint::SofiaGateway {
        uri: "18005551234".into(),
        profile: None,
        gateway: "my_provider".into(),
        variables: None,
    },
    applications: ApplicationList(vec![Application::new("park", None::<&str>)]),
    dialplan: None,
    context: None,
    cid_name: Some("Outbound Call".into()),
    cid_num: Some("5551234".into()),
    timeout: Some(30),
};

// 2.x
let cmd = Originate::application(
    Endpoint::SofiaGateway(SofiaGateway::new("my_provider", "18005551234")),
    Application::simple("park"),
)
.cid_name("Outbound Call")
.cid_num("5551234")
.timeout(Duration::from_secs(30));
```

- `ApplicationList` is gone. One application goes to `Originate::application()`, a list to `Originate::inline()`, which returns `Result` because an empty list or an argument an inline list cannot deliver is refused. `Originate::extension()` targets the dialplan.
- `timeout` takes a `Duration` rather than `u32` seconds.
- `Endpoint::SofiaGateway { uri, profile, gateway, .. }` becomes `SofiaGateway::new(gateway, uri)`, with `.with_profile(profile)` when set.
- `Endpoint::Loopback { uri, context, .. }` becomes `LoopbackEndpoint::new(uri).with_context(context)`.
- `Endpoint::Generic { uri, .. }` has no counterpart. Parse the string with `uri.parse::<Endpoint>()`, which yields the typed variant for every module listed in the [commands guide](guide/commands.md#endpoint-types). A dial string for any other module fails with `UnknownEndpointType` and cannot go through `Originate` or `BridgeDialString`; send it as a raw `bgapi` string.
- `Variables::vars_type` is private; read it with `scope()`.

Prefer the chained setters over the `set_*` methods on a built `Originate`.

## Header accessors

- Typed `HeaderLookup` accessors return `Result<Option<T>, _>`, so a header that is present but unparseable is an error rather than `None`. Some also changed type: `hangup_cause()` returns a `HangupCause`, not a `&str`.
- Propagate with `?`. A function reading several accessors can return `ParseHeaderError`; every accessor error converts into it except `sip_status_code`'s, which is wrapped by name.
- A custom `HeaderLookup` impl also implements `SipHeaderLookup` (one method, `sip_header_str`).
- `ChannelVariable` is unchanged. `VariableName` is a new trait it implements alongside the other variable enums.
- `EslArray::parse` returns `Result<EslArray, EslArrayError>` instead of `Option`.
- `MultipartBody::parse` returns `Result<Option<MultipartBody>, MultipartBodyError>`: `None` for a body that is not multipart, `Err` for a malformed one.

## Client

- `linger_timeout(Option<Duration>)` is gone; `linger(Option<Duration>)` replaces both 1.x forms.
- `connect_with_user` and `connect_with_user_and_options` are deprecated. Use `connect_with_auth(host, port, AuthMethod::user(user, password), options)`.
- Read events with `events.try_next().await?`. A `while let Some(Ok(event)) = events.recv().await` loop stops on a parse error with nothing to tell it apart from a disconnect.
- Read `getvar_opt()`, not `getvar()`. For an unset variable `getvar()` can return the switch's error text as if it were the value.

## Patterns that replace hand-written code

- `response.api_result()?` instead of reading `body()`: it turns both a refused command and one that ran and failed into `Err`. `check()` does the same when the body is not needed, and `EslError::command_failure()` exposes the text behind `-ERR`.
- `BgJobTracker` instead of a map of pending Job-UUIDs. See the [connecting guide](guide/connecting.md#background-api-calls).
- `EventSubscription` built once and applied with `apply_subscription()` on every connection, instead of repeating `subscribe_events` calls. It deserializes from config too; see the [config guide](guide/config.md#event-subscriptions).
- `is_terminal_channel_state()` instead of matching `CS_DESTROY` by hand. See [channel event ordering](guide/events.md#channel-event-ordering) for why `CHANNEL_DESTROY` is not the end.

## Upgrade steps

1. Replace `Originate` and `Endpoint` struct literals with constructors and chained setters.
2. Parse `Endpoint::Generic` strings into `Endpoint`, and move the dial strings that fail to raw `bgapi`.
3. Add `?` to typed header accessors and adjust for their new types.
4. Implement `SipHeaderLookup` beside each custom `HeaderLookup`.
5. Replace `linger_timeout`, `connect_with_user`, `recv` loops and `getvar` as listed above.

## freeswitch-types 3.0: SIP headers through your own sip-header

freeswitch-types depends publicly on [sip-header-catalog](https://docs.rs/sip-header-catalog) alone. SIP value types and parsers stay in [sip-header](https://docs.rs/sip-header) 0.4, which you add to your own dependencies; its typed accessors reach every store here through the catalog's row trait.

```toml
[dependencies]
sip-header = "0.4"
```

```rust,ignore
use sip_header::SipHeaderLookup;

let ci = event.call_info()?;          // Result<Option<UriInfo>, sip_header::ParseError>
let pai = event.p_asserted_identity()?; // Result<Option<SipHeaderAddrList>, _>
```

| 2.x | 3.0 |
|---|---|
| `freeswitch_types::sip_header`, `freeswitch_types::sip_uri` (and the same on freeswitch-esl-tokio) | your own `sip_header` and `sip_uri` dependencies |
| `SipHeaderAddr`, `UriInfo`, `UriInfoEntry`, `HistoryInfo`, `HistoryInfoEntry`, `SipGeolocation` at the crate root | `sip_header::{SipHeaderAddr, UriInfo, …}` |
| `UriInfoError`, `HistoryInfoError`, `ParseSipHeaderAddrError` | `sip_header::ParseError` |
| `HistoryInfoReason` | `sip_header::SipReason` |
| `SipGeolocationRef` | `sip_header::SipGeolocationEntry` |
| `extract_header` at the crate root | `sip_header::extract_header`, or `SipMessageHeaders` as a row store |
| `SipHeaderLookup` at the root and in the prelude | `sip_header::SipHeaderLookup` in scope (`use sip_header::SipHeaderLookup`); this crate's root and prelude carry the catalog's `SipHeaderRows` and `SipHeaderRowsExt` |
| `impl SipHeaderLookup for Store { fn sip_header_str(..) }` beside `HeaderLookup` | `impl SipHeaderRows for Store { fn sip_header_rows_str(..) -> Result<Vec<&str>, RowError> }`; a `HashMap<String, String>` field already implements it |
| `sip_header_str()` returns `Option<&str>` | `Result<Option<&str>, RowError>`, the first decoded row; the stored value is `HeaderLookup::header_str` |
| `EslHeaders::parse_uri_info(raw)`, `parse_history_info(raw)`, `split_entries(raw)` | `EslHeaders::decode_value(SipHeader::CallInfo, raw)?` gives a `SipHeaderField`; call `.call_info()`, `.history_info()`, … on it. `decode_value_raw(name, raw)` takes a header the catalog does not register |
| an over-cap `ARRAY::` value handed whole to the parser | `RowError` with `RowErrorKind::TooManyEntries`, through every accessor as `sip_header::ParseError::Row` |
| `SipPassthroughHeader::extract_from(&str) -> Vec<String>` | `extract_from(&store) -> Result<SipHeaderField, RowError>` over any row store, `sip_header::SipMessageHeaders::new(raw)` for a raw message |
| `SipHeader::is_multi_valued()` | `is_list()` (a comma list) or `may_repeat()` (may occur more than once) |
| `conference-info` feature | sip-header's own `conference-info` feature |
| `SipHeader` serialized as its variant name (`"CallId"`) | its wire name (`"Call-ID"`); deserialize accepts any spelling |
| `EventHeader` and the `*Variable` enums serialized as variant names (`"UniqueId"`) | their wire names (`"Unique-ID"`, `"sip_from_user"`); deserialize accepts any case |

- sip-header 0.4 parses through traits: `SipHeaderAddr::parse(s)` with `HeaderParse` in scope, never `s.parse::<SipHeaderAddr>()`.
- Every sip-header accessor returns `Result<Option<T>, ParseError>`; list and auth accessors that returned a bare `Vec` are wrapped too.
