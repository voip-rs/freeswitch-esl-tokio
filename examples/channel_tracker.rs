//! Channel state tracker -- reference example for ESL channel lifecycle monitoring.
//!
//! Demonstrates [`HeaderLookup`] -- the shared trait for typed header access from
//! any key-value store. `TrackedChannel` implements just two methods
//! (`header_str`, `variable_str`) and gets all typed accessors for free:
//! `channel_state()`, `call_state()`, `call_direction()`, `hangup_cause()`,
//! `timetable()`, etc.
//!
//! A channel is sighted before it is readable. A row in `show channels as
//! json`, or an event naming a channel this connection has not seen, gives up a
//! UUID and nothing else; the channel becomes readable at its CHANNEL_CREATE,
//! either the one on the wire or the one rebuilt from a `uuid_dump`. So
//! `uuids()` answers from the first moment and `get()` answers `None` until the
//! data lands.
//!
//! That listing is the only way to enumerate live channels, and this reads one
//! field out of it. Everything else comes from the dump, which is a serialized
//! event and needs no column-to-header translation.
//!
//! uuid_dump goes out over bgapi so it never blocks event processing -- results
//! arrive as BACKGROUND_JOB events matched by Job-UUID.
//!
//! Usage: RUST_LOG=info cargo run --example channel_tracker

use std::collections::HashMap;
use std::fmt::{Display, Write};
use std::time::{Duration, Instant};

mod common;

use freeswitch_esl_tokio::variables::SofiaVariable;
use freeswitch_esl_tokio::{
    parse_channel_dump, BgJobResult, BgJobTracker, CallState, ChannelState, EslClient, EslError,
    EslEvent, EslEventType, EventFormat, EventHeader, HeaderLookup, RowError, SipHeaderRows,
    VARIABLE_PREFIX,
};
use tracing::{debug, error, info, warn};

/// How long a dump may stay in flight before it is asked for again. A
/// BACKGROUND_JOB rides the connection that issued its bgapi, so a result lost
/// with that connection never arrives and never times out on its own.
const DUMP_DEADLINE: Duration = Duration::from_secs(30);

/// The previous key a CHANNEL_UUID event renames away from. Not an
/// [`EventHeader`] variant, so it is read by name.
const OLD_UNIQUE_ID: &str = "Old-Unique-ID";

/// First `n` characters of a wire value. Counted in characters, not bytes: a
/// value that decoded lossily carries replacement characters a byte slice splits.
fn truncate_chars(value: &str, n: usize) -> String {
    value
        .chars()
        .take(n)
        .collect()
}

fn short_uuid(uuid: &str) -> String {
    truncate_chars(uuid, 8)
}

fn display_or<T: Display, E: Display>(result: Result<Option<T>, E>) -> String {
    match result {
        Ok(Some(v)) => v.to_string(),
        Ok(None) => "-".into(),
        Err(e) => {
            warn!("parse error: {e}");
            "!ERR".into()
        }
    }
}

/// The live UUIDs, which is all this listing is read for. A row without that
/// field is a broken contract, not a row to skip past.
fn bootstrap_uuids(body: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let json: serde_json::Value = serde_json::from_str(body)?;
    let Some(rows) = json
        .get("rows")
        .and_then(|v| v.as_array())
    else {
        // An empty result carries a row count and no rows key at all.
        return Ok(Vec::new());
    };
    rows.iter()
        .map(|row| {
            row.get("uuid")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| "a listed channel carried no uuid".into())
        })
        .collect()
}

/// Whether this event carries the channel's data, or promises the event that
/// will: the state machine fires CS_INIT in the same iteration that goes on to
/// fire CHANNEL_CREATE.
fn describes_channel(event: &EslEvent, event_type: EslEventType) -> bool {
    match event_type {
        EslEventType::ChannelCreate | EslEventType::ChannelUuid => true,
        EslEventType::ChannelState => {
            matches!(event.channel_state(), Ok(Some(ChannelState::CsInit)))
        }
        _ => false,
    }
}

/// bgapi context for a `uuid_dump`: whose dump it is, and when it was asked for.
struct PendingDump {
    uuid: String,
    sent: Instant,
}

/// Flat data map -- all event headers and uuid_dump variables accumulated over
/// the channel's lifetime.
///
/// Implements [`HeaderLookup`] to get all typed accessors (`channel_state()`,
/// `call_direction()`, `hangup_cause()`, `timetable()`, etc.) from just two
/// methods. Use `header(EventHeader::X)` for known headers, `header_str("X")`
/// for arbitrary header names, and `variable_str("x")` for channel variables.
struct TrackedChannel {
    data: HashMap<String, String>,
}

// HeaderLookup requires the SIP row trait, which is what sip-header's typed
// accessors (`call_info()`, ...) read. The map already implements it.
impl SipHeaderRows for TrackedChannel {
    fn sip_header_rows_str<'a>(&'a self, name: &str) -> Result<Vec<&'a str>, RowError> {
        self.data
            .sip_header_rows_str(name)
    }
}

impl HeaderLookup for TrackedChannel {
    fn header_str(&self, name: &str) -> Option<&str> {
        self.data
            .get(name)
            .map(|s| s.as_str())
    }

    fn variable_str(&self, name: &str) -> Option<&str> {
        self.data
            .get(&format!("{VARIABLE_PREFIX}{name}"))
            .map(|s| s.as_str())
    }
}

impl TrackedChannel {
    fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    /// Merge all event headers into the data map.
    fn update_from_event(&mut self, event: &EslEvent) {
        for (key, value) in event.headers() {
            self.data
                .insert(key.clone(), value.clone());
        }
    }

    fn format_fields(&self) -> (String, String, String, &str, &str) {
        (
            display_or(self.channel_state()),
            display_or(self.call_state()),
            display_or(self.call_direction()),
            self.caller_id_number()
                .unwrap_or("-"),
            self.channel_name()
                .unwrap_or("-"),
        )
    }
}

/// Channels by UUID. A `None` entry is a sighting: a state event or a listing
/// row named the channel and its CHANNEL_CREATE, live or rebuilt from a dump,
/// has not arrived.
type Channels = HashMap<String, Option<TrackedChannel>>;

struct ChannelTracker {
    channels: Channels,
}

impl ChannelTracker {
    fn new() -> Self {
        Self {
            channels: HashMap::new(),
        }
    }

    /// Every channel key, readable or not. A consumer iterating channels has
    /// this from the moment the listing lands.
    fn uuids(&self) -> impl Iterator<Item = &str> {
        self.channels
            .keys()
            .map(|k| k.as_str())
    }

    /// The channel's data, or `None` while it is still awaiting its dump. A
    /// consumer wanting an attribute of a channel it just saw listed waits for
    /// that dump rather than reading a half-populated one.
    fn get(&self, uuid: &str) -> Option<&TrackedChannel> {
        self.channels
            .get(uuid)?
            .as_ref()
    }

    fn is_awaiting(&self, uuid: &str) -> bool {
        matches!(
            self.channels
                .get(uuid),
            Some(None)
        )
    }

    /// Record a UUID with nothing behind it yet.
    fn sight(&mut self, uuid: &str) {
        self.channels
            .entry(uuid.to_string())
            .or_default();
    }

    fn forget(&mut self, uuid: &str) {
        self.channels
            .remove(uuid);
    }

    /// The channel's data, promoting a sighting and creating a key that was
    /// never sighted. Merging rather than replacing: a CHANNEL_CREATE can
    /// arrive after events that already named the channel.
    fn tracked_mut(&mut self, uuid: &str) -> &mut TrackedChannel {
        self.channels
            .entry(uuid.to_string())
            .or_default()
            .get_or_insert_with(TrackedChannel::new)
    }

    /// Merge into a readable channel. An event for one still awaiting its dump
    /// is dropped: that dump is a snapshot of the channel as of the moment its
    /// job ran, so anything fired before it is already in there.
    fn update_channel(&mut self, uuid: &str, event: &EslEvent) {
        if let Some(Some(ch)) = self
            .channels
            .get_mut(uuid)
        {
            ch.update_from_event(event);
        }
    }

    /// Feed a `uuid_dump` result in as the CHANNEL_CREATE it stands for.
    ///
    /// Returns a UUID whose dump the caller should request, as
    /// [`process_event`](Self::process_event) does.
    fn apply_dump(&mut self, uuid: &str, result: &BgJobResult<'_>) -> Option<String> {
        if !self
            .channels
            .contains_key(uuid)
        {
            // Retired while its dump was in flight.
            return None;
        }

        let body = match result.parse_body() {
            Ok(body) => body,
            // A channel that hung up between the listing and its dump answers
            // `-ERR No such channel!`, which is routine, not a fault.
            Err(EslError::CommandFailed { reply_text }) => {
                debug!("uuid_dump {} answered {reply_text}", short_uuid(uuid));
                self.forget(uuid);
                return None;
            }
            Err(e) => {
                warn!("uuid_dump {} carried no result: {e}", short_uuid(uuid));
                self.forget(uuid);
                return None;
            }
        };

        let mut event = match parse_channel_dump(body) {
            Ok(event) => event,
            Err(e) => {
                warn!("uuid_dump {} did not parse: {e}", short_uuid(uuid));
                self.forget(uuid);
                return None;
            }
        };

        // Values that did not decode as UTF-8 are still merged (lossily); the
        // signal names which keys, so it must not be dropped silently.
        let lossy = event.lossy_values();
        if !lossy.is_empty() {
            warn!("uuid_dump {} decoded lossily: {lossy}", short_uuid(uuid));
        }

        if event.unique_id() != Some(uuid) {
            warn!(
                "uuid_dump {} answered for another channel",
                short_uuid(uuid)
            );
            self.forget(uuid);
            return None;
        }

        // A dump is a serialized CHANNEL_DATA event, so its keys went through
        // the same normalisation as the live events merged elsewhere and the
        // rename below is the whole translation a rebuild needs.
        event.set_header(
            EventHeader::EventName.as_str(),
            EslEventType::ChannelCreate.as_str(),
        );
        self.process_event(&event)
    }

    /// Returns a UUID whose dump the caller should request.
    fn process_event(&mut self, event: &EslEvent) -> Option<String> {
        let event_type = event.event_type()?;
        let uuid = event
            .unique_id()?
            .to_string();

        if event_type == EslEventType::ChannelState && self.retire_if_terminal(event, &uuid) {
            return None;
        }

        // A UUID this connection has never seen named. Where its own
        // CHANNEL_CREATE is still to come, that describes it; otherwise the
        // create is behind us and the listing missed it too -- the core writes
        // that row through a queue, so a channel is live before it is listed --
        // which leaves the dump.
        let mut dump = None;
        if !self
            .channels
            .contains_key(&uuid)
        {
            self.sight(&uuid);
            if !describes_channel(event, event_type) {
                dump = Some(uuid.clone());
            }
        }

        match event_type {
            EslEventType::ChannelCreate => {
                self.tracked_mut(&uuid)
                    .update_from_event(event);
                self.print_channel_event(&uuid, event_type);
            }
            EslEventType::ChannelUuid => {
                // The session was rehashed under a new key; nothing will ever
                // name the old one again.
                if let Some(old) = event.header_str(OLD_UNIQUE_ID) {
                    self.forget(old);
                }
                self.tracked_mut(&uuid)
                    .update_from_event(event);
                self.print_channel_event(&uuid, event_type);
            }
            EslEventType::ChannelDestroy => {
                // Not the end of life: CHANNEL_STATE(CS_DESTROY) follows it and
                // is the last event the channel ever sends. This one carries the
                // final variable block, so it is merged, not acted on.
                self.update_channel(&uuid, event);
                if let Some(ch) = self.get(&uuid) {
                    info!(
                        "{} {} cause={} name={}",
                        event_type,
                        short_uuid(&uuid),
                        display_or(ch.hangup_cause()),
                        ch.channel_name()
                            .unwrap_or("-"),
                    );
                }
            }
            EslEventType::ChannelHangup | EslEventType::ChannelHangupComplete => {
                self.update_channel(&uuid, event);
                let cause = self
                    .get(&uuid)
                    .map(|ch| display_or(ch.hangup_cause()))
                    .unwrap_or_else(|| "-".into());
                info!("{} {} cause={}", event_type, short_uuid(&uuid), cause);
            }
            EslEventType::ChannelUnbridge => {
                self.update_channel(&uuid, event);
                if let Some(Some(ch)) = self
                    .channels
                    .get_mut(&uuid)
                {
                    ch.data
                        .remove(EventHeader::OtherLegUniqueId.as_str());
                }
                self.print_channel_event(&uuid, event_type);
            }
            _ => {
                self.update_channel(&uuid, event);
                self.print_channel_event(&uuid, event_type);
            }
        }
        dump
    }

    /// Whether this state event is the channel's last. `CS_DESTROY` closes the
    /// life: it fires after CHANNEL_DESTROY, so retiring on the earlier one
    /// would drop the events between them.
    fn retire_if_terminal(&mut self, event: &EslEvent, uuid: &str) -> bool {
        let terminal = match event.channel_state() {
            Ok(Some(state)) => state.is_terminal(),
            Ok(None) => false,
            Err(e) => {
                warn!("{} carried an unreadable state: {e}", short_uuid(uuid));
                false
            }
        };
        if terminal {
            self.retire(uuid, EslEventType::ChannelState);
        }
        terminal
    }

    fn retire(&mut self, uuid: &str, event_type: EslEventType) {
        match self
            .channels
            .remove(uuid)
        {
            Some(Some(ch)) => info!(
                "{:<9} {} retired name={}",
                event_type,
                short_uuid(uuid),
                ch.channel_name()
                    .unwrap_or("-"),
            ),
            Some(None) => info!(
                "{:<9} {} retired, never dumped",
                event_type,
                short_uuid(uuid)
            ),
            None => info!("{:<9} {} (untracked)", event_type, short_uuid(uuid)),
        }
    }

    /// A channel awaiting its dump prints nothing: the line would be identical
    /// every time and carry no state, and the summary already lists it.
    fn print_channel_event(&self, uuid: &str, event_type: EslEventType) {
        match self
            .channels
            .get(uuid)
        {
            Some(Some(ch)) => {
                let (state, call_state, dir, cid, name) = ch.format_fields();
                info!(
                    "{:<9} {} state={} callstate={} dir={} cid={} name={}",
                    event_type,
                    short_uuid(uuid),
                    state,
                    call_state,
                    dir,
                    cid,
                    name,
                );
            }
            Some(None) => {}
            None => info!("{:<9} {} (untracked)", event_type, short_uuid(uuid)),
        }
    }

    fn print_summary(&self) {
        if self
            .channels
            .is_empty()
        {
            info!("--- No active channels ---");
            return;
        }
        info!(
            "--- {} active channel(s) ---",
            self.channels
                .len()
        );
        println!(
            "{:<36}  {:<14} {:<10} {:<8} {:<16} {:<16} NAME",
            "UUID", "STATE", "CALLSTATE", "DIR", "CID-NUM", "DEST",
        );
        let mut sorted: Vec<&str> = self
            .uuids()
            .collect();
        sorted.sort_unstable();
        for uuid in sorted {
            let Some(ch) = self.get(uuid) else {
                println!("{uuid:<36}  awaiting dump");
                continue;
            };
            let (state, call_state, dir, cid, name) = ch.format_fields();
            let dest = ch
                .destination_number()
                .unwrap_or("-");
            // `write!` into a String cannot fail, so the flags below discard it.
            let mut flags = String::new();
            if ch.call_state() == Ok(Some(CallState::Held)) {
                flags.push_str("[HELD]");
            }
            if ch
                .variable(SofiaVariable::RtpSecureMediaConfirmed)
                .is_some()
            {
                flags.push_str("[SEC]");
            }
            if let Some(other) = ch.header(EventHeader::OtherLegUniqueId) {
                let other = short_uuid(other);
                let _ = write!(flags, "[B:{other}]");
            }
            if let Some(call_id) = ch.variable(SofiaVariable::SipCallId) {
                let call_id = truncate_chars(call_id, 16);
                let _ = write!(flags, "[SIP:{call_id}]");
            }
            let flags = if flags.is_empty() {
                String::new()
            } else {
                format!(" {flags}")
            };
            println!(
                "{uuid:<36}  {state:<14} {call_state:<10} {dir:<8} {cid:<16} {dest:<16} {name}{flags}"
            );
        }
    }
}

/// Ask for a dump without waiting on it. The result arrives as a BACKGROUND_JOB
/// and is correlated in the event loop via [`BgJobTracker::try_complete`].
async fn request_dump(
    client: &EslClient,
    bg: &mut BgJobTracker<PendingDump>,
    tracker: &mut ChannelTracker,
    uuid: &str,
) {
    let pending = PendingDump {
        uuid: uuid.to_string(),
        sent: Instant::now(),
    };
    if let Err(e) = bg
        .bgapi(client, &format!("uuid_dump {uuid}"), pending)
        .await
    {
        // That dump was the channel's only route to being readable.
        warn!("bgapi uuid_dump {} failed: {e}", short_uuid(uuid));
        tracker.forget(uuid);
    }
}

/// Drop dumps that outlived [`DUMP_DEADLINE`] and hand their UUIDs back to be
/// asked for again -- a tracked job whose BACKGROUND_JOB never arrives is
/// reclaimed here or not at all.
fn sweep_stale_dumps(bg: &mut BgJobTracker<PendingDump>) -> Vec<String> {
    let mut stale = Vec::new();
    bg.retain(|_, pending| {
        if pending
            .sent
            .elapsed()
            < DUMP_DEADLINE
        {
            return true;
        }
        stale.push(
            pending
                .uuid
                .clone(),
        );
        false
    });
    stale
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let (client, mut events) = common::connect_from_env().await?;

    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::ChannelCreate,
                EslEventType::ChannelDestroy,
                EslEventType::ChannelState,
                EslEventType::ChannelCallstate,
                EslEventType::ChannelAnswer,
                EslEventType::ChannelHangup,
                EslEventType::ChannelHangupComplete,
                EslEventType::ChannelExecute,
                EslEventType::ChannelExecuteComplete,
                EslEventType::ChannelHold,
                EslEventType::ChannelUnhold,
                EslEventType::ChannelBridge,
                EslEventType::ChannelUnbridge,
                EslEventType::ChannelProgress,
                EslEventType::ChannelProgressMedia,
                EslEventType::ChannelOutgoing,
                EslEventType::ChannelPark,
                EslEventType::ChannelUnpark,
                EslEventType::ChannelApplication,
                EslEventType::ChannelOriginate,
                EslEventType::ChannelUuid,
                EslEventType::CallSecure,
                EslEventType::CallUpdate,
                EslEventType::BackgroundJob,
                EslEventType::Heartbeat,
            ],
        )
        .await?;

    info!("Subscribed to channel events + heartbeat");

    let mut tracker = ChannelTracker::new();
    let mut bg: BgJobTracker<PendingDump> = BgJobTracker::new();

    // Subscribe first, so a channel created while this runs arrives as an event
    // rather than being missed by both paths. Blocking here is the point: the
    // UUID set is what the tracker starts from, and the dumps that fill it in
    // land later, on the event loop below.
    let listing = client
        .api("show channels as json")
        .await?;
    let uuids = bootstrap_uuids(listing.api_result()?)?;
    info!("Bootstrap listed {} channel(s)", uuids.len());
    for uuid in &uuids {
        tracker.sight(uuid);
        request_dump(&client, &mut bg, &mut tracker, uuid).await;
    }

    info!("Listening for events... Press Ctrl+C to exit");

    while let Some(result) = events
        .recv()
        .await
    {
        let event = match result {
            Ok(event) => event,
            Err(e) => {
                error!("Event error: {e}");
                continue;
            }
        };

        if let Some((pending, result)) = bg.try_complete(&event) {
            if let Some(uuid) = tracker.apply_dump(&pending.uuid, &result) {
                request_dump(&client, &mut bg, &mut tracker, &uuid).await;
            }
            continue;
        }

        if event.is_event_type(EslEventType::Heartbeat) {
            tracker.print_summary();
            for uuid in sweep_stale_dumps(&mut bg) {
                if tracker.is_awaiting(&uuid) {
                    warn!(
                        "uuid_dump {} never came back, asking again",
                        short_uuid(&uuid)
                    );
                    request_dump(&client, &mut bg, &mut tracker, &uuid).await;
                }
            }
            continue;
        }

        if let Some(uuid) = tracker.process_event(&event) {
            request_dump(&client, &mut bg, &mut tracker, &uuid).await;
        }
    }

    info!("Connection closed");
    client
        .disconnect()
        .await?;

    Ok(())
}
