# Reading events

## Typed event accessors

`EslEvent` provides typed accessors that parse header values into enums instead of returning raw strings:

```rust
use freeswitch_esl_tokio::{ChannelState, HeaderLookup};
# fn demo(event: &impl HeaderLookup) {

// Typed enums parsed from headers, no string matching needed
if let Ok(Some(state)) = event.channel_state() {
    match state {
        ChannelState::CsExecute => println!("Executing app"),
        ChannelState::CsHangup => println!("Hanging up"),
        _ => {}
    }
}

// String accessors return Option<&str>: None if the header is absent
let cid = event.caller_id_number();     // Option<&str>
// Typed accessors return Result<Option<T>, _> -- each accessor has its own parse-error type
let direction = event.call_direction(); // Result<Option<CallDirection>, _>
let cause = event.hangup_cause();       // Result<Option<HangupCause>, _>
# let _ = (cid, direction, cause);
# }
# fn main() {}
```

A header value that isn't valid UTF-8 after percent-decoding (e.g. a Latin-1 byte in a dialed string or caller name) is decoded lossily (U+FFFD) by default rather than failing. The affected keys, with their unparsed on-wire value, are exposed as data on `event.lossy_values()` (events) and `response.lossy_values()` (command/`connect` replies, whose channel data FreeSWITCH percent-encodes) for the caller to log or recover -- the library never logs them itself. A hard `InvalidUtf8InHeader` error instead is opt-in with `EslConnectOptions::with_strict_header_utf8(true)`.

Upstream FreeSWITCH serializes a plain event, a channel dump and a `connect` reply through `switch_url_encode`, which copies a `%` opening a valid `%XX` through unescaped, so such a value arrives decoded: a header holding `a%41b` reads `aAb`. Nothing on the wire tells the two apart. Subscribe with `EventFormat::Json` where a value can carry one; the 1.10.13 fork encodes every `%` and delivers it intact in either format.

## Channel timetable

Call lifecycle timestamps via `ChannelTimetable`:

```rust
use freeswitch_esl_tokio::{HeaderLookup, ChannelTimetable};
# fn demo(event: &impl HeaderLookup) -> Result<(), Box<dyn std::error::Error>> {

// Extracts all Caller-*-Time headers from the event
let timetable = event.caller_timetable()?;

if let Some(tt) = timetable {
    // All fields are Option<i64> (microseconds since epoch):
    println!("Created: {:?}", tt.created);          // Caller-Channel-Created-Time
    println!("Answered: {:?}", tt.answered);        // Caller-Channel-Answered-Time
    println!("Hungup: {:?}", tt.hungup);            // Caller-Channel-Hangup-Time
    println!("Bridged: {:?}", tt.bridged);          // Caller-Channel-Bridged-Time
    println!("Progress: {:?}", tt.progress);        // Caller-Channel-Progress-Time
    println!("Progress media: {:?}", tt.progress_media); // Caller-Channel-Progress-Media-Time
    println!("Transferred: {:?}", tt.transferred);  // Caller-Channel-Transfer-Time
    println!("Hold accum: {:?}", tt.hold_accum);    // Caller-Channel-Hold-Accum
    // Also: profile_created, resurrected, last_hold
}

// Other-Leg timetable (bridged party):
let other = event.other_leg_timetable()?;
# let _ = other;
# Ok(())
# }
# fn main() {}
```

`ChannelTimetable::from_lookup` works the same way against any key-value store, not just `EslEvent` -- illustrative only below, since `headers` and `subscription_headers` stand for an arbitrary lookup and an arbitrary subscription-building collection:

```rust,ignore
use freeswitch_esl_tokio::{ChannelTimetable, TimetablePrefix};

// Works with any key-value store, not coupled to EslEvent:
let timetable = ChannelTimetable::from_lookup(
    TimetablePrefix::Caller,
    |key| headers.get(key).map(|v| v.as_str()),
)?;

// Custom prefix for dynamic headers (e.g. "Hunt-Channel-Created-Time"):
let hunt_tt = ChannelTimetable::from_lookup("Hunt", |key| headers.get(key))?;

// Build subscription filters using SUFFIXES constant:
let prefix = TimetablePrefix::Caller.as_str();
for suffix in ChannelTimetable::SUFFIXES {
    subscription_headers.insert(format!("{prefix}-{suffix}"));
}
```

## Header and variable enums

Compile-time header and variable name enums via `HeaderLookup`:

```rust
use freeswitch_esl_tokio::{HeaderLookup, EventHeader, ChannelVariable};
# fn demo(event: &impl HeaderLookup) {

// HeaderLookup trait provides typed enum lookups on EslEvent
let uid = event.header(EventHeader::UniqueId);             // Option<&str>
let codec = event.variable(ChannelVariable::ReadCodec);    // Option<&str>
# let _ = (uid, codec);
# }
# fn main() {}
```

## Custom channel tracker with `HeaderLookup`

The `HeaderLookup` trait lets any `HashMap<String, String>` wrapper share the same typed accessors as `EslEvent`. `HeaderLookup` requires the catalog's `SipHeaderRows` supertrait, which is also what sip-header's typed SIP accessors (`call_info()`, `p_asserted_identity()`, …) read, so implement three methods, get all typed accessors for free:

```rust
use std::collections::HashMap;
use freeswitch_esl_tokio::{HeaderLookup, RowError, SipHeaderRows};
use sip_header::SipHeaderLookup;

struct TrackedChannel {
    data: HashMap<String, String>,
}

impl SipHeaderRows for TrackedChannel {
    fn sip_header_rows_str<'a>(&'a self, name: &str) -> Result<Vec<&'a str>, RowError> {
        self.data.sip_header_rows_str(name)
    }
}

impl HeaderLookup for TrackedChannel {
    fn header_str(&self, name: &str) -> Option<&str> {
        self.data.get(name).map(|s| s.as_str())
    }
    fn variable_str(&self, name: &str) -> Option<&str> {
        self.data.get(&format!("variable_{}", name)).map(|s| s.as_str())
    }
}

// Now TrackedChannel has all the same typed accessors:
// ch.channel_state(), ch.call_direction(), ch.hangup_cause(),
// ch.caller_timetable(), ch.header(EventHeader::UniqueId), etc.
// and, through sip-header, the SIP ones:
let mut data = HashMap::new();
data.insert("Call-Info".to_string(), "<sip:pbx.example.com>;purpose=icon".to_string());
let ch = TrackedChannel { data };
assert_eq!(ch.call_info()?.unwrap().entries()[0].purpose(), Some("icon"));
# Ok::<(), sip_header::ParseError>(())
```

`cargo run --example channel_tracker` is a complete reference implementation using `HeaderLookup` for channel lifecycle monitoring.

## Variable parsers

```rust
use freeswitch_esl_tokio::variables::{EslArray, MultipartBody, SipPassthroughHeader};
use freeswitch_esl_tokio::HeaderLookup;
use freeswitch_esl_tokio::{EslHeaders, SipHeader};
use sip_header::SipHeaderLookup;
# use freeswitch_esl_tokio::commands::Variables;
# fn demo(event: &impl HeaderLookup, vars: &mut Variables, raw_multipart: &str) {

// ARRAY:: delimited values (used by FreeSWITCH for repeating SIP headers)
let arr = EslArray::parse("ARRAY::item1|:item2|:item3").unwrap();
assert_eq!(arr.items(), &["item1", "item2", "item3"]);

// SIP passthrough headers: typed access to sip_i_*, sip_h_*, sip_rh_*, etc.
// Reading incoming INVITE headers (requires parse-all-invite-headers on the sofia profile)
let pai = event.variable(SipPassthroughHeader::invite(SipHeader::PAssertedIdentity));
if let Some(raw) = pai {
    if let Ok(arr) = EslArray::parse(raw) {
        for identity in arr.items() {
            println!("P-Asserted-Identity: {}", identity);
        }
    }
    // Or typed through sip-header, which peels the same ARRAY encoding:
    if let Ok(field) = EslHeaders::decode_value(SipHeader::PAssertedIdentity, raw) {
        if let Ok(Some(identities)) = field.p_asserted_identity() {
            println!("{} identities", identities.len());
        }
    }
}

// Setting outgoing SIP headers via channel variables
vars.insert(SipPassthroughHeader::request(SipHeader::CallInfo), "<sip:example.com>;answer-after=0");

// SIP multipart body extraction
let body = MultipartBody::parse(raw_multipart).unwrap().unwrap();

// by_mime_type matches the stored Content-Type verbatim, parameters included.
let pidf = body.by_mime_type("application/pidf+xml");

// by_media_type/MultipartItem::media_type ignore parameters and case --
// reach for this pair unless the switch is known to emit one exact spelling.
let pidf = body.by_media_type("application/pidf+xml");
# let _ = pidf;
# }
# fn main() {}
```

Verified in [esl_array.rs](../../freeswitch-types/src/variables/esl_array.rs), [sip_passthrough.rs](../../freeswitch-types/src/variables/sip_passthrough.rs), and [sip_multipart.rs](../../freeswitch-types/src/variables/sip_multipart.rs).

## Channel event ordering

FreeSWITCH does not guarantee that `CHANNEL_CREATE` is the first event for a given UUID. The state machine fires `CHANNEL_STATE` (CS_INIT) *before* `CHANNEL_CREATE` because `set_running_state()` happens at the top of the loop iteration, while the `CHANNEL_CREATE` event fires inside the `CS_INIT` case block (`switch_core_state_machine.c`).

Similarly, `CHANNEL_DESTROY` is not the last event. `CHANNEL_STATE` with CS_DESTROY fires *after* `CHANNEL_DESTROY` because `switch_core_session_destroy_state()` is called after the destroy event (`switch_core_session.c`).

Per-channel creation order:

1. `CHANNEL_STATE` (CS_INIT)
2. `CHANNEL_CREATE`
3. `CHANNEL_ORIGINATE` (outbound only)

Per-channel teardown order:

1. `CHANNEL_HANGUP`
2. `CHANNEL_STATE` (CS_HANGUP)
3. `CHANNEL_HANGUP_COMPLETE`
4. `CHANNEL_STATE` (CS_REPORTING)
5. `CHANNEL_DESTROY`
6. `CHANNEL_STATE` (CS_DESTROY) — true final event

The two state headers do not report the same field. `switch_channel_event_set_basic_data()` in `switch_channel.c` fills `Channel-State` from the channel's `running_state` and `Channel-State-Number` from its `state`, and during teardown `state` leads. So `CHANNEL_DESTROY` carries `Channel-State: CS_REPORTING` while its `Channel-State-Number` already reads CS_DESTROY, and only the `CHANNEL_STATE` that follows reports `Channel-State: CS_DESTROY`. Read end-of-life from `Channel-State`, never from the number.

Events from different channels can interleave freely on the ESL wire. If you are tracking channel lifecycle, use `CHANNEL_STATE` (CS_INIT) as the start-of-life trigger and `CHANNEL_STATE` (CS_DESTROY) as end-of-life rather than relying on `CHANNEL_CREATE`/`CHANNEL_DESTROY`.

Start-of-life is two steps, though: `switch_channel_event_set_extended_data()` adds the `variable_*` block only for the event ids on its whitelist, and `CHANNEL_STATE` is not one of them (unless the switch runs with `verbose-events`, the channel carries `CF_VERBOSE_EVENTS`, or the event was given a `presence-data-cols` header). So CS_INIT names a channel without describing one, and `CHANNEL_CREATE` -- which is whitelisted, and fires after the endpoint's `on_init` chain -- is the first event carrying channel variables. `CHANNEL_DESTROY` is whitelisted too, so the final variable block arrives there rather than on the CS_DESTROY state event that ends the life.

## Event queue overflow

The reader hands events to `EslEventStream` over a bounded channel
(`EslConnectOptions::with_event_queue_size`). What happens when a consumer
falls behind and that channel fills is `with_event_overflow`:

```rust
use freeswitch_esl_tokio::{EslConnectOptions, EventOverflow};
use std::time::Duration;

let options = EslConnectOptions::new()
    .with_event_queue_size(5000)
    .with_event_overflow(EventOverflow::BlockFor(Duration::from_secs(10)));
```

`DropIncoming`, the default, discards the arriving event, counts it in
`dropped_event_count()`, and arms one `EslError::QueueFull` item that the
consumer sees on the stream. That item rides the *next* successful dispatch,
so a burst ending with the queue still full reports nothing in band — read the
counter, not the marker.

`BlockFor` instead parks the reader until capacity, up to the budget, and only
then drops. `event_stall_count()` and `event_stall_duration()` report the
waiting; read both, since one long stall and many short ones share a total and
mean different things. The count rises when the wait starts, so a consumer can
tell it is stalled right now.

Two things bound where `BlockFor` is usable. The reader serves command replies
from the same loop, and the switch stops reading our socket while its write to
us is stalled, so a command issued during a stall fails and its late reply is
discarded as stale — use it only on a connection that issues none, remembering
that even an events-only connection issues commands when it subscribes and
re-subscribes. And the budget belongs well short of the switch's own send-retry
window: past that the switch abandons a half-written event and the stream
desyncs, which is worse than a counted loss.
