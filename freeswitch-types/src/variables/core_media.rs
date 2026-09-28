//! RTP media statistics variables set by `switch_core_media_set_stats()`.

use std::fmt;

/// Unit of measurement for an RTP statistic channel variable.
///
/// Dimensionless statistics ([`Ratio`](Self::Ratio), [`Count`](Self::Count))
/// display as an empty string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum RtpStatUnit {
    /// Raw or media byte count.
    Bytes,
    /// RTCP octet count (RFC 3550).
    Octets,
    /// Packet count (RTP, RTCP, or jitter buffer size).
    Packets,
    /// Quality percentage (R-factor).
    Percent,
    /// Mean Opinion Score.
    Mos,
    /// Jitter variance or mean interval in milliseconds.
    Milliseconds,
    /// Dimensionless fraction in `0.0..=1.0` (loss rate, burst rate); parse as float.
    Ratio,
    /// Dimensionless integer tally (flaw total); parse as unsigned.
    Count,
}

impl fmt::Display for RtpStatUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Bytes => "bytes",
            Self::Octets => "octets",
            Self::Packets => "packets",
            Self::Percent => "%",
            Self::Mos => "MOS",
            Self::Milliseconds => "ms",
            Self::Ratio => "",
            Self::Count => "",
        })
    }
}

sip_header_catalog::define_header_enum! {
    tests_mod: core_media_variable_generated_tests,
    serde(cfg(feature = "serde")),
    error_type: ParseCoreMediaVariableError => "unknown core media variable",
    /// RTP media statistics channel variable names (the part after the `variable_` prefix).
    ///
    /// Set by `switch_core_media_set_stats()` at channel teardown for each media type
    /// (audio, video, text). Use with
    /// [`HeaderLookup::variable()`](crate::HeaderLookup::variable) for type-safe lookups.
    pub enum CoreMediaVariable {
        // --- Audio Inbound ---
        RtpAudioInRawBytes => "rtp_audio_in_raw_bytes",
        RtpAudioInMediaBytes => "rtp_audio_in_media_bytes",
        RtpAudioInPacketCount => "rtp_audio_in_packet_count",
        RtpAudioInMediaPacketCount => "rtp_audio_in_media_packet_count",
        RtpAudioInSkipPacketCount => "rtp_audio_in_skip_packet_count",
        RtpAudioInJitterPacketCount => "rtp_audio_in_jitter_packet_count",
        RtpAudioInDtmfPacketCount => "rtp_audio_in_dtmf_packet_count",
        RtpAudioInCngPacketCount => "rtp_audio_in_cng_packet_count",
        RtpAudioInFlushPacketCount => "rtp_audio_in_flush_packet_count",
        RtpAudioInLargestJbSize => "rtp_audio_in_largest_jb_size",
        RtpAudioInJitterMinVariance => "rtp_audio_in_jitter_min_variance",
        RtpAudioInJitterMaxVariance => "rtp_audio_in_jitter_max_variance",
        RtpAudioInJitterLossRate => "rtp_audio_in_jitter_loss_rate",
        RtpAudioInJitterBurstRate => "rtp_audio_in_jitter_burst_rate",
        RtpAudioInMeanInterval => "rtp_audio_in_mean_interval",
        RtpAudioInFlawTotal => "rtp_audio_in_flaw_total",
        RtpAudioInQualityPercentage => "rtp_audio_in_quality_percentage",
        RtpAudioInMos => "rtp_audio_in_mos",

        // --- Audio Outbound ---
        RtpAudioOutRawBytes => "rtp_audio_out_raw_bytes",
        RtpAudioOutMediaBytes => "rtp_audio_out_media_bytes",
        RtpAudioOutPacketCount => "rtp_audio_out_packet_count",
        RtpAudioOutMediaPacketCount => "rtp_audio_out_media_packet_count",
        RtpAudioOutSkipPacketCount => "rtp_audio_out_skip_packet_count",
        RtpAudioOutDtmfPacketCount => "rtp_audio_out_dtmf_packet_count",
        RtpAudioOutCngPacketCount => "rtp_audio_out_cng_packet_count",

        // --- Audio RTCP ---
        RtpAudioRtcpPacketCount => "rtp_audio_rtcp_packet_count",
        RtpAudioRtcpOctetCount => "rtp_audio_rtcp_octet_count",

        // --- Video Inbound ---
        RtpVideoInRawBytes => "rtp_video_in_raw_bytes",
        RtpVideoInMediaBytes => "rtp_video_in_media_bytes",
        RtpVideoInPacketCount => "rtp_video_in_packet_count",
        RtpVideoInMediaPacketCount => "rtp_video_in_media_packet_count",
        RtpVideoInSkipPacketCount => "rtp_video_in_skip_packet_count",
        RtpVideoInJitterPacketCount => "rtp_video_in_jitter_packet_count",
        RtpVideoInDtmfPacketCount => "rtp_video_in_dtmf_packet_count",
        RtpVideoInCngPacketCount => "rtp_video_in_cng_packet_count",
        RtpVideoInFlushPacketCount => "rtp_video_in_flush_packet_count",
        RtpVideoInLargestJbSize => "rtp_video_in_largest_jb_size",
        RtpVideoInJitterMinVariance => "rtp_video_in_jitter_min_variance",
        RtpVideoInJitterMaxVariance => "rtp_video_in_jitter_max_variance",
        RtpVideoInJitterLossRate => "rtp_video_in_jitter_loss_rate",
        RtpVideoInJitterBurstRate => "rtp_video_in_jitter_burst_rate",
        RtpVideoInMeanInterval => "rtp_video_in_mean_interval",
        RtpVideoInFlawTotal => "rtp_video_in_flaw_total",
        RtpVideoInQualityPercentage => "rtp_video_in_quality_percentage",
        RtpVideoInMos => "rtp_video_in_mos",

        // --- Video Outbound ---
        RtpVideoOutRawBytes => "rtp_video_out_raw_bytes",
        RtpVideoOutMediaBytes => "rtp_video_out_media_bytes",
        RtpVideoOutPacketCount => "rtp_video_out_packet_count",
        RtpVideoOutMediaPacketCount => "rtp_video_out_media_packet_count",
        RtpVideoOutSkipPacketCount => "rtp_video_out_skip_packet_count",
        RtpVideoOutDtmfPacketCount => "rtp_video_out_dtmf_packet_count",
        RtpVideoOutCngPacketCount => "rtp_video_out_cng_packet_count",

        // --- Video RTCP ---
        RtpVideoRtcpPacketCount => "rtp_video_rtcp_packet_count",
        RtpVideoRtcpOctetCount => "rtp_video_rtcp_octet_count",

        // --- Text Inbound ---
        RtpTextInRawBytes => "rtp_text_in_raw_bytes",
        RtpTextInMediaBytes => "rtp_text_in_media_bytes",
        RtpTextInPacketCount => "rtp_text_in_packet_count",
        RtpTextInMediaPacketCount => "rtp_text_in_media_packet_count",
        RtpTextInSkipPacketCount => "rtp_text_in_skip_packet_count",
        RtpTextInJitterPacketCount => "rtp_text_in_jitter_packet_count",
        RtpTextInDtmfPacketCount => "rtp_text_in_dtmf_packet_count",
        RtpTextInCngPacketCount => "rtp_text_in_cng_packet_count",
        RtpTextInFlushPacketCount => "rtp_text_in_flush_packet_count",
        RtpTextInLargestJbSize => "rtp_text_in_largest_jb_size",
        RtpTextInJitterMinVariance => "rtp_text_in_jitter_min_variance",
        RtpTextInJitterMaxVariance => "rtp_text_in_jitter_max_variance",
        RtpTextInJitterLossRate => "rtp_text_in_jitter_loss_rate",
        RtpTextInJitterBurstRate => "rtp_text_in_jitter_burst_rate",
        RtpTextInMeanInterval => "rtp_text_in_mean_interval",
        RtpTextInFlawTotal => "rtp_text_in_flaw_total",
        RtpTextInQualityPercentage => "rtp_text_in_quality_percentage",
        RtpTextInMos => "rtp_text_in_mos",

        // --- Text Outbound ---
        RtpTextOutRawBytes => "rtp_text_out_raw_bytes",
        RtpTextOutMediaBytes => "rtp_text_out_media_bytes",
        RtpTextOutPacketCount => "rtp_text_out_packet_count",
        RtpTextOutMediaPacketCount => "rtp_text_out_media_packet_count",
        RtpTextOutSkipPacketCount => "rtp_text_out_skip_packet_count",
        RtpTextOutDtmfPacketCount => "rtp_text_out_dtmf_packet_count",
        RtpTextOutCngPacketCount => "rtp_text_out_cng_packet_count",

        // --- Text RTCP ---
        RtpTextRtcpPacketCount => "rtp_text_rtcp_packet_count",
        RtpTextRtcpOctetCount => "rtp_text_rtcp_octet_count",
    }
}

/// Name suffix to unit, longest-first: `_media_packet_count` must not reach
/// `_count`, and `_jb_size` counts packets rather than bytes.
const UNIT_SUFFIXES: &[(&str, RtpStatUnit)] = &[
    ("_bytes", RtpStatUnit::Bytes),
    ("_octet_count", RtpStatUnit::Octets),
    ("_packet_count", RtpStatUnit::Packets),
    ("_largest_jb_size", RtpStatUnit::Packets),
    ("_variance", RtpStatUnit::Milliseconds),
    ("_mean_interval", RtpStatUnit::Milliseconds),
    ("_loss_rate", RtpStatUnit::Ratio),
    ("_burst_rate", RtpStatUnit::Ratio),
    ("_flaw_total", RtpStatUnit::Count),
    ("_quality_percentage", RtpStatUnit::Percent),
    ("_mos", RtpStatUnit::Mos),
];

fn unit_from_suffix(name: &str) -> Option<RtpStatUnit> {
    UNIT_SUFFIXES
        .iter()
        .find(|(suffix, _)| name.ends_with(suffix))
        .map(|&(_, unit)| unit)
}

impl CoreMediaVariable {
    /// Unit of measurement for this RTP statistic.
    ///
    /// `switch_core_media_set_stats` names every statistic after what it
    /// measures, so the unit comes off the name's suffix. A name carrying none
    /// of them reads as dimensionless, which `unit_covers_every_variant`
    /// refuses.
    pub fn unit(&self) -> RtpStatUnit {
        unit_from_suffix(self.as_str()).unwrap_or(RtpStatUnit::Count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_round_trip() {
        assert_eq!(
            CoreMediaVariable::RtpAudioInRawBytes.to_string(),
            "rtp_audio_in_raw_bytes"
        );
        assert_eq!(
            CoreMediaVariable::RtpVideoOutPacketCount.to_string(),
            "rtp_video_out_packet_count"
        );
        assert_eq!(
            CoreMediaVariable::RtpTextRtcpOctetCount.to_string(),
            "rtp_text_rtcp_octet_count"
        );
    }

    #[test]
    fn unit_reads_the_name_suffix() {
        let cases = [
            (CoreMediaVariable::RtpAudioInRawBytes, RtpStatUnit::Bytes),
            (CoreMediaVariable::RtpTextOutMediaBytes, RtpStatUnit::Bytes),
            (
                CoreMediaVariable::RtpAudioInPacketCount,
                RtpStatUnit::Packets,
            ),
            (
                CoreMediaVariable::RtpVideoOutMediaPacketCount,
                RtpStatUnit::Packets,
            ),
            (
                CoreMediaVariable::RtpAudioRtcpPacketCount,
                RtpStatUnit::Packets,
            ),
            (
                CoreMediaVariable::RtpTextInLargestJbSize,
                RtpStatUnit::Packets,
            ),
            (
                CoreMediaVariable::RtpAudioRtcpOctetCount,
                RtpStatUnit::Octets,
            ),
            (
                CoreMediaVariable::RtpAudioInJitterMinVariance,
                RtpStatUnit::Milliseconds,
            ),
            (
                CoreMediaVariable::RtpVideoInJitterMaxVariance,
                RtpStatUnit::Milliseconds,
            ),
            (
                CoreMediaVariable::RtpTextInMeanInterval,
                RtpStatUnit::Milliseconds,
            ),
            (
                CoreMediaVariable::RtpAudioInJitterLossRate,
                RtpStatUnit::Ratio,
            ),
            (
                CoreMediaVariable::RtpTextInJitterBurstRate,
                RtpStatUnit::Ratio,
            ),
            (CoreMediaVariable::RtpVideoInFlawTotal, RtpStatUnit::Count),
            (
                CoreMediaVariable::RtpAudioInQualityPercentage,
                RtpStatUnit::Percent,
            ),
            (CoreMediaVariable::RtpTextInMos, RtpStatUnit::Mos),
        ];
        for (variable, unit) in cases {
            assert_eq!(variable.unit(), unit, "{variable}");
        }
    }

    /// The fallback in `unit()` cannot be reached by a name in the enum; a
    /// variable added without a unit-bearing suffix has to be classified here.
    #[test]
    fn unit_covers_every_variant() {
        for variable in CoreMediaVariable::ALL {
            assert!(
                unit_from_suffix(variable.as_str()).is_some(),
                "{variable} matches no unit suffix"
            );
        }
    }

    #[test]
    fn unit_display() {
        assert_eq!(RtpStatUnit::Bytes.to_string(), "bytes");
        assert_eq!(RtpStatUnit::Octets.to_string(), "octets");
        assert_eq!(RtpStatUnit::Packets.to_string(), "packets");
        assert_eq!(RtpStatUnit::Percent.to_string(), "%");
        assert_eq!(RtpStatUnit::Mos.to_string(), "MOS");
        assert_eq!(RtpStatUnit::Milliseconds.to_string(), "ms");
        assert_eq!(RtpStatUnit::Ratio.to_string(), "");
        assert_eq!(RtpStatUnit::Count.to_string(), "");
    }
}
