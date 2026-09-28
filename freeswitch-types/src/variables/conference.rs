//! Typed mod_conference channel variable names.

sip_header_catalog::define_header_enum! {
    tests_mod: conference_variable_generated_tests,
    serde(cfg(feature = "serde")),
    error_type: ParseConferenceVariableError => "unknown conference variable",
    /// mod_conference channel variable names (the part after the `variable_` prefix).
    ///
    /// Use with [`HeaderLookup::variable()`](crate::HeaderLookup::variable) for
    /// type-safe lookups.
    pub enum ConferenceVariable {
        // --- Set by mod_conference ---
        ConferenceName => "conference_name",
        /// The conference object's UUID, minted per conference instance.
        ///
        /// Not a correlation key on its own: a name is reusable as soon as the
        /// conference holding it tears down, and this value reaches an event
        /// stream only through a channel dump. A consumer grouping members
        /// across a log without dumps has to mint its own instance identity.
        ConferenceUuid => "conference_uuid",
        ConferenceMemberId => "conference_member_id",
        /// `true`/`false`.
        ConferenceModerator => "conference_moderator",
        /// `true`/`false`.
        ConferenceGhost => "conference_ghost",
        /// Recording filename.
        ConferenceRecording => "conference_recording",
        /// Canvas index, 1-based.
        ConferenceRecordingCanvas => "conference_recording_canvas",
        /// `conference_<name>_<domain>_<caller_id_number>`, cleared when the
        /// member leaves.
        ConferenceCallKey => "conference_call_key",
        /// Digits that matched a caller-control binding.
        ConferenceLastMatchingDigits => "conference_last_matching_digits",
        /// Conference name, cleared once the transfer completes.
        LastTransferedConference => "last_transfered_conference",

        // --- Set by the caller ---
        ConferenceSilentEntry => "conference_silent_entry",
        ConferenceFlags => "conference_flags",
        ConferenceMemberFlags => "conference_member_flags",
        ConferenceControls => "conference_controls",
        ConferencePosition => "conference_position",
        ConferenceJoinVolumeIn => "conference_join_volume_in",
        ConferenceJoinVolumeOut => "conference_join_volume_out",
        ConferenceJoinEnergyLevel => "conference_join_energy_level",
        ConferenceMaxMembers => "conference_max_members",
        ConferenceModeratorPin => "conference_moderator_pin",
        ConferenceEnforceSecurity => "conference_enforce_security",
        ConferenceEndconferenceGraceTime => "conference_endconference_grace_time",
        ConferenceEnterSound => "conference_enter_sound",
        ConferenceExitSound => "conference_exit_sound",
        ConferenceMohSound => "conference_moh_sound",
        ConferencePerpetualSound => "conference_perpetual_sound",
        ConferencePermanentWaitModMoh => "conference_permanent_wait_mod_moh",
        ConferenceAutoRecord => "conference_auto_record",
        ConferenceInviteUri => "conference_invite_uri",
        ConferenceForceRate => "conference_force_rate",
        ConferenceForceInterval => "conference_force_interval",
        ConferenceForceChannels => "conference_force_channels",
        ConferenceForceCanvasSize => "conference_force_canvas_size",
        ConferenceAutoOutcallCallerIdName => "conference_auto_outcall_caller_id_name",
        ConferenceAutoOutcallCallerIdNumber => "conference_auto_outcall_caller_id_number",
        ConferenceAutoOutcallTimeout => "conference_auto_outcall_timeout",
        ConferenceAutoOutcallProfile => "conference_auto_outcall_profile",
        ConferenceAutoOutcallAnnounce => "conference_auto_outcall_announce",
        ConferenceAutoOutcallPrefix => "conference_auto_outcall_prefix",
        ConferenceAutoOutcallMaxwait => "conference_auto_outcall_maxwait",
        ConferenceAutoOutcallDelimiter => "conference_auto_outcall_delimiter",
        ConferenceAutoOutcallSkipMemberBeep => "conference_auto_outcall_skip_member_beep",
        ConferenceUtilsAutoOutcallFlags => "conference_utils_auto_outcall_flags",
        HangupAfterConference => "hangup_after_conference",
        /// PIN carried as a URI parameter, searched for the configured
        /// parameter name before the digits are read.
        SuppliedPin => "supplied_pin",
        /// Video overlay text, cleared by mod_conference once applied to the
        /// member.
        VideoBannerText => "video_banner_text",
    }
}
