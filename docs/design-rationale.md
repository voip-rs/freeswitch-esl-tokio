# Design Rationale

Why this library exists and the architectural decisions behind it.

## Split reader/writer architecture

Previous designs used a single handle that owned the TCP stream. Every method
took `&mut self`, making it impossible to send commands while receiving events.
The borrow checker enforced mutual exclusion: an event loop had to stop, send a
command, wait for the reply, then resume polling.

v1.0 splits the TCP stream and spawns a background reader task:

```
connect() → (EslClient, EslEventStream)

EslClient (Clone + Send)         EslEventStream
├ send commands from any task    ├ events via mpsc channel
├ writer half behind Arc<Mutex>  └ connection status via watch
└ replies via oneshot channel

Background reader task
├ owns the read half + parser
├ routes CommandReply/ApiResponse → pending oneshot
├ routes a reply with no command pending → event stream
├ routes Event → mpsc channel
├ tracks liveness (any TCP traffic resets timer)
└ broadcasts ConnectionStatus on disconnect
```

`EslClient` is `Clone` — pass it to multiple tasks. Commands are serialized
through the writer mutex (ESL is a sequential protocol). The reader task
determines event format from each message's `Content-Type` header rather than
storing state.

## Liveness detection

`set_liveness_timeout()` arms a threshold the reader trips
(`Disconnected(HeartbeatExpired)`) when no inbound traffic arrives in time. The
deliberate choice is that the library feeds this timer with nothing of its own:
it sends no keepalive, ping, or `noop`. An internal keepalive was considered and
rejected — it would put commands on the wire the caller never asked for, which
spams the FreeSWITCH command log on every interval and breaks the rule that the
caller owns and can account for every byte sent. Liveness therefore watches only
server-pushed traffic; on idle connections the caller supplies it, conventionally
by subscribing to `HEARTBEAT`. Time the reader spends waiting on queue capacity is
excluded from the threshold: the timer is for a peer that stopped sending.

That subscription can be denied — a permission-restricted user
(`esl-allowed-events` without `HEARTBEAT`) is rejected with `-ERR permission
denied`, so no heartbeats arrive and an enabled timer would trip on a healthy
idle socket. The denial is surfaced as recoverable data
(`EslError::is_permission_denied()`, a `CommandFailed`, not a connection error)
rather than worked around: the caller keeps the connection, warns, and declines
to enable idle-liveness for that user. Whether an idle restricted connection is
worth keeping is the caller's call to make, not the library's.

## Disconnection and reconnection

The library detects disconnection but never reconnects automatically. The caller
sees disconnection through:

- `events.recv()` returning `None` (channel closed)
- `events.status()` / `client.is_connected()` returning the `DisconnectReason`
- `client.api()` returning `Err(NotConnected)` after disconnect

Reconnection is the caller's responsibility. This keeps the library predictable
— the caller controls backoff strategy, re-subscription, and state recovery.

### Auto-reconnect and the ESL event model

Early on we considered adding auto-reconnect with backoff, the way most
client libraries do. The more we looked at it, the clearer it became that
ESL's event-driven nature makes transparent reconnection fundamentally
unsound — not just for this library, but for any ESL transport layer.

The core issue is that ESL is a stateful, event-driven protocol with no
replay capability. There are no sequence numbers, no gap detection, no way
to ask FreeSWITCH "what did I miss while I was gone." Events that fired
during the disconnect window are gone. A call tracker that missed a
CHANNEL_DESTROY now has a ghost channel in its map. One that missed a
CHANNEL_CREATE doesn't know a call exists. The transport layer can reconnect
the TCP socket and re-subscribe to events, but it cannot reconstruct the
application state that was lost — only the caller knows that it needs to
re-dump active channels via `show channels`, re-query registrations, or
reconcile its internal maps.

Then there are commands in flight. If bytes were written to the socket but
the reply never came back, did FreeSWITCH execute the command? An originate
might have succeeded — a call is now ringing with nobody tracking it. A
uuid_kill might have gone through and the channel is already gone. The
transport cannot make the right call here. Retry? Skip? That depends on
whether the command is idempotent and what application-level cleanup is
needed. Only the caller has that context.

`bgapi` makes it worse. Each bgapi command returns a Job-UUID immediately;
the actual result arrives later as a BACKGROUND_JOB event on the same
connection. After reconnection, the new connection has a new event stream.
Those BACKGROUND_JOB events for commands sent on the old connection will
never arrive. The transport is stuck choosing between fabricating an error
response (lying), blocking forever (leaking), or silently dropping the
pending job (losing data). None of these are acceptable.

We looked at how `cgrates/fsock` (the Go ESL library used by cgrates and
fsagent) handles this, and it confirms all three failure modes. Pending
bgapi channels block forever — the old channel map is abandoned when a new
`FSConn` is created, so callers waiting on `<-out` never receive a value.
In-flight `Send()` calls get `context.DeadlineExceeded` instead of a
disconnection error, so the caller cannot distinguish "command timed out
but connection is fine" from "connection died mid-command." Event handlers
get no notification of the reconnection at all — they just see a gap in
events with no way to detect it. On top of that, the reconnect handler
takes a read lock while performing write operations on the connection
pointer, creating a data race with concurrent readers.

The alternative we chose — return a connection error, let the caller
reconnect and rebuild state from a known-good starting point — is more
work for the caller but produces correct behavior. To keep the
timed-out/died-mid-command distinction honest, every reader-loop exit
fails the in-flight waiter: a command pending at disconnect returns
`ConnectionClosed` (`is_connection_error() == true`) immediately instead
of waiting out the command timeout the way fsock does. The `reconnecting_client`
example shows the pattern. In practice though, production ESL workloads
(call tracking, CDR generation, active call control) cannot tolerate an
event gap at all — the only scenario where the ESL connection drops without
FreeSWITCH also restarting is a bug or a network partition, and in both
cases the application state is already compromised. This is exactly the
problem that drove the re-exec mechanism (`teardown_for_reexec`): preserve
the authenticated TCP socket across binary upgrades so the event stream is
never interrupted, because reconnecting and rebuilding is not good enough
when you are the system of record for active calls.

## `show` output is not modelled

`show` answers from the switch's core database, whose schema this crate does not own,
and that store names its columns in a vocabulary used nowhere else in the switch: some
name an event header spelled differently, some a header only another event carries,
some nothing the typed names cover. Modelling the rows means carrying an approximate
translation table between the two vocabularies, hand-curated and silently wrong
wherever nobody checked — refused here for the reason a channel variable's SIP header
is curated exhaustively rather than derived from its name. A caller wanting rows parses
them; the channel state this crate speaks for is what the event stream and a channel
dump report. Which channels exist is the one question only that store answers, so the
listing is read for the set of live UUIDs and for no other field: a channel listed and
not yet dumped is a key with nothing behind it, which a consumer waits out rather than
seeds.

## Correct wire format

The ESL `text/event-plain` format uses two-part framing: an outer envelope
(`Content-Length` + `Content-Type`) followed by a body containing URL-encoded
event headers. Header values are percent-decoded on parse. This matches the real
FreeSWITCH wire protocol as implemented in `mod_event_socket.c` and consumed by
the C ESL library in `esl.c`.

## Typed state and header enums

FreeSWITCH is an entirely string-based system — channel state, call direction,
header names, and variable names are all plain strings on the wire. The C ESL
ecosystem and most client libraries preserve this. The problem: typos in header
names are silent, state comparisons are fragile string matches, and there's no
way to know at compile time whether `"Channal-State"` is a valid header.

Typed state enums (`ChannelState`, `CallState`, `AnswerState`, `CallDirection`),
`ChannelTimetable` for call lifecycle timestamps, and `EventHeader` /
`ChannelVariable` for header and variable name checking all implement
`FromStr`/`Display`. Typos surface at compile time; comparisons are exhaustive
match arms instead of fragile string equality.

### No silent failures on protocol data

Parsing channel state, timetable timestamps, and header names returns `Result`
or typed errors — never `.ok()` collapsing that hides parse failures.
`ParseTimetableError` carries the header name and the unparseable value for the
caller to diagnose with, the value as data rather than as message text. This
follows the crate's correctness-over-recovery
philosophy: if FreeSWITCH sends an unparseable timestamp, that's a signal, not
something to silently ignore.

### define_header_enum! macro

`EventHeader` and `ChannelVariable` are generated by a `define_header_enum!`
macro that produces `Display`, `FromStr` (case-insensitive), `as_str()`, and
`AsRef<str>` for each variant. Application-specific crates can use the same
macro to define their own header enums without depending on core types.

### A wire enum serializes as its wire name

Every enum with a wire spelling serializes as that spelling, the form a config
author and the switch write. It deserializes from the wire name in any ASCII case
and from the Rust variant name, the form earlier releases wrote, so stored data
keeps loading: exact wire name first, then the variant name, then the case-folded
wire name, so a variant name that equals another variant's wire name in some
casing still reads as its own variant. The rule lives once and is reached only
through the generating macros. `SipHeader` comes from sip-header-catalog and reads
wire names only.

## A carrier-dependent value renders only for a named target

Command builders in `commands/`, `app/`, and `variables/` produce and parse
strings with no dependency on `EslClient`, so they round-trip without a switch
and serve contexts beyond this library. A builder whose text is the same
wherever it is sent implements `Display` and `FromStr`. A value whose escaping
depends on the command carrying it implements neither: a variable block passes
through the switch's tokenizer a differing number of times depending on which
command took it, and each pass consumes one level of escaping, so a render that
cannot name its destination is right for one carrier and quietly wrong for the
rest. Such a value renders and parses only for a named target, and a type whose
carrier is fixed takes only what that carrier leaves open. Where the command
names its own argument separator, that split joins the target and is
escaped once, at the edge of the rendered argument. The
target includes the switch's parser revision, taken from a version the
application states rather than one read off the connection, and a version the
escaping was never measured against, a development build included, is refused
rather than assumed.

A special character is rendered so that it is still escaped entering the last
pass. That the last pass happens to keep a bare one is no contract: the final
cleanup keeps a lone quote only while no partner follows it in the same field,
so a block carrying one such value measures fine and a value carrying two loses
both.

Every parse reads text through a port of the switch's own passes for the named
target, never by inverting this crate's render, which only undoes what it
escaped itself. Each pass is modelled once, holding its escape, its inverse and
its port together, and builders only compose passes. The port models the
switch's buffer rather than its tokens, because a pass rewrites the text a later
pass reads, and it is held to the switch's C on every tracked tree, since a live
switch measures only the build it runs. Each leg of a switch-produced list keeps
its source text beside the typed view, so a caller can forward the list
unchanged, and a failure takes the scope the switch gives it: the whole list
where the originate aborts, one leg where only that leg fails.

### Serde on command builders for config-driven deployments

The serde derives on `Originate`, `Endpoint`, `Variables`, and
`BridgeDialString` exist because production callers need **config-driven
command construction**. A deployment's originate command — which gateway,
which SIP headers, which timeout — varies between environments and should
live in a YAML config file, not hardcoded in Rust.

With serde on the command builders, a callback service's whole originate
command, deployment-specific SIP headers included, is a YAML block with
`${placeholder}` template substitution:

```yaml
originate:
  command:
    endpoint:
      sofia:
        profile: internal
        destination: "${contact}"
        variables:
          sip_h_X-Incident-Id: "${incident_id}"
    applications:
    - name: park
```

This is the pattern: **the library provides typed builders with serde, the
caller deserializes from config and renders with `cmd.render(block_parse)` at
originate time, naming the switch's parser revision**.
No FreeSWITCH-specific knowledge is needed in the config layer.

## freeswitch-types as a separate, async-free crate

The domain types crate (`freeswitch-types`) has **zero async dependencies** —
no tokio, no futures. This split exists because the types are useful without
a network connection:

- CLI tools that parse and validate originate strings
- Config parsers that deserialize `Originate` from YAML
- Logging and debugging tools that format dial strings
- Other ESL transport implementations (sync, other runtimes)

Pulling in `freeswitch-esl-tokio` for types alone would force tokio as a
transitive dependency — unacceptable for a config parser or a static analysis
tool. The split keeps the dependency boundary clean: `freeswitch-types` is
pure data, `freeswitch-esl-tokio` is transport. Minor and patch releases
version independently, but a major is taken in lockstep under one number:
`freeswitch-esl-tokio` re-exports the types, so a break in them is a break in
it, and a shared major tells a user which pair belongs together.

## User strings reach the wire without a line break or NUL

ESL is a text protocol where `\n\n` ends a command, and the switch reads the
command as a C string, so a NUL ends it early and the rest is dropped without
an error. A user string carrying either runs a command the caller never wrote —
`api("status\n\nevent plain ALL")` subscribes to every event — or a truncated
one. `to_wire_format()` refuses CR, LF and NUL in every user-supplied field,
command-builder headers and bodies included. The check sits at the wire
boundary rather than at construction because builders render infallibly,
and the wire is the only place these bytes are dangerous.

A value that cannot be represented at all is refused earlier than that. An empty
variable value and one closing a bracket it never opened have no working
encoding — the switch discards the first without logging it and truncates the
block on the second — so both are rejected at every boundary that can fail,
config load included. A newline is safe as data and dangerous only on the wire;
these are impossible at any layer, and a silent drop is indistinguishable from
success for as long as nobody reads the channel back. Text the switch itself
produced is exempt: the switch accepts it, so such a pair is reported, not refused.

A variable the switch turns into an outgoing SIP header must hold a value a
header field can carry, so it also refuses every control character but tab at
each boundary that can fail. The switch copies the value into the message
verbatim, and its SIP stack ends a field only at a line break: the character
reaches the peer in a header that forbids it, or turns a registered header into
a parse error.

## Credentials and wire content in logs

ESL authentication sends passwords in cleartext over TCP, and the parsers here
read subscriber data off it. Three protections keep both out of logs:

1. **Manual `Debug` on `EslCommand`** — the derived `Debug` would print
   `Auth { password: "ClueCon" }` in any debug log. The manual impl redacts
   the password field.

2. **`redact_wire()` for wire logging** — debug-level wire logging uses
   `redact_wire()` which replaces the password in `auth` and `userauth`
   commands and strips the `\n\n` terminator for cleaner output.

3. **Errors name their input, never quote it** — a parse error's `Display`
   renders the shape of what it rejected: a field name, an entry index, a byte
   length. The bytes stay on a public field for a caller that decides to print
   them. Interpolated, they reach every line a consumer logs from `{e}`, around
   the redaction it applies to that same value on its own path. This binds
   warnings as much as errors, and errors a macro generates as much as ones
   written by hand. One exception: a codec string the caller wrote.

These exist because production ESL daemons run with debug logging enabled
during incident investigation. A sysadmin grepping logs should not find an ESL
password or a caller's number.

## Replies correlate by position, one waiter at a time

ESL carries no request IDs, so a reply belongs to a command only by its place
in the order. One waiter is installed at a time, and the writer lock is held
from installing it through its reply, never just through the write.

A command nobody awaits any more, timed out or dropped, stays owed: the next
command may be written while that reply is outstanding, and the reply is routed
by its place among the owed ones, never to the newer waiter. A command dropped
before it was fully written leaves the stream unframeable, so the next command
closes the connection rather than guess.

A command queue was rejected: the switch does not pipeline, so it would add
machinery without throughput.

## Every error is a reconnect, a retry, or a refusal

`EslError`'s two predicates sort every error into exactly one of three classes:

- `is_connection_error()`: there is no usable session, whether the socket or the
  framing broke or the way to a session timed out. Reconnect.
- `is_recoverable()`: the session is up and only this command failed. Retry it.
- Neither: a retry and a reconnect meet the same refusal, whether caller input
  refused before anything was sent, a credential refused at connect, or a value
  the switch sent that does not parse while the framing held. Fix the input or the
  configuration. A report about a session that outlives it, such as a reply with
  no command pending, sits here too.

A timeout on the way to a session is never the command timeout: that one is
recoverable, and it would tell a caller to retry on a session that never existed.
Likewise a malformed line is a framing fault only in the envelope; inside a
serialized body the message's own framing held, so the reader keeps going.

Credentials sit in the third class to prevent reconnect loops on permanent
configuration errors. The motivation: production ESL daemons were observed spinning in infinite reconnect loops on auth failure — retrying
every 500ms with exponential backoff to 30s, forever. The fix was a pattern:
auth failure exits with code 78 (`EX_CONFIG`), and systemd's
`RestartPreventExitStatus=78` keeps it down. Transient failures (connection
lost, timeout) exit with code 1, and systemd restarts normally.

The library's job is to classify the error accurately. The caller's job is to
decide what to do with it. This is why the library never reconnects
automatically — it can't know whether a failure is permanent or transient in
the caller's context.

## A refusal and a failure arrive on different halves of the same reply

A command the switch refuses answers with a reply text and no body; one it runs
and fails answers with a body and no reply text. Splitting those across two
accessors makes every caller pick one half and lose the other, so the body parser
reports the reply text's failure before reading the body. A reply text carrying
neither marker stays non-fatal there: its normal case is a returned value.

The response type is `#[must_use]`, because dropping it discards the only report
either half makes.

## Back-pressure on the event queue is opt-in and bounded

A full event queue drops the arriving event and counts it. A caller may instead have the
reader wait for capacity, supplying the budget it will stall for; past that budget the
drop path resumes. An unbounded wait is refused: the switch's send retries expire and
its write site does not check the result, so a stall outliving them truncates an event
on the wire and puts the next message's framing inside it.

Waiting is sound only where the connection issues no commands, which nothing here can
enforce. It loses the race with the re-exec stop signal and never runs during the drain,
dropping the event it held rather than spending the teardown's budget. Errors the reader
delivers on its way out never wait: the disconnect status publishes only once the loop
returns.

## Re-exec preserves the authenticated socket across binary upgrades

A production ESL daemon may keep a persistent TCP connection to
FreeSWITCH and track live channel state (active calls, channel variables,
timetables). A normal service restart loses the connection and all tracked
state. The state can be rebuilt from `show channels as json`, but events
during the reconnection gap are lost — missed hangups, missed creates, stale
channels in the tracking map.

The re-exec mechanism (`teardown_for_reexec()` + `adopt_stream()`) preserves
the TCP socket file descriptor across `exec()`, so the new binary image
inherits the already-authenticated, already-subscribed ESL connection. No
events are lost because the kernel TCP receive buffer holds data during the
brief exec window. That covers the socket and the parser's residual only —
anything already handed to the consumer's queue dies with the process image.

The drain protocol is the critical detail: the reader loop must stop at a
clean message boundary. ESL's two-part framing means that if the parser is
mid-body (headers consumed, waiting for body bytes), the residual would be
a partial body without headers — corrupt and unusable. The drain logic
continues reading until the parser returns to `WaitingForHeaders` state,
then returns the residual bytes for the new process to pre-seed its parser.

See [docs/reexec.md](docs/reexec.md) for the full API and drain protocol.

## Stop on protocol desync rather than absorb

NEventSocket (.NET) is the most mature high-level ESL client and absorbs
protocol errors to keep the stream alive: messages without Content-Type are
accepted silently, Content-Length is trusted as an allocation hint with no
upper bound, and parse exceptions are caught and discarded. In telephony a
desynced connection produces wrong call control decisions, so this library
makes the opposite trade. Content-Type is required on every message, message
and buffer sizes are capped (8 MB per message, 16 MB total), and parse errors
propagate via `EslResult` with `is_connection_error()` /
`is_recoverable()` so the caller can disconnect and reconnect from a
known-good state. The one carve-out — lossy decode of non-UTF-8 event *values* —
is the next section.

A reply with no command pending is not absorbed either, and it does not end the
connection. ESL carries no correlation IDs, so the reader cannot tell a switch
that answered unprompted, possibly a newer one, from one reply too many. The
whole reply reaches the event stream as an error that is neither a connection
error nor recoverable, and whether the session can still be trusted is the
caller's call.

## Lossy event-value decode is not a desync

A non-UTF-8 byte that appears only *after* percent-decoding a header value is no
framing break: FreeSWITCH percent-encodes serialized-event values, so the stream
is synchronized; the decoded bytes just aren't UTF-8 (a Latin-1 dialed string or
caller name, say). A production consumer took the hard `InvalidUtf8InHeader` as
non-recoverable, exited, and dragged a supervised sibling down with it — over one
stray byte. So such values now decode lossily (U+FFFD) by default while the
connection lives, and the affected keys plus their unparsed on-wire value ride
back as *data* — `EslEvent::lossy_values()` for events, `EslResponse::lossy_values()`
for command/connect replies — not a library log line or a collapsed `None`: the
signal is inspectable, and the caller owns the warning and the PII call on the
value. `strict_header_utf8` restores the old hard-fail; framing faults (missing
`Content-Type`, no colon, oversize) stay fatal. Both paths need this: the
outbound `connect` response is itself a serialized event
(`switch_event_serialize(..., SWITCH_TRUE)`), so its channel-data values are
percent-encoded and can carry the same non-UTF-8 bytes as an inbound event body —
decoding is required on both, and so is the lossy carve-out.

The same argument covers the message *body*: Content-Length frames it in bytes,
so non-UTF-8 there is no desync either — and unlike header values it is the
common case for raw payloads, which FreeSWITCH appends un-encoded (a `sendevent
NOTIFY` payload, a Latin-1 SMS body, an `api` response echoing them). The hard
`Invalid UTF-8 in body` fail killed a live fs_cli connection over one such
event. Bodies now decode lossily by default; the exact wire bytes ride back as
`raw_body` beside the substituted string, and its presence *is* the lossy
signal. Not `LossyValues`: that is a header-value type whose ASCII `raw_value`
would force percent-encoding a potentially multi-MB body — an O(n) pass and ~3×
blow-up — when `String::from_utf8`'s error already returns the original
allocation for free. `strict_header_utf8` restores the hard fail here too —
one strictness switch, not two.

## RFC 4575 conference-info XML namespace handling

RFC 4575 documents use the XML namespace `urn:ietf:params:xml:ns:conference-info`,
but producers choose their own prefix: Bell's BCF uses `confInfo:`, others use
`ci:`, and some declare it as the default namespace (no prefix). The element
names are identical in all cases — only the prefix varies.

quick-xml's serde deserializer matches element names literally, including any
prefix. A field annotated `#[serde(rename = "users")]` matches `<users>` but
not `<confInfo:users>`. The serde layer has no namespace awareness.

quick-xml does provide `NsReader` for namespace-aware event-based parsing, but
it cannot be combined with serde. Using it would mean writing a manual
event-driven parser for every RFC 4575 type — hundreds of lines of brittle code
that discards the entire value of serde derivation.

The chosen approach: **pre-process the XML to strip namespace prefixes** before
feeding it to `quick_xml::de::from_str()`. An internal `normalize` function
uses quick-xml's `Reader`/`Writer` event loop to rewrite element names
(`confInfo:users` → `users`), remove `xmlns` declarations, and preserve all
other attributes. This is correct because RFC 4575 uses a single namespace —
there are no competing prefixes to disambiguate.

The normalizer is an internal implementation detail behind `ConferenceInfo::from_xml()`.
Callers never see it. Serialization with `to_xml()` emits prefix-free XML,
which is valid RFC 4575 (using the default namespace).

## BgJobTracker: bgapi correlation as a data structure

Every production ESL daemon we've built, and the bgapi benchmark, contained
the same boilerplate: a `HashMap<String, Context>` mapping Job-UUID to application
state, a check for `BackgroundJob` type + Job-UUID match on every event,
removal from the map, and `parse_api_body()` on the body. The pattern was
identical each time, differing only in what context was attached (channel
UUID, send timestamp, call ID).

`BgJobTracker<C>` extracts this into a generic `HashMap` wrapper. The type
parameter `C` is caller-defined context attached at send time and returned
by `try_complete()` when the matching event arrives. The caller's match arm
in the event loop is the handler — the same code that would have lived in
the manual `BackgroundJob` branch, but without the UUID bookkeeping.

A callback-based dispatcher was considered but rejected: `Box<dyn FnOnce>`
handlers run inside `dispatch()` which holds `&mut self`, preventing the
handler from accessing the surrounding `&mut app_state` without
`Arc<Mutex<>>`. A future-based design (oneshot-backed handles) creates two
consumption paths for one result — the caller must both drive `dispatch()`
and `.await` handles elsewhere, hanging silently if they forget one side.
The context-return approach sidesteps both problems because the caller
already has `&mut app_state` at the `try_complete` call site.

`BgJobResult<'a>` borrows from the event rather than cloning, matching the
library's general pattern where `event.body()` and `event.job_uuid()`
return `Option<&str>`. The result is always consumed in the same event loop
iteration. The tracker lives in `freeswitch-esl-tokio` rather than
`freeswitch-types` because its `bgapi()` convenience method calls
`EslClient::bgapi()`.

## sip-header as a standalone crate

A substantial fraction of `freeswitch-types` was pure RFC SIP standard code
with zero FreeSWITCH coupling: the `SipHeaderAddr`
name-addr parser (RFC 3261), `UriInfo` (RFC 3261 §20.9), `HistoryInfo`
(RFC 7044), `SipGeolocation` (RFC 6442), the `SipHeaderLookup` trait, SIP
message header extraction, and the full RFC 4575 conference-info XML
parser. These modules had a strict "no FreeSWITCH references in sip_*
modules" policy and clean module boundaries, but they lived in a crate
named `freeswitch-types` — forcing anyone who needed RFC SIP types to
depend on FreeSWITCH-specific ESL infrastructure they had no use for.

The problem surfaced through `eido`, the NG9-1-1 (NENA-STA-024.1a) library.
`eido` depended on `freeswitch-types` solely for `UriInfo`, `UriInfoEntry`,
`SipGeolocation`, `SipGeolocationRef`, `SipHeaderAddr`, and `sip_uri` — all
RFC-standard SIP types. It used zero FreeSWITCH-specific types: no
`EslEventType`, no `ChannelState`, no `Originate`, no `HeaderLookup`.
Anyone building NG9-1-1 tooling against a non-FreeSWITCH BCF — or a SIP
Call-Info parser for a testing harness — had to pull in ESL event types,
channel state machines, and originate command builders they would never
use, from a crate whose name signalled "this is for FreeSWITCH users."

Those modules live in `sip-header` (MIT OR Apache-2.0, matching `sip-uri`).
`freeswitch-types` depends publicly on its stable catalog alone: the header
names, `define_header_enum!`, and the row trait and holders that carry header
text as received. A consumer brings its own sip-header, whose typed accessors
reach every store here through the catalog's row trait; this crate names no
value type and re-exports neither sip-header nor sip-uri. A feature
that only switched on part of sip-header goes with the re-export; the consumer
enables it on its own sip-header.

### The ARRAY encoding problem

The extraction was not a simple file move because of `EslArray`. FreeSWITCH
encodes multi-value SIP headers using a proprietary pipe-delimited format:
`ARRAY::value1|:value2|:value3`. Both `UriInfo::parse()` and
`HistoryInfo::parse()` had previously imported `EslArray` to detect and
split this format alongside standard RFC comma-separated values, and
`HistoryInfo::parse()` also stripped `[...]` bracket wrapping from
FreeSWITCH log output. Those were FreeSWITCH transport-encoding details
leaking into RFC-pure parsers.

The question was whether to inline the trivial ARRAY detection (three
lines of prefix check) inside `sip-header` itself — keeping `parse()`
transparently handling both formats — or make `parse()` strictly RFC-only
and push ESL-specific handling up into `freeswitch-types`. The pragmatic
argument for inlining: it's three lines, no dependency, and the
`SipHeaderLookup` trait's default methods call `parse()`; if `parse()` is
RFC-only, then `HashMap` users with ESL data get parse failures on ARRAY
values, and Rust's orphan rules prevent `freeswitch-types` from overriding
the `HashMap` impl that `sip-header` provides.

The purity argument won. Making `parse()` RFC-only forced the design to be
honest about the abstraction boundary: a `HashMap<String, String>` holding
ESL data is not the same thing as a `HashMap` holding standard SIP headers.
The difference is real — ESL values can be ARRAY-encoded, bracket-wrapped,
and carry `variable_` prefixed keys. Papering over that inside `sip-header`
would hide a transport distinction that callers need to reason about.

## Unified SIP passthrough headers over per-prefix enums

FreeSWITCH exposes SIP headers as channel variables through six prefixes:
`sip_i_` (incoming INVITE), `sip_h_` (outgoing request), `sip_rh_`
(outgoing response), `sip_ph_` (provisional response), `sip_bye_h_`
(BYE), and `sip_nobye_h_` (suppress on BYE). The first implementation
only covered `sip_i_*` via a fixed `SipInviteHeader` enum with ~32
variants. The other five prefixes had no typed support — users had to
pass raw strings like `"sip_h_Call-Info"` to `variable_str()`.

This was both incomplete and the wrong shape. A fixed enum cannot
cover the open-ended nature of SIP headers: `X-*` custom headers,
extension RFCs, and vendor-specific headers all pass through the same
`sip_h_*` mechanism. Meanwhile the typed `SipHeader` in sip-header's
catalog crate already enumerates all IANA-registered headers.
Duplicating that catalog as `SipInviteHeader` variants created a
maintenance burden (keeping two lists in sync) with no extra value.

`SipPassthroughHeader` unifies all six prefixes into one struct that
pairs a `SipHeaderPrefix` with a header name. The header name comes
from either `SipHeader` (for known headers) or a raw string (for custom
headers, validated against `\n`/`\r` injection). The struct pre-computes
the wire variable name and implements `VariableName` for use with
`HeaderLookup::variable()`.

The key asymmetry: the `sip_i_` prefix uses a lossy wire format
(lowercase, hyphens replaced by underscores: `sip_i_call_info`), while
all other prefixes preserve the canonical SIP header casing
(`sip_h_Call-Info`). `FromStr` reverses the `sip_i_` transformation by
trying `SipHeader::from_str` on the re-hyphenated suffix. For unknown
headers the reversal is lossy — original casing is lost — but this is
inherent to FreeSWITCH's wire format, not a library limitation.

## A channel variable names the SIP header behind it, exhaustively

The mapping from a mod_sofia channel variable to the SIP header it came from is
curated here rather than derived from the variable's wire name: the wire spelling
is inconsistent about preserving the header's casing, and many variables hold a
field parsed out of a header rather than the header. A consumer cannot compute it,
so every consumer keeping its own table has a catch-all where a row is missing.

The match answering it takes no catch-all arm, so a new variable fails this crate's
build until someone classifies it. The absence is the point rather than an
oversight: adding the arm restores that same silent omission a layer down.

A variable holding a parsed-out field answers with the header it came from, marked
as derived. Answering nothing hands the table back to the caller for precisely the
variables carrying subscriber identity, and answering unmarked asserts the value is
the field-value, which a caller redacting or rewriting headers acts on. Where the
switch fills a variable from whichever of several headers carried the value, the
answer names those candidates rather than collapsing to nothing.

## Salvage rather than fail on truncated userauth

FreeSWITCH `mod_event_socket.c` truncates long `userauth` replies into a
512-byte buffer and drops the `\n\n` terminator (see the code comment on
`salvage_truncated_auth_response` in `connection.rs` for the C-side
mechanics). The auth itself is valid on the FreeSWITCH side — `LFLAG_AUTHED`
is set before the reply is sent — and the truncated `Allowed-API` /
`Allowed-LOG` headers are informational access-policy metadata, not
required for the session to function. So `authenticate()` salvages the
partial reply, validates `Content-Type: command/reply` to be sure it's
actually an auth response (not arbitrary parser-recovery), and logs a
WARN so operators can shrink their `esl-allowed-events` list or patch
FreeSWITCH. The salvage is private to the auth path; the rest of the
parser remains strict.

## Bounded ARRAY parsing

`EslArray::parse()` caps at `MAX_ARRAY_ITEMS = 4000` to match the only
engineered ceiling in FreeSWITCH itself (`switch_event_base_add_header()`
silently drops index-addressed writes above 4000) and to prevent
heap-amplification OOM from a crafted wire value — each `|:` separator
is two bytes on the wire but ~56 bytes of heap per resulting `String`.
The cap holds on every path that builds an array, deserialization and the
programmatic constructors included, so no array this crate holds renders a
value its own parser would refuse.

## EventSubscription as a reusable, serializable builder

Production ESL daemons don't subscribe to events once at startup and
forget — they re-subscribe after every reconnect, and deployments vary
which events matter, so they load their subscription from a config file: which event types, which CUSTOM subclasses, which header
filters, what wire format. An earlier design bolted each piece onto the
client separately (`client.subscribe_events(...)`, `client.filter(...)`,
`client.filter(...)` again, etc.); on reconnect, the caller had to
remember the exact sequence and redo each call in the right order.

`EventSubscription` captures the whole thing — format, typed events,
raw-named events, custom subclasses, filters — as one value. The client
has `apply_subscription(&sub)` which issues the right wire commands in
the right order, and `resubscribe_from(&old, &new)` which diffs the two
and sends additive subscribes before removing the gone entries, so no
desired event type is ever briefly unsubscribed during the transition.

The serde impl exists for the same reason as on the command builders:
YAML config is the source of truth for deployments. A `subscription:`
block deserializes directly into `EventSubscription`, and the same value
is passed to
`apply_subscription` on the initial connect and on every reconnect.
Deserialization validates newline injection, space injection, and
empty strings at the boundary — an invalid config fails at load time
rather than on the wire.

The `with_event_raw()` / `with_events_raw()` pair is the escape hatch for events
FreeSWITCH adds before we update [`EslEventType`](../freeswitch-types/src/event/event_type.rs).
Without it, a freshly-added upstream event would force callers to bypass
`EventSubscription` entirely and drop down to raw `subscribe_events_raw`,
defeating the whole config-driven story. Raw events appear on the wire
alongside typed events in the same `event` command, and
`resubscribe_from` diffs them the same way.

## The event token list is ordered by the serialiser, not the caller

Every path that puts an event-name list on the wire goes through
`order_event_tokens`; nothing builds one by joining names itself.

The raw-string pair is the exception it cannot cover: it holds one flat token
list with the event-type/subclass distinction already gone, so a check there
would guess at intent, and guess wrong on the subclass lists this crate builds
for `nixevent` itself. It stays permissive, and both raw methods return the
one detectable case as a `#[must_use]` `SwallowedEventTypes`, filled by
`swallowed_event_types`.

## HangupCause::from_sip_response mapping

FreeSWITCH's mod_sofia translates SIP response codes to Q.850 hangup
causes in `sofia_glue_sip_cause_to_freeswitch()`. Callers bridging
inbound SIP provisional/final responses into their own hangup-tracking
logic need the same mapping — writing it again by hand is error-prone
and drifts from the FS source. `HangupCause::from_sip_response(code)`
is a direct port of that C function's match block, one-for-one, with
no reinterpretation.

The return is `Option<Self>`: codes without an explicit mapping
return `None` rather than defaulting to `NormalClearing` or
`NormalUnspecified`. Absence is a signal (an unusual SIP code the
caller should probably log) not a cause to fabricate. This matches
the crate's correctness-over-recovery policy — the library won't
invent a cause that FreeSWITCH itself wouldn't produce.

The mapping is neither injective nor surjective: multiple SIP codes
collapse to the same cause (`401/402/403/407/603/608 → CallRejected`),
and some Q.850 causes have no SIP counterpart in FS's table. Both are
a property of the source mapping, not the port. If
`sofia_glue_sip_cause_to_freeswitch` gains a new branch upstream, the
fix here is one added match arm — the test suite
(`from_sip_response_*` tests) enumerates every current code so drift
from the C source is visible in diff.

## CHANNEL_STATE drives lifecycle tracking, not CHANNEL_CREATE/DESTROY

FreeSWITCH fires `CHANNEL_STATE(CS_INIT)` *before* `CHANNEL_CREATE` and
`CHANNEL_STATE(CS_DESTROY)` *after* `CHANNEL_DESTROY`, so the intuitive
`CREATE → DESTROY` window misses state at both ends. A state event bounds the
life: the first one sighted establishes the key, `CS_DESTROY` retires it. What
the key holds arrives only with a `CHANNEL_CREATE`, off the wire or rebuilt from
a dump. Full ordering notes live in `docs/guide/events.md` — they belong with
usage docs, not here.

## Which driver a channel belongs to is answered from its name

No channel variable and no caller-profile field answers it. Where a loopback leg resigns
by handing its remaining work to the real channel that continues the call, that channel
inherits the leg's variables and its caller profile, so both then describe a channel that
is gone — the resignation marker among them. The name a channel reports for itself is
what stays true, and the accessors here read only that.

The name is not authority, though. A SIP peer can set it, so it answers what kind of
channel this is and never whether the far side may be trusted; no authorization decision
may key on it. Mechanics are in [loopback-bowout.md](loopback-bowout.md).

## The codec-string grammar as a type

No API models FreeSWITCH's codec string; the switch's own parser is internal and
consumes the buffer it reads. Three properties of the grammar are worth
centralising: qualifiers may appear in any order, a trailing letter is all that
distinguishes a rate from a packetization, a bitrate or a channel count, and a
format-parameter value can contain the character that separates entries. A type
holds those rules for reading a string back, rewriting one, or validating deployed
config, and makes round-tripping testable without a switch. Validation happens at
construction, so formatting is infallible and no shortcut can emit a corrupt
string.

SDP conversion is the second direction. The switch already exposes the remote and
local SDP as channel variables, so an ESL consumer holds the offer verbatim and can
derive a codec list from it rather than guessing one. The mapping is a port of the
switch's own conversion, not a reading of the specifications, because a
reimplementation disagrees with the switch only in production; the deviations are
named in rustdoc at each site. Format parameters are not emitted for audio by
default: without a bridged partner they never reach a generated audio offer, so
emitting them passes every test and changes nothing on a live call.

Excluding something from the codec string is not licence to reduce it. The type is both
the string's source and the typed view of what the offer said, so payloads the switch
negotiates outside the string are retained whole — payload type, clock rate, format
parameters — rather than projected onto whatever the generator needed. The switch keeps
one of each per session, picked against the codec it negotiated, and discards their
format parameters unread; that selection has no input at this layer, so what is retained
is the offer's fidelity rather than the switch's, and a caller holding the negotiated
rate applies the switch's rule itself.

The same holds a level up: every `m=` line yields a section, whatever its port or media
type, so declined and held streams stay in the inventory. A section carries what the
switch read there. Reading what the peer wrote instead, which an analyzer of SDP
captured outside the switch needs, is an option of the same parse, and it reaches only
sections no codec string or format-parameter lookup can see, so neither view widens what
the switch negotiates.

## Dedup and default-stripping follow the switch's loop

Duplicate removal keeps the switch's pass shape, dropped entries included in the
comparison, over a cleaner set-based equivalent: the two agree only because the
key is a pure function of the entry, and the switch's shape is the one a future
edge case follows. It never removes the last entry bearing a name, because a
deployment's default packetization is configured on the switch where this crate
cannot see it, so a bare name must survive to take that value at match time.
Stripping qualifiers that equal the crate's default table is allowed only on an
entry that already matched a loaded implementation; the table is per name, so an
unmatched qualifier can start matching once stripped. The mechanics, the G.722
exemption among them, are in [codec-string-format.md](codec-string-format.md).

## Composition over policy

An early version intersected the peer's offer against a preference list and
reported what it dropped. That reads as the obvious operation and is the wrong
one: a regulated callback interface requires the offer to carry a fixed list,
reorderable but never reducible, and the intersection narrowed it until a
downstream leg shared no codec at all. The switch intersects too, but only when
answering an offer, where the answer semantics demand it — generating one has no
equivalent, so there was nothing to be faithful to. What ships instead is the
grammar's own operations, and the caller's ordering carries the policy, because
deduplication keeps whichever entry came first. What the switch can load stays an
argument rather than a guess: no interface exposes the loaded implementations, and
an entry naming an absent codec or an unavailable packetization is dropped without
a log, so the caller supplies what it knows and is told what that removed.

## Sofia's leniency, not the RFC's grammar

The SDP this crate reads is held to what the switch reads, which settles what
"malformed" may cost. Rejecting a field the switch
parsed contradicts a decision that has already been made, and the punishment lands
on codecs that are not at fault: a stray tab in one rtpmap discarded every codec in
that media section, so the emitted codec string disagreed with the negotiation it
was supposed to mirror. Attribute values are therefore tokenized the way sofia
tokenizes them, whitespace decided once while consuming a field rather than trimmed
back off at each one, because the per-field spelling is what let the encoding name
go unhandled.

Both layers are ours, and the session layer is a port of sofia's control flow at
the flags the switch passes: which bytes form a line, which section a line belongs
to, which lines are skipped, and where the parse refuses the offer or stops reading
it. Neither of those last two fails the parse: an offer the switch refused or read
only in part is still what the peer sent, and SDP analyzed outside the switch has
not been through it at all. The offer is parsed as far as extraction can go, and
the refusal or the stop rides with it as a warning.

Which sections carry payload types at all is sofia's call too. It fills a section's
rtpmap list, and reads `a=rtpmap` and `a=fmtp` into it, only for the transports it
recognises as RTP — an exact set of proto strings, not a pattern, so a name merely
containing `RTP` is not one of them. The switch's own extraction never looks at the
proto and does not have to, because a section it should ignore arrives empty. Reading
the format list for any other transport would manufacture a parse failure out of a
section written correctly, so the same set gates it here, for every media type alike.

## The test mock frames like the switch, not like the parser

The scripted ESL peer ships behind a feature so downstream tests drive the same peer
rather than fork one. Its framing is taken from mod_event_socket, never derived from this
crate's parser: a mock that agrees with the parser proves nothing about the wire. Header
values alone are escaped as a superset of the switch's, since the tracked trees disagree
on which bytes they escape and every decoder must accept the wider form.

