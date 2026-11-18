//! Codec and bit rate monitor -- subscribes to CODEC events for live transcoding visibility.
//!
//! FreeSWITCH fires a CODEC event every time a read or write codec is set or changed on a
//! channel. Each event carries `Channel-Read-Codec-Bit-Rate` and `Channel-Write-Codec-Bit-Rate`
//! as event headers (these are NOT channel variables -- `uuid_getvar` cannot retrieve them).
//!
//! This example follows CODEC, CHANNEL_ANSWER and the bridge lifecycle to show:
//! - Initial codec negotiation (on answer)
//! - Codec changes mid-call (re-INVITE, offer/answer renegotiation)
//! - Bridge pairs that transcode rather than pass through
//!
//! Limitation: for AMR-WB, `bits_per_second` reflects the SDP-negotiated rate, not per-frame
//! mode changes. AMR-WB can switch modes frame-by-frame without firing a new CODEC event.
//!
//! Usage: RUST_LOG=info cargo run --example codec_monitor
//!   Configure via ESL_HOST, ESL_PORT, ESL_PASSWORD env vars.

use std::collections::HashMap;
use std::fmt;

mod common;

use freeswitch_esl_tokio::{EslEvent, EslEventType, EventFormat, EventHeader, HeaderLookup};
use tracing::{error, info, warn};

/// Enough of the UUID to correlate log lines, truncated by character: a value
/// off the wire may have decoded lossily, and slicing a replacement in half
/// panics.
fn short_uuid(uuid: &str) -> String {
    uuid.chars()
        .take(8)
        .collect()
}

/// One direction's negotiated codec. A `None` field means the event did not
/// carry that header, which is not the same as a codec whose name is `-`.
#[derive(PartialEq, Eq)]
struct Codec {
    name: Option<String>,
    rate: Option<String>,
    bitrate: Option<String>,
}

impl Codec {
    fn read_from(event: &EslEvent) -> Self {
        Self::from_headers(
            event,
            EventHeader::ChannelReadCodecName,
            EventHeader::ChannelReadCodecRate,
            EventHeader::ChannelReadCodecBitRate,
        )
    }

    fn write_from(event: &EslEvent) -> Self {
        Self::from_headers(
            event,
            EventHeader::ChannelWriteCodecName,
            EventHeader::ChannelWriteCodecRate,
            EventHeader::ChannelWriteCodecBitRate,
        )
    }

    fn from_headers(
        event: &EslEvent,
        name: EventHeader,
        rate: EventHeader,
        bitrate: EventHeader,
    ) -> Self {
        Self {
            name: event
                .header(name)
                .map(str::to_string),
            rate: event
                .header(rate)
                .map(str::to_string),
            bitrate: event
                .header(bitrate)
                .map(str::to_string),
        }
    }

    fn is_known(&self) -> bool {
        self.name
            .is_some()
    }
}

impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let field = |v: &Option<String>| {
            v.clone()
                .unwrap_or_else(|| "-".to_string())
        };
        write!(
            f,
            "{}/{}hz/{}bps",
            field(&self.name),
            field(&self.rate),
            field(&self.bitrate)
        )
    }
}

#[derive(PartialEq, Eq)]
struct CodecInfo {
    read: Codec,
    write: Codec,
}

impl CodecInfo {
    fn from_event(event: &EslEvent) -> Self {
        Self {
            read: Codec::read_from(event),
            write: Codec::write_from(event),
        }
    }

    /// Audio crossing a bridge is decoded and re-encoded when what one leg
    /// reads is not what the other leg writes. `None` while either side has
    /// reported too little to say -- silence there would read as passthrough.
    fn transcodes_with(&self, other: &CodecInfo) -> Option<bool> {
        let known = self
            .read
            .is_known()
            && self
                .write
                .is_known()
            && other
                .read
                .is_known()
            && other
                .write
                .is_known();
        known.then(|| self.read != other.write || self.write != other.read)
    }
}

impl fmt::Display for CodecInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r={} w={}", self.read, self.write)
    }
}

struct Monitor {
    channels: HashMap<String, CodecInfo>,
    /// channel UUID -> bridge partner UUID
    bridges: HashMap<String, String>,
}

impl Monitor {
    fn new() -> Self {
        Self {
            channels: HashMap::new(),
            bridges: HashMap::new(),
        }
    }

    fn handle_codec(&mut self, event: &EslEvent) {
        let Some(uuid) = event.unique_id() else {
            return;
        };
        let info = CodecInfo::from_event(event);

        match self
            .channels
            .get(uuid)
        {
            Some(previous) if *previous == info => return,
            Some(previous) => {
                info!("{} codec changed: {previous} -> {info}", short_uuid(uuid));
            }
            None => info!("{} codec set: {info}", short_uuid(uuid)),
        }

        self.channels
            .insert(uuid.to_string(), info);
        self.check_transcoding(uuid);
    }

    fn handle_bridge(&mut self, event: &EslEvent) {
        let Some(uuid) = event.unique_id() else {
            return;
        };
        let Some(other) = event.header(EventHeader::OtherLegUniqueId) else {
            return;
        };

        // The bridge event carries this leg's Channel-* headers too.
        self.channels
            .insert(uuid.to_string(), CodecInfo::from_event(event));

        self.bridges
            .insert(uuid.to_string(), other.to_string());
        self.bridges
            .insert(other.to_string(), uuid.to_string());

        self.check_transcoding(uuid);
    }

    /// An unbridged channel is still up and may bridge again, so only the
    /// pairing goes; dropping its codecs would leave the next bridge with
    /// nothing to compare until another CODEC event happened to fire.
    fn handle_unbridge(&mut self, event: &EslEvent) {
        let Some(uuid) = event.unique_id() else {
            return;
        };
        if let Some(partner) = self
            .bridges
            .remove(uuid)
        {
            self.bridges
                .remove(&partner);
        }
    }

    fn handle_destroy(&mut self, event: &EslEvent) {
        let Some(uuid) = event.unique_id() else {
            return;
        };
        self.handle_unbridge(event);
        self.channels
            .remove(uuid);
    }

    fn check_transcoding(&self, uuid: &str) {
        let Some(partner) = self
            .bridges
            .get(uuid)
        else {
            return;
        };
        let (Some(a), Some(b)) = (
            self.channels
                .get(uuid),
            self.channels
                .get(partner),
        ) else {
            return;
        };

        let (short_a, short_b) = (short_uuid(uuid), short_uuid(partner));
        match a.transcodes_with(b) {
            Some(true) => warn!("TRANSCODING {short_a}<->{short_b}: A[{a}] B[{b}]"),
            Some(false) => info!("passthrough {short_a}<->{short_b}: {a}"),
            // Saying nothing here is what made "no output" ambiguous between
            // a passthrough bridge and one never evaluated.
            None => info!("{short_a}<->{short_b}: codecs not fully reported yet: A[{a}] B[{b}]"),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let (client, mut events) = common::connect_from_env().await?;

    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::Codec,
                EslEventType::ChannelAnswer,
                EslEventType::ChannelBridge,
                EslEventType::ChannelUnbridge,
                EslEventType::ChannelDestroy,
            ],
        )
        .await?;

    let mut monitor = Monitor::new();

    while let Some(result) = events
        .recv()
        .await
    {
        let event = match result {
            Ok(event) => event,
            Err(e) => {
                error!("event error: {}", e.display_chain());
                continue;
            }
        };

        match event.event_type() {
            Some(EslEventType::Codec | EslEventType::ChannelAnswer) => {
                monitor.handle_codec(&event);
            }
            Some(EslEventType::ChannelBridge) => monitor.handle_bridge(&event),
            Some(EslEventType::ChannelUnbridge) => monitor.handle_unbridge(&event),
            Some(EslEventType::ChannelDestroy) => monitor.handle_destroy(&event),
            _ => {}
        }
    }

    info!("connection closed");
    client
        .disconnect()
        .await?;

    Ok(())
}
