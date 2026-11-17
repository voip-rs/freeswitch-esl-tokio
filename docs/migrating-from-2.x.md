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
