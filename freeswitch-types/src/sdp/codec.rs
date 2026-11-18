//! SDP media types and codec descriptors.
//!
//! Line numbers in this module index FreeSWITCH `v1.11.1`
//! (`c2c59645f6911a76589e5008c4d73349ded44b65`).

use std::fmt;
use std::str::FromStr;

/// SDP media type from an `m=` line.
///
/// Unrecognized types are represented as [`Other`](SdpMediaType::Other) rather
/// than hard-failing the parse — FreeSWITCH ignores media sections it does not
/// recognize (BFCP, MSRP, datachannel, etc.), and a converter must not drop
/// the whole session over an unknown `m=` line.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SdpMediaType {
    /// `m=audio`
    Audio,
    /// `m=video`
    Video,
    /// `m=application` — used for BFCP, MSRP, datachannel, etc.
    Application,
    /// `m=text` — used for MSRP text streams.
    Text,
    /// `m=message` — used for MSRP message streams.
    Message,
    /// Any other `m=` media type not recognized above.
    Other(String),
}

impl fmt::Display for SdpMediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Audio => f.write_str("audio"),
            Self::Video => f.write_str("video"),
            Self::Application => f.write_str("application"),
            Self::Text => f.write_str("text"),
            Self::Message => f.write_str("message"),
            Self::Other(s) => f.write_str(s),
        }
    }
}

impl FromStr for SdpMediaType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "audio" => Self::Audio,
            "video" => Self::Video,
            "application" => Self::Application,
            "text" => Self::Text,
            "message" => Self::Message,
            other => Self::Other(other.to_string()),
        })
    }
}

/// SDP direction attribute from an `a=` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SdpDirection {
    /// `a=sendrecv` — bidirectional (the default when no direction attribute is present).
    SendRecv,
    /// `a=sendonly` — the sender transmits but does not receive.
    SendOnly,
    /// `a=recvonly` — the sender receives but does not transmit.
    RecvOnly,
    /// `a=inactive` — neither side transmits.
    Inactive,
}

impl fmt::Display for SdpDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SendRecv => f.write_str("sendrecv"),
            Self::SendOnly => f.write_str("sendonly"),
            Self::RecvOnly => f.write_str("recvonly"),
            Self::Inactive => f.write_str("inactive"),
        }
    }
}

/// Errors returned when an unrecognized direction attribute is encountered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseSdpDirectionError(String);

impl fmt::Display for ParseSdpDirectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unrecognized SDP direction attribute: {:?}", self.0)
    }
}

impl std::error::Error for ParseSdpDirectionError {}

impl FromStr for SdpDirection {
    type Err = ParseSdpDirectionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "sendrecv" => Ok(Self::SendRecv),
            "sendonly" => Ok(Self::SendOnly),
            "recvonly" => Ok(Self::RecvOnly),
            "inactive" => Ok(Self::Inactive),
            other => Err(ParseSdpDirectionError(other.to_string())),
        }
    }
}

/// A codec derived from an SDP `m=` section.
///
/// Fields are private and read-only from outside the crate: a codec is what the
/// offer said. Build one with [`new`](Self::new) plus the `with_*` methods.
#[derive(Debug, Clone)]
pub struct SdpCodec {
    media: SdpMediaType,
    /// Session-local payload type from the `m=` format list.
    /// Never emitted into a codec string — it is only meaningful within
    /// this session and must not be carried across calls.
    payload_type: u8,
    name: String,
    clock_rate: u32,
    channels: Option<u32>,
    fmtp: Option<String>,
    ptime: Option<u32>,
    maxptime: Option<u32>,
    bitrate: Option<u32>,
    direction: SdpDirection,
    has_rtpmap: bool,
}

impl SdpCodec {
    /// Create a codec from the fields an `m=` line and its rtpmap always supply.
    ///
    /// The direction defaults to [`SdpDirection::SendRecv`], which is what a
    /// section carrying no direction attribute means, and `has_rtpmap` to
    /// `false` — set it with [`with_rtpmap`](Self::with_rtpmap) when the name
    /// and clock rate came off an `a=rtpmap` line rather than the switch's
    /// static table.
    pub fn new(
        media: SdpMediaType,
        payload_type: u8,
        name: impl Into<String>,
        clock_rate: u32,
    ) -> Self {
        Self {
            media,
            payload_type,
            name: name.into(),
            clock_rate,
            channels: None,
            fmtp: None,
            ptime: None,
            maxptime: None,
            bitrate: None,
            direction: SdpDirection::SendRecv,
            has_rtpmap: false,
        }
    }

    /// Set the channel count from `a=rtpmap`.
    pub fn with_channels(mut self, channels: u32) -> Self {
        self.channels = Some(channels);
        self
    }

    /// Set the format parameters from `a=fmtp`.
    pub fn with_fmtp(mut self, fmtp: impl Into<String>) -> Self {
        self.fmtp = Some(fmtp.into());
        self
    }

    /// Set the packetization time in milliseconds.
    pub fn with_ptime(mut self, ptime: u32) -> Self {
        self.ptime = Some(ptime);
        self
    }

    /// Set the maximum packetization time in milliseconds.
    pub fn with_maxptime(mut self, maxptime: u32) -> Self {
        self.maxptime = Some(maxptime);
        self
    }

    /// Set the bitrate in bits per second.
    pub fn with_bitrate(mut self, bitrate: u32) -> Self {
        self.bitrate = Some(bitrate);
        self
    }

    /// Set the media direction of the section this codec came from.
    pub fn with_direction(mut self, direction: SdpDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Record that the encoding name and clock rate came from an `a=rtpmap` line.
    pub fn with_rtpmap(mut self) -> Self {
        self.has_rtpmap = true;
        self
    }

    /// The SDP media type this codec belongs to.
    pub fn media(&self) -> &SdpMediaType {
        &self.media
    }

    /// The session-local RTP payload type.
    ///
    /// This value is assigned by the offerer for the duration of this session
    /// only. It is **never** emitted into a FreeSWITCH codec string — codec
    /// strings identify codecs by name, not by payload type number.
    pub fn payload_type(&self) -> u8 {
        self.payload_type
    }

    /// The codec encoding name (e.g. `"opus"`, `"PCMU"`, `"AMR-WB"`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The RTP clock rate in Hz.
    pub fn clock_rate(&self) -> u32 {
        self.clock_rate
    }

    /// The channel count, or `None` when not applicable (video) or stream-defined.
    pub fn channels(&self) -> Option<u32> {
        self.channels
    }

    /// The format parameters from `a=fmtp`, if any.
    pub fn fmtp(&self) -> Option<&str> {
        self.fmtp
            .as_deref()
    }

    /// The packetization time in milliseconds, or `None` if not specified.
    pub fn ptime(&self) -> Option<u32> {
        self.ptime
    }

    /// The offered `a=maxptime` in milliseconds, or `None` if not specified.
    ///
    /// Retained for offer fidelity: nothing in this crate reads it, and the switch
    /// derives no codec-string qualifier from it.
    pub fn maxptime(&self) -> Option<u32> {
        self.maxptime
    }

    /// The bitrate in bits per second, or `None` if not known.
    pub fn bitrate(&self) -> Option<u32> {
        self.bitrate
    }

    /// The media direction for this codec's stream.
    pub fn direction(&self) -> SdpDirection {
        self.direction
    }

    /// Whether the encoding name and clock rate came from an `a=rtpmap` line.
    ///
    /// `false` means they were filled in from the switch's static payload table.
    pub fn has_rtpmap(&self) -> bool {
        self.has_rtpmap
    }

    // --- mutable accessors, for this crate's own section walk ---

    /// Mutable access to the format parameters.
    pub(crate) fn fmtp_mut(&mut self) -> &mut Option<String> {
        &mut self.fmtp
    }

    /// Mutable access to ptime.
    pub(crate) fn ptime_mut(&mut self) -> &mut Option<u32> {
        &mut self.ptime
    }

    /// Mutable access to maxptime.
    pub(crate) fn maxptime_mut(&mut self) -> &mut Option<u32> {
        &mut self.maxptime
    }

    /// Mutable access to bitrate.
    pub(crate) fn bitrate_mut(&mut self) -> &mut Option<u32> {
        &mut self.bitrate
    }

    /// Mutable access to the channel count.
    pub(crate) fn channels_mut(&mut self) -> &mut Option<u32> {
        &mut self.channels
    }
}

/// A payload type FreeSWITCH negotiates outside the codec string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NonCodecKind {
    /// RFC 2833 / RFC 4733 DTMF, carried as `smh->mparams->te`.
    TelephoneEvent,
    /// Comfort noise, carried as `smh->mparams->cng_pt`.
    ComfortNoise,
}

impl fmt::Display for NonCodecKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TelephoneEvent => f.write_str("telephone-event"),
            Self::ComfortNoise => f.write_str("CN"),
        }
    }
}

/// A payload offered in an `m=` section that is negotiated outside the codec string.
///
/// This is the offer's inventory in `m=` order, undeduplicated — not a negotiation
/// outcome. FreeSWITCH keeps only one of each per session, picking the entry whose
/// clock rate matches the negotiated codec's advertised rate
/// (`switch_core_media.c:5805`, `:5816`) and forcing the retained rate to 8000 Hz
/// when it does not match (`:5829-5834`). That comparison needs a negotiated codec,
/// which does not exist at this layer, so a caller holding one applies the rule itself.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NonCodecPayload {
    /// Which payload this is, resolved from the canonical encoding name.
    pub kind: NonCodecKind,
    /// Session-local payload type from the `m=` format list.
    pub payload_type: u8,
    /// The media type of the section where this payload appeared.
    pub media_type: SdpMediaType,
    /// Clock rate in Hz, from `a=rtpmap` or the switch's static payload table.
    pub clock_rate: u32,
    /// Format parameters from `a=fmtp`, verbatim.
    ///
    /// FreeSWITCH never reads this from a received offer: both kinds leave the rtpmap
    /// walk (`switch_core_media.c:5447`, `:5456`) before `switch_core_codec_parse_fmtp`
    /// at `:5493`, and the DTMF digit range in a generated offer is synthesized from
    /// `NDLB_line_flash_16` instead (`:10653-10659`). A disagreement between two offers
    /// is therefore real but is not something the switch acted on.
    pub fmtp: Option<String>,
    /// `false` when the payload type was resolved from the switch's static table
    /// rather than declared by the peer with an `a=rtpmap` line.
    pub has_rtpmap: bool,
}

impl fmt::Display for NonCodecPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}/{}", self.payload_type, self.kind, self.clock_rate)?;
        if let Some(fmtp) = &self.fmtp {
            write!(f, " {fmtp}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- NonCodecPayload ---

    #[test]
    fn non_codec_payload_display_reproduces_its_rtpmap() {
        let te = NonCodecPayload {
            kind: NonCodecKind::TelephoneEvent,
            payload_type: 101,
            media_type: SdpMediaType::Audio,
            clock_rate: 8000,
            fmtp: Some("0-16".to_string()),
            has_rtpmap: true,
        };
        assert_eq!(te.to_string(), "101 telephone-event/8000 0-16");

        let cn = NonCodecPayload {
            kind: NonCodecKind::ComfortNoise,
            payload_type: 13,
            media_type: SdpMediaType::Audio,
            clock_rate: 8000,
            fmtp: None,
            has_rtpmap: false,
        };
        assert_eq!(cn.to_string(), "13 CN/8000");
    }

    #[test]
    fn non_codec_kind_display_is_the_canonical_encoding_name() {
        assert_eq!(NonCodecKind::TelephoneEvent.to_string(), "telephone-event");
        assert_eq!(NonCodecKind::ComfortNoise.to_string(), "CN");
    }

    // --- SdpMediaType ---

    #[test]
    fn sdp_media_type_display() {
        assert_eq!(SdpMediaType::Audio.to_string(), "audio");
        assert_eq!(SdpMediaType::Video.to_string(), "video");
        assert_eq!(SdpMediaType::Application.to_string(), "application");
        assert_eq!(SdpMediaType::Text.to_string(), "text");
        assert_eq!(SdpMediaType::Message.to_string(), "message");
        assert_eq!(
            SdpMediaType::Other("image".to_string()).to_string(),
            "image"
        );
    }

    #[test]
    fn sdp_media_type_from_str_known() {
        assert_eq!(
            "audio"
                .parse::<SdpMediaType>()
                .unwrap(),
            SdpMediaType::Audio
        );
        assert_eq!(
            "video"
                .parse::<SdpMediaType>()
                .unwrap(),
            SdpMediaType::Video
        );
        assert_eq!(
            "application"
                .parse::<SdpMediaType>()
                .unwrap(),
            SdpMediaType::Application
        );
        assert_eq!(
            "text"
                .parse::<SdpMediaType>()
                .unwrap(),
            SdpMediaType::Text
        );
        assert_eq!(
            "message"
                .parse::<SdpMediaType>()
                .unwrap(),
            SdpMediaType::Message
        );
    }

    #[test]
    fn sdp_media_type_from_str_unknown_becomes_other() {
        // Unknown types become Other instead of erroring — FreeSWITCH ignores
        // media sections it does not recognize.
        let t = "image"
            .parse::<SdpMediaType>()
            .unwrap();
        assert_eq!(t, SdpMediaType::Other("image".to_string()));
    }

    #[test]
    fn sdp_media_type_round_trip() {
        for s in &[
            "audio",
            "video",
            "application",
            "text",
            "message",
            "datachannel",
        ] {
            let parsed: SdpMediaType = s
                .parse()
                .unwrap();
            assert_eq!(&parsed.to_string(), s);
        }
    }

    // --- SdpDirection ---

    #[test]
    fn sdp_direction_display() {
        assert_eq!(SdpDirection::SendRecv.to_string(), "sendrecv");
        assert_eq!(SdpDirection::SendOnly.to_string(), "sendonly");
        assert_eq!(SdpDirection::RecvOnly.to_string(), "recvonly");
        assert_eq!(SdpDirection::Inactive.to_string(), "inactive");
    }

    #[test]
    fn sdp_direction_from_str() {
        assert_eq!(
            "sendrecv"
                .parse::<SdpDirection>()
                .unwrap(),
            SdpDirection::SendRecv
        );
        assert_eq!(
            "sendonly"
                .parse::<SdpDirection>()
                .unwrap(),
            SdpDirection::SendOnly
        );
        assert_eq!(
            "recvonly"
                .parse::<SdpDirection>()
                .unwrap(),
            SdpDirection::RecvOnly
        );
        assert_eq!(
            "inactive"
                .parse::<SdpDirection>()
                .unwrap(),
            SdpDirection::Inactive
        );
    }

    #[test]
    fn sdp_direction_from_str_error() {
        let err = "halfduplex"
            .parse::<SdpDirection>()
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("halfduplex"));
    }

    #[test]
    fn sdp_direction_round_trip() {
        for dir in &[
            SdpDirection::SendRecv,
            SdpDirection::SendOnly,
            SdpDirection::RecvOnly,
            SdpDirection::Inactive,
        ] {
            let parsed: SdpDirection = dir
                .to_string()
                .parse()
                .unwrap();
            assert_eq!(&parsed, dir);
        }
    }

    // --- SdpCodec ---

    fn make_audio_codec() -> SdpCodec {
        SdpCodec::new(SdpMediaType::Audio, 0, "PCMU", 8000)
            .with_channels(1)
            .with_ptime(20)
            .with_bitrate(64000)
    }

    #[test]
    fn sdp_codec_new_defaults() {
        let c = SdpCodec::new(SdpMediaType::Audio, 8, "PCMA", 8000);
        assert_eq!(c.channels(), None);
        assert_eq!(c.fmtp(), None);
        assert_eq!(c.ptime(), None);
        assert_eq!(c.maxptime(), None);
        assert_eq!(c.bitrate(), None);
        assert_eq!(c.direction(), SdpDirection::SendRecv);
        assert!(!c.has_rtpmap());
    }

    #[test]
    fn sdp_codec_builders_set_optional_fields() {
        let c = SdpCodec::new(SdpMediaType::Audio, 9, "G722", 8000)
            .with_maxptime(40)
            .with_direction(SdpDirection::RecvOnly);
        assert_eq!(c.maxptime(), Some(40));
        assert_eq!(c.direction(), SdpDirection::RecvOnly);
    }

    #[test]
    fn sdp_codec_accessors() {
        let c = make_audio_codec();
        assert_eq!(c.media(), &SdpMediaType::Audio);
        assert_eq!(c.payload_type(), 0);
        assert_eq!(c.name(), "PCMU");
        assert_eq!(c.clock_rate(), 8000);
        assert_eq!(c.channels(), Some(1));
        assert_eq!(c.fmtp(), None);
        assert_eq!(c.ptime(), Some(20));
        assert_eq!(c.maxptime(), None);
        assert_eq!(c.bitrate(), Some(64000));
        assert_eq!(c.direction(), SdpDirection::SendRecv);
        assert!(!c.has_rtpmap());
    }

    #[test]
    fn sdp_codec_with_rtpmap() {
        let c = SdpCodec::new(SdpMediaType::Audio, 111, "opus", 48000)
            .with_channels(2)
            .with_fmtp("minptime=10;useinbandfec=1")
            .with_ptime(20)
            .with_rtpmap();
        assert_eq!(c.name(), "opus");
        assert_eq!(c.clock_rate(), 48000);
        assert_eq!(c.channels(), Some(2));
        assert_eq!(c.fmtp(), Some("minptime=10;useinbandfec=1"));
        assert!(c.has_rtpmap());
    }

    #[test]
    fn sdp_codec_video_no_channels() {
        let c = SdpCodec::new(SdpMediaType::Video, 99, "H264", 90000).with_rtpmap();
        assert_eq!(c.media(), &SdpMediaType::Video);
        assert!(c
            .channels()
            .is_none());
    }

    #[test]
    fn sdp_codec_mutable_accessors() {
        let mut c = make_audio_codec();
        *c.ptime_mut() = Some(30);
        assert_eq!(c.ptime(), Some(30));

        *c.fmtp_mut() = Some("mode=20".into());
        assert_eq!(c.fmtp(), Some("mode=20"));

        *c.bitrate_mut() = None;
        assert!(c
            .bitrate()
            .is_none());

        *c.maxptime_mut() = Some(40);
        assert_eq!(c.maxptime(), Some(40));

        *c.channels_mut() = Some(2);
        assert_eq!(c.channels(), Some(2));
    }
}
