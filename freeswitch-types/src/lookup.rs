//! Shared trait for typed header lookups from any key-value store.
//!
//! [`HeaderLookup`] provides convenience accessors (typed channel state,
//! call direction, timetable extraction, etc.) to any type that can look up
//! headers and variables by name. Implement the two required methods and get
//! everything else for free.
//!
//! `header_str` resolves a name in any casing unless it contains `_`, which
//! is compared exactly ([`case_alias_key`](crate::case_alias_key) states the
//! rule). The switch spells one header several ways and a consumer queries
//! in whatever casing its config holds, so an exact store misses silently.
//! Every implementor keeps that contract; a store of its own wraps
//! [`EslHeaders`](crate::EslHeaders), which does, rather than a bare map.
//!
//! Accessors live on a trait rather than on [`EslEvent`](crate::EslEvent)
//! because a consumer rarely keeps events: it accumulates headers from many of
//! them into a store of its own, which gets every accessor by implementing two
//! methods. Helpers that only read headers, such as
//! [`ChannelTimetable::from_lookup`], take a closure for the same reason.
//!
//! The supertrait is the catalog's [`SipHeaderRows`], so a consumer's own
//! sip-header types every store; this crate names no sip-header type. Where the
//! switch spells one field two ways (hyphen and underscore), each spelling is
//! its own [`EventHeader`] variant and an accessor unions them with
//! [`first_header`](HeaderLookup::first_header); the store never folds them,
//! which would alias every dash/underscore pair.

use crate::channel::{
    channel_driver, AnswerState, CallDirection, CallState, ChannelState, ChannelTimetable,
    HangupCause, HangupCauseValue, ParseAnswerStateError, ParseCallDirectionError,
    ParseCallStateError, ParseChannelStateError, ParseHangupCauseError, ParseTimetableError,
};
#[cfg(feature = "esl")]
use crate::event::{EslEventPriority, ParsePriorityError};
use crate::headers::{EventHeader, EventHeaderName};
use crate::sofia::{
    GatewayPingStatus, GatewayRegState, ParseGatewayPingStatusError, ParseGatewayRegStateError,
    ParseSipUserPingStatusError, ParseSofiaEventSubclassError, SipUserPingStatus,
    SofiaEventSubclass,
};
use crate::variables::{
    ChannelVariable, LoopbackLeg, LoopbackResignation, LoopbackVariable, ParseLoopbackLegError,
    ParseReadResultError, ParseSipHangupDispositionError, ReadResult, SipHangupDisposition,
    SofiaVariable, VariableName,
};
use sip_header_catalog::SipHeaderRows;
use std::str::FromStr;

/// Wire key of a channel variable: the [`VARIABLE_PREFIX`](crate::VARIABLE_PREFIX)
/// ahead of its bare name.
pub fn variable_key(name: &str) -> String {
    format!("{}{name}", crate::VARIABLE_PREFIX)
}

/// Parse an optional wire header value into a typed result.
///
/// Collapses the common `match self.header(...) { Some(s) => Ok(Some(s.parse()?)),
/// None => Ok(None) }` pattern used by many `HeaderLookup` accessors.
#[inline]
fn parse_opt<T: FromStr>(raw: Option<&str>) -> Result<Option<T>, T::Err> {
    raw.map(str::parse)
        .transpose()
}

parse_error! { ParseSipStatusCodeError("SIP status code"); }

fn parse_sip_status_code(raw: &str) -> Result<u16, ParseSipStatusCodeError> {
    match raw.as_bytes() {
        [b'1'..=b'9', b'0'..=b'9', b'0'..=b'9'] => raw
            .parse()
            .map_err(|_| ParseSipStatusCodeError(raw.to_string())),
        _ => Err(ParseSipStatusCodeError(raw.to_string())),
    }
}

/// Trait for looking up ESL headers and channel variables from any key-value store.
///
/// Implementors provide two methods -- `header_str(&str)` and `variable_str(&str)` --
/// and get all typed accessors (`channel_state()`, `call_direction()`, `timetable()`,
/// etc.) as default implementations.
///
/// `HeaderLookup: SipHeaderRows` — every `HeaderLookup` implementor is also a
/// [`SipHeaderRows`] store, which sip-header's blanket impl turns into the typed
/// RFC SIP accessors (`call_info()`, `history_info()`, `geolocation()`) for a
/// consumer that depends on sip-header. A store whose SIP headers sit in the
/// same flat namespace as its ESL headers returns each value as one row.
/// Stores with FreeSWITCH-specific transport encoding (ARRAY, `[bracket]`
/// wrapping) should use [`EslHeaders`](crate::EslHeaders), whose rows have
/// those quirks peeled.
///
/// This trait must be in scope to call its methods on `EslEvent` -- including
/// `unique_id()`, `hangup_cause()`, and `channel_state()`. Import it directly
/// or via the prelude:
///
/// ```ignore
/// use freeswitch_esl_tokio::prelude::*;
/// // or: use freeswitch_esl_tokio::HeaderLookup;
/// // or: use freeswitch_types::HeaderLookup;
/// ```
///
/// # Example
///
#[cfg_attr(feature = "esl", doc = "```")]
#[cfg_attr(not(feature = "esl"), doc = "```ignore")]
/// use freeswitch_types::{ChannelVariable, EslHeaders, EventHeader, HeaderLookup};
/// use freeswitch_types::{RowError, SipHeaderRows};
///
/// // A store of its own wraps EslHeaders, which keeps the case contract.
/// struct MyStore {
///     headers: EslHeaders,
/// }
///
/// // HeaderLookup has SipHeaderRows as a supertrait.
/// impl SipHeaderRows for MyStore {
///     fn sip_header_rows_str<'a>(&'a self, name: &str) -> Result<Vec<&'a str>, RowError> {
///         self.headers.sip_header_rows_str(name)
///     }
/// }
///
/// impl HeaderLookup for MyStore {
///     fn header_str(&self, name: &str) -> Option<&str> {
///         self.headers.header_str(name)
///     }
///     fn variable_str(&self, name: &str) -> Option<&str> {
///         self.headers.variable_str(name)
///     }
/// }
///
/// // Keys arrive in whatever casing the source spelled them.
/// let headers = [("channel-state", "CS_EXECUTE"), ("variable_read_codec", "PCMU")]
///     .into_iter()
///     .collect();
/// let store = MyStore { headers };
///
/// assert_eq!(store.header_str("CHANNEL-STATE"), Some("CS_EXECUTE"));
///
/// // Typed accessor from the trait (returns Result<Option<T>, E>):
/// assert!(store.channel_state().unwrap().is_some());
///
/// // Enum-based lookups:
/// assert_eq!(store.header(EventHeader::ChannelState), Some("CS_EXECUTE"));
/// assert_eq!(store.variable(ChannelVariable::ReadCodec), Some("PCMU"));
/// ```
pub trait HeaderLookup: SipHeaderRows {
    /// Look up a header by its raw wire name (e.g. `"Unique-ID"`).
    fn header_str(&self, name: &str) -> Option<&str>;

    /// Look up a channel variable by its bare name (e.g. `"sip_call_id"`).
    ///
    /// Implementations typically prepend `variable_` and delegate to `header_str`.
    fn variable_str(&self, name: &str) -> Option<&str>;

    /// Look up a header by a typed name: an [`EventHeader`], or a consumer enum
    /// implementing [`EventHeaderName`].
    fn header(&self, name: impl EventHeaderName) -> Option<&str> {
        self.header_str(name.as_str())
    }

    /// Look up a channel variable by its typed enum variant.
    fn variable(&self, name: impl VariableName) -> Option<&str> {
        self.variable_str(name.as_str())
    }

    /// First of `names` the store carries, probed in order.
    ///
    /// For a field the switch spells more than one way: each spelling is its
    /// own [`EventHeader`] variant (or a consumer enum's), and this unions them
    /// without deciding which event carries which.
    fn first_header<H: EventHeaderName, const N: usize>(&self, names: [H; N]) -> Option<&str> {
        names
            .into_iter()
            .find_map(|name| self.header(name))
    }

    /// `Unique-ID` header, falling back to `Caller-Unique-ID`.
    fn unique_id(&self) -> Option<&str> {
        self.first_header([EventHeader::UniqueId, EventHeader::CallerUniqueId])
    }

    /// `Job-UUID` header from `bgapi` `BACKGROUND_JOB` events.
    ///
    /// The only thing separating your job's result from another ESL client's:
    /// `BACKGROUND_JOB` is fired globally, so seeing one you did not issue is
    /// routine rather than exceptional.
    fn job_uuid(&self) -> Option<&str> {
        self.header(EventHeader::JobUuid)
    }

    /// `Channel-Name` header (e.g. `sofia/internal/1000@domain`).
    fn channel_name(&self) -> Option<&str> {
        self.header(EventHeader::ChannelName)
    }

    /// Endpoint module the emitting channel belongs to, from its own
    /// `Channel-Name`.
    ///
    /// Never from `Caller-Channel-Name`, which after a loopback resignation
    /// names the leg that left. See
    /// [`channel_driver()`](crate::channel::channel_driver) and
    /// [`LoopbackChannelName`](crate::variables::LoopbackChannelName).
    fn channel_driver(&self) -> Option<&str> {
        self.channel_name()
            .and_then(channel_driver)
    }

    /// `loopback_leg` variable, for a channel already known to be a loopback.
    ///
    /// Not an answer to "is this a loopback channel": the execute-time
    /// resignation copies every variable onto the real channel that continues
    /// the call, so this reads `Some` on a survivor that never was one. That
    /// question is [`LoopbackChannelName`](crate::variables::LoopbackChannelName)'s.
    fn loopback_leg(&self) -> Result<Option<LoopbackLeg>, ParseLoopbackLegError> {
        parse_opt(self.variable(LoopbackVariable::LoopbackLeg))
    }

    /// `read_result` variable, outcome of the last digit read.
    ///
    /// `Ok(None)` means no read completed its status mapping, not that the
    /// caller hung up; see [`ReadResult`] for what each value does not say.
    fn read_result(&self) -> Result<Option<ReadResult>, ParseReadResultError> {
        parse_opt(self.variable(ChannelVariable::ReadResult))
    }

    /// `sip_hangup_disposition` variable, which SIP message ended this leg.
    ///
    /// Read at `CHANNEL_HANGUP_COMPLETE` or later; on `CHANNEL_HANGUP` only a
    /// received CANCEL is set yet. `Ok(None)` means no SIP path recorded how the
    /// dialog ended, not that this side ended it.
    fn sip_hangup_disposition(
        &self,
    ) -> Result<Option<SipHangupDisposition>, ParseSipHangupDispositionError> {
        parse_opt(self.variable(SofiaVariable::SipHangupDisposition))
    }

    /// `Caller-Caller-ID-Number` header.
    fn caller_id_number(&self) -> Option<&str> {
        self.header(EventHeader::CallerCallerIdNumber)
    }

    /// `Caller-Caller-ID-Name` header.
    fn caller_id_name(&self) -> Option<&str> {
        self.header(EventHeader::CallerCallerIdName)
    }

    /// `Caller-Destination-Number` header.
    fn destination_number(&self) -> Option<&str> {
        self.header(EventHeader::CallerDestinationNumber)
    }

    /// `Caller-Callee-ID-Number` header.
    fn callee_id_number(&self) -> Option<&str> {
        self.header(EventHeader::CallerCalleeIdNumber)
    }

    /// `Caller-Callee-ID-Name` header.
    fn callee_id_name(&self) -> Option<&str> {
        self.header(EventHeader::CallerCalleeIdName)
    }

    /// `Channel-Presence-ID` header (e.g. `1000@example.com`).
    fn channel_presence_id(&self) -> Option<&str> {
        self.header(EventHeader::ChannelPresenceId)
    }

    /// `Presence-Call-Direction` header, parsed into a [`CallDirection`].
    fn presence_call_direction(&self) -> Result<Option<CallDirection>, ParseCallDirectionError> {
        parse_opt(self.header(EventHeader::PresenceCallDirection))
    }

    /// `Event-Date-Timestamp` header (microseconds since epoch).
    fn event_date_timestamp(&self) -> Option<&str> {
        self.header(EventHeader::EventDateTimestamp)
    }

    /// `Event-Sequence` header (sequential event counter).
    fn event_sequence(&self) -> Option<&str> {
        self.header(EventHeader::EventSequence)
    }

    /// `DTMF-Duration` header (digit duration in milliseconds).
    fn dtmf_duration(&self) -> Option<&str> {
        self.header(EventHeader::DtmfDuration)
    }

    /// `DTMF-Source` header (e.g. `rtp`, `inband`).
    fn dtmf_source(&self) -> Option<&str> {
        self.header(EventHeader::DtmfSource)
    }

    /// The `Hangup-Cause` header; a name this crate does not list is kept as
    /// [`HangupCauseValue::Unrecognized`]. `None` if the header is absent.
    fn hangup_cause(&self) -> Option<HangupCauseValue> {
        self.header(EventHeader::HangupCause)
            .map(|name| match name.parse::<HangupCause>() {
                Ok(cause) => HangupCauseValue::Known(cause),
                Err(_) => HangupCauseValue::Unrecognized(name.into()),
            })
    }

    /// Detect a mod_loopback bowout on a `CHANNEL_HANGUP_COMPLETE`.
    ///
    /// `Some` means a loopback leg resigned and the call it carried is still up
    /// on [`other_uuid()`](LoopbackResignation::other_uuid) -- re-anchor
    /// tracking there rather than treating the hangup as a teardown. `None` is
    /// a genuine teardown.
    ///
    /// `Some` is conclusive only once the emitting channel is known to be a
    /// loopback: the marker is copied onto the real channel that continues the
    /// call, so acting on it unqualified tears down a live one. Qualify with
    /// [`LoopbackChannelName::parse()`](crate::variables::LoopbackChannelName::parse)
    /// over [`channel_name()`](Self::channel_name).
    ///
    /// Keyed on the presence of `loopback_hangup_cause`, never its value:
    /// mod_loopback writes a different token depending on which internal path
    /// resigned, and matching one of them silently mis-disposes every call
    /// that took the other.
    fn loopback_resignation(&self) -> Option<LoopbackResignation<'_>> {
        self.variable(LoopbackVariable::LoopbackHangupCause)
            .map(|cause| {
                LoopbackResignation::new(
                    cause,
                    self.variable(LoopbackVariable::LoopbackBowoutOtherUuid),
                )
            })
    }

    /// `Event-Subclass` header for `CUSTOM` events (e.g. `sofia::register`).
    fn event_subclass(&self) -> Option<&str> {
        self.header(EventHeader::EventSubclass)
    }

    /// Parse `Event-Subclass` as a typed [`SofiaEventSubclass`].
    ///
    /// Returns `Ok(None)` if the header is absent, `Err` if present but not a
    /// recognized `sofia::*` subclass.
    fn sofia_event_subclass(
        &self,
    ) -> Result<Option<SofiaEventSubclass>, ParseSofiaEventSubclassError> {
        parse_opt(self.event_subclass())
    }

    /// `Gateway` header from `sofia::gateway_state` / `sofia::gateway_add` events.
    fn gateway(&self) -> Option<&str> {
        self.header(EventHeader::Gateway)
    }

    /// Sofia profile name, from whichever of the two wire spellings the event
    /// carries.
    ///
    /// Probes [`EventHeader::ProfileName`] (`profile-name`, on the
    /// registration, gateway and user-state `CUSTOM` subclasses) then
    /// [`EventHeader::ProfileNameSnake`] (`profile_name`, on mod_sofia's
    /// SIP-method-mirror core events and on
    /// [`SofiaEventSubclass::ProfileStart`]).
    /// The spelling does not follow from the event type, so use
    /// `header(EventHeader::…)` when a spelling-exact lookup is what you want.
    fn profile_name(&self) -> Option<&str> {
        self.first_header([EventHeader::ProfileName, EventHeader::ProfileNameSnake])
    }

    /// `module_name` header -- the literal `mod_sofia` on the events above.
    fn module_name(&self) -> Option<&str> {
        self.header(EventHeader::ModuleName)
    }

    /// `profile_uri` header -- bind URL of the profile named by
    /// [`profile_name()`](Self::profile_name).
    fn profile_uri(&self) -> Option<&str> {
        self.header(EventHeader::ProfileUri)
    }

    /// `Phrase` header (SIP reason phrase) from sofia state events.
    fn phrase(&self) -> Option<&str> {
        self.header(EventHeader::Phrase)
    }

    /// `Status` header (SIP response code) from `sofia::gateway_state` and
    /// `sofia::sip_user_state` events.
    ///
    /// Three digits, first one nonzero (RFC 3261 §7.2). 9xx is accepted: sofia-sip's
    /// nua reports its own failures as 9xx, and these events carry them.
    fn sip_status_code(&self) -> Result<Option<u16>, ParseSipStatusCodeError> {
        self.header(EventHeader::Status)
            .map(parse_sip_status_code)
            .transpose()
    }

    /// Parse the `State` header as a [`GatewayRegState`].
    ///
    /// Returns `Ok(None)` if absent, `Err` if present but unparseable.
    fn gateway_reg_state(&self) -> Result<Option<GatewayRegState>, ParseGatewayRegStateError> {
        parse_opt(self.header(EventHeader::State))
    }

    /// Parse `Ping-Status` as a [`GatewayPingStatus`].
    ///
    /// Use on `sofia::gateway_state` events. For `sofia::sip_user_state`, use
    /// [`sip_user_ping_status()`](Self::sip_user_ping_status) instead.
    fn gateway_ping_status(
        &self,
    ) -> Result<Option<GatewayPingStatus>, ParseGatewayPingStatusError> {
        parse_opt(self.header(EventHeader::PingStatus))
    }

    /// Parse `Ping-Status` as a [`SipUserPingStatus`].
    ///
    /// Use on `sofia::sip_user_state` events. For `sofia::gateway_state`, use
    /// [`gateway_ping_status()`](Self::gateway_ping_status) instead.
    fn sip_user_ping_status(
        &self,
    ) -> Result<Option<SipUserPingStatus>, ParseSipUserPingStatusError> {
        parse_opt(self.header(EventHeader::PingStatus))
    }

    /// `pl_data` header -- SIP NOTIFY body content from `NOTIFY_IN` events.
    ///
    /// Contains the JSON payload (already percent-decoded by the ESL parser).
    /// For NG9-1-1 events this is the inner object without the wrapper key
    /// (FreeSWITCH strips it).
    fn pl_data(&self) -> Option<&str> {
        self.header(EventHeader::PlData)
    }

    /// `event` header -- SIP event package name from `NOTIFY_IN` events.
    ///
    /// Examples: `emergency-AbandonedCall`, `emergency-ServiceState`.
    fn sip_event(&self) -> Option<&str> {
        self.header(EventHeader::SipEvent)
    }

    /// `gateway_name` header -- gateway that received a SIP NOTIFY.
    fn gateway_name(&self) -> Option<&str> {
        self.header(EventHeader::GatewayName)
    }

    /// Parse the `Channel-State` header into a [`ChannelState`].
    ///
    /// Returns `Ok(None)` if the header is absent, `Err` if present but unparseable.
    fn channel_state(&self) -> Result<Option<ChannelState>, ParseChannelStateError> {
        parse_opt(self.header(EventHeader::ChannelState))
    }

    /// Parse the `Channel-State-Number` header into a [`ChannelState`].
    ///
    /// Returns `Ok(None)` if the header is absent, `Err` if present but unparseable.
    ///
    /// This reports the channel's `state`, which leads `Channel-State`
    /// (`running_state`) during teardown, so it is not the end-of-life signal --
    /// see [`is_terminal_channel_state`](Self::is_terminal_channel_state).
    fn channel_state_number(&self) -> Result<Option<ChannelState>, ParseChannelStateError> {
        let Some(raw) = self.header(EventHeader::ChannelStateNumber) else {
            return Ok(None);
        };
        let rejected = || ParseChannelStateError(raw.to_string());
        parse_opt::<u8>(Some(raw))
            .map_err(|_| rejected())?
            .and_then(ChannelState::from_number)
            .ok_or_else(rejected)
            .map(Some)
    }

    /// Whether this event is the one that ends the channel: a `CHANNEL_STATE`
    /// carrying `Channel-State: CS_DESTROY`.
    ///
    /// `CHANNEL_DESTROY` is not that event -- its `Channel-State` still reads
    /// `CS_REPORTING`. Never derive end-of-life from
    /// [`channel_state_number()`](Self::channel_state_number), which already
    /// reads `CS_DESTROY` on `CHANNEL_DESTROY` and so fires twice.
    ///
    /// `Err` when `Channel-State` is present but unparseable; `Ok(false)` when
    /// it is absent.
    fn is_terminal_channel_state(&self) -> Result<bool, ParseChannelStateError> {
        Ok(self
            .channel_state()?
            .is_some_and(|state| state.is_terminal()))
    }

    /// Parse the `Channel-Call-State` header into a [`CallState`].
    ///
    /// Returns `Ok(None)` if the header is absent, `Err` if present but unparseable.
    fn call_state(&self) -> Result<Option<CallState>, ParseCallStateError> {
        parse_opt(self.header(EventHeader::ChannelCallState))
    }

    /// Parse the `Answer-State` header into an [`AnswerState`].
    ///
    /// Returns `Ok(None)` if the header is absent, `Err` if present but unparseable.
    fn answer_state(&self) -> Result<Option<AnswerState>, ParseAnswerStateError> {
        parse_opt(self.header(EventHeader::AnswerState))
    }

    /// Parse the `Call-Direction` header into a [`CallDirection`].
    ///
    /// Returns `Ok(None)` if the header is absent, `Err` if present but unparseable.
    fn call_direction(&self) -> Result<Option<CallDirection>, ParseCallDirectionError> {
        parse_opt(self.header(EventHeader::CallDirection))
    }

    /// Parse the `priority` header value.
    ///
    /// Returns `Ok(None)` if the header is absent, `Err` if present but unparseable.
    #[cfg(feature = "esl")]
    fn priority(&self) -> Result<Option<EslEventPriority>, ParsePriorityError> {
        parse_opt(self.header(EventHeader::Priority))
    }

    /// Extract timetable from timestamp headers with the given prefix.
    ///
    /// Returns `Ok(None)` if no timestamp headers with this prefix are present.
    /// Returns `Err` if a header is present but contains an invalid value.
    fn timetable(&self, prefix: &str) -> Result<Option<ChannelTimetable>, ParseTimetableError> {
        ChannelTimetable::from_lookup(prefix, |key| self.header_str(key))
    }

    /// Caller-leg channel timetable (`Caller-*-Time` headers).
    fn caller_timetable(&self) -> Result<Option<ChannelTimetable>, ParseTimetableError> {
        self.timetable("Caller")
    }

    /// Other-leg channel timetable (`Other-Leg-*-Time` headers).
    fn other_leg_timetable(&self) -> Result<Option<ChannelTimetable>, ParseTimetableError> {
        self.timetable("Other-Leg")
    }
}

// No impl on a bare `HashMap` or `IndexMap`: an exact-key map breaks the case
// contract in this module's notes. Wrap in `EslHeaders`.

/// The union a caller reaches when `?`-ing several typed [`HeaderLookup`]
/// accessors into one error type.
///
/// Each accessor returns its own parser's error; a function calling more than
/// one of them has nothing to name in its return type without this. `Display`
/// names the value that did not parse; the parser's own error is the
/// [`source`](std::error::Error::source), so a chain printer shows it once.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseHeaderError {
    /// From [`channel_state`](HeaderLookup::channel_state) and its siblings.
    ChannelState(ParseChannelStateError),
    /// From [`call_state`](HeaderLookup::call_state).
    CallState(ParseCallStateError),
    /// From [`answer_state`](HeaderLookup::answer_state).
    AnswerState(ParseAnswerStateError),
    /// From [`call_direction`](HeaderLookup::call_direction).
    CallDirection(ParseCallDirectionError),
    /// From a strict [`HangupCause`] parse; [`hangup_cause`](HeaderLookup::hangup_cause)
    /// itself keeps an unlisted name instead of failing.
    HangupCause(ParseHangupCauseError),
    /// From [`gateway_reg_state`](HeaderLookup::gateway_reg_state).
    GatewayRegState(ParseGatewayRegStateError),
    /// From [`loopback_leg`](HeaderLookup::loopback_leg).
    LoopbackLeg(ParseLoopbackLegError),
    /// From [`priority`](HeaderLookup::priority).
    #[cfg(feature = "esl")]
    Priority(ParsePriorityError),
    /// From [`caller_timetable`](HeaderLookup::caller_timetable) and
    /// [`other_leg_timetable`](HeaderLookup::other_leg_timetable).
    Timetable(ParseTimetableError),
    /// From [`sip_status_code`](HeaderLookup::sip_status_code).
    SipStatusCode(ParseSipStatusCodeError),
}

/// `From` for each variant, plus the two trait impls over every variant.
///
/// Every variant carries an error type this crate owns: a `From` for a foreign
/// one would relabel every unrelated parse in the caller as a header fault.
macro_rules! parse_header_error_impls {
    (
        $($(#[$attr:meta])* $Variant:ident($Error:ty) => $what:literal),+ $(,)?
    ) => {
        $(
            $(#[$attr])*
            impl From<$Error> for ParseHeaderError {
                fn from(e: $Error) -> Self {
                    Self::$Variant(e)
                }
            }
        )+

        impl ::std::fmt::Display for ParseHeaderError {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                match self {
                    $($(#[$attr])* Self::$Variant(_) => f.write_str(concat!($what, " did not parse")),)+
                }
            }
        }

        impl ::std::error::Error for ParseHeaderError {
            fn source(&self) -> Option<&(dyn ::std::error::Error + 'static)> {
                match self {
                    $($(#[$attr])* Self::$Variant(e) => Some(e),)+
                }
            }
        }
    };
}

parse_header_error_impls! {
    ChannelState(ParseChannelStateError) => "channel state",
    CallState(ParseCallStateError) => "call state",
    AnswerState(ParseAnswerStateError) => "answer state",
    CallDirection(ParseCallDirectionError) => "call direction",
    HangupCause(ParseHangupCauseError) => "hangup cause",
    GatewayRegState(ParseGatewayRegStateError) => "gateway registration state",
    LoopbackLeg(ParseLoopbackLegError) => "loopback leg",
    #[cfg(feature = "esl")]
    Priority(ParsePriorityError) => "event priority",
    Timetable(ParseTimetableError) => "channel timetable",
    SipStatusCode(ParseSipStatusCodeError) => "SIP status code",
}

#[cfg(all(test, feature = "esl"))]
mod tests {
    use super::*;
    use crate::variables::{LoopbackChannelName, LoopbackHangupCause, LoopbackLeg};
    use crate::EslHeaders;
    use std::error::Error;

    #[test]
    fn sip_status_code_keeps_the_rejected_value() {
        let mut map = EslHeaders::new();
        assert_eq!(map.sip_status_code(), Ok(None));
        map.insert("Status", "486");
        assert_eq!(map.sip_status_code(), Ok(Some(486)));
        map.insert("Status", "70000");
        assert_eq!(
            map.sip_status_code(),
            Err(ParseSipStatusCodeError("70000".into()))
        );
    }

    /// RFC 3261 §7.2: a status code is three digits; the 9xx nua reports are kept.
    #[test]
    fn sip_status_code_takes_three_digits_with_a_nonzero_first() {
        let mut map = EslHeaders::new();
        for (raw, code) in [("100", 100), ("699", 699), ("904", 904), ("999", 999)] {
            map.insert("Status", raw);
            assert_eq!(map.sip_status_code(), Ok(Some(code)), "{raw}");
        }
        for raw in ["+200", "0200", "042", "42", "1000", "20 0", "", "099"] {
            map.insert("Status", raw);
            assert_eq!(
                map.sip_status_code(),
                Err(ParseSipStatusCodeError(raw.into())),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn every_accessor_error_converts_into_the_union() {
        let cases: Vec<(ParseHeaderError, &str)> = vec![
            (
                ParseChannelStateError("x".into()).into(),
                "ChannelState(ParseChannelStateError",
            ),
            (
                ParseCallStateError("x".into()).into(),
                "CallState(ParseCallStateError",
            ),
            (
                ParseAnswerStateError("x".into()).into(),
                "AnswerState(ParseAnswerStateError",
            ),
            (
                ParseCallDirectionError("x".into()).into(),
                "CallDirection(ParseCallDirectionError",
            ),
            (
                ParseHangupCauseError("x".into()).into(),
                "HangupCause(ParseHangupCauseError",
            ),
            (
                ParseGatewayRegStateError("x".into()).into(),
                "GatewayRegState(ParseGatewayRegStateError",
            ),
            (
                ParseLoopbackLegError("x".into()).into(),
                "LoopbackLeg(ParseLoopbackLegError",
            ),
            (
                ParseTimetableError::new("Caller-Channel-Created-Time", "x").into(),
                "Timetable(ParseTimetableError",
            ),
            (
                ParseSipStatusCodeError("x".into()).into(),
                "SipStatusCode(ParseSipStatusCodeError",
            ),
        ];
        for (err, expected) in cases {
            assert!(
                format!("{err:?}").starts_with(expected),
                "{err:?} should be {expected}"
            );
            let cause = err
                .source()
                .expect("the inner error is the source")
                .to_string();
            assert!(!err
                .to_string()
                .contains(&cause));
        }
    }

    #[cfg(feature = "esl")]
    #[test]
    fn priority_error_converts_into_the_union() {
        let err: ParseHeaderError = ParsePriorityError("x".into()).into();
        assert!(matches!(err, ParseHeaderError::Priority(_)));
        assert!(err
            .source()
            .is_some());
    }

    fn store_with(pairs: &[(&str, &str)]) -> EslHeaders {
        pairs
            .iter()
            .copied()
            .collect()
    }

    #[test]
    fn header_str_direct() {
        let s = store_with(&[("Unique-ID", "abc-123")]);
        assert_eq!(s.header_str("Unique-ID"), Some("abc-123"));
        assert_eq!(s.header_str("Missing"), None);
    }

    #[test]
    fn header_by_enum() {
        let s = store_with(&[("Unique-ID", "abc-123")]);
        assert_eq!(s.header(EventHeader::UniqueId), Some("abc-123"));
    }

    #[test]
    fn variable_str_direct() {
        let s = store_with(&[("variable_read_codec", "PCMU")]);
        assert_eq!(s.variable_str("read_codec"), Some("PCMU"));
        assert_eq!(s.variable_str("missing"), None);
    }

    #[test]
    fn variable_by_enum() {
        let s = store_with(&[("variable_read_codec", "PCMU")]);
        assert_eq!(s.variable(ChannelVariable::ReadCodec), Some("PCMU"));
    }

    #[test]
    fn unique_id_primary() {
        let s = store_with(&[("Unique-ID", "uuid-1")]);
        assert_eq!(s.unique_id(), Some("uuid-1"));
    }

    #[test]
    fn unique_id_fallback() {
        let s = store_with(&[("Caller-Unique-ID", "uuid-2")]);
        assert_eq!(s.unique_id(), Some("uuid-2"));
    }

    #[test]
    fn unique_id_none() {
        let s = store_with(&[]);
        assert_eq!(s.unique_id(), None);
    }

    #[test]
    fn job_uuid() {
        let s = store_with(&[("Job-UUID", "job-1")]);
        assert_eq!(s.job_uuid(), Some("job-1"));
    }

    #[test]
    fn channel_name() {
        let s = store_with(&[("Channel-Name", "sofia/internal/1000@example.com")]);
        assert_eq!(s.channel_name(), Some("sofia/internal/1000@example.com"));
    }

    #[test]
    fn caller_id_number_and_name() {
        let s = store_with(&[
            ("Caller-Caller-ID-Number", "1000"),
            ("Caller-Caller-ID-Name", "Alice"),
        ]);
        assert_eq!(s.caller_id_number(), Some("1000"));
        assert_eq!(s.caller_id_name(), Some("Alice"));
    }

    #[test]
    fn hangup_cause_typed() {
        let s = store_with(&[("Hangup-Cause", "NORMAL_CLEARING")]);
        assert_eq!(
            s.hangup_cause(),
            Some(HangupCauseValue::Known(HangupCause::NormalClearing))
        );
        assert_eq!(store_with(&[]).hangup_cause(), None);
    }

    /// `switch_channel_cause2str` answers `UNKNOWN` for a cause outside its
    /// table; it and any newer name are kept as received.
    #[test]
    fn hangup_cause_unlisted_is_kept_verbatim() {
        for name in ["UNKNOWN", "SOME_FUTURE_CAUSE", "normal_clearing"] {
            let cause = store_with(&[("Hangup-Cause", name)])
                .hangup_cause()
                .expect("present");
            assert_eq!(cause, HangupCauseValue::Unrecognized(name.into()));
            assert_eq!(cause.as_str(), name);
            assert_eq!(cause.known(), None);
        }
    }

    #[test]
    fn destination_number() {
        let s = store_with(&[("Caller-Destination-Number", "1000")]);
        assert_eq!(s.destination_number(), Some("1000"));
    }

    #[test]
    fn callee_id() {
        let s = store_with(&[
            ("Caller-Callee-ID-Number", "2000"),
            ("Caller-Callee-ID-Name", "Bob"),
        ]);
        assert_eq!(s.callee_id_number(), Some("2000"));
        assert_eq!(s.callee_id_name(), Some("Bob"));
    }

    #[test]
    fn event_subclass() {
        let s = store_with(&[("Event-Subclass", "sofia::register")]);
        assert_eq!(s.event_subclass(), Some("sofia::register"));
    }

    #[test]
    fn sofia_event_subclass_typed() {
        let s = store_with(&[("Event-Subclass", "sofia::gateway_state")]);
        assert_eq!(
            s.sofia_event_subclass()
                .unwrap(),
            Some(crate::sofia::SofiaEventSubclass::GatewayState)
        );
    }

    #[test]
    fn sofia_event_subclass_absent() {
        let s = store_with(&[]);
        assert_eq!(
            s.sofia_event_subclass()
                .unwrap(),
            None
        );
    }

    #[test]
    fn sofia_event_subclass_non_sofia_is_error() {
        let s = store_with(&[("Event-Subclass", "conference::maintenance")]);
        assert!(s
            .sofia_event_subclass()
            .is_err());
    }

    #[test]
    fn gateway_reg_state_typed() {
        let s = store_with(&[("State", "REGED")]);
        assert_eq!(
            s.gateway_reg_state()
                .unwrap(),
            Some(crate::sofia::GatewayRegState::Reged)
        );
    }

    #[test]
    fn gateway_reg_state_invalid_is_error() {
        let s = store_with(&[("State", "BOGUS")]);
        assert!(s
            .gateway_reg_state()
            .is_err());
    }

    #[test]
    fn gateway_ping_status_typed() {
        let s = store_with(&[("Ping-Status", "UP")]);
        assert_eq!(
            s.gateway_ping_status()
                .unwrap(),
            Some(crate::sofia::GatewayPingStatus::Up)
        );
    }

    #[test]
    fn sip_user_ping_status_typed() {
        let s = store_with(&[("Ping-Status", "REACHABLE")]);
        assert_eq!(
            s.sip_user_ping_status()
                .unwrap(),
            Some(crate::sofia::SipUserPingStatus::Reachable)
        );
    }

    #[test]
    fn gateway_accessor() {
        let s = store_with(&[("Gateway", "my-gateway")]);
        assert_eq!(s.gateway(), Some("my-gateway"));
    }

    #[test]
    fn profile_name_accessor() {
        let s = store_with(&[("profile-name", "internal")]);
        assert_eq!(s.profile_name(), Some("internal"));
    }

    #[test]
    fn profile_name_spellings_resolve_distinct_variants() {
        let hyphen = store_with(&[("profile-name", "internal")]);
        assert_eq!(hyphen.header(EventHeader::ProfileName), Some("internal"));
        assert_eq!(hyphen.header(EventHeader::ProfileNameSnake), None);
        assert_eq!(hyphen.profile_name(), Some("internal"));

        let underscore = store_with(&[("profile_name", "external")]);
        assert_eq!(underscore.header(EventHeader::ProfileName), None);
        assert_eq!(
            underscore.header(EventHeader::ProfileNameSnake),
            Some("external")
        );
        assert_eq!(underscore.profile_name(), Some("external"));
    }

    #[test]
    fn profile_name_probes_hyphen_before_underscore() {
        let s = store_with(&[("profile-name", "internal"), ("profile_name", "external")]);
        assert_eq!(s.profile_name(), Some("internal"));
    }

    #[test]
    fn module_name_accessor() {
        let s = store_with(&[("module_name", "mod_sofia")]);
        assert_eq!(s.module_name(), Some("mod_sofia"));
    }

    #[test]
    fn profile_uri_accessor() {
        let s = store_with(&[("profile_uri", "sip:mod_sofia@192.0.2.1:5060")]);
        assert_eq!(s.profile_uri(), Some("sip:mod_sofia@192.0.2.1:5060"));
    }

    #[test]
    fn phrase_accessor() {
        let s = store_with(&[("Phrase", "OK")]);
        assert_eq!(s.phrase(), Some("OK"));
    }

    #[test]
    fn channel_state_typed() {
        let s = store_with(&[("Channel-State", "CS_EXECUTE")]);
        assert_eq!(
            s.channel_state()
                .unwrap(),
            Some(ChannelState::CsExecute)
        );
    }

    #[test]
    fn channel_state_number_typed() {
        let s = store_with(&[("Channel-State-Number", "4")]);
        assert_eq!(
            s.channel_state_number()
                .unwrap(),
            Some(ChannelState::CsExecute)
        );
    }

    #[test]
    fn terminal_state_is_the_final_channel_state_event() {
        let s = store_with(&[("Channel-State", "CS_DESTROY")]);
        assert_eq!(s.is_terminal_channel_state(), Ok(true));
    }

    #[test]
    fn channel_destroy_shape_is_not_terminal() {
        let s = store_with(&[
            ("Channel-State", "CS_REPORTING"),
            ("Channel-State-Number", "12"),
        ]);
        assert_eq!(s.is_terminal_channel_state(), Ok(false));
    }

    #[test]
    fn hangup_state_is_not_terminal() {
        let s = store_with(&[("Channel-State", "CS_HANGUP")]);
        assert_eq!(s.is_terminal_channel_state(), Ok(false));
    }

    #[test]
    fn unparseable_channel_state_is_an_error() {
        let s = store_with(&[("Channel-State", "CS_BOGUS")]);
        assert!(s
            .is_terminal_channel_state()
            .is_err());
    }

    #[test]
    fn missing_channel_state_is_not_terminal() {
        let s = store_with(&[("Event-Name", "HEARTBEAT")]);
        assert_eq!(s.is_terminal_channel_state(), Ok(false));
    }

    #[test]
    fn call_state_typed() {
        let s = store_with(&[("Channel-Call-State", "ACTIVE")]);
        assert_eq!(
            s.call_state()
                .unwrap(),
            Some(CallState::Active)
        );
    }

    #[test]
    fn answer_state_typed() {
        let s = store_with(&[("Answer-State", "answered")]);
        assert_eq!(
            s.answer_state()
                .unwrap(),
            Some(AnswerState::Answered)
        );
    }

    #[test]
    fn call_direction_typed() {
        let s = store_with(&[("Call-Direction", "inbound")]);
        assert_eq!(
            s.call_direction()
                .unwrap(),
            Some(CallDirection::Inbound)
        );
    }

    #[cfg(feature = "esl")]
    #[test]
    fn priority_typed() {
        let s = store_with(&[("priority", "HIGH")]);
        assert_eq!(
            s.priority()
                .unwrap(),
            Some(EslEventPriority::High)
        );
        assert_eq!(
            store_with(&[])
                .priority()
                .unwrap(),
            None
        );
        assert!(store_with(&[("priority", "BOGUS")])
            .priority()
            .is_err());
    }

    #[test]
    fn timetable_extraction() {
        let s = store_with(&[
            ("Caller-Channel-Created-Time", "1700000001000000"),
            ("Caller-Channel-Answered-Time", "1700000005000000"),
        ]);
        let tt = s
            .caller_timetable()
            .unwrap()
            .expect("should have timetable");
        assert_eq!(tt.created, Some(1700000001000000));
        assert_eq!(tt.answered, Some(1700000005000000));
        assert_eq!(tt.hungup, None);
    }

    #[test]
    fn timetable_other_leg() {
        let s = store_with(&[("Other-Leg-Channel-Created-Time", "1700000001000000")]);
        let tt = s
            .other_leg_timetable()
            .unwrap()
            .expect("should have timetable");
        assert_eq!(tt.created, Some(1700000001000000));
    }

    #[test]
    fn timetable_none_when_absent() {
        let s = store_with(&[]);
        assert_eq!(
            s.caller_timetable()
                .unwrap(),
            None
        );
    }

    #[test]
    fn timetable_invalid_is_error() {
        let s = store_with(&[("Caller-Channel-Created-Time", "not_a_number")]);
        let err = s
            .caller_timetable()
            .unwrap_err();
        assert_eq!(err.header, "Caller-Channel-Created-Time");
    }

    // --- loopback resignation ---

    #[test]
    fn loopback_resignation_absent_on_a_real_teardown() {
        let s = store_with(&[("variable_hangup_cause", "NORMAL_CLEARING")]);
        assert!(s
            .loopback_resignation()
            .is_none());
    }

    #[test]
    fn loopback_resignation_on_execute_masquerade_path() {
        let s = store_with(&[
            ("variable_loopback_hangup_cause", "bowout"),
            ("variable_loopback_bowout_other_uuid", "survivor-uuid"),
        ]);
        let r = s
            .loopback_resignation()
            .expect("marker present");
        assert_eq!(r.cause(), Ok(LoopbackHangupCause::Bowout));
        assert_eq!(r.cause_raw(), "bowout");
        assert_eq!(r.other_uuid(), Some("survivor-uuid"));
    }

    #[test]
    fn loopback_resignation_on_frame_count_path() {
        let s = store_with(&[
            ("variable_loopback_hangup_cause", "bridge"),
            ("variable_loopback_bowout_other_uuid", "survivor-uuid"),
        ]);
        let r = s
            .loopback_resignation()
            .expect("marker present");
        assert_eq!(r.cause(), Ok(LoopbackHangupCause::Bridge));
        assert_eq!(r.other_uuid(), Some("survivor-uuid"));
    }

    // The whole point of the shape: a path mod_loopback grows later still
    // reports a resignation, so the surviving leg is never mis-disposed.
    #[test]
    fn loopback_resignation_survives_an_unknown_token() {
        let s = store_with(&[
            ("variable_loopback_hangup_cause", "some_future_path"),
            ("variable_loopback_bowout_other_uuid", "survivor-uuid"),
        ]);
        let r = s
            .loopback_resignation()
            .expect("presence is the signal, not the value");
        assert_eq!(r.other_uuid(), Some("survivor-uuid"));
        assert_eq!(r.cause_raw(), "some_future_path");
        assert!(r
            .cause()
            .is_err());
    }

    #[test]
    fn loopback_resignation_without_a_surviving_uuid() {
        let s = store_with(&[("variable_loopback_hangup_cause", "bridge")]);
        let r = s
            .loopback_resignation()
            .expect("marker present");
        assert_eq!(r.other_uuid(), None);
    }

    // --- channel kind ---

    #[test]
    fn the_resigning_leg_names_itself() {
        let leg = store_with(&[
            ("Channel-Name", "loopback/9199-a"),
            ("variable_loopback_hangup_cause", "bowout"),
            ("variable_loopback_leg", "A"),
        ]);
        assert_eq!(leg.channel_driver(), Some("loopback"));
        let name = leg
            .channel_name()
            .and_then(LoopbackChannelName::parse)
            .expect("a loopback name");
        assert_eq!(name.extension(), "9199");
        assert_eq!(name.leg(), LoopbackLeg::A);
        assert_eq!(
            leg.loopback_leg()
                .unwrap(),
            Some(LoopbackLeg::A)
        );
    }

    // The masquerade copies every variable and the caller profile onto the
    // survivor, so only its own Channel-Name still answers for its driver.
    #[test]
    fn a_survivor_that_inherited_the_marker_is_not_a_loopback() {
        let survivor = store_with(&[
            ("Channel-Name", "sofia/internal/1000@example.com"),
            ("Caller-Channel-Name", "loopback/9199-a"),
            ("variable_loopback_hangup_cause", "bowout"),
            ("variable_is_loopback", "true"),
            ("variable_loopback_leg", "A"),
        ]);
        assert!(survivor
            .loopback_resignation()
            .is_some());
        assert_eq!(
            survivor
                .loopback_leg()
                .unwrap(),
            Some(LoopbackLeg::A)
        );

        assert_eq!(survivor.channel_driver(), Some("sofia"));
        assert_eq!(
            survivor
                .channel_name()
                .and_then(LoopbackChannelName::parse),
            None
        );
    }

    #[test]
    fn channel_kind_accessors_absent() {
        let s = store_with(&[]);
        assert_eq!(s.channel_driver(), None);
        assert_eq!(
            s.loopback_leg()
                .unwrap(),
            None
        );
    }

    #[test]
    fn read_result_reads_its_own_variable() {
        let s = store_with(&[
            ("variable_read_result", "timeout"),
            ("variable_read_terminator_used", "#"),
        ]);
        assert_eq!(s.read_result(), Ok(Some(ReadResult::Timeout)));
    }

    #[test]
    fn read_result_absent_is_none() {
        let s = store_with(&[("variable_read_terminator_used", "#")]);
        assert_eq!(s.read_result(), Ok(None));
    }

    #[test]
    fn read_result_unknown_value_is_error() {
        let s = store_with(&[("variable_read_result", "hangup")]);
        assert!(s
            .read_result()
            .is_err());
    }

    #[test]
    fn sip_hangup_disposition_reads_its_own_variable() {
        let s = store_with(&[
            ("variable_sip_hangup_disposition", "recv_cancel"),
            ("variable_sip_term_status", "487"),
        ]);
        assert_eq!(
            s.sip_hangup_disposition(),
            Ok(Some(SipHangupDisposition::RecvCancel))
        );
    }

    #[test]
    fn sip_hangup_disposition_absent_is_none() {
        let s = store_with(&[("variable_sip_term_status", "487")]);
        assert_eq!(s.sip_hangup_disposition(), Ok(None));
    }

    #[test]
    fn sip_hangup_disposition_unknown_value_is_error() {
        let s = store_with(&[("variable_sip_hangup_disposition", "recv_refer")]);
        assert!(s
            .sip_hangup_disposition()
            .is_err());
    }

    #[test]
    fn loopback_leg_invalid_is_error() {
        let s = store_with(&[("variable_loopback_leg", "a")]);
        assert!(s
            .loopback_leg()
            .is_err());
    }

    /// Accessor under test, named for the assertion message.
    type RawAccessor = (&'static str, fn(&EslHeaders) -> Option<&str>);
    /// An accessor reported as two booleans: parsed to nothing, and refused.
    type TypedAccessor = (
        &'static str,
        fn(&EslHeaders) -> bool,
        fn(&EslHeaders) -> bool,
    );

    const RAW_ACCESSORS: &[RawAccessor] = &[
        ("channel_name", |s| s.channel_name()),
        ("caller_id_number", |s| s.caller_id_number()),
        ("caller_id_name", |s| s.caller_id_name()),
        ("destination_number", |s| s.destination_number()),
        ("callee_id_number", |s| s.callee_id_number()),
        ("callee_id_name", |s| s.callee_id_name()),
        ("event_subclass", |s| s.event_subclass()),
        ("job_uuid", |s| s.job_uuid()),
        ("pl_data", |s| s.pl_data()),
        ("sip_event", |s| s.sip_event()),
        ("gateway_name", |s| s.gateway_name()),
        ("channel_presence_id", |s| s.channel_presence_id()),
        ("event_date_timestamp", |s| s.event_date_timestamp()),
        ("event_sequence", |s| s.event_sequence()),
        ("dtmf_duration", |s| s.dtmf_duration()),
        ("dtmf_source", |s| s.dtmf_source()),
    ];

    const TYPED_ACCESSORS: &[TypedAccessor] = &[
        (
            "channel_state",
            |s| {
                s.channel_state()
                    .unwrap()
                    .is_none()
            },
            |s| {
                s.channel_state()
                    .is_err()
            },
        ),
        (
            "channel_state_number",
            |s| {
                s.channel_state_number()
                    .unwrap()
                    .is_none()
            },
            |s| {
                s.channel_state_number()
                    .is_err()
            },
        ),
        (
            "call_state",
            |s| {
                s.call_state()
                    .unwrap()
                    .is_none()
            },
            |s| {
                s.call_state()
                    .is_err()
            },
        ),
        (
            "answer_state",
            |s| {
                s.answer_state()
                    .unwrap()
                    .is_none()
            },
            |s| {
                s.answer_state()
                    .is_err()
            },
        ),
        (
            "call_direction",
            |s| {
                s.call_direction()
                    .unwrap()
                    .is_none()
            },
            |s| {
                s.call_direction()
                    .is_err()
            },
        ),
        (
            "presence_call_direction",
            |s| {
                s.presence_call_direction()
                    .unwrap()
                    .is_none()
            },
            |s| {
                s.presence_call_direction()
                    .is_err()
            },
        ),
    ];

    #[test]
    fn missing_headers_return_none() {
        let s = store_with(&[]);
        for (name, accessor) in RAW_ACCESSORS {
            assert_eq!(accessor(&s), None, "{name}");
        }
        for (name, absent_is_none, _) in TYPED_ACCESSORS {
            assert!(absent_is_none(&s), "{name}");
        }
    }

    #[test]
    fn notify_in_headers() {
        let s = store_with(&[
            ("pl_data", r#"{"invite":"INVITE ..."}"#),
            ("event", "emergency-AbandonedCall"),
            ("gateway_name", "ng911-bcf"),
        ]);
        assert_eq!(s.pl_data(), Some(r#"{"invite":"INVITE ..."}"#));
        assert_eq!(s.sip_event(), Some("emergency-AbandonedCall"));
        assert_eq!(s.gateway_name(), Some("ng911-bcf"));
    }

    #[test]
    fn channel_presence_id() {
        let s = store_with(&[("Channel-Presence-ID", "1000@example.com")]);
        assert_eq!(s.channel_presence_id(), Some("1000@example.com"));
    }

    #[test]
    fn presence_call_direction_typed() {
        let s = store_with(&[("Presence-Call-Direction", "outbound")]);
        assert_eq!(
            s.presence_call_direction()
                .unwrap(),
            Some(CallDirection::Outbound)
        );
    }

    #[test]
    fn event_date_timestamp() {
        let s = store_with(&[("Event-Date-Timestamp", "1700000001000000")]);
        assert_eq!(s.event_date_timestamp(), Some("1700000001000000"));
    }

    #[test]
    fn event_sequence() {
        let s = store_with(&[("Event-Sequence", "12345")]);
        assert_eq!(s.event_sequence(), Some("12345"));
    }

    #[test]
    fn dtmf_duration() {
        let s = store_with(&[("DTMF-Duration", "2000")]);
        assert_eq!(s.dtmf_duration(), Some("2000"));
    }

    #[test]
    fn dtmf_source() {
        let s = store_with(&[("DTMF-Source", "rtp")]);
        assert_eq!(s.dtmf_source(), Some("rtp"));
    }

    #[test]
    fn invalid_values_return_err() {
        let s = store_with(&[
            ("Channel-State", "BOGUS"),
            ("Channel-State-Number", "999"),
            ("Channel-Call-State", "BOGUS"),
            ("Answer-State", "bogus"),
            ("Call-Direction", "bogus"),
            ("Presence-Call-Direction", "bogus"),
            ("Hangup-Cause", "BOGUS"),
        ]);
        for (name, _, unparseable_is_err) in TYPED_ACCESSORS {
            assert!(unparseable_is_err(&s), "{name}");
        }
    }
}
