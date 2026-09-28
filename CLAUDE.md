## Project Type

This is a **library-first** crate. There is an examples/ folder buildable binaries
`Cargo.lock` is gitignored per Cargo convention for libraries.

## Reference Documentation

@docs/

Before changing an area, read the doc covering it — the whole file, not the
section that looks relevant. These carry wire-format and FreeSWITCH behaviour
detail that is in neither the source nor `design-rationale.md`, which records
only the decisions and deliberately leaves the mechanics here. Citing a line
range out of one you have half-read is how a reference goes stale.

## `freeswitch-types` Version Requirement

While the workspace is on a beta, the root `Cargo.toml` requires the full
three-component prerelease — a caret only resolves prereleases when it carries
one itself, and one that does floats to later betas of that version and to any
later stable in the major. Move the floor to the version being released each
time; never `=`-pin the exact beta.

## Evaluating Feature and Bug Requests

Read [docs/evaluating-requests.md](docs/evaluating-requests.md) in full before
answering a `FEATURE-REQUEST-*.md` or `BUG-*.md`. It carries the admission test
for whether a wire name belongs in this crate at all, how to verify a request's
evidence against `$FREESWITCH_SOURCE` rather than inherit it, and where the
answer gets recorded.

## Enum Variant Ordering — Append Only

New variants on public enums without `#[repr(...)]` **must be appended at
the end**. Inserting in the middle shifts implicit discriminant values,
which `cargo semver-checks` flags as a breaking change (callers using
`as isize` casts see different values). Grouping by category is fine
within a single commit that introduces the enum, but subsequent additions
always go at the tail.

## `#[non_exhaustive]` Policy

All public enums and public structs **with public fields** have
`#[non_exhaustive]`. Structs with all-private fields do **not** need
`#[non_exhaustive]` — privacy already prevents external construction and
destructuring, and omitting the attribute lets internal code benefit from
exhaustive compiler checks.

**Public fields split:** Types with invariants or builder APIs
(`ExecuteOptions`, `EslConnectOptions`, `Application`, `BridgeDialString`)
use private fields + accessor methods. Pure data/DTO structs with
`#[non_exhaustive]` and constructors (`SofiaGateway`, `SofiaEndpoint`,
`SofiaContact`, `LoopbackEndpoint`, `UserEndpoint`, `UriInfoEntry`,
`ConferenceInfo` and its children) keep public fields — no validation
constraints on individual fields.

**Exception:** single-field error newtypes (`pub struct ParseFooError(pub String)`)
are exempt. These will never grow additional fields, and adding `#[non_exhaustive]`
would break destructuring (`let ParseFooError(msg) = err`) for zero practical
semver benefit.

Because `#[non_exhaustive]` prevents struct literal construction from external
crates (including `examples/`), such a struct needs a constructor (`new()` or
named constructors) **only when something outside the crate has to build one** —
a test fixture, an example, or a downstream app with a legitimate reason to
construct the value. Types that only ever come out of a parser or a wire decode
need none; add one when a caller actually asks. Optional fields use builder
methods (`with_foo()`).

## SIP Modules Are Protocol-Agnostic

Modules under the `sip_*` namespace (`sip_header`, `sip_header_addr`,
`UriInfo`) are pure SIP standard types with no FreeSWITCH coupling.
Doc comments, module-level docs, and error messages in these modules must
not reference FreeSWITCH, mod_sofia, ESL, NOTIFY_IN, or any FS-specific
concepts. FreeSWITCH integration context belongs in `lookup.rs` (the
`HeaderLookup` trait methods) or `variables/` (e.g. `SipPassthroughHeader`
for `sip_h_*`/`sip_i_*` mappings).

## API Boundary Rules

- **Never expose dependency types in public signatures.** Return `impl Iterator`
  (not `indexmap::map::Iter`), wrap dependency errors (not `#[from] serde_json::Error`).
  A dependency major-version bump becomes a semver break if its types leak.
  **Exceptions:** `sip-uri` is an accepted public dependency of `freeswitch-types`
  (same author, narrow scope, stable). The `pub use sip_uri;` re-export and
  `SipHeaderAddr` returning `sip_uri::Uri` are intentional. `indexmap` is treated
  as a basic collection type (like `HashMap`) and may appear in public signatures.
- **`pub(crate)` modules can still leak types.** If a public function returns a
  type from a `pub(crate)` module, that type is visible but unnameable by callers.
  Either re-export the type or don't return it.
- **Struct fields that control behavior should be private.** Expose via accessor
  methods (e.g. `scope()` not `pub vars_type`). This prevents callers from
  mutating invariants after construction.
- **`constants` module is `pub(crate)`.** Only `DEFAULT_ESL_PORT` is re-exported.
  Internal protocol constants are implementation details.
- **Never `impl From<E>` for an error type this workspace does not own.**
  `From<std::num::ParseIntError>`, `From<std::str::Utf8Error>` and friends are
  global claims: `?` on *any* integer parse anywhere in a caller's function then
  produces our error, relabelled as a header or wire fault it never touched. It
  also burns the single `From<E>` slot, so the next accessor that fails the same
  way can never have its own variant. Give the variant a name and convert with
  `.map_err(Variant)` at the one call site. `ParseHeaderError::SipStatusCode` is
  the standing example. Wrap the foreign error in an owned newtype when a whole
  family of accessors starts needing it — planned for `sip_status_code` at the
  next major, since narrowing its return type is a break.

## Method Signature Conventions

- **`FromStr` casing rules.** Wire protocol types use **strict canonical case**.
  User-facing config types use `eq_ignore_ascii_case`. `Display` always emits
  the canonical form.
- **Typed + raw method pairs.** `filter()` / `filter_raw()`,
  `subscribe_events()` / `subscribe_events_raw()`. Always provide the `_raw`
  escape hatch for values not yet in the enum.
- **Options structs for optional wire headers.** Keep the base method simple;
  add `_with_options()` variant. Never grow parameter lists.
- **Preserve wire context in error/status enums.** Disconnect notices, auth
  responses carry useful context — never discard it.
- **Crate root re-exports: core, dptools, and wire state enums.** Command
  builders that belong to one module (conference, sofia gateway commands) stay
  in their submodules; sofia state enums and `SofiaChannelName` are core.

- **`_mut()` accessors on serde command builders.** Structs that are both
  serde-deserializable and command builders (e.g. `Originate`, endpoint types)
  must pair each read accessor for an owned field (`&T`, `&Enum`) with a
  `_mut()` variant. Callers deserialize from config then need to tweak fields.

Follow existing patterns in the codebase for `impl Into<String>`, `Duration`,
`impl IntoIterator<Item = impl Borrow<T>>`, `HeaderLookup`, `From<Concrete>`.

## FreeSWITCH Sources

The FreeSWITCH C source tree is at `$FREESWITCH_SOURCE`. If the env var
is not set, ask the user for the path. Use it to verify wire protocol
behavior, event header handling, and SIP/ESL internals. If tasking an
agent resolve the variable and give the value to the agent.

## Public Type Rename/Removal Checklist

When renaming, removing, or replacing a public type, update **all** of
these locations (not just the Rust source):

- `freeswitch-types/src/variables/mod.rs` — module declaration and re-exports
- `freeswitch-types/src/lib.rs` — crate root re-exports
- `src/lib.rs` (freeswitch-esl-tokio) — ESL crate re-exports
- `freeswitch-types/README.md` — module table and code examples
- `README.md` — badges, quick start, feature list
- `docs/guide/*.md` — code examples, file path references
- `CONTRIBUTING.md` — hooks list
- `CLAUDE.md` — any references to the old type name
- `docs/design-rationale.md` — update or add section explaining the change
- `examples/*.rs` — update imports and usage to the new typed API
- `hooks/*.sh` — update scripts that grep for the old type or file
- `tests/*.rs` — update any direct usage (raw strings in tests are fine)

Run `grep -r OldTypeName --include='*.{rs,md,toml,sh}'` to catch stragglers.

## Build & Test Workflow

The pre-commit hook is the gate: `Cargo.lock` kept off branches, fmt, clippy
over `--workspace --all-features --all-targets` (examples included),
`-D missing_docs` and broken intra-doc links, the full test suite with doctests,
and the enum/source-ref sync checks. The source-ref check needs
`FREESWITCH_SOURCE` exported and refuses the commit without it.
Do not re-run any of those separately — a failing hook rejects the commit and
prints the same thing.

Run before committing:

```sh
cargo clippy --workspace --all-features --fix --allow-dirty --message-format=short
cargo fmt --all
cargo check -p freeswitch-types --no-default-features --message-format=short
```

The first two fix rather than check. The third is the one build the hook never
does: it is `--all-features` throughout, so an item that should sit behind
`#[cfg(feature = "sdp")]` or `"conference-info"` and does not passes every hook
stage and breaks only for a consumer on the default feature set.

**When adding new `EslEventType` variants**, check whether they belong in any
of the event group constants (`CHANNEL_EVENTS`, `MEDIA_EVENTS`,
`PRESENCE_EVENTS`, `SYSTEM_EVENTS`, `CONFERENCE_EVENTS`) in
`freeswitch-types/src/event.rs` and update them accordingly.

When a live test switch is available, also run live tests:
`ESL_PORT=<port> cargo test --release --test 'live_*' -- --ignored`
(`ESL_HOST` / `ESL_PORT` / `ESL_PASSWORD`, default `localhost:8022`).

Live tests run in parallel against one shared switch, so a new one must
correlate every event to its own channel UUID and reap the channels it created
*before* asserting. See [docs/live-test-switch.md](docs/live-test-switch.md)
for that and for what the switch must provide (dialplan, modules, users).

To watch a switch by hand, use `event_listener` / `event_filter` and `fs_cli`,
never a hand-rolled socket — `docs/live-test-switch.md` covers the flags. Both
examples default to port 8021, so a live-switch run needs `-P <port>`; `fs_cli`
needs nothing but that port, since host and password are already its defaults.

## Documentation Style

All public items must have doc comments — the pre-commit hook enforces
`-D missing_docs`. Brief one-liners are fine for self-evident items.

No "captain obvious" docs. Don't restate the struct/function name as the doc comment.
Only document when it adds value: non-obvious behavior, FreeSWITCH-specific semantics,
wire format details, gotchas. Silence over noise. If the name and signature tell the
whole story, a brief one-liner suffices.

**No hardcoded counts in prose.** Don't write "26 variants" or "54 variables" in
markdown files or comments — these go stale when variants are added. Use dynamic
badges (CI-generated) in README or just omit the count.

**FreeSWITCH line references are pinned and indexed.** Prefer the symbol name
alone (`cleanup_separated_string` in `switch_utils.c`) — it survives edits and a
line number does not. Where a line number is genuinely needed (a branch, an
`else`-less block, a comparison inside a long function), the file that carries
it must name the pinned commit in its own header or module docs, every
reference must name its file, and the target must be indexed in
`hooks/source-refs.yaml`. Regenerate with `hooks/check-source-refs.py --update`
after bumping the pin, then re-verify each reference the diff names.

Never read a cited range out of `$FREESWITCH_SOURCE` — the checkout is not
parked on the pin, so the text at that line is some other commit's and the
reference reads as stale when it is correct. Use
`hooks/check-source-refs.py --show switch_core_media.c:13650-13651`, which
prints from the pinned blob. Only the FreeSWITCH tree is indexed: sofia-sip is
a separate repository, so its references are symbol-only (`sdp_media_has_rtp`
in `sdp_parse.c`) and carry no line number to verify.

**design-rationale is rationale, not changelog.** Version bumps, MSRV changes,
dep updates, and plain feature adds belong in the release tag's ChangeLog.
Only genuine design decisions (new layering, reversed trade-off, new invariant)
get a rationale section.

## Logging and Credential Safety

Logged wire data must be accurate. Never use `.trim()` on wire content for
cosmetic reasons — it can eat meaningful whitespace. Strip only known protocol
suffixes by name (e.g. `strip_suffix(HEADER_TERMINATOR)`).

Types carrying secrets (passwords, auth tokens) need manual `Debug` impls
that redact sensitive fields. Wire logging uses `redact_wire()` to replace
passwords in `auth`/`userauth` commands. ESL sends passwords in cleartext
over TCP — debug logs in production must never expose them.

## Library Code Rules

**No `assert!`/`panic!`/`unwrap()` in library code** outside of tests. This is
a library crate — panics crash the caller's application. Return `Result` or
`Option` instead. The only exception is logic errors that truly cannot happen
(document why with a comment), and even then prefer `debug_assert!`.

**Wire security: validate user strings.** ESL is a text protocol where
`\n\n` terminates a command and a NUL truncates it. Any user-provided string
reaching the wire without validation can inject or cut ESL commands.
`to_wire_format()` validates all user-supplied fields and rejects `\n`, `\r`
and NUL. A value bound for an outgoing SIP header (the `sip_h_`-style
prefixes) also rejects every control character but tab. See
[docs/design-rationale.md](docs/design-rationale.md) for the full story.

**Never autonomously send commands to the server.** The library puts a command
on the wire only as the direct result of an explicit caller action. No internal
keepalives, liveness pings, polls, or background `noop`/`api` traffic. The caller
owns what hits the socket — every command is theirs to account for in the
FreeSWITCH logs and the protocol's serial reply correlation. A convenience that
would require the library to emit an unrequested command is rejected by design;
surface the condition as data (a `Result`/status) and let the caller decide.

## Correctness Over Recovery

Correctness is the highest priority. Never silently absorb protocol violations
or leave the system in an unknown state to "recover." If an invariant is broken
(e.g. missing mandatory header, impossible framing), return an error and let
the caller disconnect. A clean reconnection from a known-good state is always
preferable to continuing with a potentially corrupt stream.

Concretely: never use `unwrap_or` / default values to paper over missing
mandatory protocol fields. If the ESL spec says a field must be present,
its absence is a hard error — not a recoverable condition.

Never use `.parse().ok()` to silently discard parse errors on protocol
data. If a header is present but its value doesn't parse, that's a
protocol violation — return `Err`, don't collapse it into `None` where
it becomes indistinguishable from a missing header.

Two carve-outs exist, both recorded in
[docs/design-rationale.md](docs/design-rationale.md): non-UTF-8 event values
and bodies decode lossily with the wire bytes carried as data, and a
`userauth` reply the switch truncated is salvaged on the auth path only, with
a warning. Neither is a framing fault, and framing faults stay fatal.

## Warnings Ride as Data, Never as a Flag in `Result`

Best-effort recovery (e.g. lossy UTF-8 decode) carries its signal as **data on
the result type**, never a positional flag in the `Ok` payload, never a library
log line. Thread a `&mut`-accumulator through parse helpers (`Option<&mut _>`
folds strict-vs-lenient into one param: `None` = hard-error, `Some` = record);
the `Ok` payload stays the pure value; warnings land on a named, `Display`-able
type that keeps the **unparsed on-wire source** for the caller to re-decode.
`LossyValues`/`LossyValue` on `EslEvent` is the reference example (cf. eido's
`Parsed<T>`). `Err` stays for hard/structural failures only.

## Design Principles

See [docs/design-rationale.md](docs/design-rationale.md) for the full
motivation and production lessons behind these decisions.

- **No coupling to `EslEvent`**: accept closures or `HeaderLookup` trait,
  not `&EslEvent`. Callers store headers in `HashMap`, not `EslEvent`.
- **Transport only**: `EslClient` sends strings, returns `EslResponse`.
  No response parsing. Future wrapper crate owns typed command-and-response.
- **Command builders are `Display`/`FromStr`**: no transport dependency.
  Round-trip testable without a FreeSWITCH connection.
- **No automatic reconnection**: the library classifies errors
  (`is_connection_error()` / `is_recoverable()`), the caller decides
  what to do. `is_recoverable()` returns `false` for
  `AuthenticationFailed`, which serves the reconnect-loop prevention
  purpose.

## Outbound ESL Mode

See [docs/outbound-esl-quirks.md](docs/outbound-esl-quirks.md) for details.

- `connect_session()` must be the first command after `accept_outbound()`
- `async full` mode required for api/bgapi/linger/event commands
- Socket app args need quoting in originate — `Originate` builder handles this
  via `originate_quote()`/`originate_unquote()` in `commands/mod.rs`; with
  `Originate::with_argv_separator` the `^^<sep>` line bypasses both and escapes
  each argument for that split instead
- `cargo run --example outbound_test` exercises outbound against a live switch named by `ESL_PORT`

## Examples — Write for the New User

Examples are the first thing a new user reads. Write them for someone who has
never used this library before.

- **Comment the "why", not the "what".** Explain non-obvious return types inline.
- **Explain unwrap() calls** — if safe, say why in a comment.
- Examples use `ESL_HOST`/`ESL_PORT`/`ESL_PASSWORD` env vars with defaults.
- **Keep examples in sync** — build all examples after public API changes.
- **Use the typed API, not C ESL string patterns.** Use `HeaderLookup` trait,
  typed accessors, `EslEventType::Display`, `response.into_result()`. Never
  raw `header_str()` for headers with accessors, never `starts_with("+OK")`.
  LLMs default to the C ESL string-based pattern — review for this anti-pattern.

## Development Methodology — TDD

This project follows test-driven development:

1. Write failing tests that reproduce the bug or specify the new behavior
2. Confirm tests fail (`cargo test --lib`)
3. `cargo fmt && git commit --no-verify` (red phase — clippy/tests will fail, but code must be formatted)
4. Implement the fix/feature
5. Confirm all tests pass
6. Commit the implementation (hooks run normally)

### Test failures reveal bugs, not inconveniences

When a test fails against real FreeSWITCH, **assume the library has a bug**
until proven otherwise. Never work around a test failure by removing the
triggering input (e.g. dropping a timeout value, switching to a simpler
endpoint). If the library produces a command that FreeSWITCH rejects, the
serialization is wrong — fix the serializer, not the test.

## Test Data

Tests and examples must use sanitized domains (`example.com`, `pbx.example.com`,
`10.0.0.1`, `192.0.2.x`) — never real production or internal domains.

## Dual Stack by Default

Every bind, connect and address default in this crate, its examples and its
tests must reach both families. Bind `[::]`, never `0.0.0.0`. Default a host to
`localhost`, never to a literal: a literal pins the example to one family, and
the shipped `event_socket.conf.xml` listens on `::`.

`EslClient::connect` takes host and port as separate arguments, so a bare IPv6
literal needs no brackets. Anything that reassembles `host:port` into one string
must bracket it, and anything that splits one apart must not assume a single
colon.
