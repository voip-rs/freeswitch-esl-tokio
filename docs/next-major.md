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

### It inherits whatever `freeswitch-types` 3.0 breaks

The types above are re-exported from the crate root, so any change to their
public surface breaks this crate's too. There is no separate work item — the
bump is the work. Sequence the release accordingly: `freeswitch-types` 3.0
publishes first, then this crate.
