//! Converts an SDP offer into a FreeSWITCH codec string and models the codec-string grammar.
//!
//! The codec-string grammar is documented in `docs/codec-string-format.md`. This module
//! provides the building blocks needed to derive a codec string from an SDP session
//! description: static payload type resolution, canonical FreeSWITCH encoding
//! name normalization, and static bitrate lookup.
//!
//! Line numbers in this module index FreeSWITCH `v1.11.1`
//! (`c2c59645f6911a76589e5008c4d73349ded44b65`).

/// Loaded-implementation modelling: which codec-string entries survive matching.
pub mod available;
/// SDP media types and codec descriptors.
pub mod codec;
/// FreeSWITCH codec-string grammar: typed construction and round-trip serialisation.
pub mod codec_string;
/// Collection of codecs parsed from a complete SDP session description.
pub mod codecs;
/// Error and warning types for SDP codec parsing and codec-string construction.
pub mod error;
pub(crate) mod num;
/// Options controlling which qualifiers are emitted when converting an SDP codec entry.
pub mod options;
pub(crate) mod session;
pub(crate) mod static_payload;

pub use available::CodecImplementation;
pub use codec::{
    NonCodecKind, NonCodecPayload, ParseSdpDirectionError, SdpCodec, SdpDirection, SdpMediaType,
};
pub use codec_string::{CodecString, CodecStringEntry};
pub use codecs::{SdpCodecEntry, SdpCodecs, SdpMediaSection, SdpParseOptions};
pub use error::{CodecStringError, SdpCodecError, SdpWarning, SwitchRejection, UnmappedPayload};
pub use options::CodecStringOptions;
pub use static_payload::{default_ptime, default_rate};
