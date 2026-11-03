# Examples

Start with `inbound_client`, then `event_listener`. Between them they cover the
whole shape of an ESL client: authenticate, run a command and read its reply,
and process the stream FreeSWITCH pushes back.

All of them read ESL_HOST, ESL_PORT and ESL_PASSWORD, defaulting to
localhost:8021 with the stock password; ESL_HOST takes a bare IPv6 literal
without brackets. That reading lives in `common/env.rs`, and the connect
alongside it — including the message when nothing is listening — in
`common/mod.rs`, which each example includes with `mod common;`.

| Example | What it teaches | Needs |
|---|---|---|
| `inbound_client` | Connect, `api` and its two failure layers, `bgapi` correlated with `BgJobTracker` | a switch |
| `event_listener` | `EventSubscription`, per-call state off the event stream, `-d` wire dump | a switch |
| `event_filter` | Server-side `filter` (repeatable, any match passes), userauth, a bounded run, four output formats | a switch |
| `reconnecting_client` | Error classification as policy: backoff, `EX_CONFIG`, liveness gated on a heartbeat subscription that may be denied | a switch |
| `channel_tracker` | `HeaderLookup` on your own type; a channel is sighted before it is readable, and `uuid_dump` is how it becomes readable | a switch |
| `codec_monitor` | CODEC event headers, which are not channel variables; transcoding across a bridge | a switch with calls |
| `originate_examples` | Every endpoint type and targeting mode as a wire string (part 1 needs nothing), then a live call rendered for the parser revision `FREESWITCH_VERSION` or `ESL_BLOCK_PARSE` names | part 2 needs a switch |
| `originate_loopback_yaml` | An originate deserialized from YAML; variables reach both loopback legs | `mod_loopback`, ext 9199 in context `test` |
| `originate_loopback_bowout` | mod_loopback removing itself from a live call, and how to detect that rather than a teardown | `mod_loopback`, ext 9199 in context `test` |
| `originate_multipart_file` | A document on the INVITE via an `execute_on_originate` Lua loader, so it never crosses the dial string; `load_multipart.lua` is the loader | `mod_lua`, a SIP destination, paths the switch can read |
| `outbound_server` | Outbound call control: `myevents` scoping, `linger`, an IVR | a dialplan `socket` action |
| `outbound_test` | The outbound verbs in order, driving itself through a loopback call | `mod_loopback`, ext 9199 in context `test` |
| `sdp_codec_string` | SDP offer to codec string, and the two escaping paths to the wire | `--features sdp`, a switch |
| `bgapi_bench` | `bgapi` throughput and round-trip latency | a switch |
| `reexec_demo` | Handing the authenticated socket to a new binary image without dropping the event stream | Unix, a switch |

`docs/live-test-switch.md` describes what a switch has to provide for the ones
that place calls.
