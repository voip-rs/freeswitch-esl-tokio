# Deferred to the next major

Changes that are right but cannot ship under the current major. Read this when
bumping both crates to 3.0, and delete each entry as it lands. The crates take
a major in lockstep: `freeswitch-types` goes from 1.x straight to 3.0 so its
version names the `freeswitch-esl-tokio` major that re-exports it.

Every entry names a symbol. If the symbol is gone, the entry is stale — remove
it rather than guess what it meant. Nothing here is a promise; a decision may be
revisited when it is finally actionable.

## freeswitch-types 3.0

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
