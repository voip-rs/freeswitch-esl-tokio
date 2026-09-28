//! Typed channel variable names from FreeSWITCH core.

sip_header_catalog::define_header_enum! {
    tests_mod: channel_variable_generated_tests,
    serde(cfg(feature = "serde")),
    error_type: ParseChannelVariableError => "unknown channel variable",
    /// Core FreeSWITCH channel variable names (the part after the `variable_` prefix).
    ///
    /// Use with [`HeaderLookup::variable()`](crate::HeaderLookup::variable) for type-safe lookups. Only includes
    /// variables set by the FreeSWITCH core -- module-specific variables (SIP,
    /// conference, etc.) belong in separate enums.
    pub enum ChannelVariable {
        // --- Identity ---
        Uuid => "uuid",
        CallUuid => "call_uuid",
        Direction => "direction",
        IsOutbound => "is_outbound",
        CallerIdName => "caller_id_name",
        CallerIdNumber => "caller_id_number",
        CalleeIdName => "callee_id_name",
        CalleeIdNumber => "callee_id_number",
        EffectiveCallerIdName => "effective_caller_id_name",
        EffectiveCallerIdNumber => "effective_caller_id_number",
        EffectiveCalleeIdName => "effective_callee_id_name",
        EffectiveCalleeIdNumber => "effective_callee_id_number",
        EffectiveAni => "effective_ani",
        EffectiveAniii => "effective_aniii",
        Ani => "ani",
        Rdnis => "rdnis",
        DestinationNumber => "destination_number",
        ChannelName => "channel_name",
        Context => "context",
        Dialplan => "dialplan",
        DomainName => "domain_name",
        UserName => "user_name",
        UserContext => "user_context",
        PresenceId => "presence_id",
        Accountcode => "accountcode",
        TollAllow => "toll_allow",

        // --- State ---
        HangupCause => "hangup_cause",
        HangupCauseQ850 => "hangup_cause_q850",
        EndpointDisposition => "endpoint_disposition",
        CurrentApplication => "current_application",
        CurrentApplicationData => "current_application_data",
        LastApp => "last_app",
        LastArg => "last_arg",
        TransferSource => "transfer_source",
        TransferHistory => "transfer_history",
        Recovered => "recovered",
        DigitsDialed => "digits_dialed",
        ProcessCdr => "process_cdr",
        FailureCauses => "failure_causes",

        // --- Bridge ---
        BridgeUuid => "bridge_uuid",
        SignalBond => "signal_bond",
        SignalBridge => "signal_bridge",
        LastBridgeTo => "last_bridge_to",
        LastBridgeRole => "last_bridge_role",
        BridgeHangupCause => "bridge_hangup_cause",
        BridgeTerminateKey => "bridge_terminate_key",
        BridgeFilterDtmf => "bridge_filter_dtmf",
        BridgeAnswerTimeout => "bridge_answer_timeout",
        BridgeGenerateComfortNoise => "bridge_generate_comfort_noise",
        UuidBridgeContinueOnCancel => "uuid_bridge_continue_on_cancel",
        UuidBridgeParkOnCancel => "uuid_bridge_park_on_cancel",
        HangupAfterBridge => "hangup_after_bridge",
        ParkAfterBridge => "park_after_bridge",
        ParkTimeout => "park_timeout",

        // --- Codec ---
        ReadCodec => "read_codec",
        ReadRate => "read_rate",
        WriteCodec => "write_codec",
        WriteRate => "write_rate",
        CodecString => "codec_string",
        AbsoluteCodecString => "absolute_codec_string",
        EpCodecString => "ep_codec_string",
        InheritCodec => "inherit_codec",
        OriginalReadCodec => "original_read_codec",
        OriginalReadRate => "original_read_rate",
        VideoReadCodec => "video_read_codec",
        VideoReadRate => "video_read_rate",
        VideoWriteCodec => "video_write_codec",
        VideoWriteRate => "video_write_rate",

        // --- Media ---
        DtmfType => "dtmf_type",
        JitterbufferMsec => "jitterbuffer_msec",
        VideoPossible => "video_possible",
        TextPossible => "text_possible",
        MediaWebrtc => "media_webrtc",
        BypassMedia => "bypass_media",
        BypassMediaAfterBridge => "bypass_media_after_bridge",
        ProxyMedia => "proxy_media",
        RtpSecureMedia => "rtp_secure_media",
        RtpUseCodecName => "rtp_use_codec_name",
        RtpUseCodecRate => "rtp_use_codec_rate",
        RtpUseCodecPtime => "rtp_use_codec_ptime",
        RtpUseCodecString => "rtp_use_codec_string",
        RtpLocalSdpStr => "rtp_local_sdp_str",
        SendSilenceWhenIdle => "send_silence_when_idle",

        // --- Originate ---
        OriginateDisposition => "originate_disposition",
        OriginateFailedCause => "originate_failed_cause",
        OriginatedLegs => "originated_legs",
        OriginatingLegUuid => "originating_leg_uuid",
        OriginationUuid => "origination_uuid",
        OriginationCallerIdName => "origination_caller_id_name",
        OriginationCallerIdNumber => "origination_caller_id_number",
        OriginationCalleeIdName => "origination_callee_id_name",
        OriginationCalleeIdNumber => "origination_callee_id_number",
        OriginationCancelKey => "origination_cancel_key",
        OriginationChannelName => "origination_channel_name",
        Ringback => "ringback",
        TransferRingback => "transfer_ringback",
        LegTimeout => "leg_timeout",
        LegDelayStart => "leg_delay_start",
        LegProgressTimeout => "leg_progress_timeout",
        LegRequired => "leg_required",

        // --- Playback / Record ---
        SoundPrefix => "sound_prefix",
        TtsEngine => "tts_engine",
        TtsVoice => "tts_voice",
        Language => "language",
        DefaultLanguage => "default_language",
        PlaybackTerminators => "playback_terminators",
        PlaybackTimeoutSec => "playback_timeout_sec",
        RecordingFollowTransfer => "recording_follow_transfer",
        RecordingFollowAttxfer => "recording_follow_attxfer",

        // --- Behavior ---
        ExportVars => "export_vars",
        BridgeExportVars => "bridge_export_vars",
        CallTimeout => "call_timeout",
        OriginateTimeout => "originate_timeout",
        ContinueOnFail => "continue_on_fail",
        IgnoreEarlyMedia => "ignore_early_media",
        ProgressTimeout => "progress_timeout",
        HoldMusic => "hold_music",
        SocketResume => "socket_resume",

        // --- CDR timing (auto-set by switch_channel.c) ---
        StartEpoch => "start_epoch",
        StartUepoch => "start_uepoch",
        AnswerEpoch => "answer_epoch",
        AnswerUepoch => "answer_uepoch",
        BridgeEpoch => "bridge_epoch",
        BridgeUepoch => "bridge_uepoch",
        EndEpoch => "end_epoch",
        EndUepoch => "end_uepoch",
        ProgressEpoch => "progress_epoch",
        ProgressUepoch => "progress_uepoch",
        ProgressMediaEpoch => "progress_media_epoch",
        ProgressMediaUepoch => "progress_media_uepoch",
        Duration => "duration",
        Billsec => "billsec",
        Billmsec => "billmsec",
        Billusec => "billusec",
        FlowBillsec => "flow_billsec",
        FlowBillmsec => "flow_billmsec",
        FlowBillusec => "flow_billusec",
        Progresssec => "progresssec",
        Progressmsec => "progressmsec",
        Progressusec => "progressusec",
        ProgressMediasec => "progress_mediasec",
        ProgressMediamsec => "progress_mediamsec",
        ProgressMediausec => "progress_mediausec",
        Waitsec => "waitsec",
        Waitmsec => "waitmsec",
        Waitusec => "waitusec",
        Answersec => "answersec",
        Answermsec => "answermsec",
        Answerusec => "answerusec",
        HoldAccumSeconds => "hold_accum_seconds",
        HoldAccumMs => "hold_accum_ms",
        HoldAccumUsec => "hold_accum_usec",
        /// Remote SDP received from the far end (`SWITCH_R_SDP_VARIABLE`).
        SwitchRSdp => "switch_r_sdp",
        /// Local SDP offered to the far end (`SWITCH_L_SDP_VARIABLE`).
        SwitchLSdp => "switch_l_sdp",
        /// Overrides the fmtp for the chosen primary audio payload in the generated SDP offer.
        RtpForceAudioFmtp => "rtp_force_audio_fmtp",

        // --- RFC 7989 Session-ID ---
        /// `local-uuid` half of the received `Session-ID` header.
        SessionUuid => "session_uuid",
        /// `remote=` half of the received `Session-ID` header.
        RemoteSessionUuid => "remote_session_uuid",
        /// UUID an application stamps on the session, sent as the local half of
        /// `Session-ID`. mod_conference writes the conference UUID here.
        AppSessionUuid => "app_session_uuid",
        /// Generic parameters carried alongside the `Session-ID` UUIDs.
        GenericParamSessionUuid => "generic_param_session_uuid",

        // --- Hooks ---
        /// Application `switch_ivr_originate` runs on a new channel before its
        /// session thread starts; value shape is [`ExecuteOn`](crate::commands::ExecuteOn).
        ExecuteOnOriginate => "execute_on_originate",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_round_trip() {
        assert_eq!(ChannelVariable::Direction.to_string(), "direction");
        assert_eq!(ChannelVariable::CallerIdName.to_string(), "caller_id_name");
        assert_eq!(
            ChannelVariable::EffectiveCallerIdNumber.to_string(),
            "effective_caller_id_number"
        );
        assert_eq!(
            ChannelVariable::HangupCauseQ850.to_string(),
            "hangup_cause_q850"
        );
    }

    #[test]
    fn as_ref_str() {
        let v: &str = ChannelVariable::BridgeUuid.as_ref();
        assert_eq!(v, "bridge_uuid");
    }

    #[test]
    fn from_str_case_insensitive() {
        assert_eq!(
            "direction".parse::<ChannelVariable>(),
            Ok(ChannelVariable::Direction)
        );
        assert_eq!(
            "DIRECTION".parse::<ChannelVariable>(),
            Ok(ChannelVariable::Direction)
        );
        assert_eq!(
            "Direction".parse::<ChannelVariable>(),
            Ok(ChannelVariable::Direction)
        );
    }

    #[test]
    fn from_str_unknown() {
        let err = "nonexistent_var".parse::<ChannelVariable>();
        assert!(err.is_err());
        assert_eq!(
            err.unwrap_err()
                .to_string(),
            "unknown channel variable (15 bytes)"
        );
    }

    #[test]
    fn switch_r_sdp_round_trip() {
        assert_eq!(ChannelVariable::SwitchRSdp.to_string(), "switch_r_sdp");
        assert_eq!(
            "switch_r_sdp".parse::<ChannelVariable>(),
            Ok(ChannelVariable::SwitchRSdp)
        );
        assert_eq!(
            "SWITCH_R_SDP".parse::<ChannelVariable>(),
            Ok(ChannelVariable::SwitchRSdp)
        );
    }

    #[test]
    fn switch_l_sdp_round_trip() {
        assert_eq!(ChannelVariable::SwitchLSdp.to_string(), "switch_l_sdp");
        assert_eq!(
            "switch_l_sdp".parse::<ChannelVariable>(),
            Ok(ChannelVariable::SwitchLSdp)
        );
        assert_eq!(
            "SWITCH_L_SDP".parse::<ChannelVariable>(),
            Ok(ChannelVariable::SwitchLSdp)
        );
    }

    #[test]
    fn rtp_force_audio_fmtp_round_trip() {
        assert_eq!(
            ChannelVariable::RtpForceAudioFmtp.to_string(),
            "rtp_force_audio_fmtp"
        );
        assert_eq!(
            "rtp_force_audio_fmtp".parse::<ChannelVariable>(),
            Ok(ChannelVariable::RtpForceAudioFmtp)
        );
        assert_eq!(
            "RTP_FORCE_AUDIO_FMTP".parse::<ChannelVariable>(),
            Ok(ChannelVariable::RtpForceAudioFmtp)
        );
    }
}
