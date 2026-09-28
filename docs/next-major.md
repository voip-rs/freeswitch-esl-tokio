# Deferred to the next major

Changes that are right but cannot ship under the current major. Read this when
bumping both crates to 3.0, and delete each entry as it lands. The crates take
a major in lockstep: `freeswitch-types` goes from 1.x straight to 3.0 so its
version names the `freeswitch-esl-tokio` major that re-exports it.

Every entry names a symbol. If the symbol is gone, the entry is stale — remove
it rather than guess what it meant. Nothing here is a promise; a decision may be
revisited when it is finally actionable.

## freeswitch-types 3.0

### `DialplanType` should carry any dialplan name

`originate_function` hands whatever word sits in the dialplan slot to the
transfer, so the type covers two of an open set. A variant holding the name
would make it lose `Copy`, a break; `Originate::dialplan_raw` and
`dialplan_name` carry the name beside it until then. Fold them into the variant
and drop the pair.

### `AudioEndpoint::fmt_with_prefix` should be private

It takes a `&mut fmt::Formatter`, which a caller can only obtain inside a
`Display` impl, and nothing outside the module calls it. It stays public only
because it shipped in 1.4.0; the carrier-aware `write_with_prefix` beside it is
already `pub(super)`. Make the public one private and drop the duplicate.

### `DialString: fmt::Display` forces an invalid endpoint string

The supertrait bound obliges `AudioEndpoint` to have a `Display` impl, and that
impl has no module prefix to render, so it emits `audio` — not a FreeSWITCH
endpoint. Its own rustdoc warns against calling it. Either drop the `Display`
bound from `DialString`, or drop the impl and let the three `Endpoint` variants
be the only way to render an audio endpoint.

### `Variables::insert` and `with_vars` need to be fallible

Some values cannot be represented in a bracket block at all: an empty value is
discarded by the switch under every encoding, and a value carrying an unbalanced
closing bracket truncates the block. A value carrying the block's own `^^`
separator is the third: `with_separator` checks the values present when it is
called, and an `insert` after it splits into a pair nobody wrote. A single quote
in a channel-scope value is the fourth: the switch pairs it with the next quote
before it parses the block, at any escaping depth. A value carrying `:_:` is the
fifth: any occurrence in a dial string sends the originate down the enterprise
split, whatever the quoting. A key is the sixth: an empty one is installed
nowhere, and one carrying `:_:`, an unbalanced bracket, a quote in channel
scope or the block's `^^` separator is split or paired before the switch
installs it. One carrying `[` is read as an array index and installed under the
text before it, and two differing only in case install as one.
A value carrying a control character but tab under a prefix mod_sofia sends
as an outgoing header is the seventh: it ships in a header field that cannot
hold it. `UuidSetVar::new` and `set_var` carry the same values with no
fallible step at all.
Refusing them is the only correct handling,
and it cannot live at render time — `Display` is infallible and `ToString`
panics on a `fmt::Error`, which would put a panic in a library. Until these
return `Result`, such a value is built silently and lost on the wire.

`Deserialize` and `FromStr` already return `Result`, so they can reject at the
boundary without waiting for the major — that covers config-driven construction
but not the programmatic path.

### Endpoint fields need to refuse what the module misreads

A sofia profile carrying `/`, `^` or `@` or reading `gateway`, a gateway or
gateway profile carrying `@`, a gateway key whose `::` reads at another place,
a loopback extension or context carrying `/`, an empty loopback context or
dialplan or either after an `app=` extension, a user name carrying `@` or empty
without a domain, an empty audio destination, a `sofia_contact` or `group_call`
field carrying its function's separator or what a pass reads ahead of the
expansion, an empty `sofia_contact` domain or profile, and `:_:` in any field
build and render, then reach the module or function as something else. Parse and config load refuse them
(`OriginateError::UndeliverableEndpointField`); the constructors, the `with_*`
builders and the public fields cannot until those fields sit behind fallible
setters. `BridgeDialString::new` and `groups_mut` accept a group whose bracket
spans legs, which parse and config load refuse as
`OriginateError::BracketSpansLegs`. `Originate::application`, `extension`,
`inline` and `endpoint_mut` accept a `SofiaContact` or `GroupCall`, which
`originate` over the API never expands; parse at the API carrier and
`Originate` config load refuse it as `OriginateError::UnexpandedExpression`.
`SofiaContact::from_str` and `GroupCall::from_str` parse at `EslApi`, where
that refusal makes them always return `Err`, and neither type has a public
`parse_for`: drop those `FromStr` impls, or give both types `parse_for(target)`.
`expand originate …` is an API carrier on which an expression endpoint does
expand; `DialStringCarrier` does not model it.

### `Originate` setters need to refuse `undef`

`originate_function` reads every argument equal to `undef` as absent, so a
positional set to it arrives as unset, and a target set to it trips the switch's
assert on the missing extension. Parse and config load refuse both; `extension`,
`context`, `cid_name`, `cid_num` and `dialplan_raw` cannot until they return
`Result`.

### `Originate::extension` and `application` need to refuse what `originate_function` misreads

`originate_function` runs any target opening `&` and more as an application,
and ends that application's arguments at the first `)`. An extension opening
`&`, and an application whose name carries a parenthesis or whose arguments
carry `)`, build and render, then run as something else. Parse and config load
refuse them; the constructors, `target_mut`, `Application::new` and
`Application::args_mut` cannot until they return `Result`.

### `Originate::set_dialplan` and `target_mut` need to refuse a mismatched dialplan

`originate_function` hands the target to the inline hunt only under the `inline`
dialplan, so an extension under `inline` and inline applications under any other
dialplan misfire: an inline action list set under `XML` is transferred as an
extension, no application runs, and the originate still answers `+OK` on the
wire. `dialplan`, `dialplan_raw` and config load refuse both;
`set_dialplan` and a target swapped through `target_mut` cannot until they
return `Result`.

### Consider removing `Display` for `Variables` and `Endpoint`

Both render for `DialStringCarrier::EslApi` at the default `BlockParse`, which
is right for what this crate mostly drives but silently wrong for a block
hand-spliced into a dialplan string or bound for a switch on another parser
revision. Removing the impls would force `display_for(target)` at every call
site, making a wrong target unrepresentable rather than merely unlikely.
`Originate` and `BridgeDialString` render with the default revision the same
way and belong in the same decision. The
cost is ergonomic and it breaks `format!("{vars}")` everywhere, so it is worth
weighing against how often the default is actually wrong in practice.


## freeswitch-esl-tokio 3.0

### `subscribe_events_raw` and `nixevent_raw` should return what they swallowed

Both send their token list verbatim, and `CUSTOM` is terminal, so an event type
placed after it is registered as a subclass name and never subscribed — with
`+OK` on the wire and nothing in the reply to say so. `swallowed_event_types`
detects that case today, but only for a caller who already knows to ask.

Returning it instead of `()`, behind a `#[must_use]` carrier, puts the warning
in front of a caller who does not: existing `.await?;` call sites keep
compiling and start warning. It waits for the major because changing the `Ok`
type is a break, not because the diagnostic is optional.

### It inherits whatever `freeswitch-types` 3.0 breaks

The types above are re-exported from the crate root, so any change to their
public surface breaks this crate's too. There is no separate work item — the
bump is the work. Sequence the release accordingly: `freeswitch-types` 3.0
publishes first, then this crate.

### `EslHeaders::parse_uri_info` and `parse_history_info` flatten the ARRAY error

Both map an `EslArrayError` into `UriInfoError::Malformed(String)` /
`HistoryInfoError::Malformed(String)`, so a caller cannot tell a cap breach
from a malformed URI without matching message text, and the source chain is
gone. The fix is an `EslHeaders`-owned error wrapping both, which changes the
return type.

### `SipPassthroughHeader::is_array_header` answers `false` for unknown headers

An unknown header name is neither single- nor multi-valued; `bool` cannot say
so, and a custom `X-*` header carrying a comma list is reported single-valued.
Return `Option<bool>`.

### `UuidHold.off: bool` should be `HoldAction`

`ConferenceHold` spells hold-versus-unhold with the `HoldAction` wire enum;
`UuidHold` spells it with a bare bool. Reuse the enum.

### `EventFormat::from_str` is case-insensitive on a wire token

The value goes on the wire in `event <format> ...`, so under the FromStr casing
rule it should be strict canonical like every other `wire_enum!`. Tightening
rejects input it accepts today.

### `Originate` uses three mutation vocabularies

`name_mut`/`args_mut`, five `set_*` for `Option` fields, and
`endpoint_mut`/`target_mut` on one builder; readers `dialplan_type`,
`context_str`, `caller_id_name` do not match builders `dialplan`, `context`,
`cid_name`. Pick `_mut()` pairs and one field vocabulary; add `Variables::get_mut`
with it. The `set_*` methods take `Option<impl Into<String>>`, so clearing a field
needs a turbofish (`set_context(None::<String>)`); the replacement takes a
concrete `Option` or offers `clear_*`.

### `EventSubscription::custom_subclass_list` should be `custom_subclasses`

Every other pair on the type is `x()`/`x_mut()`; this reader is the odd one.

### `InvalidHeaderName` should follow the `Parse*Error` / `*Error` naming

Its siblings `ParseSipPassthroughError` and `EslArrayError` set the two
conventions; this one follows neither.

### `parse_originate_target` should be `pub(crate)`

Public, re-exported nowhere, one caller inside `Originate::from_str`.

### `originate_split` should be `pub(crate)`

Public, re-exported nowhere, one caller inside `Originate::parse_with`. A caller
wanting the argument split already has it through `Originate`.


### `FilterDelete { header: "all" }` duplicates `FilterDeleteAll`

Both emit the same wire; the first warns at runtime that its `value` is
ignored. Reject `"all"` in `FilterDelete` or drop the special case.

### `getvar` should be `getvar_raw`

Its `Ok` may be the `-ERR`-shaped filler the switch returns for an unset
variable; `getvar_opt` is the honest accessor and wants the plain name.

### `AppCommand::transfer` takes two positional `Option`s

`transfer(extension, Option<DialplanType>, Option<&str>)` documents the
interaction of its options instead of typing it. An options struct or named
variants.

### `EslError::ReexecFailed`, `JsonError`, `XmlError` carry no source

Each stringifies the underlying error into a `String` field. Boxed `#[source]`
fields keep the dependency types out of the signature and restore the chain;
changing the field types is the break.

### `impl futures_util::Stream for EslEventStream` puts futures-util in the public API

A futures-util major becomes a semver break for this crate. Move behind a
feature, or wait for `Stream` in std.
