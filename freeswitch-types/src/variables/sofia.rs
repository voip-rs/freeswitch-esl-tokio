//! Typed mod_sofia / SIP channel variable names.

use sip_header_catalog::SipHeader;

sip_header_catalog::define_header_enum! {
    tests_mod: sofia_variable_generated_tests,
    serde(cfg(feature = "serde")),
    error_type: ParseSofiaVariableError => "unknown sofia variable",
    /// mod_sofia / SIP channel variable names (the part after the `variable_` prefix).
    ///
    /// Use with [`HeaderLookup::variable()`](crate::HeaderLookup::variable) for type-safe lookups.
    /// Core FreeSWITCH variables belong in [`ChannelVariable`](super::ChannelVariable).
    pub enum SofiaVariable {
        // --- SIP From ---
        SipFromUser => "sip_from_user",
        SipFromHost => "sip_from_host",
        SipFromPort => "sip_from_port",
        SipFromUri => "sip_from_uri",
        SipFromDisplay => "sip_from_display",
        SipFromTag => "sip_from_tag",
        SipFromComment => "sip_from_comment",
        SipFromUserStripped => "sip_from_user_stripped",
        SipFullFrom => "sip_full_from",

        // --- SIP To ---
        SipToUser => "sip_to_user",
        SipToHost => "sip_to_host",
        SipToPort => "sip_to_port",
        SipToUri => "sip_to_uri",
        SipToDisplay => "sip_to_display",
        SipToTag => "sip_to_tag",
        SipToComment => "sip_to_comment",
        SipFullTo => "sip_full_to",

        // --- SIP Contact ---
        SipContactUser => "sip_contact_user",
        SipContactHost => "sip_contact_host",
        SipContactPort => "sip_contact_port",
        SipContactUri => "sip_contact_uri",
        SipContactParams => "sip_contact_params",

        // --- SIP Request ---
        SipReqUser => "sip_req_user",
        SipReqHost => "sip_req_host",
        SipReqPort => "sip_req_port",
        SipReqUri => "sip_req_uri",

        // --- SIP Via ---
        SipViaHost => "sip_via_host",
        SipViaPort => "sip_via_port",
        SipViaRport => "sip_via_rport",
        SipViaProtocol => "sip_via_protocol",
        SipFullVia => "sip_full_via",
        SipFullRoute => "sip_full_route",

        // --- SIP Session ---
        SipCallId => "sip_call_id",
        SipCseq => "sip_cseq",
        SipUserAgent => "sip_user_agent",
        SipSubject => "sip_subject",
        SipAllow => "sip_allow",
        SipAcceptLanguage => "sip_accept_language",
        SipCallInfo => "sip_call_info",
        SipDateEpochTime => "sip_date_epoch_time",

        // --- SIP Network ---
        SipReceivedIp => "sip_received_ip",
        SipReceivedPort => "sip_received_port",
        SipNetworkIp => "sip_network_ip",
        SipNetworkPort => "sip_network_port",
        SipNatDetected => "sip_nat_detected",
        SipTransport => "sip_transport",
        SipReplyHost => "sip_reply_host",

        // --- SIP Auth ---
        SipAuthUsername => "sip_auth_username",
        SipAuthPassword => "sip_auth_password",
        SipAuthorized => "sip_authorized",
        SipAclAuthedBy => "sip_acl_authed_by",
        SipAclToken => "sip_acl_token",
        SipChallengeRealm => "sip_challenge_realm",

        // --- SIP Failure / Hangup ---
        SipInviteFailureStatus => "sip_invite_failure_status",
        SipInviteFailurePhrase => "sip_invite_failure_phrase",
        SipHangupDisposition => "sip_hangup_disposition",
        SipTermStatus => "sip_term_status",
        SipTermCause => "sip_term_cause",
        SipReason => "sip_reason",

        // --- SIP Identity / Privacy ---
        SipPAssertedIdentity => "sip_P-Asserted-Identity",
        SipPPreferredIdentity => "sip_P-Preferred-Identity",
        SipPrivacy => "sip_Privacy",
        SipRemotePartyId => "sip_Remote-Party-ID",
        SipStirShakenAttest => "sip_stir_shaken_attest",
        SipVerstat => "sip_verstat",
        SipVerstatDetailed => "sip_verstat_detailed",

        // --- SIP Invite Details ---
        SipInviteCallId => "sip_invite_call_id",
        SipInviteCseq => "sip_invite_cseq",
        SipInviteFullFrom => "sip_invite_full_from",
        SipInviteFullTo => "sip_invite_full_to",
        SipInviteFullVia => "sip_invite_full_via",
        SipInviteFromUri => "sip_invite_from_uri",
        SipInviteToUri => "sip_invite_to_uri",
        SipInviteReqUri => "sip_invite_req_uri",
        SipInviteRecordRoute => "sip_invite_record_route",
        SipInviteRouteUri => "sip_invite_route_uri",
        SipInviteDomain => "sip_invite_domain",
        SipInviteParams => "sip_invite_params",

        // --- SIP Features ---
        SipAutoAnswer => "sip_auto_answer",
        SipAutoSimplify => "sip_auto_simplify",
        SipEnableSoa => "sip_enable_soa",
        SipCopyCustomHeaders => "sip_copy_custom_headers",
        SipCopyMultipart => "sip_copy_multipart",
        SipLoopedCall => "sip_looped_call",

        // --- SIP Redirect / Transfer ---
        SipRedirectedTo => "sip_redirected_to",
        SipRedirectedBy => "sip_redirected_by",
        SipRedirectDialstring => "sip_redirect_dialstring",
        SipReferReply => "sip_refer_reply",
        SipReferStatusCode => "sip_refer_status_code",
        SipReferredByFull => "sip_referred_by_full",
        SipReferredByCid => "sip_referred_by_cid",
        SipReinviteSdp => "sip_reinvite_sdp",

        // --- SIP Gateway ---
        SipGateway => "sip_gateway",
        SipGatewayName => "sip_gateway_name",
        SipUseGateway => "sip_use_gateway",
        SipDestinationUrl => "sip_destination_url",

        // --- Sofia Profile ---
        SipProfileName => "sip_profile_name",
        SofiaProfileName => "sofia_profile_name",
        SofiaProfileUrl => "sofia_profile_url",
        SofiaProfileDomainName => "sofia_profile_domain_name",

        // --- RTP / SRTP (set via mod_sofia / switch_core_media) ---
        RtpSecureMediaConfirmed => "rtp_secure_media_confirmed",
        Rtp2833SendPayload => "rtp_2833_send_payload",
        Rtp2833RecvPayload => "rtp_2833_recv_payload",
        RtpDisableHold => "rtp_disable_hold",
        RtpJitterBufferPlc => "rtp_jitter_buffer_plc",
        RtpVideoMaxBandwidthIn => "rtp_video_max_bandwidth_in",
        RtpVideoMaxBandwidthOut => "rtp_video_max_bandwidth_out",

        // --- SIP Callee / Display ---
        SipCalleeIdName => "sip_callee_id_name",
        SipCalleeIdNumber => "sip_callee_id_number",
        SipCidType => "sip_cid_type",

        // --- SIP RTP Stats ---
        SipRtpRxstat => "sip_rtp_rxstat",
        SipRtpTxstat => "sip_rtp_txstat",
        SipPRtpStat => "sip_p_rtp_stat",

        // --- SIP History ---
        SipHistoryInfo => "sip_history_info",
        SipGeolocation => "sip_geolocation",

        // --- SIP Body ---
        SipMultipart => "sip_multipart",

        // --- SIP Referred-By ---
        SipReferredByUser => "sip_referred_by_user",
        SipReferredByUserStripped => "sip_referred_by_user_stripped",
        SipReferredByHost => "sip_referred_by_host",
        SipReferredByPort => "sip_referred_by_port",
        SipReferredByUri => "sip_referred_by_uri",
        SipReferredByParams => "sip_referred_by_params",

        // --- Parsed sub-fields ---
        SipFromParams => "sip_from_params",
        SipToParams => "sip_to_params",
        SipReqParams => "sip_req_params",
        SipNameParams => "sip_name_params",
        SipAcceptLanguageCount => "sip_accept_language_count",

        // --- Auth / directory ---
        SipAuthRealm => "sip_auth_realm",
        SipNumberAlias => "sip_number_alias",

        // --- Recovery / outbound Contact ---
        SipRecoverVia => "sip_recover_via",
        SipRecoverContact => "sip_recover_contact",
        SipOutgoingContactUri => "sip_outgoing_contact_uri",

        // --- Local transport / timing ---
        SipInviteStamp => "sip_invite_stamp",
        SipReplyPort => "sip_reply_port",
        SipLocalNetworkAddr => "sip_local_network_addr",
    }
}

/// Relationship between a [`SofiaVariable`] and the SIP header behind it.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CarriedHeader {
    /// The variable holds the header's field-value verbatim.
    Verbatim(SipHeader),
    /// The variable holds a field mod_sofia parsed out of the header. Lossy: the
    /// header is the source of truth and the parts cannot rebuild it.
    Derived(SipHeader),
    /// As [`Derived`](Self::Derived), for a variable mod_sofia fills from whichever
    /// of these headers the message carried.
    DerivedOneOf(&'static [SipHeader]),
}

impl SofiaVariable {
    /// The SIP header this variable's value came from, if any.
    ///
    /// `None` covers everything carrying no header field-value: switch-side config
    /// and knobs, transport addresses, the Request-URI, values computed rather than
    /// read off the wire, and values the caller sets for mod_sofia to build a header
    /// from.
    ///
    /// The match is exhaustive by design. A new variant has to be classified here,
    /// and a catch-all arm would answer `None` for it in silence.
    pub fn carried_header(&self) -> Option<CarriedHeader> {
        use CarriedHeader::{Derived, DerivedOneOf, Verbatim};
        use SipHeader as H;

        match self {
            Self::SipFromUser
            | Self::SipFromHost
            | Self::SipFromPort
            | Self::SipFromUri
            | Self::SipFromDisplay
            | Self::SipFromTag
            | Self::SipFromComment
            | Self::SipFromUserStripped
            | Self::SipFromParams => Some(Derived(H::From)),
            Self::SipFullFrom => Some(Verbatim(H::From)),

            Self::SipToUser
            | Self::SipToHost
            | Self::SipToPort
            | Self::SipToUri
            | Self::SipToDisplay
            | Self::SipToTag
            | Self::SipToComment
            | Self::SipToParams => Some(Derived(H::To)),
            Self::SipFullTo => Some(Verbatim(H::To)),

            Self::SipContactUser
            | Self::SipContactHost
            | Self::SipContactPort
            | Self::SipContactUri
            | Self::SipContactParams => Some(Derived(H::Contact)),

            // The Request-URI is on the request line, not in a header.
            Self::SipReqUser
            | Self::SipReqHost
            | Self::SipReqPort
            | Self::SipReqUri
            | Self::SipReqParams
            | Self::SipInviteReqUri => None,

            Self::SipViaHost | Self::SipViaPort | Self::SipViaRport | Self::SipViaProtocol => {
                Some(Derived(H::Via))
            }
            Self::SipFullVia | Self::SipRecoverVia => Some(Verbatim(H::Via)),
            Self::SipFullRoute => Some(Verbatim(H::Route)),

            Self::SipCallId => Some(Verbatim(H::CallId)),
            Self::SipCseq => Some(Verbatim(H::Cseq)),
            // Falls back to Server when the message that carried it was a response.
            Self::SipUserAgent => Some(Verbatim(H::UserAgent)),
            Self::SipSubject => Some(Verbatim(H::Subject)),
            Self::SipAllow => Some(Verbatim(H::Allow)),
            // Only the first entry of the list.
            Self::SipAcceptLanguage | Self::SipAcceptLanguageCount => {
                Some(Derived(H::AcceptLanguage))
            }
            Self::SipCallInfo => Some(Verbatim(H::CallInfo)),
            Self::SipDateEpochTime => Some(Derived(H::Date)),

            Self::SipReceivedIp
            | Self::SipReceivedPort
            | Self::SipNetworkIp
            | Self::SipNetworkPort
            | Self::SipNatDetected
            | Self::SipTransport
            | Self::SipReplyHost
            | Self::SipReplyPort
            | Self::SipLocalNetworkAddr => None,

            Self::SipAuthUsername
            | Self::SipAuthPassword
            | Self::SipAuthorized
            | Self::SipAclAuthedBy
            | Self::SipAclToken
            | Self::SipChallengeRealm
            | Self::SipNumberAlias => None,
            Self::SipAuthRealm => Some(DerivedOneOf(&[H::Authorization, H::ProxyAuthorization])),

            Self::SipInviteFailureStatus
            | Self::SipInviteFailurePhrase
            | Self::SipHangupDisposition
            | Self::SipTermStatus
            | Self::SipTermCause => None,
            Self::SipReason => Some(Verbatim(H::Reason)),

            Self::SipPAssertedIdentity => Some(Verbatim(H::PAssertedIdentity)),
            Self::SipPPreferredIdentity => Some(Verbatim(H::PPreferredIdentity)),
            Self::SipPrivacy => Some(Verbatim(H::Privacy)),
            Self::SipRemotePartyId => Some(Verbatim(H::RemotePartyId)),
            // A verification verdict and the attestation level asked of us, neither
            // of them a field of the Identity header they concern.
            Self::SipStirShakenAttest | Self::SipVerstat | Self::SipVerstatDetailed => None,

            Self::SipInviteCallId => Some(Verbatim(H::CallId)),
            Self::SipInviteCseq => Some(Verbatim(H::Cseq)),
            Self::SipInviteFullFrom => Some(Verbatim(H::From)),
            Self::SipInviteFullTo => Some(Verbatim(H::To)),
            Self::SipInviteFullVia => Some(Verbatim(H::Via)),
            Self::SipInviteFromUri => Some(Derived(H::From)),
            Self::SipInviteToUri => Some(Derived(H::To)),
            Self::SipInviteRecordRoute => Some(Verbatim(H::RecordRoute)),
            Self::SipInviteRouteUri => Some(Derived(H::Route)),
            Self::SipInviteDomain | Self::SipInviteParams | Self::SipInviteStamp => None,

            Self::SipAutoAnswer
            | Self::SipAutoSimplify
            | Self::SipEnableSoa
            | Self::SipCopyCustomHeaders
            | Self::SipCopyMultipart
            | Self::SipLoopedCall => None,

            Self::SipRedirectedTo | Self::SipRecoverContact | Self::SipOutgoingContactUri => {
                Some(Verbatim(H::Contact))
            }
            Self::SipRedirectedBy => Some(Verbatim(H::Diversion)),
            Self::SipReferredByFull => Some(Verbatim(H::ReferredBy)),
            Self::SipReferredByCid
            | Self::SipReferredByUser
            | Self::SipReferredByUserStripped
            | Self::SipReferredByHost
            | Self::SipReferredByPort
            | Self::SipReferredByUri
            | Self::SipReferredByParams => Some(Derived(H::ReferredBy)),
            // The user part comes from whichever identity header mod_sofia resolved
            // the caller from.
            Self::SipNameParams => Some(DerivedOneOf(&[
                H::From,
                H::PAssertedIdentity,
                H::PPreferredIdentity,
                H::RemotePartyId,
            ])),
            Self::SipRedirectDialstring
            | Self::SipReferReply
            | Self::SipReferStatusCode
            | Self::SipReinviteSdp => None,

            Self::SipGateway
            | Self::SipGatewayName
            | Self::SipUseGateway
            | Self::SipDestinationUrl
            | Self::SipProfileName
            | Self::SofiaProfileName
            | Self::SofiaProfileUrl
            | Self::SofiaProfileDomainName => None,

            Self::RtpSecureMediaConfirmed
            | Self::Rtp2833SendPayload
            | Self::Rtp2833RecvPayload
            | Self::RtpDisableHold
            | Self::RtpJitterBufferPlc
            | Self::RtpVideoMaxBandwidthIn
            | Self::RtpVideoMaxBandwidthOut => None,

            // Caller-supplied identity mod_sofia builds an outbound header from; which
            // header depends on sip_cid_type, so none of them carries one.
            Self::SipCalleeIdName | Self::SipCalleeIdNumber | Self::SipCidType => None,

            // P-RTP-Stat has no SipHeader variant to name.
            Self::SipRtpRxstat | Self::SipRtpTxstat | Self::SipPRtpStat => None,

            Self::SipHistoryInfo => Some(Verbatim(H::HistoryInfo)),
            Self::SipGeolocation => Some(Verbatim(H::Geolocation)),

            Self::SipMultipart => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_round_trip() {
        assert_eq!(SofiaVariable::SipCallId.to_string(), "sip_call_id");
        assert_eq!(
            SofiaVariable::SipFromDisplay.to_string(),
            "sip_from_display"
        );
        assert_eq!(
            SofiaVariable::SofiaProfileName.to_string(),
            "sofia_profile_name"
        );
    }

    #[test]
    fn as_ref_str() {
        let v: &str = SofiaVariable::SipNetworkIp.as_ref();
        assert_eq!(v, "sip_network_ip");
    }

    #[test]
    fn from_str_case_insensitive() {
        assert_eq!(
            "sip_call_id".parse::<SofiaVariable>(),
            Ok(SofiaVariable::SipCallId)
        );
        assert_eq!(
            "SIP_CALL_ID".parse::<SofiaVariable>(),
            Ok(SofiaVariable::SipCallId)
        );
    }

    #[test]
    fn from_str_unknown() {
        let err = "nonexistent_sip_var".parse::<SofiaVariable>();
        assert!(err.is_err());
    }

    #[test]
    fn carried_header_verbatim() {
        assert_eq!(
            SofiaVariable::SipPrivacy.carried_header(),
            Some(CarriedHeader::Verbatim(SipHeader::Privacy))
        );
        assert_eq!(
            SofiaVariable::SipPAssertedIdentity.carried_header(),
            Some(CarriedHeader::Verbatim(SipHeader::PAssertedIdentity))
        );
        assert_eq!(
            SofiaVariable::SipRemotePartyId.carried_header(),
            Some(CarriedHeader::Verbatim(SipHeader::RemotePartyId))
        );
        assert_eq!(
            SofiaVariable::SipFullFrom.carried_header(),
            Some(CarriedHeader::Verbatim(SipHeader::From))
        );
    }

    #[test]
    fn carried_header_derived() {
        assert_eq!(
            SofiaVariable::SipFromUser.carried_header(),
            Some(CarriedHeader::Derived(SipHeader::From))
        );
    }

    #[test]
    fn recovery_and_outbound_contact_are_verbatim() {
        assert_eq!(
            SofiaVariable::SipRecoverVia.carried_header(),
            Some(CarriedHeader::Verbatim(SipHeader::Via))
        );
        assert_eq!(
            SofiaVariable::SipRecoverContact.carried_header(),
            Some(CarriedHeader::Verbatim(SipHeader::Contact))
        );
        assert_eq!(
            SofiaVariable::SipOutgoingContactUri.carried_header(),
            Some(CarriedHeader::Verbatim(SipHeader::Contact))
        );
    }

    #[test]
    fn parsed_sub_fields_are_derived() {
        assert_eq!(
            SofiaVariable::SipFromParams.carried_header(),
            Some(CarriedHeader::Derived(SipHeader::From))
        );
        assert_eq!(
            SofiaVariable::SipToParams.carried_header(),
            Some(CarriedHeader::Derived(SipHeader::To))
        );
        assert_eq!(
            SofiaVariable::SipReferredByParams.carried_header(),
            Some(CarriedHeader::Derived(SipHeader::ReferredBy))
        );
        assert_eq!(
            SofiaVariable::SipAcceptLanguageCount.carried_header(),
            Some(CarriedHeader::Derived(SipHeader::AcceptLanguage))
        );
    }

    /// mod_sofia fills these from whichever of several headers was present, so
    /// neither a single header nor `None` is a true answer.
    #[test]
    fn runtime_chosen_source_names_the_candidates() {
        assert_eq!(
            SofiaVariable::SipNameParams.carried_header(),
            Some(CarriedHeader::DerivedOneOf(&[
                SipHeader::From,
                SipHeader::PAssertedIdentity,
                SipHeader::PPreferredIdentity,
                SipHeader::RemotePartyId,
            ]))
        );
        assert_eq!(
            SofiaVariable::SipAuthRealm.carried_header(),
            Some(CarriedHeader::DerivedOneOf(&[
                SipHeader::Authorization,
                SipHeader::ProxyAuthorization,
            ]))
        );
    }

    #[test]
    fn switch_side_variables_carry_no_header() {
        for var in [
            SofiaVariable::SipInviteStamp,
            SofiaVariable::SipReplyPort,
            SofiaVariable::SipLocalNetworkAddr,
            SofiaVariable::SipNumberAlias,
            SofiaVariable::SipReqParams,
        ] {
            assert_eq!(var.carried_header(), None, "{}", var.as_str());
        }
    }

    #[test]
    fn carried_header_none() {
        // The Request-URI is not a header, and a gateway name is switch config.
        assert_eq!(SofiaVariable::SipReqUri.carried_header(), None);
        assert_eq!(SofiaVariable::SipGateway.carried_header(), None);
    }

    /// A wire name that already spells the header keeps its exact canonical
    /// casing, so any typo in either table shows up as a mismatch here.
    #[test]
    fn hyphenated_wire_names_match_their_header() {
        for var in SofiaVariable::ALL {
            let Some(suffix) = var
                .as_str()
                .strip_prefix("sip_")
            else {
                continue;
            };
            if !suffix.contains('-') {
                continue;
            }
            let Some(carried) = var.carried_header() else {
                panic!("{} spells a header but carries none", var.as_str());
            };
            let CarriedHeader::Verbatim(header) = carried else {
                panic!("{} spells a header but is not verbatim", var.as_str());
            };
            assert_eq!(header.as_str(), suffix, "for {}", var.as_str());
        }
    }
}
