//! Channel-related data types extracted from ESL event headers.

use std::fmt;

wire_enum! {
    /// Channel state from `switch_channel_state_t` -- carried in the `Channel-State` header
    /// as a string (`CS_ROUTING`) and in `Channel-State-Number` as an integer.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    #[repr(u8)]
    pub enum ChannelState {
        CsNew = 0 => "CS_NEW",
        CsInit = 1 => "CS_INIT",
        CsRouting = 2 => "CS_ROUTING",
        CsSoftExecute = 3 => "CS_SOFT_EXECUTE",
        CsExecute = 4 => "CS_EXECUTE",
        CsExchangeMedia = 5 => "CS_EXCHANGE_MEDIA",
        CsPark = 6 => "CS_PARK",
        CsConsumeMedia = 7 => "CS_CONSUME_MEDIA",
        CsHibernate = 8 => "CS_HIBERNATE",
        CsReset = 9 => "CS_RESET",
        CsHangup = 10 => "CS_HANGUP",
        CsReporting = 11 => "CS_REPORTING",
        CsDestroy = 12 => "CS_DESTROY",
        CsNone = 13 => "CS_NONE",
    }
    error ParseChannelStateError("channel state");
    numeric: from_number(u8);
    tests: channel_state_wire_tests;
}

impl ChannelState {
    /// Whether the channel is gone: `CS_DESTROY`, and nothing else.
    ///
    /// The session's run loop exits on `CS_DESTROY`; `CS_NONE` is a
    /// `running_state` sentinel the state machine aborts on if it ever reaches
    /// it, so it ends nothing despite sorting above `CS_DESTROY`.
    ///
    /// `CHANNEL_STATE` events carry no `variable_*` headers, so end-of-life
    /// detected here cannot be paired with any channel variable from the same
    /// event. The variable-bearing terminal event is `CHANNEL_HANGUP_COMPLETE`,
    /// which is also where `variable_hangup_cause` first appears --
    /// `CHANNEL_HANGUP` carries only the `Hangup-Cause` header.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::CsDestroy)
    }
}

/// Endpoint module a channel name belongs to: the segment before the first `/`
/// (`sofia`, `loopback`, `null`, ...).
///
/// No enum, because any module can register an endpoint. A channel name is also
/// what the channel calls itself rather than proof of its driver -- see
/// [`LoopbackChannelName`](crate::variables::LoopbackChannelName) for what that
/// costs after a loopback resignation.
pub fn channel_driver(name: &str) -> Option<&str> {
    let (driver, _) = name.split_once('/')?;
    if driver.is_empty() {
        return None;
    }
    Some(driver)
}

wire_enum! {
    /// Call state from `switch_channel_callstate_t` -- carried in the `Channel-Call-State` header.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum CallState {
        Down => "DOWN",
        Dialing => "DIALING",
        Ringing => "RINGING",
        Early => "EARLY",
        Active => "ACTIVE",
        Held => "HELD",
        RingWait => "RING_WAIT",
        Hangup => "HANGUP",
        Unheld => "UNHELD",
    }
    error ParseCallStateError("call state");
    tests: call_state_wire_tests;
}

wire_enum! {
    /// Answer state from the `Answer-State` header. Wire format is lowercase.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum AnswerState {
        Hangup => "hangup",
        Answered => "answered",
        Early => "early",
        Ringing => "ringing",
    }
    error ParseAnswerStateError("answer state");
    tests: answer_state_wire_tests;
}

wire_enum! {
    /// Call direction from the `Call-Direction` header. Wire format is lowercase.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum CallDirection {
        Inbound => "inbound",
        Outbound => "outbound",
    }
    error ParseCallDirectionError("call direction");
    tests: call_direction_wire_tests;
}

wire_enum! {
    /// Hangup cause from `switch_cause_t` (Q.850 + FreeSWITCH extensions).
    ///
    /// Carried in the `Hangup-Cause` header. Wire format is `SCREAMING_SNAKE_CASE`
    /// (e.g. `NORMAL_CLEARING`). The numeric value matches the Q.850 cause code
    /// for standard causes, or a FreeSWITCH-internal range for extensions.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    #[repr(u16)]
    pub enum HangupCause {
        None = 0 => "NONE",
        UnallocatedNumber = 1 => "UNALLOCATED_NUMBER",
        NoRouteTransitNet = 2 => "NO_ROUTE_TRANSIT_NET",
        NoRouteDestination = 3 => "NO_ROUTE_DESTINATION",
        ChannelUnacceptable = 6 => "CHANNEL_UNACCEPTABLE",
        CallAwardedDelivered = 7 => "CALL_AWARDED_DELIVERED",
        NormalClearing = 16 => "NORMAL_CLEARING",
        UserBusy = 17 => "USER_BUSY",
        NoUserResponse = 18 => "NO_USER_RESPONSE",
        NoAnswer = 19 => "NO_ANSWER",
        SubscriberAbsent = 20 => "SUBSCRIBER_ABSENT",
        CallRejected = 21 => "CALL_REJECTED",
        NumberChanged = 22 => "NUMBER_CHANGED",
        RedirectionToNewDestination = 23 => "REDIRECTION_TO_NEW_DESTINATION",
        ExchangeRoutingError = 25 => "EXCHANGE_ROUTING_ERROR",
        DestinationOutOfOrder = 27 => "DESTINATION_OUT_OF_ORDER",
        InvalidNumberFormat = 28 => "INVALID_NUMBER_FORMAT",
        FacilityRejected = 29 => "FACILITY_REJECTED",
        ResponseToStatusEnquiry = 30 => "RESPONSE_TO_STATUS_ENQUIRY",
        NormalUnspecified = 31 => "NORMAL_UNSPECIFIED",
        NormalCircuitCongestion = 34 => "NORMAL_CIRCUIT_CONGESTION",
        NetworkOutOfOrder = 38 => "NETWORK_OUT_OF_ORDER",
        NormalTemporaryFailure = 41 => "NORMAL_TEMPORARY_FAILURE",
        SwitchCongestion = 42 => "SWITCH_CONGESTION",
        AccessInfoDiscarded = 43 => "ACCESS_INFO_DISCARDED",
        RequestedChanUnavail = 44 => "REQUESTED_CHAN_UNAVAIL",
        PreEmpted = 45 => "PRE_EMPTED",
        FacilityNotSubscribed = 50 => "FACILITY_NOT_SUBSCRIBED",
        OutgoingCallBarred = 52 => "OUTGOING_CALL_BARRED",
        IncomingCallBarred = 54 => "INCOMING_CALL_BARRED",
        BearercapabilityNotauth = 57 => "BEARERCAPABILITY_NOTAUTH",
        BearercapabilityNotavail = 58 => "BEARERCAPABILITY_NOTAVAIL",
        ServiceUnavailable = 63 => "SERVICE_UNAVAILABLE",
        BearercapabilityNotimpl = 65 => "BEARERCAPABILITY_NOTIMPL",
        ChanNotImplemented = 66 => "CHAN_NOT_IMPLEMENTED",
        FacilityNotImplemented = 69 => "FACILITY_NOT_IMPLEMENTED",
        ServiceNotImplemented = 79 => "SERVICE_NOT_IMPLEMENTED",
        InvalidCallReference = 81 => "INVALID_CALL_REFERENCE",
        IncompatibleDestination = 88 => "INCOMPATIBLE_DESTINATION",
        InvalidMsgUnspecified = 95 => "INVALID_MSG_UNSPECIFIED",
        MandatoryIeMissing = 96 => "MANDATORY_IE_MISSING",
        MessageTypeNonexist = 97 => "MESSAGE_TYPE_NONEXIST",
        WrongMessage = 98 => "WRONG_MESSAGE",
        IeNonexist = 99 => "IE_NONEXIST",
        InvalidIeContents = 100 => "INVALID_IE_CONTENTS",
        WrongCallState = 101 => "WRONG_CALL_STATE",
        RecoveryOnTimerExpire = 102 => "RECOVERY_ON_TIMER_EXPIRE",
        MandatoryIeLengthError = 103 => "MANDATORY_IE_LENGTH_ERROR",
        ProtocolError = 111 => "PROTOCOL_ERROR",
        Interworking = 127 => "INTERWORKING",
        Success = 142 => "SUCCESS",
        OriginatorCancel = 487 => "ORIGINATOR_CANCEL",
        Crash = 700 => "CRASH",
        SystemShutdown = 701 => "SYSTEM_SHUTDOWN",
        LoseRace = 702 => "LOSE_RACE",
        ManagerRequest = 703 => "MANAGER_REQUEST",
        BlindTransfer = 800 => "BLIND_TRANSFER",
        AttendedTransfer = 801 => "ATTENDED_TRANSFER",
        AllottedTimeout = 802 => "ALLOTTED_TIMEOUT",
        UserChallenge = 803 => "USER_CHALLENGE",
        MediaTimeout = 804 => "MEDIA_TIMEOUT",
        PickedOff = 805 => "PICKED_OFF",
        UserNotRegistered = 806 => "USER_NOT_REGISTERED",
        ProgressTimeout = 807 => "PROGRESS_TIMEOUT",
        InvalidGateway = 808 => "INVALID_GATEWAY",
        GatewayDown = 809 => "GATEWAY_DOWN",
        InvalidUrl = 810 => "INVALID_URL",
        InvalidProfile = 811 => "INVALID_PROFILE",
        NoPickup = 812 => "NO_PICKUP",
        SrtpReadError = 813 => "SRTP_READ_ERROR",
        Bowout = 814 => "BOWOUT",
        BusyEverywhere = 815 => "BUSY_EVERYWHERE",
        Decline = 816 => "DECLINE",
        DoesNotExistAnywhere = 817 => "DOES_NOT_EXIST_ANYWHERE",
        NotAcceptable = 818 => "NOT_ACCEPTABLE",
        Unwanted = 819 => "UNWANTED",
        NoIdentity = 820 => "NO_IDENTITY",
        BadIdentityInfo = 821 => "BAD_IDENTITY_INFO",
        UnsupportedCertificate = 822 => "UNSUPPORTED_CERTIFICATE",
        InvalidIdentity = 823 => "INVALID_IDENTITY",
        /// Stale Date (STIR/SHAKEN).
        StaleDate = 824 => "STALE_DATE",
        /// Reject all calls.
        RejectAll = 825 => "REJECT_ALL",
    }
    error ParseHangupCauseError("hangup cause");
    numeric: from_number(u16);
    tests: hangup_cause_wire_tests;
}

impl HangupCause {
    /// Map a SIP response code to the corresponding FreeSWITCH hangup cause.
    ///
    /// Uses the same mapping as mod_sofia's `sofia_glue_sip_cause_to_freeswitch()`.
    /// Returns `None` for codes without an explicit mapping.
    pub fn from_sip_response(code: u16) -> Option<Self> {
        match code {
            200 => Some(Self::NormalClearing),
            401 | 402 | 403 | 407 | 603 | 608 => Some(Self::CallRejected),
            607 => Some(Self::Unwanted),
            404 => Some(Self::UnallocatedNumber),
            485 | 604 => Some(Self::NoRouteDestination),
            408 | 504 => Some(Self::RecoveryOnTimerExpire),
            410 => Some(Self::NumberChanged),
            413 | 414 | 416 | 420 | 421 | 423 | 505 | 513 => Some(Self::Interworking),
            480 => Some(Self::NoUserResponse),
            400 | 481 | 500 | 503 => Some(Self::NormalTemporaryFailure),
            486 | 600 => Some(Self::UserBusy),
            484 => Some(Self::InvalidNumberFormat),
            488 | 606 => Some(Self::IncompatibleDestination),
            502 => Some(Self::NetworkOutOfOrder),
            405 => Some(Self::ServiceUnavailable),
            406 | 415 | 501 => Some(Self::ServiceNotImplemented),
            482 | 483 => Some(Self::ExchangeRoutingError),
            487 => Some(Self::OriginatorCancel),
            428 => Some(Self::NoIdentity),
            429 => Some(Self::BadIdentityInfo),
            437 => Some(Self::UnsupportedCertificate),
            438 => Some(Self::InvalidIdentity),
            _ => None,
        }
    }
}

/// A `Hangup-Cause` value as the switch sent it, typed when this crate knows
/// the name.
///
/// Every value round-trips byte for byte through [`as_str`](Self::as_str),
/// `Display`, `FromStr` and serde. Serde reads a name the way [`HangupCause`]'s
/// does, so a value stored as a variant name reads back as `Known`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HangupCauseValue {
    /// A cause this crate lists.
    Known(HangupCause),
    /// A name this crate does not list, as received. Includes the switch's own
    /// `UNKNOWN`, which it emits for a cause value outside its table.
    Unrecognized(Box<str>),
}

impl HangupCauseValue {
    /// The wire name, or the received text for an unrecognized cause.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Known(cause) => cause.as_str(),
            Self::Unrecognized(name) => name,
        }
    }

    /// The typed cause, `None` when unrecognized.
    pub fn known(&self) -> Option<HangupCause> {
        match self {
            Self::Known(cause) => Some(*cause),
            Self::Unrecognized(_) => None,
        }
    }
}

impl From<HangupCause> for HangupCauseValue {
    fn from(cause: HangupCause) -> Self {
        Self::Known(cause)
    }
}

impl fmt::Display for HangupCauseValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for HangupCauseValue {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(s.parse::<HangupCause>()
            .map_or_else(|_| Self::Unrecognized(s.into()), Self::Known))
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for HangupCauseValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for HangupCauseValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::IntoDeserializer;

        // serde's own message quotes the value; serde_name replaces it the same way.
        let name = String::deserialize(deserializer).map_err(|_| {
            <D::Error as serde::de::Error>::custom("invalid hangup cause: expected a string")
        })?;
        let as_cause: serde::de::value::StrDeserializer<'_, serde::de::value::Error> = name
            .as_str()
            .into_deserializer();
        Ok(HangupCause::deserialize(as_cause)
            .map_or_else(|_| Self::Unrecognized(name.into()), Self::Known))
    }
}

/// Channel timing data from FreeSWITCH's `switch_channel_timetable_t`.
///
/// Timestamps are epoch microseconds (`i64`). A value of `0` means the
/// corresponding event never occurred (e.g., `hungup == Some(0)` means
/// the channel has not hung up yet). `None` means the header was absent
/// or unparseable.
///
/// Extracted from ESL event headers using a prefix (typically `"Caller"`
/// or `"Other-Leg"`). The wire header format is `{prefix}-{suffix}`.
///
/// ## Headers extracted
///
/// `from_lookup()` extracts headers with suffixes from [`SUFFIXES`](Self::SUFFIXES).
/// With `TimetablePrefix::Caller`, extracts `Caller-Channel-Created-Time` →
/// `created`, `Caller-Channel-Hangup-Time` → `hungup`, etc.
///
/// Use `TimetablePrefix::OtherLeg` for `Other-Leg-*` headers,
/// `TimetablePrefix::Channel` for outbound ESL `Channel-*` headers, or pass
/// a custom string prefix to `from_lookup()`. See [`SUFFIXES`](Self::SUFFIXES)
/// for the complete list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ChannelTimetable {
    /// When the caller profile was created.
    pub profile_created: Option<i64>,
    /// When the channel was created.
    pub created: Option<i64>,
    /// When the channel was answered.
    pub answered: Option<i64>,
    /// When early media (183) was received.
    pub progress: Option<i64>,
    /// When media-bearing early media arrived.
    pub progress_media: Option<i64>,
    /// When the channel hung up.
    pub hungup: Option<i64>,
    /// When the channel was transferred.
    pub transferred: Option<i64>,
    /// When the channel was resurrected.
    pub resurrected: Option<i64>,
    /// When the channel was bridged.
    pub bridged: Option<i64>,
    /// Timestamp of the last hold event.
    pub last_hold: Option<i64>,
    /// Accumulated hold time in microseconds.
    pub hold_accum: Option<i64>,
}

wire_enum! {
    /// Header prefix identifying which call leg's timetable to extract.
    ///
    /// FreeSWITCH emits timetable headers as `{prefix}-Channel-Created-Time`, etc.
    /// The prefix varies by context -- `Caller` for the primary leg, `Other-Leg`
    /// for the bridged party, `Channel` in outbound ESL mode, etc.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum TimetablePrefix {
        /// Primary call leg.
        Caller => "Caller",
        /// Bridged party.
        OtherLeg => "Other-Leg",
        /// Outbound ESL channel profile.
        Channel => "Channel",
        /// XML dialplan hunt.
        Hunt => "Hunt",
        /// Bridge debug originator.
        Originator => "ORIGINATOR",
        /// Bridge debug originatee.
        Originatee => "ORIGINATEE",
        /// Post-bridge debug originator.
        PostOriginator => "POST-ORIGINATOR",
        /// Post-bridge debug originatee.
        PostOriginatee => "POST-ORIGINATEE",
    }
    error ParseTimetablePrefixError("timetable prefix");
    tests: timetable_prefix_wire_tests;
}

/// Error returned when a timetable header is present but not a valid `i64`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParseTimetableError {
    /// Full header name (e.g. `Caller-Channel-Created-Time`).
    pub header: String,
    /// The unparseable value found in the header.
    pub value: String,
}

impl fmt::Display for ParseTimetableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid timetable value for {} ({} bytes)",
            self.header,
            self.value
                .len()
        )
    }
}

impl std::error::Error for ParseTimetableError {}

impl ParseTimetableError {
    /// Create a new timetable parse error.
    pub fn new(header: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            header: header.into(),
            value: value.into(),
        }
    }
}

/// Generate [`TimetableField`], `ChannelTimetable::SUFFIXES` and
/// `ChannelTimetable::from_lookup` from a single
/// `(field => Variant => "Suffix")` table, eliminating the risk of drift between
/// the named fields, the advertised suffix list and the headers actually
/// extracted.
macro_rules! channel_timetable_fields {
    ($($field:ident => $Variant:ident => $suffix:literal),+ $(,)?) => {
        /// One timetable header, named rather than positional.
        ///
        /// [`header_name()`](Self::header_name) composes the wire header a
        /// consumer wanting a single timestamp has to subscribe to, filter on or
        /// look up. The alternative is indexing
        /// [`ChannelTimetable::SUFFIXES`](ChannelTimetable::SUFFIXES) by
        /// position, which changes meaning if the table is ever reordered.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        #[non_exhaustive]
        pub enum TimetableField {
            $(
                #[doc = concat!("`", $suffix, "`, into [`ChannelTimetable::", stringify!($field), "`].")]
                $Variant,
            )+
        }

        impl TimetableField {
            /// Header suffix, without any prefix.
            pub fn as_str(&self) -> &'static str {
                match self {
                    $(Self::$Variant => $suffix,)+
                }
            }

            /// Full wire header name for a call leg.
            ///
            /// Takes the same prefix forms as
            /// [`ChannelTimetable::from_lookup`](ChannelTimetable::from_lookup):
            /// a [`TimetablePrefix`] or a raw string for the dynamic `Call-<n>`
            /// prefixes.
            ///
            /// ```
            /// use freeswitch_types::{TimetableField, TimetablePrefix};
            ///
            /// assert_eq!(
            ///     TimetableField::Answered.header_name(TimetablePrefix::OtherLeg),
            ///     "Other-Leg-Channel-Answered-Time"
            /// );
            /// ```
            pub fn header_name(&self, prefix: impl AsRef<str>) -> String {
                format!("{}-{}", prefix.as_ref(), self.as_str())
            }
        }

        impl ChannelTimetable {
        /// Header suffixes extracted by `from_lookup()`.
        ///
        /// Combine with a `TimetablePrefix` to build full header names for event
        /// subscriptions or filters:
        ///
        /// ```
        /// use freeswitch_types::{ChannelTimetable, TimetablePrefix};
        ///
        /// let prefix = TimetablePrefix::Caller.as_str();
        /// let headers: Vec<String> = ChannelTimetable::SUFFIXES
        ///     .iter()
        ///     .map(|suffix| format!("{prefix}-{suffix}"))
        ///     .collect();
        /// assert!(headers.contains(&"Caller-Channel-Created-Time".to_string()));
        /// ```
        pub const SUFFIXES: &'static [&'static str] = &[$($suffix),+];

        /// Extract a timetable by looking up prefixed header names via a closure.
        ///
        /// The closure receives full header names (e.g. `"Caller-Channel-Created-Time"`)
        /// and should return the raw value if present, in any casing the store holds
        /// it: a [`HeaderLookup`](crate::HeaderLookup) store's `header_str` does.
        ///
        /// Returns `Ok(None)` if no timestamp headers with this prefix are present.
        /// Returns `Err` if a header is present but contains an invalid (non-`i64`) value.
        ///
        #[cfg_attr(feature = "esl", doc = "```")]
        #[cfg_attr(not(feature = "esl"), doc = "```ignore")]
        /// use freeswitch_types::{ChannelTimetable, EslHeaders, HeaderLookup, TimetablePrefix};
        ///
        /// let mut headers = EslHeaders::new();
        /// headers.insert("caller-channel-created-time", "1700000001000000");
        ///
        /// // With enum:
        /// let tt = ChannelTimetable::from_lookup(TimetablePrefix::Caller, |k| headers.header_str(k));
        /// assert!(tt.unwrap().unwrap().created.is_some());
        ///
        /// // With raw string (e.g. for dynamic "Call-1" prefix):
        /// let tt = ChannelTimetable::from_lookup("Caller", |k| headers.header_str(k));
        /// assert!(tt.unwrap().unwrap().created.is_some());
        /// ```
        pub fn from_lookup<'a>(
            prefix: impl AsRef<str>,
            lookup: impl Fn(&str) -> Option<&'a str>,
        ) -> Result<Option<Self>, ParseTimetableError> {
            let prefix = prefix.as_ref();
            let mut tt = Self::default();
            let mut found = false;
            $(
                {
                    let header = TimetableField::$Variant.header_name(prefix);
                    if let Some(raw) = lookup(&header) {
                        let v: i64 = raw.parse().map_err(|_| ParseTimetableError {
                            header: header.clone(),
                            value: raw.to_string(),
                        })?;
                        tt.$field = Some(v);
                        found = true;
                    }
                }
            )+
            if found {
                Ok(Some(tt))
            } else {
                Ok(None)
            }
        }
        }
    };
}

channel_timetable_fields! {
    profile_created => ProfileCreated => "Profile-Created-Time",
    created         => Created        => "Channel-Created-Time",
    answered        => Answered       => "Channel-Answered-Time",
    progress        => Progress       => "Channel-Progress-Time",
    progress_media  => ProgressMedia  => "Channel-Progress-Media-Time",
    hungup          => Hungup         => "Channel-Hangup-Time",
    transferred     => Transferred    => "Channel-Transfer-Time",
    resurrected     => Resurrected    => "Channel-Resurrect-Time",
    bridged         => Bridged        => "Channel-Bridged-Time",
    last_hold       => LastHold       => "Channel-Last-Hold",
    hold_accum      => HoldAccum      => "Channel-Hold-Accum",
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ChannelState tests (display / from_str / wrong-case / unknown
    //     are generated by the `tests: channel_state_wire_tests` clause
    //     on the wire_enum! invocation above). ---

    #[test]
    fn channel_driver_is_the_segment_before_the_first_slash() {
        assert_eq!(
            channel_driver("sofia/internal/1000@example.com"),
            Some("sofia")
        );
        assert_eq!(channel_driver("loopback/9199-a"), Some("loopback"));
        assert_eq!(channel_driver("null/9199"), Some("null"));
    }

    #[test]
    fn channel_driver_none_without_a_segment() {
        for name in ["", "sofia", "/internal/1000"] {
            assert_eq!(channel_driver(name), None, "{name:?}");
        }
    }

    #[test]
    fn timetable_error_display_omits_the_value() {
        let err = ParseTimetableError::new("Caller-Channel-Created-Time", "5551234567");
        let msg = err.to_string();
        assert!(!msg.contains("5551234567"), "error quoted its input: {msg}");
        assert!(msg.contains("Caller-Channel-Created-Time"), "{msg}");
        assert_eq!(err.value, "5551234567");
    }

    #[test]
    fn test_channel_state_from_number() {
        assert_eq!(ChannelState::from_number(0), Some(ChannelState::CsNew));
        assert_eq!(ChannelState::from_number(4), Some(ChannelState::CsExecute));
        assert_eq!(ChannelState::from_number(10), Some(ChannelState::CsHangup));
        assert_eq!(ChannelState::from_number(13), Some(ChannelState::CsNone));
        assert_eq!(ChannelState::from_number(14), None);
        assert_eq!(ChannelState::from_number(255), None);
    }

    #[test]
    fn test_channel_state_as_number() {
        assert_eq!(ChannelState::CsNew.as_number(), 0);
        assert_eq!(ChannelState::CsExecute.as_number(), 4);
        assert_eq!(ChannelState::CsHangup.as_number(), 10);
        assert_eq!(ChannelState::CsNone.as_number(), 13);
    }

    #[test]
    fn channel_state_ordering_follows_lifecycle() {
        assert!(ChannelState::CsNew < ChannelState::CsInit);
        assert!(ChannelState::CsInit < ChannelState::CsRouting);
        assert!(ChannelState::CsRouting < ChannelState::CsExecute);
        assert!(ChannelState::CsExecute < ChannelState::CsHangup);
        assert!(ChannelState::CsHangup < ChannelState::CsReporting);
        assert!(ChannelState::CsReporting < ChannelState::CsDestroy);
    }

    // Negated >= reads as "not yet in teardown", which is the real consumer intent.
    #[allow(clippy::nonminimal_bool)]
    #[test]
    fn channel_state_teardown_check() {
        assert!(ChannelState::CsHangup >= ChannelState::CsHangup);
        assert!(ChannelState::CsReporting >= ChannelState::CsHangup);
        assert!(ChannelState::CsDestroy >= ChannelState::CsHangup);
        assert!(!(ChannelState::CsExecute >= ChannelState::CsHangup));
        assert!(!(ChannelState::CsPark >= ChannelState::CsHangup));
    }

    #[test]
    fn only_destroy_is_terminal() {
        assert!(ChannelState::CsDestroy.is_terminal());
        for state in ChannelState::ALL {
            if *state == ChannelState::CsDestroy {
                continue;
            }
            assert!(!state.is_terminal(), "{state} is not an end of life");
        }
    }

    // CS_NONE sorts above CS_DESTROY, so the `>= CsHangup` idiom above sweeps it
    // in; the switch aborts if its state machine ever reaches it, so it ends
    // nothing.
    #[test]
    fn cs_none_outranks_destroy_without_being_terminal() {
        assert!(ChannelState::CsNone > ChannelState::CsDestroy);
        assert!(ChannelState::CsNone >= ChannelState::CsHangup);
        assert!(!ChannelState::CsNone.is_terminal());
    }

    // --- CallState tests (display / from_str / wrong-case / unknown are
    //     generated by the `tests: call_state_wire_tests` clause on the
    //     wire_enum! invocation above; only the ordering check stays
    //     manual because it depends on enum-lexical order, not wire
    //     format). ---

    #[test]
    fn call_state_ordering_matches_c_enum() {
        assert!(CallState::Down < CallState::Dialing);
        assert!(CallState::Dialing < CallState::Ringing);
        assert!(CallState::Early < CallState::Active);
        assert!(CallState::Active < CallState::Hangup);
    }

    // --- AnswerState / CallDirection wire tests are generated by
    //     `tests: answer_state_wire_tests` / `tests: call_direction_wire_tests`
    //     clauses on their wire_enum! invocations above. ---

    // --- HangupCause tests ---

    #[test]
    fn hangup_cause_as_number_q850() {
        assert_eq!(HangupCause::None.as_number(), 0);
        assert_eq!(HangupCause::UnallocatedNumber.as_number(), 1);
        assert_eq!(HangupCause::NormalClearing.as_number(), 16);
        assert_eq!(HangupCause::UserBusy.as_number(), 17);
        assert_eq!(HangupCause::NoAnswer.as_number(), 19);
        assert_eq!(HangupCause::CallRejected.as_number(), 21);
        assert_eq!(HangupCause::NormalUnspecified.as_number(), 31);
        assert_eq!(HangupCause::Interworking.as_number(), 127);
    }

    #[test]
    fn hangup_cause_as_number_freeswitch_extensions() {
        assert_eq!(HangupCause::Success.as_number(), 142);
        assert_eq!(HangupCause::OriginatorCancel.as_number(), 487);
        assert_eq!(HangupCause::Crash.as_number(), 700);
        assert_eq!(HangupCause::BlindTransfer.as_number(), 800);
        assert_eq!(HangupCause::InvalidIdentity.as_number(), 823);
    }

    #[test]
    fn hangup_cause_from_number_round_trip() {
        let codes: &[u16] = &[0, 1, 16, 17, 19, 21, 31, 127, 142, 487, 700, 800, 823];
        for &code in codes {
            let cause = HangupCause::from_number(code).unwrap();
            assert_eq!(cause.as_number(), code);
        }
    }

    #[test]
    fn hangup_cause_from_number_unknown() {
        assert!(HangupCause::from_number(999).is_none());
        assert!(HangupCause::from_number(4).is_none());
    }

    #[test]
    fn hangup_cause_value_round_trips_every_known_cause() {
        for &cause in HangupCause::ALL {
            let value: HangupCauseValue = cause
                .as_str()
                .parse()
                .unwrap();
            assert_eq!(value, HangupCauseValue::Known(cause));
            assert_eq!(value.as_str(), cause.as_str());
            assert_eq!(value.to_string(), cause.as_str());
            assert_eq!(value.known(), Some(cause));
        }
    }

    #[test]
    fn hangup_cause_value_keeps_an_unlisted_name_verbatim() {
        for name in ["UNKNOWN", "SOME_FUTURE_CAUSE", "Normal_Clearing", ""] {
            let value: HangupCauseValue = name
                .parse()
                .unwrap();
            assert_eq!(value, HangupCauseValue::Unrecognized(name.into()));
            assert_eq!(value.to_string(), name);
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn hangup_cause_value_serde_round_trips() {
        let mut values: Vec<HangupCauseValue> = HangupCause::ALL
            .iter()
            .map(|&c| c.into())
            .collect();
        values.push(HangupCauseValue::Unrecognized("UNKNOWN".into()));
        values.push(HangupCauseValue::Unrecognized("SOME_FUTURE_CAUSE".into()));
        for value in values {
            let json = serde_json::to_string(&value).unwrap();
            assert_eq!(json, format!("\"{}\"", value.as_str()));
            let back: HangupCauseValue = serde_json::from_str(&json).unwrap();
            assert_eq!(back, value);
        }
    }

    /// Stored data from 2.x wrote the variant name; it must read back as Known.
    #[cfg(feature = "serde")]
    #[test]
    fn hangup_cause_value_serde_reads_variant_names() {
        for &cause in HangupCause::ALL {
            let json = serde_json::to_string(&format!("{cause:?}")).unwrap();
            let back: HangupCauseValue = serde_json::from_str(&json).unwrap();
            assert_eq!(back, HangupCauseValue::Known(cause), "{json}");
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn hangup_cause_value_serde_error_does_not_quote_the_input() {
        let err = serde_json::from_str::<HangupCauseValue>("1234567")
            .unwrap_err()
            .to_string();
        assert!(err.contains("hangup cause"), "{err}");
        assert!(!err.contains("1234567"), "{err}");
    }

    // --- from_sip_response tests (mapping from sofia_glue_sip_cause_to_freeswitch) ---

    #[test]
    fn from_sip_response_success() {
        assert_eq!(
            HangupCause::from_sip_response(200),
            Some(HangupCause::NormalClearing)
        );
    }

    #[test]
    fn from_sip_response_4xx_auth_rejection() {
        for code in [401, 402, 403, 407] {
            assert_eq!(
                HangupCause::from_sip_response(code),
                Some(HangupCause::CallRejected),
                "SIP {code}"
            );
        }
    }

    #[test]
    fn from_sip_response_4xx_routing() {
        assert_eq!(
            HangupCause::from_sip_response(404),
            Some(HangupCause::UnallocatedNumber)
        );
        assert_eq!(
            HangupCause::from_sip_response(485),
            Some(HangupCause::NoRouteDestination)
        );
        assert_eq!(
            HangupCause::from_sip_response(484),
            Some(HangupCause::InvalidNumberFormat)
        );
        assert_eq!(
            HangupCause::from_sip_response(410),
            Some(HangupCause::NumberChanged)
        );
    }

    #[test]
    fn from_sip_response_4xx_service() {
        assert_eq!(
            HangupCause::from_sip_response(405),
            Some(HangupCause::ServiceUnavailable)
        );
        for code in [406, 415, 501] {
            assert_eq!(
                HangupCause::from_sip_response(code),
                Some(HangupCause::ServiceNotImplemented),
                "SIP {code}"
            );
        }
    }

    #[test]
    fn from_sip_response_4xx_interworking() {
        for code in [413, 414, 416, 420, 421, 423, 505, 513] {
            assert_eq!(
                HangupCause::from_sip_response(code),
                Some(HangupCause::Interworking),
                "SIP {code}"
            );
        }
    }

    #[test]
    fn from_sip_response_4xx_timeout_and_busy() {
        assert_eq!(
            HangupCause::from_sip_response(408),
            Some(HangupCause::RecoveryOnTimerExpire)
        );
        assert_eq!(
            HangupCause::from_sip_response(504),
            Some(HangupCause::RecoveryOnTimerExpire)
        );
        assert_eq!(
            HangupCause::from_sip_response(480),
            Some(HangupCause::NoUserResponse)
        );
        assert_eq!(
            HangupCause::from_sip_response(486),
            Some(HangupCause::UserBusy)
        );
        assert_eq!(
            HangupCause::from_sip_response(487),
            Some(HangupCause::OriginatorCancel)
        );
    }

    #[test]
    fn from_sip_response_4xx_temporary_failure() {
        for code in [400, 481, 500, 503] {
            assert_eq!(
                HangupCause::from_sip_response(code),
                Some(HangupCause::NormalTemporaryFailure),
                "SIP {code}"
            );
        }
    }

    #[test]
    fn from_sip_response_4xx_exchange_routing() {
        for code in [482, 483] {
            assert_eq!(
                HangupCause::from_sip_response(code),
                Some(HangupCause::ExchangeRoutingError),
                "SIP {code}"
            );
        }
    }

    #[test]
    fn from_sip_response_4xx_media() {
        assert_eq!(
            HangupCause::from_sip_response(488),
            Some(HangupCause::IncompatibleDestination)
        );
        assert_eq!(
            HangupCause::from_sip_response(606),
            Some(HangupCause::IncompatibleDestination)
        );
    }

    #[test]
    fn from_sip_response_5xx() {
        assert_eq!(
            HangupCause::from_sip_response(502),
            Some(HangupCause::NetworkOutOfOrder)
        );
    }

    #[test]
    fn from_sip_response_6xx() {
        assert_eq!(
            HangupCause::from_sip_response(600),
            Some(HangupCause::UserBusy)
        );
        assert_eq!(
            HangupCause::from_sip_response(603),
            Some(HangupCause::CallRejected)
        );
        assert_eq!(
            HangupCause::from_sip_response(604),
            Some(HangupCause::NoRouteDestination)
        );
        assert_eq!(
            HangupCause::from_sip_response(607),
            Some(HangupCause::Unwanted)
        );
        assert_eq!(
            HangupCause::from_sip_response(608),
            Some(HangupCause::CallRejected)
        );
    }

    #[test]
    fn from_sip_response_stir_shaken() {
        assert_eq!(
            HangupCause::from_sip_response(428),
            Some(HangupCause::NoIdentity)
        );
        assert_eq!(
            HangupCause::from_sip_response(429),
            Some(HangupCause::BadIdentityInfo)
        );
        assert_eq!(
            HangupCause::from_sip_response(437),
            Some(HangupCause::UnsupportedCertificate)
        );
        assert_eq!(
            HangupCause::from_sip_response(438),
            Some(HangupCause::InvalidIdentity)
        );
    }

    #[test]
    fn from_sip_response_unmapped_returns_none() {
        // Provisional/success ranges without explicit mapping
        for code in [100, 180, 183, 301, 302] {
            assert_eq!(
                HangupCause::from_sip_response(code),
                None,
                "SIP {code} should be None"
            );
        }
        // Unmapped 4xx/5xx
        for code in [409, 411, 412, 422, 489, 491, 493, 506, 580] {
            assert_eq!(
                HangupCause::from_sip_response(code),
                None,
                "SIP {code} should be None"
            );
        }
    }

    #[test]
    fn timetable_field_names_one_header() {
        assert_eq!(
            TimetableField::ProfileCreated.as_str(),
            "Profile-Created-Time"
        );
        assert_eq!(
            TimetableField::Created.header_name(TimetablePrefix::Caller),
            "Caller-Channel-Created-Time"
        );
        // Raw prefixes too, for the dynamic `Call-<n>` form from_lookup accepts.
        assert_eq!(
            TimetableField::Hungup.header_name("Call-1"),
            "Call-1-Channel-Hangup-Time"
        );
    }

    #[test]
    fn timetable_fields_and_suffixes_come_from_one_table() {
        let fields = [
            TimetableField::ProfileCreated,
            TimetableField::Created,
            TimetableField::Answered,
            TimetableField::Progress,
            TimetableField::ProgressMedia,
            TimetableField::Hungup,
            TimetableField::Transferred,
            TimetableField::Resurrected,
            TimetableField::Bridged,
            TimetableField::LastHold,
            TimetableField::HoldAccum,
        ];
        assert_eq!(fields.len(), ChannelTimetable::SUFFIXES.len());
        for (field, suffix) in fields
            .iter()
            .zip(ChannelTimetable::SUFFIXES)
        {
            assert_eq!(field.as_str(), *suffix);
        }
    }

    #[cfg(feature = "esl")]
    mod from_events {
        use super::*;
        use crate::event::EslEvent;
        use crate::lookup::HeaderLookup;

        #[test]
        fn caller_timetable_all_fields() {
            let mut event = EslEvent::new();
            event.set_header("Caller-Profile-Created-Time", "1700000000000000");
            event.set_header("Caller-Channel-Created-Time", "1700000001000000");
            event.set_header("Caller-Channel-Answered-Time", "1700000005000000");
            event.set_header("Caller-Channel-Progress-Time", "1700000002000000");
            event.set_header("Caller-Channel-Progress-Media-Time", "1700000003000000");
            event.set_header("Caller-Channel-Hangup-Time", "0");
            event.set_header("Caller-Channel-Transfer-Time", "0");
            event.set_header("Caller-Channel-Resurrect-Time", "0");
            event.set_header("Caller-Channel-Bridged-Time", "1700000006000000");
            event.set_header("Caller-Channel-Last-Hold", "0");
            event.set_header("Caller-Channel-Hold-Accum", "0");

            let tt = event
                .caller_timetable()
                .unwrap()
                .expect("should have timetable");
            assert_eq!(tt.profile_created, Some(1700000000000000));
            assert_eq!(tt.created, Some(1700000001000000));
            assert_eq!(tt.answered, Some(1700000005000000));
            assert_eq!(tt.progress, Some(1700000002000000));
            assert_eq!(tt.progress_media, Some(1700000003000000));
            assert_eq!(tt.hungup, Some(0));
            assert_eq!(tt.transferred, Some(0));
            assert_eq!(tt.resurrected, Some(0));
            assert_eq!(tt.bridged, Some(1700000006000000));
            assert_eq!(tt.last_hold, Some(0));
            assert_eq!(tt.hold_accum, Some(0));
        }

        #[test]
        fn other_leg_timetable() {
            let mut event = EslEvent::new();
            event.set_header("Other-Leg-Profile-Created-Time", "1700000000000000");
            event.set_header("Other-Leg-Channel-Created-Time", "1700000001000000");
            event.set_header("Other-Leg-Channel-Answered-Time", "1700000005000000");
            event.set_header("Other-Leg-Channel-Progress-Time", "0");
            event.set_header("Other-Leg-Channel-Progress-Media-Time", "0");
            event.set_header("Other-Leg-Channel-Hangup-Time", "0");
            event.set_header("Other-Leg-Channel-Transfer-Time", "0");
            event.set_header("Other-Leg-Channel-Resurrect-Time", "0");
            event.set_header("Other-Leg-Channel-Bridged-Time", "1700000006000000");
            event.set_header("Other-Leg-Channel-Last-Hold", "0");
            event.set_header("Other-Leg-Channel-Hold-Accum", "0");

            let tt = event
                .other_leg_timetable()
                .unwrap()
                .expect("should have timetable");
            assert_eq!(tt.created, Some(1700000001000000));
            assert_eq!(tt.bridged, Some(1700000006000000));
        }

        #[test]
        fn timetable_no_headers() {
            let event = EslEvent::new();
            assert_eq!(
                event
                    .caller_timetable()
                    .unwrap(),
                None
            );
            assert_eq!(
                event
                    .other_leg_timetable()
                    .unwrap(),
                None
            );
        }

        #[test]
        fn timetable_partial_headers() {
            let mut event = EslEvent::new();
            event.set_header("Caller-Channel-Created-Time", "1700000001000000");

            let tt = event
                .caller_timetable()
                .unwrap()
                .expect("at least one field parsed");
            assert_eq!(tt.created, Some(1700000001000000));
            assert_eq!(tt.answered, None);
            assert_eq!(tt.profile_created, None);
        }

        #[test]
        fn timetable_invalid_value_is_error() {
            let mut event = EslEvent::new();
            event.set_header("Caller-Channel-Created-Time", "not_a_number");

            let err = event
                .caller_timetable()
                .unwrap_err();
            assert_eq!(err.header, "Caller-Channel-Created-Time");
            assert_eq!(err.value, "not_a_number");
        }

        #[test]
        fn timetable_valid_then_invalid_is_error() {
            let mut event = EslEvent::new();
            event.set_header("Caller-Profile-Created-Time", "1700000000000000");
            event.set_header("Caller-Channel-Created-Time", "garbage");

            let err = event
                .caller_timetable()
                .unwrap_err();
            assert_eq!(err.header, "Caller-Channel-Created-Time");
            assert_eq!(err.value, "garbage");
        }

        #[test]
        fn timetable_zero_preserved() {
            let mut event = EslEvent::new();
            event.set_header("Caller-Channel-Hangup-Time", "0");

            let tt = event
                .caller_timetable()
                .unwrap()
                .expect("should have timetable");
            assert_eq!(tt.hungup, Some(0));
        }

        #[test]
        fn timetable_field_header_name_is_what_from_lookup_reads() {
            let mut event = EslEvent::new();
            event.set_header(
                TimetableField::Bridged.header_name(TimetablePrefix::OtherLeg),
                "1700000006000000",
            );

            let tt = event
                .other_leg_timetable()
                .unwrap()
                .expect("should have timetable");
            assert_eq!(tt.bridged, Some(1700000006000000));
        }

        #[test]
        fn timetable_custom_prefix() {
            let mut event = EslEvent::new();
            event.set_header("Channel-Channel-Created-Time", "1700000001000000");

            let tt = event
                .timetable("Channel")
                .unwrap()
                .expect("custom prefix should work");
            assert_eq!(tt.created, Some(1700000001000000));
        }
    }
}
