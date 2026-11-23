//! Typed Sofia event subclass names (`Event-Subclass` header values).

wire_enum! {
    /// Sofia event subclass values from `mod_sofia.h` `MY_EVENT_*` defines.
    ///
    /// These appear as the `Event-Subclass` header on `CUSTOM` events fired by
    /// mod_sofia. Use with [`EventSubscription::with_sofia_event()`](crate::EventSubscription::with_sofia_event)
    /// for typed subscriptions, or parse from received events via
    /// [`HeaderLookup::sofia_event_subclass()`](crate::HeaderLookup::sofia_event_subclass).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum SofiaEventSubclass {
        Register => "sofia::register",
        PreRegister => "sofia::pre_register",
        RegisterAttempt => "sofia::register_attempt",
        RegisterFailure => "sofia::register_failure",
        Unregister => "sofia::unregister",
        Expire => "sofia::expire",
        GatewayState => "sofia::gateway_state",
        SipUserState => "sofia::sip_user_state",
        NotifyRefer => "sofia::notify_refer",
        Reinvite => "sofia::reinvite",
        GatewayAdd => "sofia::gateway_add",
        GatewayDelete => "sofia::gateway_delete",
        GatewayInvalidDigestReq => "sofia::gateway_invalid_digest_req",
        RecoveryRecv => "sofia::recovery_recv",
        RecoverySend => "sofia::recovery_send",
        RecoveryRecovered => "sofia::recovery_recovered",
        Error => "sofia::error",
        ProfileStart => "sofia::profile_start",
        NotifyWatchedHeader => "sofia::notify_watched_header",
        WrongCallState => "sofia::wrong_call_state",
        Transferor => "sofia::transferor",
        Transferee => "sofia::transferee",
        Replaced => "sofia::replaced",
        Intercepted => "sofia::intercepted",
        ByeResponse => "sofia::bye_response",
    }
    error ParseSofiaEventSubclassError("sofia event subclass");
    tests: sofia_event_subclass_wire_tests;
}

impl SofiaEventSubclass {
    /// Registration-related subclasses.
    pub const REGISTRATION_EVENTS: &[SofiaEventSubclass] = &[
        Self::Register,
        Self::PreRegister,
        Self::RegisterAttempt,
        Self::RegisterFailure,
        Self::Unregister,
        Self::Expire,
    ];

    /// Gateway lifecycle and state subclasses.
    pub const GATEWAY_EVENTS: &[SofiaEventSubclass] = &[
        Self::GatewayState,
        Self::GatewayAdd,
        Self::GatewayDelete,
        Self::GatewayInvalidDigestReq,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_events_subset() {
        for &ev in SofiaEventSubclass::REGISTRATION_EVENTS {
            assert!(SofiaEventSubclass::ALL.contains(&ev), "{ev} not in ALL");
        }
    }

    #[test]
    fn gateway_events_subset() {
        for &ev in SofiaEventSubclass::GATEWAY_EVENTS {
            assert!(SofiaEventSubclass::ALL.contains(&ev), "{ev} not in ALL");
        }
    }
}
