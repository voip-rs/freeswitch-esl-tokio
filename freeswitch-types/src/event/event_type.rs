use std::fmt;
use std::str::FromStr;

/// Generates `EslEventType` enum with `Display`, `FromStr`, `as_str`, `parse_event_type`,
/// and the six predefined event-group constants.
///
/// Each variant row carries an optional `[group, ...]` tag list. The macro
/// generates `CHANNEL_EVENTS`, `IN_CALL_EVENTS`, `MEDIA_EVENTS`,
/// `PRESENCE_EVENTS`, `SYSTEM_EVENTS`, and `CONFERENCE_EVENTS` by filtering the
/// variant table — no separate sync step required.
macro_rules! esl_event_types {
    // ------------------------------------------------------------------
    // Filter arms — TT-munch through variant tuples `(Variant [tags...])`,
    // accumulating those whose tag list contains the target group.
    // Accumulator is `[$($acc:ident),*]`; items are space-separated tuples.
    // ------------------------------------------------------------------

    // channel
    (@filter_channel [$($acc:ident),*]) => { &[$(EslEventType::$acc,)*] };
    (@filter_channel [$($acc:ident),*] ($v:ident [channel $(,$_g:ident)*]) $($tail:tt)*) => {
        esl_event_types!(@filter_channel [$($acc,)* $v] $($tail)*)
    };
    (@filter_channel [$($acc:ident),*] ($v:ident [$_h:ident $(,$tg:ident)*]) $($tail:tt)*) => {
        esl_event_types!(@filter_channel [$($acc),*] ($v [$($tg),*]) $($tail)*)
    };
    (@filter_channel [$($acc:ident),*] ($v:ident []) $($tail:tt)*) => {
        esl_event_types!(@filter_channel [$($acc),*] $($tail)*)
    };

    // in_call
    (@filter_in_call [$($acc:ident),*]) => { &[$(EslEventType::$acc,)*] };
    (@filter_in_call [$($acc:ident),*] ($v:ident [in_call $(,$_g:ident)*]) $($tail:tt)*) => {
        esl_event_types!(@filter_in_call [$($acc,)* $v] $($tail)*)
    };
    (@filter_in_call [$($acc:ident),*] ($v:ident [$_h:ident $(,$tg:ident)*]) $($tail:tt)*) => {
        esl_event_types!(@filter_in_call [$($acc),*] ($v [$($tg),*]) $($tail)*)
    };
    (@filter_in_call [$($acc:ident),*] ($v:ident []) $($tail:tt)*) => {
        esl_event_types!(@filter_in_call [$($acc),*] $($tail)*)
    };

    // media
    (@filter_media [$($acc:ident),*]) => { &[$(EslEventType::$acc,)*] };
    (@filter_media [$($acc:ident),*] ($v:ident [media $(,$_g:ident)*]) $($tail:tt)*) => {
        esl_event_types!(@filter_media [$($acc,)* $v] $($tail)*)
    };
    (@filter_media [$($acc:ident),*] ($v:ident [$_h:ident $(,$tg:ident)*]) $($tail:tt)*) => {
        esl_event_types!(@filter_media [$($acc),*] ($v [$($tg),*]) $($tail)*)
    };
    (@filter_media [$($acc:ident),*] ($v:ident []) $($tail:tt)*) => {
        esl_event_types!(@filter_media [$($acc),*] $($tail)*)
    };

    // presence
    (@filter_presence [$($acc:ident),*]) => { &[$(EslEventType::$acc,)*] };
    (@filter_presence [$($acc:ident),*] ($v:ident [presence $(,$_g:ident)*]) $($tail:tt)*) => {
        esl_event_types!(@filter_presence [$($acc,)* $v] $($tail)*)
    };
    (@filter_presence [$($acc:ident),*] ($v:ident [$_h:ident $(,$tg:ident)*]) $($tail:tt)*) => {
        esl_event_types!(@filter_presence [$($acc),*] ($v [$($tg),*]) $($tail)*)
    };
    (@filter_presence [$($acc:ident),*] ($v:ident []) $($tail:tt)*) => {
        esl_event_types!(@filter_presence [$($acc),*] $($tail)*)
    };

    // system
    (@filter_system [$($acc:ident),*]) => { &[$(EslEventType::$acc,)*] };
    (@filter_system [$($acc:ident),*] ($v:ident [system $(,$_g:ident)*]) $($tail:tt)*) => {
        esl_event_types!(@filter_system [$($acc,)* $v] $($tail)*)
    };
    (@filter_system [$($acc:ident),*] ($v:ident [$_h:ident $(,$tg:ident)*]) $($tail:tt)*) => {
        esl_event_types!(@filter_system [$($acc),*] ($v [$($tg),*]) $($tail)*)
    };
    (@filter_system [$($acc:ident),*] ($v:ident []) $($tail:tt)*) => {
        esl_event_types!(@filter_system [$($acc),*] $($tail)*)
    };

    // conference
    (@filter_conference [$($acc:ident),*]) => { &[$(EslEventType::$acc,)*] };
    (@filter_conference [$($acc:ident),*] ($v:ident [conference $(,$_g:ident)*]) $($tail:tt)*) => {
        esl_event_types!(@filter_conference [$($acc,)* $v] $($tail)*)
    };
    (@filter_conference [$($acc:ident),*] ($v:ident [$_h:ident $(,$tg:ident)*]) $($tail:tt)*) => {
        esl_event_types!(@filter_conference [$($acc),*] ($v [$($tg),*]) $($tail)*)
    };
    (@filter_conference [$($acc:ident),*] ($v:ident []) $($tail:tt)*) => {
        esl_event_types!(@filter_conference [$($acc),*] $($tail)*)
    };

    // ------------------------------------------------------------------
    // Main arm — generate enum, Display, FromStr, and group constants.
    // Each variant row: `Variant => "WIRE_NAME" [group, ...]`
    // Empty tag list `[]` = no group membership.
    // ------------------------------------------------------------------
    (
        $(
            $(#[$attr:meta])*
            $variant:ident => $wire:literal [$($vgroup:ident),*]
        ),+ $(,)?
        ;
        // Extra variants not in the main match (after All)
        $(
            $(#[$extra_attr:meta])*
            $extra_variant:ident => $extra_wire:literal [$($egroup:ident),*]
        ),* $(,)?
    ) => {
        /// FreeSWITCH event types matching the canonical order from `esl_event.h`
        /// and `switch_event.c` EVENT_NAMES[].
        ///
        /// Variant names are the canonical wire names (e.g. `ChannelCreate` = `CHANNEL_CREATE`).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        #[non_exhaustive]
        #[allow(missing_docs)]
        pub enum EslEventType {
            $(
                $(#[$attr])*
                $variant,
            )+
            $(
                $(#[$extra_attr])*
                $extra_variant,
            )*
        }

        impl fmt::Display for EslEventType {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl EslEventType {
            /// Returns the canonical wire name as a static string slice.
            pub const fn as_str(&self) -> &'static str {
                match self {
                    $( EslEventType::$variant => $wire, )+
                    $( EslEventType::$extra_variant => $extra_wire, )*
                }
            }

            /// Parse event type from wire name (canonical case).
            pub fn parse_event_type(s: &str) -> Option<Self> {
                match s {
                    $( $wire => Some(EslEventType::$variant), )+
                    $( $extra_wire => Some(EslEventType::$extra_variant), )*
                    _ => None,
                }
            }

            // Group constants, filtered from the `[group, ...]` tag on each row.

            #[doc = "Every `CHANNEL_*` event type."]
            #[doc = ""]
            #[doc = "Covers the full channel lifecycle: creation, state changes, execution,"]
            #[doc = "bridging, hold, park, progress, originate, and destruction."]
            #[doc = ""]
            #[doc = "```rust"]
            #[doc = "use freeswitch_types::EslEventType;"]
            #[doc = "assert!(EslEventType::CHANNEL_EVENTS.contains(&EslEventType::ChannelCreate));"]
            #[doc = "assert!(EslEventType::CHANNEL_EVENTS.contains(&EslEventType::ChannelHangupComplete));"]
            #[doc = "```"]
            pub const CHANNEL_EVENTS: &[EslEventType] = esl_event_types!(
                @filter_channel []
                $(($variant [$($vgroup),*]))+
                $(($extra_variant [$($egroup),*]))*
            );

            #[doc = "In-call events: DTMF, VAD speech detection, media security, and call updates."]
            #[doc = ""]
            #[doc = "Events that fire during an established call, tied to RTP/media activity"]
            #[doc = "rather than signaling state transitions."]
            #[doc = ""]
            #[doc = "```rust"]
            #[doc = "use freeswitch_types::EslEventType;"]
            #[doc = "assert!(EslEventType::IN_CALL_EVENTS.contains(&EslEventType::Dtmf));"]
            #[doc = "assert!(EslEventType::IN_CALL_EVENTS.contains(&EslEventType::Talk));"]
            #[doc = "```"]
            pub const IN_CALL_EVENTS: &[EslEventType] = esl_event_types!(
                @filter_in_call []
                $(($variant [$($vgroup),*]))+
                $(($extra_variant [$($egroup),*]))*
            );

            #[doc = "Media-related events: playback, recording, media bugs, and detection."]
            #[doc = ""]
            #[doc = "Useful for IVR applications that need to track media operations without"]
            #[doc = "subscribing to the full channel lifecycle."]
            #[doc = ""]
            #[doc = "```rust"]
            #[doc = "use freeswitch_types::EslEventType;"]
            #[doc = "assert!(EslEventType::MEDIA_EVENTS.contains(&EslEventType::PlaybackStart));"]
            #[doc = "assert!(EslEventType::MEDIA_EVENTS.contains(&EslEventType::DetectedSpeech));"]
            #[doc = "```"]
            pub const MEDIA_EVENTS: &[EslEventType] = esl_event_types!(
                @filter_media []
                $(($variant [$($vgroup),*]))+
                $(($extra_variant [$($egroup),*]))*
            );

            #[doc = "Presence and messaging events."]
            #[doc = ""]
            #[doc = "For applications that track user presence (BLF, buddy lists) or"]
            #[doc = "message-waiting indicators (voicemail MWI)."]
            #[doc = ""]
            #[doc = "```rust"]
            #[doc = "use freeswitch_types::EslEventType;"]
            #[doc = "assert!(EslEventType::PRESENCE_EVENTS.contains(&EslEventType::PresenceIn));"]
            #[doc = "assert!(EslEventType::PRESENCE_EVENTS.contains(&EslEventType::MessageWaiting));"]
            #[doc = "```"]
            pub const PRESENCE_EVENTS: &[EslEventType] = esl_event_types!(
                @filter_presence []
                $(($variant [$($vgroup),*]))+
                $(($extra_variant [$($egroup),*]))*
            );

            #[doc = "System lifecycle events."]
            #[doc = ""]
            #[doc = "Server startup/shutdown, heartbeats, module loading, and XML reloads."]
            #[doc = "Useful for monitoring dashboards and operational tooling."]
            #[doc = ""]
            #[doc = "```rust"]
            #[doc = "use freeswitch_types::EslEventType;"]
            #[doc = "assert!(EslEventType::SYSTEM_EVENTS.contains(&EslEventType::Heartbeat));"]
            #[doc = "assert!(EslEventType::SYSTEM_EVENTS.contains(&EslEventType::Shutdown));"]
            #[doc = "```"]
            pub const SYSTEM_EVENTS: &[EslEventType] = esl_event_types!(
                @filter_system []
                $(($variant [$($vgroup),*]))+
                $(($extra_variant [$($egroup),*]))*
            );

            #[doc = "Conference-related events."]
            #[doc = ""]
            #[doc = "```rust"]
            #[doc = "use freeswitch_types::EslEventType;"]
            #[doc = "assert!(EslEventType::CONFERENCE_EVENTS.contains(&EslEventType::ConferenceData));"]
            #[doc = "```"]
            pub const CONFERENCE_EVENTS: &[EslEventType] = esl_event_types!(
                @filter_conference []
                $(($variant [$($vgroup),*]))+
                $(($extra_variant [$($egroup),*]))*
            );
        }

        impl FromStr for EslEventType {
            type Err = ParseEventTypeError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse_event_type(s).ok_or_else(|| ParseEventTypeError(s.to_string()))
            }
        }

        #[cfg(test)]
        mod esl_event_type_serde_tests {
            use super::*;

            wire_serde_test! {
                EslEventType,
                [$($variant => $wire,)+ $($extra_variant => $extra_wire),*]
            }
        }
    };
}

esl_event_types! {
    Custom               => "CUSTOM"                   [],
    Clone                => "CLONE"                    [],
    ChannelCreate        => "CHANNEL_CREATE"           [channel],
    ChannelDestroy       => "CHANNEL_DESTROY"          [channel],
    ChannelState         => "CHANNEL_STATE"            [channel],
    ChannelCallstate     => "CHANNEL_CALLSTATE"        [channel],
    ChannelAnswer        => "CHANNEL_ANSWER"           [channel],
    ChannelHangup        => "CHANNEL_HANGUP"           [channel],
    ChannelHangupComplete => "CHANNEL_HANGUP_COMPLETE" [channel],
    ChannelExecute       => "CHANNEL_EXECUTE"          [channel],
    ChannelExecuteComplete => "CHANNEL_EXECUTE_COMPLETE" [channel],
    ChannelHold          => "CHANNEL_HOLD"             [channel],
    ChannelUnhold        => "CHANNEL_UNHOLD"           [channel],
    ChannelBridge        => "CHANNEL_BRIDGE"           [channel],
    ChannelUnbridge      => "CHANNEL_UNBRIDGE"         [channel],
    ChannelProgress      => "CHANNEL_PROGRESS"         [channel],
    ChannelProgressMedia => "CHANNEL_PROGRESS_MEDIA"   [channel],
    ChannelOutgoing      => "CHANNEL_OUTGOING"         [channel],
    ChannelPark          => "CHANNEL_PARK"             [channel],
    ChannelUnpark        => "CHANNEL_UNPARK"           [channel],
    ChannelApplication   => "CHANNEL_APPLICATION"      [channel],
    ChannelOriginate     => "CHANNEL_ORIGINATE"        [channel],
    ChannelUuid          => "CHANNEL_UUID"             [channel],
    Api                  => "API"                      [],
    Log                  => "LOG"                      [],
    InboundChan          => "INBOUND_CHAN"             [],
    OutboundChan         => "OUTBOUND_CHAN"            [],
    Startup              => "STARTUP"                  [system],
    Shutdown             => "SHUTDOWN"                 [system],
    Publish              => "PUBLISH"                  [],
    Unpublish            => "UNPUBLISH"                [],
    Talk                 => "TALK"                     [in_call],
    Notalk               => "NOTALK"                   [in_call],
    SessionCrash         => "SESSION_CRASH"            [system],
    ModuleLoad           => "MODULE_LOAD"              [system],
    ModuleUnload         => "MODULE_UNLOAD"            [system],
    Dtmf                 => "DTMF"                     [in_call],
    Message              => "MESSAGE"                  [],
    PresenceIn           => "PRESENCE_IN"              [presence],
    NotifyIn             => "NOTIFY_IN"                [],
    PresenceOut          => "PRESENCE_OUT"             [presence],
    PresenceProbe        => "PRESENCE_PROBE"           [presence],
    MessageWaiting       => "MESSAGE_WAITING"          [presence],
    MessageQuery         => "MESSAGE_QUERY"            [presence],
    Roster               => "ROSTER"                   [presence],
    Codec                => "CODEC"                    [],
    BackgroundJob        => "BACKGROUND_JOB"           [],
    DetectedSpeech       => "DETECTED_SPEECH"          [media],
    DetectedTone         => "DETECTED_TONE"            [media],
    PrivateCommand       => "PRIVATE_COMMAND"          [],
    Heartbeat            => "HEARTBEAT"                [system],
    Trap                 => "TRAP"                     [],
    AddSchedule          => "ADD_SCHEDULE"             [],
    DelSchedule          => "DEL_SCHEDULE"             [],
    ExeSchedule          => "EXE_SCHEDULE"             [],
    ReSchedule           => "RE_SCHEDULE"              [],
    ReloadXml            => "RELOADXML"                [system],
    Notify               => "NOTIFY"                   [],
    PhoneFeature         => "PHONE_FEATURE"            [],
    PhoneFeatureSubscribe => "PHONE_FEATURE_SUBSCRIBE" [],
    SendMessage          => "SEND_MESSAGE"             [],
    RecvMessage          => "RECV_MESSAGE"             [],
    RequestParams        => "REQUEST_PARAMS"           [],
    ChannelData          => "CHANNEL_DATA"             [channel],
    General              => "GENERAL"                  [],
    Command              => "COMMAND"                  [],
    SessionHeartbeat     => "SESSION_HEARTBEAT"        [system],
    ClientDisconnected   => "CLIENT_DISCONNECTED"      [],
    ServerDisconnected   => "SERVER_DISCONNECTED"      [],
    SendInfo             => "SEND_INFO"                [],
    RecvInfo             => "RECV_INFO"                [],
    RecvRtcpMessage      => "RECV_RTCP_MESSAGE"        [in_call],
    SendRtcpMessage      => "SEND_RTCP_MESSAGE"        [in_call],
    CallSecure           => "CALL_SECURE"              [in_call],
    Nat                  => "NAT"                      [],
    RecordStart          => "RECORD_START"             [media],
    RecordStop           => "RECORD_STOP"              [media],
    PlaybackStart        => "PLAYBACK_START"           [media],
    PlaybackStop         => "PLAYBACK_STOP"            [media],
    CallUpdate           => "CALL_UPDATE"              [in_call],
    Failure              => "FAILURE"                  [],
    SocketData           => "SOCKET_DATA"              [],
    MediaBugStart        => "MEDIA_BUG_START"          [media],
    MediaBugStop         => "MEDIA_BUG_STOP"           [media],
    ConferenceDataQuery  => "CONFERENCE_DATA_QUERY"    [conference],
    ConferenceData       => "CONFERENCE_DATA"          [conference],
    CallSetupReq         => "CALL_SETUP_REQ"           [],
    CallSetupResult      => "CALL_SETUP_RESULT"        [],
    CallDetail           => "CALL_DETAIL"              [],
    DeviceState          => "DEVICE_STATE"             [],
    Text                 => "TEXT"                     [],
    ShutdownRequested    => "SHUTDOWN_REQUESTED"       [system],
    /// Subscribe to all events
    All                  => "ALL"                      [];
    // --- Not in libs/esl/ EVENT_NAMES[], only in switch_event.c ---
    // check-event-types.sh stops scanning at the All variant above.
    /// Present in `switch_event.c` but not in `libs/esl/` EVENT_NAMES[].
    StartRecording => "START_RECORDING" [media],
}

parse_error! { ParseEventTypeError("event type"); }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notify_in_parse() {
        assert_eq!(
            EslEventType::parse_event_type("NOTIFY_IN"),
            Some(EslEventType::NotifyIn)
        );
        assert_eq!(EslEventType::parse_event_type("notify_in"), None);
    }

    #[test]
    fn test_notify_in_display() {
        assert_eq!(EslEventType::NotifyIn.to_string(), "NOTIFY_IN");
    }

    #[test]
    fn test_notify_in_distinct_from_notify() {
        assert_ne!(EslEventType::Notify, EslEventType::NotifyIn);
        assert_ne!(
            EslEventType::Notify.to_string(),
            EslEventType::NotifyIn.to_string()
        );
    }

    #[test]
    fn test_wire_names_match_c_esl() {
        assert_eq!(
            EslEventType::ChannelOutgoing.to_string(),
            "CHANNEL_OUTGOING"
        );
        assert_eq!(EslEventType::Api.to_string(), "API");
        assert_eq!(EslEventType::ReloadXml.to_string(), "RELOADXML");
        assert_eq!(EslEventType::PresenceIn.to_string(), "PRESENCE_IN");
        assert_eq!(EslEventType::Roster.to_string(), "ROSTER");
        assert_eq!(EslEventType::Text.to_string(), "TEXT");
        assert_eq!(EslEventType::ReSchedule.to_string(), "RE_SCHEDULE");

        assert_eq!(
            EslEventType::parse_event_type("CHANNEL_OUTGOING"),
            Some(EslEventType::ChannelOutgoing)
        );
        assert_eq!(
            EslEventType::parse_event_type("API"),
            Some(EslEventType::Api)
        );
        assert_eq!(
            EslEventType::parse_event_type("RELOADXML"),
            Some(EslEventType::ReloadXml)
        );
        assert_eq!(
            EslEventType::parse_event_type("PRESENCE_IN"),
            Some(EslEventType::PresenceIn)
        );
    }

    #[test]
    fn test_event_type_from_str() {
        assert_eq!(
            "CHANNEL_ANSWER".parse::<EslEventType>(),
            Ok(EslEventType::ChannelAnswer)
        );
        assert!("channel_answer"
            .parse::<EslEventType>()
            .is_err());
        assert!("UNKNOWN_EVENT"
            .parse::<EslEventType>()
            .is_err());
    }
}
