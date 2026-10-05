# The live test switch

Every `tests/live_*.rs` file runs against a real FreeSWITCH. The tests are
`#[ignore]`d so a normal `cargo test` skips them:

```sh
ESL_PORT=<port> cargo test --release --test 'live_*' -- --ignored
```

Everything below is what those tests assume. A missing piece shows up as one
test failing for a reason that has nothing to do with the library, so check
here first when a live test fails in isolation.

This file stays generic. What one machine runs — which FreeSWITCH builds, on
which ports, managed how — belongs in the untracked `CLAUDE.local.md` beside it,
not here. Running the suite against more than one build (an upstream tree beside
a vendor fork) is worth doing, since the two can differ on what the tests
measure.

## Connection

| | |
|---|---|
| Listener | `ESL_HOST` and `ESL_PORT`, default `localhost:8022` |
| Password | `ESL_PASSWORD`, default `ClueCon` (the crate's `DEFAULT_ESL_PASSWORD`) |
| Concurrent connections | up to `MAX_CONCURRENT_CONNECTIONS` (5) |

The variables are the ones the examples read, parsed the same way: a malformed
`ESL_PORT` fails the run rather than dialling the default. The suite runs tests
in parallel against this one switch, so every connection
sees every other test's events. Tests must correlate on their own channel's
UUID; see [Writing a live test](#writing-a-live-test).

## Session admission rate

**This is the one that bites.** The suite raises `sessions-per-second` and
`max-sessions` to 1000 before the first test connects, and leaves them raised.
They are issued once per live test binary rather than once per run -- `fsctl sps`
and `fsctl max_sessions` are idempotent, so a switch shared across binaries just
gets the same values set more than once.

A loopback originate costs two sessions and the bowout pair costs four, so five
tests originating at once burst well past a stock rate limit — peaks of ~90/s
are normal. Over the limit, `switch_core_session_request_uuid()` returns NULL
(`src/switch_core_session.c`, "Throttle Error!"), mod_loopback turns that into
`SWITCH_CAUSE_DESTINATION_OUT_OF_ORDER`, and the test sees
`-ERR DESTINATION_OUT_OF_ORDER` from an originate that is perfectly valid.

The symptom is distinctive and misleading: a different test fails each run,
every one passes on its own, and nothing correlates with the code under test.
FreeSWITCH's own default is 30, which is not enough either. A `max-sessions` cap
fails the same way once concurrent sessions reach it, and the escaping matrices
hold enough at once to reach a small one.

It is raised at runtime rather than required in config so a fresh switch works
unconfigured. It is not restored: a parallel suite has no reliable
last-test-finished hook, and a half-restored throttle would reintroduce exactly
the flakiness this removes. If that matters on your switch, set
`sessions-per-second` in `autoload_configs/switch.conf.xml` and know that the
suite will raise it anyway.

The ESL user therefore needs `fsctl` — an `esl-allowed-api` that omits it makes
every live test fail at `connect()` with a clear message.

## Heartbeat interval, and why the suite takes ~40s

Two tests wait on FreeSWITCH's own `HEARTBEAT` event, which is the floor on
the suite's wall time — not CPU, and not test concurrency. Raising
`MAX_CONCURRENT_CONNECTIONS` buys nothing for that reason.

`event-heartbeat-interval` defaults to 20 seconds and is read from
`autoload_configs/switch.conf.xml` at startup only; there is no runtime API
for it, so changing it needs a restart. The scheduler re-reads the value on
every tick, so a lower value takes effect from the next heartbeat:

```xml
<param name="event-heartbeat-interval" value="5"/>
```

At 5 seconds the suite finishes in roughly a third of the time. Nothing
depends on the default, so this is optional.

## Dialplan

Context `test`, extension `9199`:

```xml
<extension name="echo_9199">
  <condition field="destination_number" expression="^9199$">
    <action application="answer"/>
    <action application="sched_hangup" data="+8"/>
    <action application="echo"/>
  </condition>
</extension>
```

Tests reach it as `loopback/9199/test` and as a bare `loopback/9199`. The
`answer` matters — bowout requires both legs answered — and `sched_hangup`
bounds anything a test fails to reap.

## Endpoints and modules

- **mod_loopback** — provides both the `loopback/` and `null/` endpoints. Every
  originate in the suite uses one or both. The bowout tests additionally depend
  on its default configuration: an absent `loopback.conf.xml` is fine, but
  `fire-bowout-on-bridge` must stay off (the default) because the tests key on
  channel variables rather than the `loopback::direct` event.
- **mod_commands** — `originate`, `uuid_kill`, `uuid_exists`, `uuid_getvar`,
  `uuid_setvar`, `hupall`, `fsctl`, `status`.
- **mod_dptools** — `park`, `echo`, `bridge`, `sched_hangup`, `socket`.

## Directory users

`live_connect_userauth_truncated_response` needs a `many-events@default` user
whose `esl-allowed-events` list is long enough to overflow mod_event_socket's
512-byte reply buffer, plus:

```xml
<param name="esl-allowed-api" value="show uuid_dump originate uuid_kill"/>
<param name="esl-allowed-log" value="false"/>
```

The test asserts the reply arrives truncated, so shortening the event list
breaks it. See [freeswitch-directory-users.md](freeswitch-directory-users.md).

## Outbound

`live_outbound_connect_response_preserves_underscored_case` binds an ephemeral
local port and originates with the `socket` application pointed back at it, so
FreeSWITCH must be able to reach `127.0.0.1` on an arbitrary high port.

## Directory groups for switch-produced dial strings

`live_flattened.rs` reads real `group_call` output, so the switch needs the
`flattened-probe` groups in domain `default`: one group per member shape (a
`^^` block, `{}`/`<>`/`[]` scopes with every `local_var_clobber` placement, a
`|` or `:_:` inside a member, an empty expansion, each `error/` cause spelling,
the escaping and quoting cases), a group mixing them, an empty group and one
whose pointer names no user. Their captured output is committed under
`freeswitch-types/tests/fixtures/flattened/`, whose README lists the
substitution applied to it.

Define the real users inside the first `<group>` and point at them from the
rest. A pointer resolves to the first `<user>` whose id matches, groups are
searched before the domain's `<users>`, and a pointer entry matches its own id:
with the real users only under `<users>`, every pointer resolves to itself and
`group_call` emits `user/<id>@<domain>` for all of them.

Registered members come from the `lab-lo` profile registering to itself:

- `multiple-registrations` set to `contact`, or the second registration replaces
  the first and a member never yields two legs. It takes a profile restart.
- Two gateways registering as the same directory user, each with its own name so
  their contacts differ, `realm` and `from-domain` equal to the directory domain.
- Their `extension` naming an extension in context `test` that answers and hangs
  up by itself. The context's catch-all bridges to a real gateway, and every
  INVITE a test sends to a registered contact lands there otherwise.

A self-registering IPv6 twin of that profile never sends its REGISTER on the
measured build: `nua_register` reports an invalid handle and nothing leaves the
socket. Bracketed contacts therefore come only from the captured PBX output.

A gateway registering over IPv6 to a remote registrar has a trap of its own.
With an IPv6 literal as its `realm`, the digest challenge never matches:
`[addr]:port` was challenged for realm `[addr]`, and `[addr]` alone for the
registrar's domain, both ending in *Cannot locate any authentication
credentials*. The same gateway named by DNS registers, as does an IPv4 literal
with a port. Name an IPv6 registrar by DNS.

`sofia loglevel all 9` writes each digest's input, the gateway password
included, into the log. Set it back to 0 before sharing a capture.

## Escaping tests and the parser revision

The escaping tests in `live_channel.rs` render for the crate's default block-parser
revision. Set `FREESWITCH_BLOCK_PARSE` to a revision name (`pair_split_cleans`) to
measure a switch against another; run them after every switch upgrade, since a
switch that fails them parses blocks in a way no revision describes yet.

## Writing a live test

Rules learned from tests that passed, or failed, for the wrong reason:

**Correlate to your own channel.** Keep the UUID the originate returns and
filter every event against it. Matching on event type alone, on a channel-name
prefix, or on "the first event to arrive" will eventually assert against
another test's channel. For a loopback pair, both legs carry the partner's UUID
in `other_loopback_leg_uuid`, which ties either leg back to the originate
without a second API call.

**Count legs by how they end.** The session thread announces CHANNEL_CREATE from
`CS_INIT`, so a leg that loses a simultaneous ring before its thread runs that
state is hung up unannounced. Every leg reaches CHANNEL_HANGUP_COMPLETE, which
carries the channel's variables too; a test comparing every leg an originate
dialled reads that.

**Reap what you create, before you assert.** A panic between creating a channel
and killing it strands that channel for the rest of the run, and stranded
channels burn the session budget until later originates start failing. Collect
UUIDs as you learn them, kill them, then assert. Use `kill_channel()` (logs
rather than swallowing) for cleanup, and `channel_exists()` when a test needs
to *prove* a channel died — cleanup deliberately tolerates "already gone", so
it cannot tell you that.

**A bridge's outcome isn't safe to read before its wait is over.**
`switch_ivr_originate` writes `DIALSTATUS` and `originate_disposition` as a
placeholder (`INVALIDARGS`, `failure`) before it has even parsed the dial
string, and only overwrites either once the attempt actually settles. Reading
either to short-circuit a `wait_for_var` on `bridge_uuid` can catch that
placeholder and report a bridge that is about to succeed as already failed —
measured on `&bridge(...)` queued inline on an originate's own application
list, where the ESL reply can win the race against the application actually
starting. Polling `bridge_uuid` for the whole deadline and reading
`DIALSTATUS` only once that deadline is spent avoids it: a bridge that is
going to succeed does so in milliseconds, so nothing legitimate is still
sitting on the placeholder once a multi-second deadline has run out. A bridge
that never settles at all — the far leg's own auto-answer starved long enough
that FreeSWITCH's own default 60s origination timeout (`SWITCH_DEFAULT_TIMEOUT`
in `switch_types.h`) is what eventually gives up on it — may still read as the
placeholder at any deadline well short of that, so the diagnostic names a
cause only once the switch itself has one to give.

**Execute on an anchor only once its application runs.** `originate ... &park()`
replies after `originate_function` in `mod_commands.c` has only requested
CS_EXECUTE. A `bridge` sent in that window can be parsed while the transition
is still pending, where `switch_channel_test_ready` reports the caller not
ready, and the originate loop settles at once as `ORIGINATOR_CANCEL`
(`DIALSTATUS=CANCEL`). Under parallel load the window is wide enough to hit.
`parked_anchor()` returns only once `current_application` reads `park`.

## Checking state by hand

```sh
fs_cli -P <port> -x "show channels count"
fs_cli -P <port> -x "fsctl sps"
fs_cli -P <port> -x "status"     # sessions per Sec out of max
fs_cli -P <port> -x "hupall NORMAL_CLEARING"
```

A clean switch reports `0 total.` before and after a suite run. Anything left
behind is a test that failed to reap.

## Watching events and logs by hand

Use the crate's own examples rather than a hand-rolled socket. Both take
`-P <port>`, since they default to the standard 8021:

```sh
cargo run --example event_listener -- -P <port>
cargo run --example event_filter -- -P <port> -e CHANNEL_EXECUTE_COMPLETE -f Unique-ID -v <uuid> -c 1
```

`event_filter` filters client-side on any header, takes `/regex/` values, and
prints raw wire form with `-r` or one JSON object per event with `-j`. That
last pair is what to reach for when the question is "what exactly did this
event carry" — `-r` is the bytes, so nothing the library does can flatter or
hide them.

To wait for a call to finish, use `-U` rather than `-c`, and never poll
`show channels count` in a loop. A two-second `null/` call emits twenty-odd
events, so the count a `-c` needs is not knowable in advance, and a guess that
runs over hangs until something kills it.

```sh
cargo run --example event_filter -- -P <port> -e ALL \
  -f Unique-ID -v <uuid> -U Channel-State=CS_DESTROY -T 60
```

`-U` takes a bare event name or `Header=Value`, and the end of a call needs the
second form: `CHANNEL_DESTROY` fires *before* the final `CHANNEL_STATE`, which
is the one carrying CS_DESTROY and the settled hangup cause, so stopping on the
event name truncates the run one event early. `-T` bounds a run whose marker
never arrives. `-c N` remains the right tool when you want a fixed number of
events and do not care what follows.

For the switch's own log, `fs_cli --log-file -` streams it to stdout, and `-X`
runs a command as a background job so the process stays alive to keep
streaming; `--job-timeout <ms>` bounds that wait. A plain `-x` exits as soon as
the command returns, which cuts the log off before anything interesting
happens:

```sh
fs_cli -P <port> -l debug --log-file - \
  -X "originate null/probe &sleep(30000)" --job-timeout 35000
```

`-l` sets the level. Note that mod_logfile is not necessarily loaded, so
`global_getvar log_dir` can name a directory that does not exist — the ESL log
stream is the reliable source.
