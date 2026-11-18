//! Collection of codecs parsed from a complete SDP session description.
//!
//! An SDP is read the way FreeSWITCH reads it: sofia-sip's `sdp_parse` decides which bytes form a
//! line, which section a line belongs to and which lines are skipped. Where that parser would
//! refuse the offer, or stop reading it early, the parse here goes on as far as extraction can,
//! and says so: [`SdpWarning::SwitchRejects`] names the first refusal and
//! [`SdpWarning::SwitchStopsReading`] the byte where the switch's reading ends, each with a byte
//! offset into the input. The switch read exactly the input before a stop, so parsing that prefix
//! gives the switch's view of the offer.
//!
//! Only the lines extraction reads (`m=`, `a=`) are interpreted. The values of the others (`o=`,
//! `c=`, `b=`, `t=`, `r=`, `z=`, `k=`) are not validated, so an offer the switch refuses for a
//! malformed value on one of them alone carries no warning. Sections the codec string excludes
//! are still parsed, so they can warn where the switch has no occasion to.
//!
//! Line numbers in this module index FreeSWITCH `v1.11.1`
//! (`c2c59645f6911a76589e5008c4d73349ded44b65`).

mod attrs;
#[cfg(test)]
mod c_oracle;
mod media_section;
#[cfg(test)]
mod switch_reads;
#[cfg(test)]
mod testutil;

use attrs::{direction_from_attrs, ptime_from_attrs};
use media_section::{is_image, parse_media_section, SessionDefaults};

use crate::sdp::{
    codec::{NonCodecPayload, SdpCodec, SdpDirection, SdpMediaType},
    codec_string::{CodecString, CodecStringEntry},
    error::{CodecStringError, SdpCodecError, SdpWarning, UnmappedPayload},
    options::CodecStringOptions,
    session,
};

/// A single entry in a parsed SDP offer.
///
/// Payloads negotiated outside the codec string are excluded from entries and
/// retained whole by [`SdpMediaSection::non_codec_payloads`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SdpCodecEntry {
    /// An RTP codec negotiated via `a=rtpmap` or the switch's static payload table.
    Rtp(SdpCodec),
    /// T.38 fax relay, derived from any `m=image` section.
    ///
    /// The proto and fmt fields are not inspected — FreeSWITCH negotiates T.38
    /// parameters independently of the codec string. Whether the section reaches
    /// a codec string is [`SdpMediaSection::is_negotiable`].
    T38,
}

/// One `m=` section of an SDP offer, whatever its port or media type.
///
/// Sections that contribute nothing to a codec string are retained all the same:
/// a port-0 section is the offer's held or declined stream and is exactly what a
/// reader needs when a call has no audio. [`is_negotiable`](Self::is_negotiable)
/// separates the two.
// qual:allow(srp, god_struct) reason: "public data carrier; accessors read disjoint fields"
#[derive(Debug, Clone)]
pub struct SdpMediaSection {
    media_type: SdpMediaType,
    port: u16,
    proto: String,
    formats: String,
    direction: SdpDirection,
    entries: Vec<SdpCodecEntry>,
    unmapped: Vec<UnmappedPayload>,
    non_codec: Vec<NonCodecPayload>,
}

impl SdpMediaSection {
    /// The media type from the `m=` line.
    pub fn media_type(&self) -> &SdpMediaType {
        &self.media_type
    }

    /// The transport port; `0` means the peer declined or held this stream.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The transport protocol from the `m=` line, verbatim.
    pub fn proto(&self) -> &str {
        &self.proto
    }

    /// The `m=` format list, verbatim — the only content a non-RTP section carries.
    pub fn formats(&self) -> &str {
        &self.formats
    }

    /// The direction the peer wrote, defaulting to the session's.
    ///
    /// Sofia forces a port-0 section to inactive regardless, so on a held section
    /// this is what was offered, not what the switch acted on.
    pub fn direction(&self) -> SdpDirection {
        self.direction
    }

    /// Codec entries in `m=` format-list order.
    pub fn entries(&self) -> &[SdpCodecEntry] {
        &self.entries
    }

    /// Payload types this section named that could not be resolved to a codec.
    pub fn unmapped(&self) -> &[UnmappedPayload] {
        &self.unmapped
    }

    /// Payloads the switch negotiates outside the codec string, in format-list order.
    ///
    /// Not deduplicated. See [`NonCodecPayload`] for what the switch itself retains.
    pub fn non_codec_payloads(&self) -> &[NonCodecPayload] {
        &self.non_codec
    }

    /// Whether this section feeds the codec string and the top-level views.
    pub fn is_negotiable(&self) -> bool {
        let derives_entries = matches!(self.media_type, SdpMediaType::Audio | SdpMediaType::Video)
            || is_image(&self.media_type);
        self.port != 0 && derives_entries
    }
}

/// Codecs and ancillary data extracted from an SDP session description.
///
/// Produced by [`SdpCodecs::parse`]. The accessors that feed a codec string reflect
/// the same extraction logic as FreeSWITCH's
/// `switch_core_media_set_r_sdp_codec_string`; [`sections`](Self::sections) is the
/// wider view, every `m=` line the offer carried. See `docs/codec-string-format.md`
/// for the mapping rules.
#[derive(Debug, Clone)]
pub struct SdpCodecs {
    sections: Vec<SdpMediaSection>,
    warnings: Vec<SdpWarning>,
}

impl SdpCodecs {
    /// Parse an SDP session description from a UTF-8 string.
    ///
    /// Fails only where extraction cannot proceed: the first line is not exactly `v=0`, an `m=`
    /// port is above 65535, or (from [`parse_bytes`](Self::parse_bytes)) an `m=` or `a=` line is
    /// not UTF-8. Everything the switch would refuse comes back as a warning instead.
    pub fn parse(sdp: &str) -> Result<Self, SdpCodecError> {
        Self::parse_bytes(sdp.as_bytes())
    }

    /// Parse an SDP session description from bytes; see [`parse`](Self::parse).
    pub fn parse_bytes(bytes: &[u8]) -> Result<Self, SdpCodecError> {
        let session = session::parse(bytes)
            .map_err(|e| SdpCodecError::parse_failure("failed to parse SDP", e))?;
        Ok(Self::from_session(session))
    }

    fn from_session(session: session::Session) -> Self {
        let session::Session {
            attributes,
            medias,
            warnings,
        } = session;
        let mut result = Self {
            sections: Vec::new(),
            warnings,
        };

        // Session-level defaults; media-level attributes override these per section.
        let session_ptime = ptime_from_attrs(&attributes, "ptime", &mut result.warnings);
        let session_maxptime = ptime_from_attrs(&attributes, "maxptime", &mut result.warnings);
        let session_direction = direction_from_attrs(&attributes, &mut result.warnings)
            .unwrap_or(SdpDirection::SendRecv);

        for media in &medias {
            // SdpMediaType::from_str is infallible (Err = std::convert::Infallible).
            let media_type: SdpMediaType = match media
                .media
                .parse()
            {
                Ok(t) => t,
                Err(e) => match e {},
            };

            let mut section = SdpMediaSection {
                media_type: media_type.clone(),
                port: media.port,
                proto: media
                    .proto
                    .clone(),
                formats: media
                    .fmt
                    .clone(),
                direction: direction_from_attrs(&media.attributes, &mut result.warnings)
                    .unwrap_or(session_direction),
                entries: Vec::new(),
                unmapped: Vec::new(),
                non_codec: Vec::new(),
            };

            if is_image(&media_type) {
                // The proto and fmt are not inspected; the section is the T.38 stream.
                section
                    .entries
                    .push(SdpCodecEntry::T38);
            } else if session::proto_has_rtp(&media.proto) {
                // Staged locally: one structurally broken section must not discard
                // what every other section already parsed.
                let defaults = SessionDefaults {
                    ptime: session_ptime,
                    maxptime: session_maxptime,
                    direction: section.direction,
                };
                match parse_media_section(media, media_type.clone(), &defaults) {
                    Ok(parsed) => {
                        section.entries = parsed.entries;
                        section.unmapped = parsed.unmapped;
                        section.non_codec = parsed.non_codec;
                        result
                            .warnings
                            .extend(parsed.warnings);
                    }
                    Err(e) => {
                        result
                            .warnings
                            .push(SdpWarning::malformed_media_section(
                                media_type.to_string(),
                                e.to_string(),
                            ));
                    }
                }
            }

            result
                .sections
                .push(section);
        }

        result
    }

    /// Every `m=` section the offer carried, in offer order.
    ///
    /// This is the inventory, including the sections no codec string can reach — a
    /// held or declined stream, or a media type the switch derives no codec from.
    pub fn sections(&self) -> &[SdpMediaSection] {
        &self.sections
    }

    fn negotiable(&self) -> impl Iterator<Item = &SdpMediaSection> {
        self.sections
            .iter()
            .filter(|s| s.is_negotiable())
    }

    /// Codec entries from the negotiable sections, in SDP offer order.
    pub fn entries(&self) -> impl Iterator<Item = &SdpCodecEntry> {
        self.negotiable()
            .flat_map(|s| {
                s.entries
                    .iter()
            })
    }

    /// Number of codec entries; payloads from [`non_codec_payloads`](Self::non_codec_payloads)
    /// are not counted.
    pub fn len(&self) -> usize {
        self.entries()
            .count()
    }

    /// `true` when the offer negotiates no codecs, ignoring any non-codec payloads.
    pub fn is_empty(&self) -> bool {
        self.entries()
            .next()
            .is_none()
    }

    /// Iterator over audio RTP codecs only.
    pub fn audio(&self) -> impl Iterator<Item = &SdpCodec> {
        self.rtp_of(SdpMediaType::Audio)
    }

    /// Iterator over video RTP codecs only.
    pub fn video(&self) -> impl Iterator<Item = &SdpCodec> {
        self.rtp_of(SdpMediaType::Video)
    }

    fn rtp_of(&self, media: SdpMediaType) -> impl Iterator<Item = &SdpCodec> {
        self.entries()
            .filter_map(move |e| match e {
                SdpCodecEntry::Rtp(c) if c.media() == &media => Some(c),
                _ => None,
            })
    }

    /// Payload types that could not be named (no `a=rtpmap`, no static-table entry).
    ///
    /// These are surfaced as data rather than an error so the caller can distinguish
    /// "never offered" from "offered but unresolvable". A non-negotiable section's
    /// unresolvable payloads stay on [`SdpMediaSection::unmapped`].
    pub fn unmapped(&self) -> impl Iterator<Item = &UnmappedPayload> {
        self.negotiable()
            .flat_map(|s| {
                s.unmapped
                    .iter()
            })
    }

    /// Recoverable parse warnings (e.g. unparseable numeric attributes).
    ///
    /// Covers every section, negotiable or not — the excluded ones are parsed rather
    /// than skipped, so they warn where the switch has no occasion to.
    pub fn warnings(&self) -> &[SdpWarning] {
        &self.warnings
    }

    /// Payloads the switch negotiates outside the codec string, in `m=` order.
    ///
    /// Never included in [`entries`](Self::entries) or [`unmapped`](Self::unmapped),
    /// and drawn from the negotiable sections only; a held section's are on
    /// [`SdpMediaSection::non_codec_payloads`]. Not deduplicated: two sections offering
    /// the same kind at one clock rate under different payload types yield two entries.
    /// See [`NonCodecPayload`] for what the switch itself retains from these.
    pub fn non_codec_payloads(&self) -> impl Iterator<Item = &NonCodecPayload> {
        self.negotiable()
            .flat_map(|s| {
                s.non_codec
                    .iter()
            })
    }

    /// Build a FreeSWITCH codec string from this offer's audio codecs.
    ///
    /// Emits a literal `t38` entry for each `m=image` section the offer carried, in
    /// its original m-line position among the audio entries — C writes `,t38` inside
    /// the same per-m-line loop that emits audio codecs (`switch_core_media.c:13651`),
    /// so an `m=image` section before an `m=audio` section puts `t38` first, not last.
    /// Does not deduplicate — an offer carrying the AMR octet-aligned/bandwidth-efficient
    /// pair yields two identical entries under default options (they differ only in
    /// fmtp, which is off by default for audio). Call [`CodecString::dedup`] after
    /// composing the final string.
    ///
    /// `warnings = None` is strict: any unrepresentable codec name or fmtp is `Err`.
    /// `warnings = Some(acc)` is lenient: an unrepresentable fmtp is cleared (the codec
    /// is still emitted) and an unrepresentable codec name is skipped (the codec is
    /// dropped); both push a warning to `acc` rather than failing the whole call. An
    /// offer past [`CodecString::MAX_SWITCH_ENTRIES`] is
    /// [`CodecStringError::TooManyEntries`] in either mode — the limit is structural,
    /// not a per-codec oddity.
    pub fn audio_codec_string(
        &self,
        options: &CodecStringOptions,
        mut warnings: Option<&mut Vec<SdpWarning>>,
    ) -> Result<CodecString, CodecStringError> {
        let mut out = CodecString::new();
        for entry in self.entries() {
            match entry {
                // Infallible for this literal; propagated so a future rename of it
                // surfaces instead of dropping the entry.
                SdpCodecEntry::T38 => out.push(CodecStringEntry::new("t38")?)?,
                SdpCodecEntry::Rtp(codec) if codec.media() == &SdpMediaType::Audio => {
                    if let Some(entry) =
                        codec_to_entry_lenient(codec, options, warnings.as_deref_mut())?
                    {
                        out.push(entry)?;
                    }
                }
                SdpCodecEntry::Rtp(_) => {}
            }
        }
        Ok(out)
    }

    /// Build a FreeSWITCH codec string from this offer's video codecs.
    ///
    /// Selects straight from the video codecs, where the audio builder walks every entry
    /// instead: only the audio string interleaves T.38, which has to keep its m-line
    /// position. Does not deduplicate; see
    /// [`audio_codec_string`](Self::audio_codec_string) for why. `warnings` follows the
    /// same strict/lenient convention.
    pub fn video_codec_string(
        &self,
        options: &CodecStringOptions,
        mut warnings: Option<&mut Vec<SdpWarning>>,
    ) -> Result<CodecString, CodecStringError> {
        let mut out = CodecString::new();
        for codec in self.video() {
            if let Some(entry) = codec_to_entry_lenient(codec, options, warnings.as_deref_mut())? {
                out.push(entry)?;
            }
        }
        Ok(out)
    }

    /// Look up the fmtp of an offered audio codec by name and clock rate.
    ///
    /// Drives `rtp_force_audio_fmtp` once the caller knows which codec ended up first
    /// in its final, composed, filtered codec string — "first codec in the offer" is
    /// routinely not that codec, so this takes the name and rate explicitly rather than
    /// exposing a "primary" accessor that would often name the wrong payload map.
    ///
    /// Several payload types commonly share one name+rate with different fmtp — the
    /// AMR octet-aligned/bandwidth-efficient pair is the recurring case. When they
    /// disagree, the first one carrying an fmtp wins; a caller driving
    /// `rtp_force_audio_fmtp` from this can otherwise pin octet-aligned when the
    /// switch negotiated bandwidth-efficient.
    pub fn fmtp_for(&self, name: &str, clock_rate: u32) -> Option<&str> {
        self.audio()
            .filter(|c| {
                c.name()
                    .eq_ignore_ascii_case(name)
                    && c.clock_rate() == clock_rate
            })
            .find_map(|c| c.fmtp())
    }
}

/// Convert one [`SdpCodec`] to a [`CodecStringEntry`], splitting the two failure classes
/// `audio_codec_string`/`video_codec_string` must handle differently: a name that cannot
/// be embedded at all (`Ok(None)` lenient / `Err` strict, the codec is skipped) versus an
/// fmtp that cannot be embedded (the entry is still emitted with fmtp cleared).
/// The fault, named without the value the error's own `Display` quotes back: these
/// reasons describe a peer's bytes, not a codec string this caller wrote.
fn unrepresentable_reason(e: &CodecStringError) -> &'static str {
    match e {
        CodecStringError::AtInFmtp(_) => "contains '@', which the qualifier split takes first",
        CodecStringError::DotInFmtpWithoutModule(_) => {
            "contains '.' with no module prefix, which the modname split takes first"
        }
        CodecStringError::WireInjection { .. } => "contains a newline or a carriage return",
        CodecStringError::InvalidCharInName { .. } => "contains a codec-string grammar delimiter",
        CodecStringError::InvalidCodecName(_) => "is empty",
        _ => "has no representation in the codec-string grammar",
    }
}

fn codec_to_entry_lenient(
    codec: &SdpCodec,
    options: &CodecStringOptions,
    mut warnings: Option<&mut Vec<SdpWarning>>,
) -> Result<Option<CodecStringEntry>, CodecStringError> {
    let mut entry = match CodecStringEntry::new(codec.name()) {
        Ok(e) => e,
        Err(e) => {
            return match warnings.as_deref_mut() {
                None => Err(e),
                Some(acc) => {
                    acc.push(SdpWarning::codec_name_unrepresentable(
                        codec.name(),
                        unrepresentable_reason(&e),
                    ));
                    Ok(None)
                }
            };
        }
    };

    if options.emits_fmtp() {
        if let Some(fmtp) = codec.fmtp() {
            match entry
                .clone()
                .with_fmtp(fmtp)
            {
                Ok(with_fmtp) => entry = with_fmtp,
                Err(e) => match warnings {
                    None => return Err(e),
                    Some(acc) => {
                        acc.push(SdpWarning::fmtp_unrepresentable(
                            codec.name(),
                            fmtp,
                            unrepresentable_reason(&e),
                        ));
                        // entry keeps no fmtp; qualifiers below still apply.
                    }
                },
            }
        }
    }

    Ok(Some(options.apply_qualifiers(entry, codec)))
}

#[cfg(test)]
mod tests {
    use super::testutil::{codec_named, rtp_codec, sdp_header};
    use super::*;
    use crate::sdp::NonCodecKind;

    // --- static payload table ---

    #[test]
    fn static_only_pcmu_pcma_g729_no_rtpmap() {
        let sdp = format!("{}m=audio 5004 RTP/AVP 0 8 18\r\n", sdp_header());
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let entries = codecs.entries();
        let rtp = rtp_codec(entries);
        assert_eq!(rtp.len(), 3);
        assert!(codec_named(&rtp, "PCMU").is_some());
        assert!(codec_named(&rtp, "PCMA").is_some());
        assert!(codec_named(&rtp, "G729").is_some());
        for c in &rtp {
            assert!(!c.has_rtpmap(), "static types must have has_rtpmap=false");
        }
    }

    // --- AMR-WB channel count normalization ---

    #[test]
    fn amrwb_no_channel_field_equals_explicit_one() {
        // a=rtpmap without a channel field and a=rtpmap with /1 must yield the same
        // channel count — both normalize to Some(1) for audio sections.
        let sdp_no_ch = format!(
            "{}m=audio 5004 RTP/AVP 100\r\na=rtpmap:100 AMR-WB/16000\r\n",
            sdp_header()
        );
        let sdp_ch1 = format!(
            "{}m=audio 5004 RTP/AVP 100\r\na=rtpmap:100 AMR-WB/16000/1\r\n",
            sdp_header()
        );
        let c_no_ch = SdpCodecs::parse(&sdp_no_ch).unwrap();
        let c_ch1 = SdpCodecs::parse(&sdp_ch1).unwrap();
        let rtp_no_ch = rtp_codec(c_no_ch.entries());
        let rtp_ch1 = rtp_codec(c_ch1.entries());
        let amrwb_no_ch = codec_named(&rtp_no_ch, "AMR-WB").expect("AMR-WB must be present");
        let amrwb_ch1 = codec_named(&rtp_ch1, "AMR-WB").expect("AMR-WB must be present");
        assert_eq!(
            amrwb_no_ch.channels(),
            amrwb_ch1.channels(),
            "missing /1 and explicit /1 must normalize to the same channel count"
        );
        assert_eq!(amrwb_ch1.channels(), Some(1));
    }

    #[test]
    fn a_channel_count_above_a_byte_parses() {
        // RFC 4566 puts no ceiling on rtpmap's encoding parameters, so a count
        // that overflows a byte is a legal offer, not a malformed rtpmap.
        let sdp = format!(
            "{}m=audio 5004 RTP/AVP 100\r\na=rtpmap:100 L16/48000/300\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).expect("a legal channel count must not fail the parse");
        let rtp = rtp_codec(codecs.entries());
        let l16 = codec_named(&rtp, "L16").expect("L16 must be present");
        assert_eq!(l16.channels(), Some(300));
    }

    // --- ptime precedence ---

    #[test]
    fn ptime_opus_fmtp_overrides_session_ptime() {
        // Sequential overwrite: a=ptime:20 sets ptime=20, then opus fmtp ptime=40
        // overrides it. The final result must be 40.
        let sdp = format!(
            "{}a=ptime:20\r\nm=audio 5004 RTP/AVP 111\r\na=rtpmap:111 opus/48000/2\r\na=fmtp:111 ptime=40\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let rtp = rtp_codec(codecs.entries());
        let opus = codec_named(&rtp, "opus").expect("opus must be present");
        assert_eq!(opus.ptime(), Some(40), "fmtp ptime= must override a=ptime");
    }

    #[test]
    fn ptime_ilbc_no_fmtp_overrides_explicit_ptime() {
        // iLBC without fmtp overrides even an explicit a=ptime.
        let sdp = format!(
            "{}m=audio 5004 RTP/AVP 97\r\na=rtpmap:97 iLBC/8000\r\na=ptime:20\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let rtp = rtp_codec(codecs.entries());
        let ilbc = codec_named(&rtp, "iLBC").expect("iLBC must be present");
        assert_eq!(
            ilbc.ptime(),
            Some(30),
            "iLBC without fmtp must always yield ptime=30"
        );
    }

    #[test]
    fn ptime_ilbc_fmtp_mode20_yields_20_no_bitrate() {
        let sdp = format!(
            "{}m=audio 5004 RTP/AVP 97\r\na=rtpmap:97 iLBC/8000\r\na=fmtp:97 mode=20\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let rtp = rtp_codec(codecs.entries());
        let ilbc = codec_named(&rtp, "iLBC").expect("iLBC must be present");
        assert_eq!(ilbc.ptime(), Some(20));
        assert_eq!(
            ilbc.bitrate(),
            None,
            "fmtp present — bitrate not set by static table"
        );
    }

    // --- section ordering ---

    #[test]
    fn sections_keep_m_line_order_across_negotiable_and_not() {
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 5004 RTP/AVP 0\r\n",
                "m=video 0 RTP/AVP 99\r\n",
                "a=rtpmap:99 H264/90000\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let sections = codecs.sections();
        assert_eq!(sections[0].media_type(), &SdpMediaType::Audio);
        assert!(sections[0].is_negotiable());
        assert_eq!(sections[1].media_type(), &SdpMediaType::Video);
        assert!(!sections[1].is_negotiable());

        let rtp = rtp_codec(codecs.entries());
        assert_eq!(rtp.len(), 1, "the declined video must not reach entries()");
        assert!(codec_named(&rtp, "PCMU").is_some());
    }

    // --- IPv6 connection ---

    #[test]
    fn ipv6_connection_parses_without_error() {
        let sdp = format!(
            "{}c=IN IP6 2001:db8::1\r\nm=audio 5004 RTP/AVP 0\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(codecs.len(), 1);
    }

    // --- unknown media type ---

    #[test]
    fn application_m_line_does_not_fail() {
        let sdp = format!(
            "{}m=application 9 UDP/BFCP *\r\nm=audio 5004 RTP/AVP 0\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        // application section contributes no codecs
        assert_eq!(codecs.len(), 1);
        let rtp = rtp_codec(codecs.entries());
        assert!(codec_named(&rtp, "PCMU").is_some());
    }

    // --- telephone-event and CN ---

    #[test]
    fn telephone_event_retains_payload_type_rate_and_fmtp() {
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 5004 RTP/AVP 0 101 102\r\n",
                "a=rtpmap:101 telephone-event/8000\r\n",
                "a=fmtp:101 0-16\r\n",
                "a=rtpmap:102 telephone-event/16000\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let te: Vec<_> = codecs
            .non_codec_payloads()
            .collect();
        assert_eq!(
            te.len(),
            2,
            "both telephone-event payloads must be retained"
        );

        assert_eq!(te[0].kind, NonCodecKind::TelephoneEvent);
        assert_eq!(te[0].payload_type, 101);
        assert_eq!(te[0].clock_rate, 8000);
        assert_eq!(
            te[0]
                .fmtp
                .as_deref(),
            Some("0-16"),
            "the offered digit range is what an operator reads on a DTMF fault"
        );
        assert!(te[0].has_rtpmap);
        assert_eq!(te[0].media_type, SdpMediaType::Audio);

        assert_eq!(te[1].payload_type, 102);
        assert_eq!(te[1].clock_rate, 16000);
        assert!(te[1]
            .fmtp
            .is_none());

        // Still excluded from entries() and distinguishable from an unresolvable payload.
        let rtp = rtp_codec(codecs.entries());
        assert!(codec_named(&rtp, "telephone-event").is_none());
        assert_eq!(
            codecs
                .unmapped()
                .count(),
            0
        );
    }

    #[test]
    fn comfort_noise_from_static_table_has_no_rtpmap() {
        let sdp = format!("{}m=audio 5004 RTP/AVP 0 13\r\n", sdp_header());
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let cn: Vec<_> = codecs
            .non_codec_payloads()
            .collect();
        assert_eq!(cn.len(), 1);
        assert_eq!(cn[0].kind, NonCodecKind::ComfortNoise);
        assert_eq!(cn[0].payload_type, 13);
        assert_eq!(cn[0].clock_rate, 8000);
        assert!(cn[0]
            .fmtp
            .is_none());
        assert!(
            !cn[0].has_rtpmap,
            "PT 13 resolved from the static table, not declared by the peer"
        );

        let rtp = rtp_codec(codecs.entries());
        assert!(codec_named(&rtp, "CN").is_none());
        assert_eq!(
            codecs
                .unmapped()
                .count(),
            0
        );
    }

    #[test]
    fn comfort_noise_with_explicit_rtpmap_is_distinguishable() {
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 5004 RTP/AVP 0 13\r\n",
                "a=rtpmap:13 CN/8000\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let cn: Vec<_> = codecs
            .non_codec_payloads()
            .collect();
        assert_eq!(cn.len(), 1);
        assert!(cn[0].has_rtpmap);
    }

    #[test]
    fn non_codec_payloads_are_an_inventory_not_a_set() {
        // Two sections offering DTMF at one rate under different payload types: both
        // survive, in offer order. Collapsing them would lose which side offered what.
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 5004 RTP/AVP 0 101\r\n",
                "a=rtpmap:101 telephone-event/8000\r\n",
                "m=audio 5006 RTP/AVP 8 96\r\n",
                "a=rtpmap:96 telephone-event/8000\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let te: Vec<_> = codecs
            .non_codec_payloads()
            .collect();
        assert_eq!(te.len(), 2);
        assert_eq!(te[0].payload_type, 101);
        assert_eq!(te[1].payload_type, 96);
    }

    #[test]
    fn non_codec_payloads_never_reach_the_codec_string() {
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 5004 RTP/AVP 0 13 101\r\n",
                "a=rtpmap:101 telephone-event/8000\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(
            codecs
                .non_codec_payloads()
                .count(),
            2
        );
        let cs = codecs
            .audio_codec_string(&CodecStringOptions::default(), None)
            .unwrap();
        assert_eq!(cs.len(), 1, "only PCMU may reach the codec string");
        let rendered = cs.to_string();
        assert!(!rendered.contains("telephone-event"));
        assert!(!rendered.contains("CN"));
    }

    // --- unmapped payload types ---

    #[test]
    fn dynamic_pt_without_rtpmap_is_unmapped_not_error() {
        let sdp = format!("{}m=audio 5004 RTP/AVP 0 99\r\n", sdp_header());
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let unmapped: Vec<_> = codecs
            .unmapped()
            .collect();
        assert_eq!(unmapped.len(), 1);
        assert_eq!(unmapped[0].payload_type, 99);
        // PT 0 (PCMU) is still there; PT 99 is unmapped, not silently dropped
        let rtp = rtp_codec(codecs.entries());
        assert_eq!(rtp.len(), 1);
        assert!(codec_named(&rtp, "PCMU").is_some());
    }

    // --- audio_codec_string / video_codec_string / fmtp_for ---

    #[test]
    fn amr_oa_be_pair_collapses_after_dedup() {
        // Both AMR/8000 registrations differ only in fmtp; default options don't emit
        // fmtp, so the two entries are identical and only dedup() tells them apart.
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 5004 RTP/AVP 96 97\r\n",
                "a=rtpmap:96 AMR/8000\r\n",
                "a=fmtp:96 octet-align=1\r\n",
                "a=rtpmap:97 AMR/8000\r\n",
                "a=fmtp:97 octet-align=0\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let cs = codecs
            .audio_codec_string(&CodecStringOptions::default(), None)
            .unwrap();
        assert_eq!(cs.len(), 2, "both AMR entries must be emitted");
        assert_eq!(
            cs.entries()[0],
            cs.entries()[1],
            "entries must be identical under default options (no fmtp)"
        );
        let mut deduped = cs.clone();
        deduped.dedup();
        assert_eq!(
            deduped.len(),
            1,
            "duplicate AMR entries must collapse under dedup()"
        );
    }

    #[test]
    fn fmtp_for_falls_through_to_later_codec_with_fmtp() {
        // Matches the AMR octet-aligned / bandwidth-efficient pair's shape: the
        // first payload matching name+rate has no fmtp, the second does. The
        // second's fmtp must not be shadowed by the first's absence.
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 5004 RTP/AVP 97 96\r\n",
                "a=rtpmap:97 AMR/8000\r\n",
                "a=rtpmap:96 AMR/8000\r\n",
                "a=fmtp:96 octet-align=1\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(
            codecs.fmtp_for("AMR", 8000),
            Some("octet-align=1"),
            "fmtp_for must fall through a fmtp-less match to a later one that has fmtp"
        );
    }

    #[test]
    fn amrwb_absent_and_explicit_channel_collapse_after_dedup() {
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 5004 RTP/AVP 100 101\r\n",
                "a=rtpmap:100 AMR-WB/16000\r\n",
                "a=rtpmap:101 AMR-WB/16000/1\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let cs = codecs
            .audio_codec_string(&CodecStringOptions::default(), None)
            .unwrap();
        assert_eq!(cs.len(), 2);
        let mut deduped = cs.clone();
        deduped.dedup();
        assert_eq!(
            deduped.len(),
            1,
            "absent and explicit /1 channel count must normalize identically"
        );
    }

    #[test]
    fn evs_dotted_fmtp_unreachable_under_default_options() {
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 5004 RTP/AVP 110\r\n",
                "a=rtpmap:110 EVS/16000\r\n",
                "a=fmtp:110 br=13.2-24.4;bw=nb-swb\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let cs = codecs
            .audio_codec_string(&CodecStringOptions::default(), None)
            .unwrap();
        assert_eq!(cs.len(), 1);
        assert!(
            cs.entries()[0]
                .fmtp()
                .is_none(),
            "fmtp emission is off by default; the dotted fmtp path is unreachable"
        );
    }

    #[test]
    fn evs_dotted_fmtp_lenient_clears_and_warns() {
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 5004 RTP/AVP 110\r\n",
                "a=rtpmap:110 EVS/16000\r\n",
                "a=fmtp:110 br=13.2-24.4;bw=nb-swb\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let opts = CodecStringOptions::default().with_fmtp(true);
        let mut warnings = Vec::new();
        let cs = codecs
            .audio_codec_string(&opts, Some(&mut warnings))
            .unwrap();
        assert_eq!(cs.len(), 1, "the codec must still be emitted");
        assert!(
            cs.entries()[0]
                .fmtp()
                .is_none(),
            "unrepresentable fmtp must be cleared, not left dangling"
        );
        assert_eq!(warnings.len(), 1);
        assert!(matches!(
            warnings[0],
            SdpWarning::FmtpUnrepresentable { .. }
        ));
    }

    #[test]
    fn evs_dotted_fmtp_strict_is_err() {
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 5004 RTP/AVP 110\r\n",
                "a=rtpmap:110 EVS/16000\r\n",
                "a=fmtp:110 br=13.2-24.4;bw=nb-swb\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let opts = CodecStringOptions::default().with_fmtp(true);
        let result = codecs.audio_codec_string(&opts, None);
        assert!(
            result.is_err(),
            "strict mode must fail on an unrepresentable fmtp"
        );
    }

    #[test]
    fn g722_rate_is_8000_never_16000() {
        let sdp = format!(
            "{}m=audio 5004 RTP/AVP 9\r\na=rtpmap:9 G722/8000\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        let cs = codecs
            .audio_codec_string(&CodecStringOptions::default(), None)
            .unwrap();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs.entries()[0].name(), "G722");
        assert_eq!(cs.entries()[0].rate(), Some(8000));
        assert_eq!(cs.entries()[0].ptime(), Some(20));
    }

    #[test]
    fn video_codec_string_carries_no_rate_qualifier() {
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 5004 RTP/AVP 0\r\n",
                "m=video 5006 RTP/AVP 96\r\n",
                "a=rtpmap:96 H264/90000\r\n",
                "m=image 5008 udptl t38\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();

        let video_cs = codecs
            .video_codec_string(&CodecStringOptions::video(), None)
            .unwrap();
        assert_eq!(video_cs.len(), 1);
        assert_eq!(video_cs.entries()[0].name(), "H264");
        assert!(
            video_cs.entries()[0]
                .rate()
                .is_none(),
            "CodecStringOptions::video() must not emit a rate qualifier"
        );

        let audio_cs = codecs
            .audio_codec_string(&CodecStringOptions::default(), None)
            .unwrap();
        assert!(
            audio_cs
                .entries()
                .iter()
                .any(|e| e.name() == "t38"),
            "an offered m=image section must append a literal t38 entry to the audio string"
        );
    }

    #[test]
    fn codec_name_with_delimiter_is_skipped_lenient_and_erred_strict() {
        // A comma in the a=rtpmap encoding name cannot be represented as a codec-string
        // entry at all — distinct from UnmappedPayload (no rtpmap name whatsoever).
        let sdp = format!(
            "{}m=audio 5004 RTP/AVP 100\r\na=rtpmap:100 bad,name/8000\r\n",
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();

        let mut warnings = Vec::new();
        let cs = codecs
            .audio_codec_string(&CodecStringOptions::default(), Some(&mut warnings))
            .unwrap();
        assert!(cs.is_empty(), "the unrepresentable codec must be skipped");
        assert_eq!(warnings.len(), 1);
        assert!(matches!(
            warnings[0],
            SdpWarning::CodecNameUnrepresentable { .. }
        ));

        let result = codecs.audio_codec_string(&CodecStringOptions::default(), None);
        assert!(result.is_err(), "strict mode must fail");
    }

    // --- fmtp_for ---

    #[test]
    fn fmtp_for_looks_up_by_name_and_rate_case_insensitively() {
        let sdp = format!(
            concat!(
                "{}",
                "m=audio 5004 RTP/AVP 100\r\n",
                "a=rtpmap:100 AMR-WB/16000\r\n",
                "a=fmtp:100 octet-align=1\r\n"
            ),
            sdp_header()
        );
        let codecs = SdpCodecs::parse(&sdp).unwrap();
        assert_eq!(
            codecs.fmtp_for("amr-wb", 16000),
            Some("octet-align=1"),
            "lookup must be case-insensitive on the codec name"
        );
        assert_eq!(
            codecs.fmtp_for("AMR-WB", 8000),
            None,
            "clock rate must also match"
        );
        assert_eq!(
            codecs.fmtp_for("PCMU", 8000),
            None,
            "codec not in the offer"
        );
    }
}
