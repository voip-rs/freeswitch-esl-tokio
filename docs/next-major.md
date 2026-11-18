# Deferred to the next major

Changes that are right but cannot ship under the current major. Read this when
bumping both crates to 3.0, and delete each entry as it lands. The crates take
a major in lockstep: `freeswitch-types` goes from 1.x straight to 3.0 so its
version names the `freeswitch-esl-tokio` major that re-exports it.

Every entry names a symbol. If the symbol is gone, the entry is stale — remove
it rather than guess what it meant. Nothing here is a promise; a decision may be
revisited when it is finally actionable.

## freeswitch-types 3.0

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
`expand originate …` is an API carrier on which an expression endpoint does
expand; `DialStringCarrier` does not model it.

### `Originate` setters need to refuse `undef`

`originate_function` reads every argument equal to `undef` as absent, so a
positional set to it arrives as unset, and a target set to it trips the switch's
assert on the missing extension. Parse and config load refuse both; `extension`,
`context`, `cid_name` and `cid_num` cannot until they return `Result`.

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
wire. `dialplan` and config load refuse both;
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
`cid_name`. Pick one field vocabulary; `Variables` has no `get_mut`, since a
value changes only through `insert`. The `set_*` methods take
`Option<impl Into<String>>`, so clearing a field needs a turbofish
(`set_context(None::<String>)`); the replacement takes a concrete `Option`
or offers `clear_*`.

### `EventSubscription::custom_subclass_list` should be `custom_subclasses`

Every other pair on the type is `x()`/`x_mut()`; this reader is the odd one.

### `InvalidHeaderName` should follow the `Parse*Error` / `*Error` naming

Its siblings `ParseSipPassthroughError` and `EslArrayError` set the two
conventions; this one follows neither.

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
