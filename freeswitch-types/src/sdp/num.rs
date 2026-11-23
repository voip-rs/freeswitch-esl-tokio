//! The leading-digit read both the codec-string and the SDP attribute parsers do.

/// Value of the leading ASCII-digit run and the remainder after it; `None` for no
/// leading digit and for a run past `u32`, where C's `atoi` is undefined.
pub(crate) fn atoi_prefix(s: &str) -> (Option<u32>, &str) {
    let end = s
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(s.len());
    let (digits, rest) = s.split_at(end);
    (
        digits
            .parse::<u32>()
            .ok(),
        rest,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atoi_prefix_table() {
        let cases: &[(&str, Option<u32>, &str)] = &[
            ("8000h", Some(8000), "h"),
            ("20", Some(20), ""),
            ("h", None, "h"),
            ("", None, ""),
            ("9999999999h", None, "h"),
            ("20.5", Some(20), ".5"),
        ];
        for (input, value, rest) in cases {
            assert_eq!(atoi_prefix(input), (*value, *rest), "input {input:?}");
        }
    }
}
