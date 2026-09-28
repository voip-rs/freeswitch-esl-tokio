// The esl_event_types! macro uses TT-munching filter arms over ~90 variants,
// requiring a higher recursion limit than the default 128.
#![recursion_limit = "512"]

//! FreeSWITCH protocol types: channel state, events, headers, commands, and variables.
//!
//! This crate provides the domain types for FreeSWITCH's Event Socket Library (ESL)
//! protocol without any async runtime dependency. Use it standalone for CDR parsing,
//! config generation, command building, or channel variable validation.
//!
//! For async ESL transport (connecting to FreeSWITCH, sending commands, receiving events),
//! see the [`freeswitch-esl-tokio`](https://docs.rs/freeswitch-esl-tokio) crate which
//! re-exports everything from this crate.
//!
//! # SIP headers
//!
//! Header names and the row trait come from
//! [`sip-header-catalog`](https://docs.rs/sip-header-catalog), re-exported at
//! the crate root. Every header store here ([`HeaderLookup`] implementors,
//! [`EslHeaders`] and the stores built on it) is a [`SipHeaderRows`] store, so
//! the typed accessors of [`sip-header`](https://docs.rs/sip-header) reach it
//! through that crate's blanket impl: depend on sip-header and
//! `use sip_header::prelude::*`. A raw channel-variable value decodes into a
//! [`SipHeaderField`] through `EslHeaders::decode_value`, or `decode_value_raw`
//! for a header by name, which the same accessors read. This crate names no
//! sip-header value type.

#[macro_use]
mod macros;

pub mod channel;
#[cfg(feature = "esl")]
pub mod commands;
#[cfg(feature = "esl")]
pub mod event;
pub mod headers;
pub mod log_level;
pub mod lookup;
#[cfg(feature = "esl")]
pub mod lossy_values;
pub mod prelude;
#[cfg(feature = "sdp")]
pub mod sdp;
pub mod sofia;
#[cfg(any(feature = "esl", feature = "sdp"))]
pub(crate) mod switch_passes;
#[cfg(all(test, feature = "esl"))]
mod test_text;
pub mod variables;
pub mod version;
#[doc(hidden)]
pub mod wire_safety;

/// Default FreeSWITCH ESL port for inbound connections.
pub const DEFAULT_ESL_PORT: u16 = 8021;

/// Default FreeSWITCH ESL password (`ClueCon`).
pub const DEFAULT_ESL_PASSWORD: &str = "ClueCon";

/// Header-name prefix a channel variable carries in an event.
///
/// Compose with [`VariableName::header_name()`](crate::VariableName::header_name)
/// rather than by hand.
pub const VARIABLE_PREFIX: &str = "variable_";

pub use channel::{
    channel_driver, AnswerState, CallDirection, CallState, ChannelState, ChannelTimetable,
    HangupCause, ParseAnswerStateError, ParseCallDirectionError, ParseCallStateError,
    ParseChannelStateError, ParseHangupCauseError, ParseTimetableError, TimetableField,
    TimetablePrefix,
};
#[cfg(feature = "esl")]
pub use commands::{
    Application, BlockParse, BridgeDialString, BridgeDialStringDisplay, DialString,
    DialStringCarrier, DialStringTarget, DialplanType, Endpoint, EndpointDisplay,
    EndpointFieldFault, ExecuteOn, ExecuteOnFault, GroupCallOrder, InvalidArgvSeparator, Originate,
    OriginateDisplay, OriginateError, OriginateTarget, ParseBlockParseError,
    ParseDialplanTypeError, ParseGroupCallOrderError, ParseHoldActionError, ParseMuteActionError,
    UnvouchedVersion, UuidAnswer, UuidBridge, UuidDeflect, UuidGetVar, UuidHold, UuidKill,
    UuidSendDtmf, UuidSetVar, UuidTransfer, Variables, VariablesDisplay, VariablesType,
};
#[cfg(feature = "esl")]
pub use event::{
    EslEvent, EslEventPriority, EslEventType, EventFormat, EventSubscription,
    EventSubscriptionError, ParseEventFormatError, ParseEventTypeError, ParsePriorityError,
};
pub use headers::{case_alias_key, normalize_header_key, EventHeader, ParseEventHeaderError};
pub use log_level::{LogLevel, ParseLogLevelError};
pub use lookup::{variable_key, HeaderLookup, ParseHeaderError};
#[cfg(feature = "esl")]
pub use lossy_values::{LossyValue, LossyValues};
pub use sip_header_catalog::{
    define_header_enum, ParseSipHeaderError, RowError, RowErrorKind, SipHeader, SipHeaderField,
    SipHeaderFields, SipHeaderRows, SipHeaderRowsExt,
};
pub use sofia::{
    GatewayPingStatus, GatewayRegState, ParseGatewayPingStatusError, ParseGatewayRegStateError,
    ParseSipUserPingStatusError, ParseSofiaEventSubclassError, SipUserPingStatus, SofiaChannelName,
    SofiaEventSubclass,
};
#[cfg(feature = "esl")]
pub use variables::EslHeaders;
pub use variables::{
    CarriedHeader, ChannelVariable, CoreMediaVariable, EslArray, EslArrayError, InvalidHeaderName,
    MultipartBody, MultipartBodyError, MultipartItem, ParseChannelVariableError,
    ParseCoreMediaVariableError, ParseSipPassthroughError, RtpStatUnit, SipHeaderPrefix,
    SipPassthroughHeader, VariableName, MAX_ARRAY_ITEMS,
};
pub use version::{FreeswitchVersion, ParseFreeswitchVersionError};

#[cfg(all(doctest, feature = "esl"))]
mod readme {
    #![doc = include_str!("../README.md")]
}
