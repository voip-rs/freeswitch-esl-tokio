//! Static payload types, and FreeSWITCH's default rate, ptime and bitrate tables.
//!
//! The static table follows sofia-sip's `sdp_rtpmap_well_known` in `sdp_parse.c`, not RFC 3551
//! alone: the switch names an `a=rtpmap`-less payload type from sofia's table, which also assigns
//! 1 (`1016`), 2 (`G721`) and 19 (`CN`), numbers RFC 3551 reserves.
//!
//! Line numbers in this module index FreeSWITCH `v1.11.1`
//! (`c2c59645f6911a76589e5008c4d73349ded44b65`).

/// Static payload type descriptor.
pub(crate) struct StaticPayloadType {
    pub encoding_name: &'static str,
    pub clock_rate: u32,
    /// Audio channel count; `None` for video and combined AV types.
    ///
    /// `MPA` (PT 14) is audio but its channel count is stream-defined by
    /// the MPEG bitstream, so it also carries `None`.
    pub channels: Option<u32>,
}

/// The static payload descriptor sofia names `pt` with, `None` where it names none.
// qual:allow(complexity, max_cyclomatic=27) reason: "one arm per sdp_rtpmap_well_known row"
pub(crate) fn static_payload_type(pt: u8) -> Option<StaticPayloadType> {
    macro_rules! pt {
        ($name:expr, $rate:expr, $ch:expr) => {
            Some(StaticPayloadType {
                encoding_name: $name,
                clock_rate: $rate,
                channels: Some($ch),
            })
        };
        // Video and stream-defined-channel types carry no channel count.
        ($name:expr, $rate:expr) => {
            Some(StaticPayloadType {
                encoding_name: $name,
                clock_rate: $rate,
                channels: None,
            })
        };
    }
    match pt {
        0 => pt!("PCMU", 8000, 1),
        1 => pt!("1016", 8000, 1),
        2 => pt!("G721", 8000, 1),
        3 => pt!("GSM", 8000, 1),
        4 => pt!("G723", 8000, 1),
        5 => pt!("DVI4", 8000, 1),
        6 => pt!("DVI4", 16000, 1),
        7 => pt!("LPC", 8000, 1),
        8 => pt!("PCMA", 8000, 1),
        // G.722 is advertised at 8000 Hz in SDP (RFC 3551 quirk); actual rate is 16 kHz.
        9 => pt!("G722", 8000, 1),
        10 => pt!("L16", 44100, 2),
        11 => pt!("L16", 44100, 1),
        12 => pt!("QCELP", 8000, 1),
        13 => pt!("CN", 8000, 1),
        // MPA clock rate is 90000; channel count is defined by the MPEG bitstream, not the m= line.
        14 => pt!("MPA", 90000),
        15 => pt!("G728", 8000, 1),
        16 => pt!("DVI4", 11025, 1),
        17 => pt!("DVI4", 22050, 1),
        18 => pt!("G729", 8000, 1),
        19 => pt!("CN", 8000, 1),
        // 20-24: unassigned
        25 => pt!("CelB", 90000),
        26 => pt!("JPEG", 90000),
        // 27: unassigned
        28 => pt!("nv", 90000),
        // 29, 30: unassigned
        31 => pt!("H261", 90000),
        32 => pt!("MPV", 90000),
        33 => pt!("MP2T", 90000),
        34 => pt!("H263", 90000),
        _ => None,
    }
}

/// Lookup table mapping case-insensitive encoding names to their canonical
/// FreeSWITCH-registered spelling.
///
/// Spellings verified against `switch_core_codec_add_implementation` calls in
/// `src/mod/codecs/`, `src/mod/applications/mod_spandsp/`, and the built-in
/// codec handlers (`switch_speex.c`, `switch_pcm.c`, `switch_vpx.c`).
static CANONICAL_NAMES: &[(&str, &str)] = &[
    ("pcmu", "PCMU"),
    ("pcma", "PCMA"),
    ("g722", "G722"),
    ("gsm", "GSM"),
    ("lpc", "LPC"),
    ("dvi4", "DVI4"),
    ("g723", "G723"),
    ("g729", "G729"),
    // mod_opus registers in lowercase.
    ("opus", "opus"),
    // mod_ilbc registers as "iLBC" (mixed case).
    ("ilbc", "iLBC"),
    ("amr", "AMR"),
    ("amr-wb", "AMR-WB"),
    ("silk", "SILK"),
    ("bv16", "BV16"),
    ("bv32", "BV32"),
    ("g7221", "G7221"),
    ("g726-16", "G726-16"),
    ("g726-24", "G726-24"),
    ("g726-32", "G726-32"),
    ("g726-40", "G726-40"),
    ("aal2-g726-16", "AAL2-G726-16"),
    ("aal2-g726-24", "AAL2-G726-24"),
    ("aal2-g726-32", "AAL2-G726-32"),
    ("aal2-g726-40", "AAL2-G726-40"),
    ("codec2", "CODEC2"),
    // switch_pcm.c registers as "L16".
    ("l16", "L16"),
    // switch_speex.c registers iananame as "SPEEX" (all caps), not "speex".
    ("speex", "SPEEX"),
    // switch_vpx.c (built-in) registers both video codecs.
    ("vp8", "VP8"),
    ("vp9", "VP9"),
    // mod_av registers H.26x video codecs.
    ("h264", "H264"),
    ("h263", "H263"),
    ("h263-1998", "H263-1998"),
    // mod_b64 registers in lowercase.
    ("b64", "b64"),
    // mod_yuv registers raw video.
    ("i420", "I420"),
];

/// Normalizes an encoding name to the spelling FreeSWITCH uses in its codec hash.
///
/// Matching is case-insensitive throughout FreeSWITCH (`switch_core_hash_init_nocase`),
/// so this normalization is cosmetic: readable output and well-defined comparisons.
/// Unknown names pass through unchanged.
///
/// Spellings verified against `switch_core_codec_add_implementation` calls in the
/// codec modules under `src/mod/codecs/` and `src/mod/applications/mod_spandsp/`.
pub(crate) fn canonical_iananame(name: &str) -> &str {
    CANONICAL_NAMES
        .iter()
        .find(|&&(key, _)| name.eq_ignore_ascii_case(key))
        .map(|&(_, canonical)| canonical)
        .unwrap_or(name)
}

/// Default sample rate for a codec, ported from `switch_default_rate` (`switch_core.c:2033`).
///
/// Exact match: `opus` → 48000. Prefix `h26` (3 chars) → 90000 (H.26x family).
/// Prefix `vp` (2 chars) → 90000 (VP8, VP9). Everything else → 8000.
/// No runtime config hook exists for this table.
pub fn default_rate(name: &str) -> u32 {
    // Mirrors switch_default_rate: exact "opus" then prefix "h26", then "vp", else 8000.
    if name.eq_ignore_ascii_case("opus") {
        48000
    } else if name
        .get(..3)
        .is_some_and(|pfx| pfx.eq_ignore_ascii_case("h26"))
        || name
            .get(..2)
            .is_some_and(|pfx| pfx.eq_ignore_ascii_case("vp"))
    {
        90000
    } else {
        8000
    }
}

/// Built-in default ptime in milliseconds, ported from the static hash entries
/// in `switch_load_core_config` (`switch_core.c:2053-2055`).
///
/// iLBC, iSAC, and G.723 default to 30 ms; everything else defaults to 20 ms.
/// A deployment can add entries via `switch.conf` `<default-ptimes>` (`:2061-2085`)
/// that this function cannot know about. The invariant `dedup()` relies on is that no
/// name present in the input disappears — a mandatory codec is always covered by at
/// least its bare name, which picks up the configured ptime at match time.
pub fn default_ptime(name: &str) -> u32 {
    if name.eq_ignore_ascii_case("ilbc")
        || name.eq_ignore_ascii_case("isac")
        || name.eq_ignore_ascii_case("G723")
    {
        30
    } else {
        20
    }
}

/// Returns the fixed bitrate for RFC 3551 static payload types, ported from
/// `switch_known_bitrate` in `switch_utils.h`.
///
/// Returns `None` for payload types not in the upstream table.
pub(crate) fn known_bitrate(pt: u8) -> Option<u32> {
    match pt {
        0 => Some(64000), // PCMU
        3 => Some(13200), // GSM
        4 => Some(6300),  // G723
        7 => Some(2400),  // LPC
        8 => Some(64000), // PCMA
        9 => Some(64000), // G722
        18 => Some(8000), // G729
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcmu_resolves() {
        let pt = static_payload_type(0).unwrap();
        assert_eq!(pt.encoding_name, "PCMU");
        assert_eq!(pt.clock_rate, 8000);
        assert_eq!(pt.channels, Some(1));
    }

    #[test]
    fn g722_clock_rate_is_8000() {
        // RFC 3551 quirk: G.722 is advertised at 8000 Hz in SDP even though it runs at
        // 16 kHz. switch_core_media_set_r_sdp_codec_string emits @8000h for G.722.
        let pt = static_payload_type(9).unwrap();
        assert_eq!(pt.encoding_name, "G722");
        assert_eq!(pt.clock_rate, 8000);
    }

    #[test]
    fn g729_resolves() {
        let pt = static_payload_type(18).unwrap();
        assert_eq!(pt.encoding_name, "G729");
        assert_eq!(pt.clock_rate, 8000);
        assert_eq!(pt.channels, Some(1));
    }

    #[test]
    fn video_type_channels_is_none() {
        // Video types have no channel count in the RFC 3551 table.
        let h261 = static_payload_type(31).unwrap();
        assert_eq!(h261.encoding_name, "H261");
        assert!(h261
            .channels
            .is_none());

        let h263 = static_payload_type(34).unwrap();
        assert_eq!(h263.encoding_name, "H263");
        assert!(h263
            .channels
            .is_none());

        let jpeg = static_payload_type(26).unwrap();
        assert_eq!(jpeg.encoding_name, "JPEG");
        assert!(jpeg
            .channels
            .is_none());
    }

    #[test]
    fn mp2t_channels_is_none() {
        // MP2T is audio+video; no meaningful channel count.
        let mp2t = static_payload_type(33).unwrap();
        assert_eq!(mp2t.encoding_name, "MP2T");
        assert!(mp2t
            .channels
            .is_none());
    }

    #[test]
    fn mpa_channels_none() {
        // MPA channel count is stream-defined by the MPEG bitstream, not the m= line.
        let mpa = static_payload_type(14).unwrap();
        assert_eq!(mpa.encoding_name, "MPA");
        assert!(mpa
            .channels
            .is_none());
    }

    #[test]
    fn unassigned_pt21_is_none() {
        assert!(static_payload_type(21).is_none());
    }

    #[test]
    fn dynamic_range_is_none() {
        assert!(static_payload_type(96).is_none());
        assert!(static_payload_type(255).is_none());
    }

    #[test]
    fn canonical_name_opus_is_lowercase() {
        // mod_opus registers iananame "opus" (lowercase).
        assert_eq!(canonical_iananame("OPUS"), "opus");
        assert_eq!(canonical_iananame("Opus"), "opus");
        assert_eq!(canonical_iananame("opus"), "opus");
    }

    #[test]
    fn canonical_name_ilbc_casing() {
        // mod_ilbc registers iananame "iLBC".
        assert_eq!(canonical_iananame("ILBC"), "iLBC");
        assert_eq!(canonical_iananame("ilbc"), "iLBC");
        assert_eq!(canonical_iananame("iLBC"), "iLBC");
    }

    #[test]
    fn canonical_name_speex_is_all_caps() {
        // switch_speex.c registers iananame "SPEEX" (all caps).
        assert_eq!(canonical_iananame("speex"), "SPEEX");
        assert_eq!(canonical_iananame("Speex"), "SPEEX");
        assert_eq!(canonical_iananame("SPEEX"), "SPEEX");
    }

    #[test]
    fn canonical_name_l16() {
        assert_eq!(canonical_iananame("l16"), "L16");
        assert_eq!(canonical_iananame("L16"), "L16");
    }

    #[test]
    fn canonical_name_video() {
        assert_eq!(canonical_iananame("vp8"), "VP8");
        assert_eq!(canonical_iananame("VP8"), "VP8");
        assert_eq!(canonical_iananame("vp9"), "VP9");
        assert_eq!(canonical_iananame("h264"), "H264");
        assert_eq!(canonical_iananame("H264"), "H264");
        assert_eq!(canonical_iananame("h263"), "H263");
        assert_eq!(canonical_iananame("H263-1998"), "H263-1998");
        assert_eq!(canonical_iananame("h263-1998"), "H263-1998");
    }

    #[test]
    fn canonical_name_b64_is_lowercase() {
        // mod_b64 registers iananame "b64" (lowercase).
        assert_eq!(canonical_iananame("B64"), "b64");
        assert_eq!(canonical_iananame("b64"), "b64");
    }

    #[test]
    fn canonical_name_unknown_passthrough() {
        assert_eq!(canonical_iananame("EVS"), "EVS");
        assert_eq!(canonical_iananame("unknown-codec"), "unknown-codec");
    }

    #[test]
    fn bitrate_pcmu() {
        assert_eq!(known_bitrate(0), Some(64000));
    }

    #[test]
    fn bitrate_g729() {
        assert_eq!(known_bitrate(18), Some(8000));
    }

    #[test]
    fn bitrate_absent_for_l16() {
        // PT 10 (L16) is assigned in RFC 3551 but absent from switch_known_bitrate.
        assert_eq!(known_bitrate(10), None);
    }

    #[test]
    fn bitrate_absent_for_dynamic() {
        assert_eq!(known_bitrate(96), None);
    }

    // --- default_rate and default_ptime ---

    #[test]
    fn default_rate_opus_is_48k() {
        assert_eq!(default_rate("opus"), 48000);
        assert_eq!(default_rate("OPUS"), 48000);
    }

    #[test]
    fn default_rate_h26x_is_90k() {
        assert_eq!(default_rate("H264"), 90000);
        assert_eq!(default_rate("h264"), 90000);
        assert_eq!(default_rate("H263"), 90000);
        assert_eq!(default_rate("H263-1998"), 90000);
    }

    #[test]
    fn default_rate_vp_is_90k() {
        assert_eq!(default_rate("VP8"), 90000);
        assert_eq!(default_rate("vp8"), 90000);
        assert_eq!(default_rate("VP9"), 90000);
    }

    #[test]
    fn default_rate_audio_is_8k() {
        assert_eq!(default_rate("PCMU"), 8000);
        assert_eq!(default_rate("G722"), 8000);
        assert_eq!(default_rate("AMR"), 8000);
        assert_eq!(default_rate("iLBC"), 8000);
        assert_eq!(default_rate("unknown"), 8000);
    }

    #[test]
    fn default_ptime_30ms_codecs() {
        assert_eq!(default_ptime("iLBC"), 30);
        assert_eq!(default_ptime("ilbc"), 30);
        assert_eq!(default_ptime("ILBC"), 30);
        assert_eq!(default_ptime("isac"), 30);
        assert_eq!(default_ptime("ISAC"), 30);
        assert_eq!(default_ptime("G723"), 30);
        assert_eq!(default_ptime("g723"), 30);
    }

    #[test]
    fn default_ptime_20ms_for_others() {
        assert_eq!(default_ptime("PCMU"), 20);
        assert_eq!(default_ptime("opus"), 20);
        assert_eq!(default_ptime("G722"), 20);
        assert_eq!(default_ptime("AMR"), 20);
    }
}
