//! Error and warning types for SDP codec parsing and codec-string construction.
//!
//! Line numbers in this module index FreeSWITCH `v1.11.1`
//! (`c2c59645f6911a76589e5008c4d73349ded44b65`).

use crate::sdp::codec::SdpMediaType;
use std::fmt;

/// Failures parsing an SDP payload or converting it to a codec string.
#[derive(Debug)]
#[non_exhaustive]
pub enum SdpCodecError {
    /// The `sdp-types` crate could not parse the SDP blob.
    ///
    /// The underlying parse error is accessible via [`std::error::Error::source`].
    /// The `sdp-types` type is not exposed in any public signature.
    ParseFailure {
        /// Human-readable description of what failed.
        message: String,
        /// The underlying parse error, preserved for `Error::source()`.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// An `a=rtpmap` line was malformed (wrong number of fields, non-numeric payload
    /// type, non-numeric clock rate, etc.).
    MalformedRtpmap(String),
    /// A payload type in the `m=` format list could not be parsed as a number.
    NonNumericPayloadType(String),
    /// An `a=fmtp` line was malformed (non-numeric payload type).
    ///
    /// A non-numeric payload type in `a=fmtp` is the same structural breakage as
    /// in `a=rtpmap` — the line is uninterpretable and cannot be recovered from.
    MalformedFmtp(String),
}

impl SdpCodecError {
    /// Construct a [`SdpCodecError::ParseFailure`] from a message and a source error.
    ///
    /// The source error is boxed so the `sdp-types` concrete type does not appear
    /// in any public signature.
    pub(crate) fn parse_failure(
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::ParseFailure {
            message: message.into(),
            source: Box::new(source),
        }
    }
}

impl fmt::Display for SdpCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParseFailure { message, .. } => write!(f, "SDP parse failure: {message}"),
            Self::MalformedRtpmap(raw) => {
                write!(f, "malformed a=rtpmap ({} bytes)", raw.len())
            }
            Self::NonNumericPayloadType(raw) => {
                write!(
                    f,
                    "non-numeric payload type in m= format list ({} bytes)",
                    raw.len()
                )
            }
            Self::MalformedFmtp(raw) => {
                write!(f, "malformed a=fmtp ({} bytes)", raw.len())
            }
        }
    }
}

impl std::error::Error for SdpCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ParseFailure { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

/// Failures building or parsing a FreeSWITCH codec-string entry.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CodecStringError {
    /// A format-parameter value contains `@`.
    ///
    /// The `@` split precedes the `~` split in FreeSWITCH's parser, so an `@`
    /// inside fmtp is read as a qualifier rather than fmtp content — silently
    /// producing a wrong codec entry.
    AtInFmtp(String),
    /// A format-parameter value contains `.` with no module prefix.
    ///
    /// The `.` split also precedes the `~` split, so the first dot becomes the
    /// module-name boundary and the mangled name misses the hash — the codec is
    /// dropped with no log.
    DotInFmtpWithoutModule(String),
    /// The codec name is empty or otherwise invalid.
    InvalidCodecName(String),
    /// A codec name or field contains `\n` or `\r`.
    ///
    /// ESL is a text protocol terminated by `\n\n`; any user-provided string
    /// reaching the wire without validation can inject arbitrary ESL commands.
    WireInjection {
        /// Name of the field that contained the invalid character.
        field: String,
        /// The value that triggered the error.
        value: String,
    },
    /// A codec-string qualifier value could not be parsed.
    ///
    /// In strict mode (the [`FromStr`](std::str::FromStr) impl), a qualifier
    /// that overflows `u32` or carries no recognised letter (`h`/`k`/`i`/`b`/`c`)
    /// is a hard error: the data would otherwise be silently lost. In lenient mode
    /// ([`CodecString::parse_lenient`](super::codec_string::CodecString::parse_lenient)) the same condition records a
    /// [`SdpWarning::CodecStringQualifier`] and continues.
    QualifierParseError {
        /// The raw qualifier part that could not be parsed (e.g. `"9999999999h"`).
        raw: String,
        /// Why parsing failed.
        reason: String,
    },
    /// A codec name or module name contains a character that is a grammar delimiter.
    ///
    /// Names and module names are emitted unescaped in the codec-string `Display`
    /// output. A delimiter character (`,` `@` `~` `.` `'` `\` or ASCII whitespace)
    /// in a name would corrupt the grammar when the string is re-parsed, breaking
    /// the round-trip invariant. The caller must not use such characters in codec
    /// names — no real FreeSWITCH codec name contains any of them.
    InvalidCharInName {
        /// Name of the field (`"name"` or `"modname"`).
        field: String,
        /// The character that triggered the error.
        ch: char,
        /// The full value that contained the invalid character.
        value: String,
    },
    /// The input contains multiple comma-separated entries where a single entry was expected.
    ///
    /// [`FromStr`](std::str::FromStr) for [`CodecStringEntry`](super::codec_string::CodecStringEntry)
    /// requires exactly one entry. A top-level (unescaped, unquoted) comma signals
    /// multiple entries — a silent take-first would hide the caller's mistake.
    MultipleEntries(String),
    /// More entries than FreeSWITCH parses from one codec string.
    ///
    /// `switch_separate_string` stops at `SWITCH_MAX_CODECS`
    /// (`switch_types.h:595`) and the entries past it are lost with no log and no
    /// return-value signal, so a string built past the cap negotiates something
    /// other than what it says.
    TooManyEntries {
        /// How many entries the operation would have produced.
        entries: usize,
        /// The cap, [`CodecString::MAX_SWITCH_ENTRIES`](super::codec_string::CodecString::MAX_SWITCH_ENTRIES).
        limit: usize,
    },
}

impl CodecStringError {
    /// Construct an [`AtInFmtp`](CodecStringError::AtInFmtp) error.
    pub(crate) fn at_in_fmtp(value: impl Into<String>) -> Self {
        Self::AtInFmtp(value.into())
    }

    /// Construct a [`DotInFmtpWithoutModule`](CodecStringError::DotInFmtpWithoutModule) error.
    pub(crate) fn dot_in_fmtp_without_module(value: impl Into<String>) -> Self {
        Self::DotInFmtpWithoutModule(value.into())
    }

    /// Construct an [`InvalidCodecName`](CodecStringError::InvalidCodecName) error.
    pub(crate) fn invalid_codec_name(value: impl Into<String>) -> Self {
        Self::InvalidCodecName(value.into())
    }

    /// Construct a [`WireInjection`](CodecStringError::WireInjection) error.
    pub(crate) fn wire_injection(field: impl Into<String>, value: impl Into<String>) -> Self {
        Self::WireInjection {
            field: field.into(),
            value: value.into(),
        }
    }

    /// Construct a [`QualifierParseError`](CodecStringError::QualifierParseError) error.
    pub(crate) fn qualifier_parse_error(raw: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::QualifierParseError {
            raw: raw.into(),
            reason: reason.into(),
        }
    }

    /// Construct an [`InvalidCharInName`](CodecStringError::InvalidCharInName) error.
    pub(crate) fn invalid_char_in_name(
        field: impl Into<String>,
        ch: char,
        value: impl Into<String>,
    ) -> Self {
        Self::InvalidCharInName {
            field: field.into(),
            ch,
            value: value.into(),
        }
    }

    /// Construct a [`MultipleEntries`](CodecStringError::MultipleEntries) error.
    pub(crate) fn multiple_entries(value: impl Into<String>) -> Self {
        Self::MultipleEntries(value.into())
    }

    /// Construct a [`TooManyEntries`](CodecStringError::TooManyEntries) error.
    pub(crate) fn too_many_entries(entries: usize, limit: usize) -> Self {
        Self::TooManyEntries { entries, limit }
    }
}

impl fmt::Display for CodecStringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AtInFmtp(v) => write!(
                f,
                "format-parameter value {v:?} contains '@': the '@' split precedes the '~' \
                 split, so it would be read as a qualifier rather than fmtp content"
            ),
            Self::DotInFmtpWithoutModule(v) => write!(
                f,
                "format-parameter value {v:?} contains '.' with no module prefix: the first \
                 dot becomes the module-name boundary and the codec would be silently dropped"
            ),
            Self::InvalidCodecName(v) => write!(f, "invalid or empty codec name: {v:?}"),
            Self::WireInjection { field, value } => write!(
                f,
                "field {field:?} contains '\\n' or '\\r' in value {value:?}: \
                 newline/CR in ESL wire data can inject arbitrary commands"
            ),
            Self::QualifierParseError { raw, reason } => write!(
                f,
                "codec-string qualifier {raw:?} could not be parsed: {reason}"
            ),
            Self::InvalidCharInName { field, ch, value } => write!(
                f,
                "field {field:?} value {value:?} contains {ch:?}: names and module names \
                 are emitted unescaped, so grammar delimiters (`,`, `@`, `~`, `.`, `'`, \
                 `\\`, whitespace) would corrupt the codec-string on re-parse"
            ),
            Self::MultipleEntries(v) => write!(
                f,
                "input {v:?} contains multiple comma-separated entries where a single \
                 CodecStringEntry was expected"
            ),
            Self::TooManyEntries { entries, limit } => write!(
                f,
                "{entries} codec-string entries exceeds the {limit} FreeSWITCH parses; \
                 the rest would be dropped with no log"
            ),
        }
    }
}

impl std::error::Error for CodecStringError {}

/// A payload type present in an `m=` format list that cannot be named.
///
/// This occurs when a payload type has no `a=rtpmap` and is not one of the
/// RFC 3551 static types (PT 0–34). Dropping it silently would be
/// indistinguishable from the peer never having offered it, so it is surfaced
/// as data rather than an error.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnmappedPayload {
    /// The payload type number from the `m=` format list.
    pub payload_type: u8,
    /// The media type of the section where this payload type appeared.
    pub media_type: SdpMediaType,
}

impl UnmappedPayload {
    /// Records a dynamic payload type that has no `a=rtpmap` and no static-table entry.
    pub(crate) fn new(payload_type: u8, media_type: SdpMediaType) -> Self {
        Self {
            payload_type,
            media_type,
        }
    }
}

impl fmt::Display for UnmappedPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "payload type {} in {} section has no rtpmap and is not a static type",
            self.payload_type, self.media_type
        )
    }
}

/// A recoverable oddity recorded during SDP parsing rather than failed on.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SdpWarning {
    /// A numeric SDP attribute could not be parsed as a positive integer.
    ///
    /// FreeSWITCH's `atoi` accepts `"abc"` as 0 and treats 0 as "not set".
    /// A strict parser records this as a warning rather than hard-failing so
    /// the session is not dropped over a cosmetic attribute.
    UnparseableNumericAttribute {
        /// The attribute name, e.g. `"ptime"` or `"maxptime"`.
        attribute: String,
        /// The raw on-wire value that could not be parsed.
        raw_value: String,
    },
    /// A codec-string qualifier value was skipped during lenient parsing.
    ///
    /// Produced by [`CodecString::parse_lenient`](super::codec_string::CodecString::parse_lenient) when a qualifier overflows `u32`
    /// or carries no recognised letter. The qualifier is omitted from the parsed entry.
    /// In strict mode ([`FromStr`](std::str::FromStr)) the same condition is a hard error.
    CodecStringQualifier {
        /// The raw qualifier part that could not be parsed (e.g. `"9999999999h"`).
        raw: String,
        /// Why parsing failed.
        reason: String,
    },
    /// A codec's fmtp could not be embedded in the codec-string entry and was cleared.
    ///
    /// Occurs when the offer's `a=fmtp` value contains characters that are unrepresentable
    /// in the codec-string format (e.g. a dotted fmtp like `br=13.2-24.4` against a bare
    /// policy entry that has no module prefix). The codec entry is still emitted; only the
    /// fmtp qualifier is absent. The original fmtp is preserved here for inspection.
    FmtpUnrepresentable {
        /// The codec name for which fmtp could not be embedded.
        codec_name: String,
        /// The fmtp value that could not be represented.
        fmtp: String,
        /// Human-readable reason the fmtp could not be embedded.
        reason: String,
    },
    /// An offered codec's `a=rtpmap` encoding name cannot be represented as a
    /// codec-string entry at all, and the codec was skipped.
    ///
    /// Occurs when the name itself contains a codec-string grammar delimiter
    /// (`,` `@` `~` `.` `'` `\` or ASCII whitespace). Unlike
    /// [`FmtpUnrepresentable`](SdpWarning::FmtpUnrepresentable), there is no entry to fall
    /// back to — the whole codec is dropped, narrowing the emitted list. Distinct from
    /// [`UnmappedPayload`]: that payload type had no rtpmap name whatsoever, whereas this
    /// one has a name that simply cannot survive the codec-string grammar.
    CodecNameUnrepresentable {
        /// The raw, unrepresentable encoding name from `a=rtpmap`.
        codec_name: String,
        /// Human-readable reason the name could not be embedded.
        reason: String,
    },
    /// A single `m=` section was structurally broken and was skipped entirely.
    ///
    /// Occurs when an `a=rtpmap`, `a=fmtp`, or `m=` format-list entry in one media
    /// section cannot be parsed at all (as opposed to [`CodecNameUnrepresentable`](SdpWarning::CodecNameUnrepresentable),
    /// which drops a single codec). Every other section's codecs are still parsed
    /// and returned.
    MalformedMediaSection {
        /// The media type of the skipped section (e.g. `"audio"`).
        media_type: String,
        /// Human-readable reason the section could not be parsed.
        reason: String,
    },
    /// A lenient parse read more comma-separated slots than FreeSWITCH would.
    ///
    /// Every entry parsed is returned; the switch itself keeps only the first
    /// `limit` slots (`switch_types.h:595`), empty ones included, and loses the
    /// rest with no log.
    CodecStringTruncated {
        /// How many comma-separated slots the input carried.
        entries: usize,
        /// The cap, [`CodecString::MAX_SWITCH_ENTRIES`](super::codec_string::CodecString::MAX_SWITCH_ENTRIES).
        limit: usize,
    },
    /// A value-less attribute names a direction in a case RFC 8866 does not accept.
    ///
    /// It is not read as one, so the section keeps the direction of the level above
    /// it — which is what the peer wrote only by coincidence.
    NonCanonicalDirectionAttribute {
        /// The attribute name as the peer spelled it.
        attribute: String,
    },
    /// FreeSWITCH's SDP parser refuses this offer, so the switch derives no codecs from it.
    ///
    /// Only the first refusal is reported: the switch's parse ends there.
    SwitchRejects {
        /// What the switch's parser refuses.
        reason: SwitchRejection,
        /// The byte offset of the line it refuses at, `None` for a line missing from the offer.
        offset: Option<usize>,
    },
    /// The switch's SDP parser ends at this byte, so nothing from here on reached it.
    ///
    /// It ends at a line of at most one character after leading spaces and tabs, or at a NUL;
    /// `&sdp[..offset]` is what it read, and parsing that prefix gives the switch's view.
    SwitchStopsReading {
        /// The byte offset where the switch's parse ends.
        offset: usize,
    },
    /// The switch drops this `a=rtpmap` or `a=fmtp`: its payload type is unreadable.
    PayloadAttributeIgnored {
        /// The byte offset of the attribute's line.
        offset: usize,
    },
}

/// Why FreeSWITCH's SDP parser refuses an offer, as sofia-sip's `sdp_parse` decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SwitchRejection {
    /// A session-level line that is not key=value or has a key longer than one character.
    BadLine,
    /// A session-level key the parser does not know.
    UnknownField,
    /// `r=` with no earlier `t=`.
    RepeatWithoutTime,
    /// `z=` with no `t=` since the last `z=`.
    ZoneWithoutTime,
    /// An `a=` line whose name has no token characters.
    InvalidAttributeName,
    /// An `m=` line whose media, port or proto cannot be read.
    InvalidMediaLine,
    /// An RTP format that is not a number below 128.
    InvalidRtpFormat,
    /// An `a=rtpmap` with no readable clock rate.
    InvalidRtpmap,
    /// No `o=` line.
    MissingOrigin,
    /// A section with a nonzero port and no `c=` on it or at session level.
    MissingConnection,
}

impl fmt::Display for SwitchRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::BadLine => "a session-level line is not key=value with a one-character key",
            Self::UnknownField => "a session-level line has a key the parser does not know",
            Self::RepeatWithoutTime => "r= comes before any t=",
            Self::ZoneWithoutTime => "z= has no t= since the last z=",
            Self::InvalidAttributeName => "an a= line has no attribute name",
            Self::InvalidMediaLine => "an m= line's media, port or proto cannot be read",
            Self::InvalidRtpFormat => "an RTP format is not a number below 128",
            Self::InvalidRtpmap => "an a=rtpmap has no readable clock rate",
            Self::MissingOrigin => "there is no o= line",
            Self::MissingConnection => "a live section has no c= on it or at session level",
        })
    }
}

impl SdpWarning {
    /// Construct an [`UnparseableNumericAttribute`](SdpWarning::UnparseableNumericAttribute) warning.
    pub(crate) fn unparseable_numeric_attribute(
        attribute: impl Into<String>,
        raw_value: impl Into<String>,
    ) -> Self {
        Self::UnparseableNumericAttribute {
            attribute: attribute.into(),
            raw_value: raw_value.into(),
        }
    }

    /// Construct a [`CodecStringQualifier`](SdpWarning::CodecStringQualifier) warning.
    pub(crate) fn codec_string_qualifier(
        raw: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self::CodecStringQualifier {
            raw: raw.into(),
            reason: reason.into(),
        }
    }

    /// Construct an [`FmtpUnrepresentable`](SdpWarning::FmtpUnrepresentable) warning.
    pub(crate) fn fmtp_unrepresentable(
        codec_name: impl Into<String>,
        fmtp: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self::FmtpUnrepresentable {
            codec_name: codec_name.into(),
            fmtp: fmtp.into(),
            reason: reason.into(),
        }
    }

    /// Construct a [`CodecNameUnrepresentable`](SdpWarning::CodecNameUnrepresentable) warning.
    pub(crate) fn codec_name_unrepresentable(
        codec_name: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self::CodecNameUnrepresentable {
            codec_name: codec_name.into(),
            reason: reason.into(),
        }
    }

    /// Construct a [`NonCanonicalDirectionAttribute`](SdpWarning::NonCanonicalDirectionAttribute) warning.
    pub(crate) fn non_canonical_direction_attribute(attribute: impl Into<String>) -> Self {
        Self::NonCanonicalDirectionAttribute {
            attribute: attribute.into(),
        }
    }

    /// Construct a [`CodecStringTruncated`](SdpWarning::CodecStringTruncated) warning.
    pub(crate) fn codec_string_truncated(entries: usize, limit: usize) -> Self {
        Self::CodecStringTruncated { entries, limit }
    }

    /// Construct a [`MalformedMediaSection`](SdpWarning::MalformedMediaSection) warning.
    pub(crate) fn malformed_media_section(
        media_type: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self::MalformedMediaSection {
            media_type: media_type.into(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for SdpWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnparseableNumericAttribute {
                attribute,
                raw_value,
            } => write!(
                f,
                "a={attribute} value ({} bytes) is not a valid positive integer; \
                 treated as not set",
                raw_value.len()
            ),
            Self::CodecStringQualifier { raw, reason } => write!(
                f,
                "codec-string qualifier {raw:?} skipped during lenient parse: {reason}"
            ),
            Self::FmtpUnrepresentable {
                codec_name,
                fmtp,
                reason,
            } => write!(
                f,
                "the a=fmtp ({} bytes) of an offered codec ({} bytes) could not be embedded \
                 in the codec-string entry and was cleared: it {reason}",
                fmtp.len(),
                codec_name.len()
            ),
            Self::CodecNameUnrepresentable { codec_name, reason } => write!(
                f,
                "an offered codec name ({} bytes) cannot be a codec-string entry and was \
                 skipped: it {reason}",
                codec_name.len()
            ),
            Self::MalformedMediaSection { media_type, reason } => write!(
                f,
                "{media_type} section could not be parsed and was skipped entirely: {reason}"
            ),
            Self::NonCanonicalDirectionAttribute { attribute } => write!(
                f,
                "direction attribute ({} bytes) is not spelled as RFC 8866 defines it \
                 and was not read as a direction",
                attribute.len()
            ),
            Self::CodecStringTruncated { entries, limit } => write!(
                f,
                "codec string carries {entries} comma-separated slots; FreeSWITCH keeps \
                 the first {limit} and drops the rest with no log"
            ),
            Self::SwitchRejects {
                reason,
                offset: Some(offset),
            } => write!(f, "FreeSWITCH refuses this SDP at byte {offset}: {reason}"),
            Self::SwitchRejects {
                reason,
                offset: None,
            } => write!(f, "FreeSWITCH refuses this SDP: {reason}"),
            Self::SwitchStopsReading { offset } => write!(
                f,
                "FreeSWITCH stops reading this SDP at byte {offset}; nothing after it \
                 reached the switch"
            ),
            Self::PayloadAttributeIgnored { offset } => write!(
                f,
                "the payload attribute at byte {offset} has an unreadable payload type; \
                 FreeSWITCH drops it"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdp::codec::SdpMediaType;

    // --- SdpCodecError ---

    #[test]
    fn sdp_codec_error_parse_failure_display() {
        use std::fmt;

        #[derive(Debug)]
        struct DummyErr;
        impl fmt::Display for DummyErr {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("no version line")
            }
        }
        impl std::error::Error for DummyErr {}

        let e = SdpCodecError::parse_failure("missing v= line", DummyErr);
        let msg = e.to_string();
        assert!(msg.contains("SDP parse failure"));
        assert!(msg.contains("missing v= line"));
    }

    #[test]
    fn sdp_codec_error_source_preserved() {
        use std::fmt;

        #[derive(Debug)]
        struct Root;
        impl fmt::Display for Root {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("root cause")
            }
        }
        impl std::error::Error for Root {}

        let e = SdpCodecError::parse_failure("wrapper", Root);
        let src = std::error::Error::source(&e).expect("source must be present");
        assert_eq!(src.to_string(), "root cause");
    }

    #[test]
    fn sdp_codec_error_malformed_rtpmap_display() {
        let e = SdpCodecError::MalformedRtpmap("opus/abc/2".into());
        let msg = e.to_string();
        assert!(msg.contains("malformed a=rtpmap"));
        assert!(!msg.contains("opus/abc/2"), "error quoted its input: {msg}");
        assert!(msg.contains("10 bytes"), "{msg}");
        let SdpCodecError::MalformedRtpmap(raw) = &e else {
            panic!("wrong variant");
        };
        assert_eq!(raw, "opus/abc/2");
    }

    #[test]
    fn sdp_codec_error_non_numeric_pt_display() {
        let e = SdpCodecError::NonNumericPayloadType("abc".into());
        let msg = e.to_string();
        assert!(msg.contains("non-numeric payload type"));
        assert!(!msg.contains("abc"), "error quoted its input: {msg}");
        assert!(msg.contains("3 bytes"), "{msg}");
    }

    #[test]
    fn sdp_codec_error_other_variants_have_no_source() {
        let e = SdpCodecError::MalformedRtpmap("x".into());
        assert!(std::error::Error::source(&e).is_none());
    }

    #[test]
    fn sdp_codec_error_malformed_fmtp_display() {
        let e = SdpCodecError::MalformedFmtp("x minptime=10".into());
        let msg = e.to_string();
        assert!(msg.contains("malformed a=fmtp"));
        assert!(!msg.contains("minptime"), "error quoted its input: {msg}");
        assert!(msg.contains("13 bytes"), "{msg}");
    }

    // --- CodecStringError ---

    #[test]
    fn codec_string_error_at_in_fmtp() {
        let e = CodecStringError::at_in_fmtp("br=13.2@24.4");
        let msg = e.to_string();
        assert!(msg.contains('@'));
        assert!(msg.contains("qualifier"));
    }

    #[test]
    fn codec_string_error_dot_in_fmtp_without_module() {
        let e = CodecStringError::dot_in_fmtp_without_module("br=13.2-24.4");
        let msg = e.to_string();
        assert!(msg.contains('.'));
        assert!(msg.contains("silently dropped"));
    }

    #[test]
    fn codec_string_error_invalid_codec_name() {
        let e = CodecStringError::invalid_codec_name("");
        let msg = e.to_string();
        assert!(msg.contains("invalid or empty codec name"));
    }

    #[test]
    fn codec_string_error_wire_injection() {
        let e = CodecStringError::wire_injection("name", "bad\ncodec");
        let msg = e.to_string();
        assert!(msg.contains("\\n") || msg.contains("newline"));
        assert!(msg.contains("name"));
    }

    #[test]
    fn codec_string_error_equality() {
        assert_eq!(
            CodecStringError::at_in_fmtp("x"),
            CodecStringError::at_in_fmtp("x")
        );
        assert_ne!(
            CodecStringError::at_in_fmtp("x"),
            CodecStringError::at_in_fmtp("y")
        );
    }

    // --- UnmappedPayload ---

    #[test]
    fn unmapped_payload_accessors() {
        let u = UnmappedPayload::new(99, SdpMediaType::Audio);
        assert_eq!(u.payload_type, 99);
        assert_eq!(u.media_type, SdpMediaType::Audio);
    }

    #[test]
    fn unmapped_payload_display() {
        let u = UnmappedPayload::new(99, SdpMediaType::Video);
        let msg = u.to_string();
        assert!(msg.contains("99"));
        assert!(msg.contains("video") || msg.contains("Video"));
    }

    // --- SdpWarning ---

    #[test]
    fn sdp_warning_unparseable_numeric_attribute() {
        let w = SdpWarning::unparseable_numeric_attribute("ptime", "abc");
        match &w {
            SdpWarning::UnparseableNumericAttribute {
                attribute,
                raw_value,
            } => {
                assert_eq!(attribute, "ptime");
                assert_eq!(raw_value, "abc");
            }
            #[allow(unreachable_patterns)]
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn sdp_warning_display_names_fields_and_lengths_not_peer_values() {
        let cases: &[(SdpWarning, &str, &str)] = &[
            (
                SdpWarning::unparseable_numeric_attribute("maxptime", "abcdef"),
                "maxptime",
                "abcdef",
            ),
            (
                SdpWarning::fmtp_unrepresentable("EVS", "br=13.2-24.4", "contains a dot"),
                "12 bytes",
                "br=13.2-24.4",
            ),
            (
                SdpWarning::codec_name_unrepresentable("bad,name", "contains a delimiter"),
                "8 bytes",
                "bad,name",
            ),
            (
                SdpWarning::non_canonical_direction_attribute("SENDONLY"),
                "8 bytes",
                "SENDONLY",
            ),
        ];
        for (warning, expected, forbidden) in cases {
            let msg = warning.to_string();
            assert!(msg.contains(expected), "{msg}");
            assert!(!msg.contains(forbidden), "warning quoted its input: {msg}");
        }
    }

    #[test]
    fn switch_warnings_name_the_offset_and_the_reason() {
        let at = SdpWarning::SwitchRejects {
            reason: SwitchRejection::RepeatWithoutTime,
            offset: Some(42),
        }
        .to_string();
        assert!(at.contains("byte 42"), "{at}");
        assert!(
            at.contains(&SwitchRejection::RepeatWithoutTime.to_string()),
            "{at}"
        );
        let missing = SdpWarning::SwitchRejects {
            reason: SwitchRejection::MissingOrigin,
            offset: None,
        }
        .to_string();
        assert!(!missing.contains("byte"), "{missing}");
        let stop = SdpWarning::SwitchStopsReading { offset: 7 }.to_string();
        assert!(stop.contains("byte 7"), "{stop}");
    }

    #[test]
    fn sdp_warning_equality() {
        let a = SdpWarning::unparseable_numeric_attribute("ptime", "abc");
        let b = SdpWarning::unparseable_numeric_attribute("ptime", "abc");
        assert_eq!(a, b);
    }
}
