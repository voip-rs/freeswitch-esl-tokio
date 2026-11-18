//! sofia-sip's byte scanners from `sdp_parse.c` — `next()`, `token()` and `parse_ul()` — and its
//! `TOKEN` set, over slices instead of NUL-terminated buffers. A slice's end stands for the NUL
//! sofia writes over the separator it consumed, so "the byte after the token" exists exactly when
//! the C's `retval[n]` is not NUL.

pub(crate) const SP_TAB: &[u8] = b" \t";
const SP: &[u8] = b" ";

/// `TOKEN`: `ALPHA DIGIT "-!#$%&'*+.^_`{|}~"`.
pub(crate) fn is_token(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-!#$%&'*+.^_`{|}~".contains(&b)
}

/// `"/" TOKEN`, the legal set `parse_media` reads a proto with.
pub(crate) fn is_slash_token(b: u8) -> bool {
    b == b'/' || is_token(b)
}

/// `s` past its leading run of bytes from `set`, as `strspn` skips them.
pub(crate) fn skip<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    let n = s
        .iter()
        .take_while(|b| set.contains(b))
        .count();
    &s[n..]
}

/// `token(&r, sep, legal, strip)`: the token and what follows it.
///
/// After `strip` is skipped, the token is the run of `legal` bytes, or with no `legal` the run up
/// to a `sep` byte. The byte ending the token is consumed whatever it is, then a run of `sep`.
pub(crate) fn token<'a>(
    s: &'a [u8],
    sep: &[u8],
    legal: Option<fn(u8) -> bool>,
    strip: &[u8],
) -> Option<(&'a [u8], &'a [u8])> {
    let s = skip(s, strip);
    let n = match legal {
        Some(legal) => s
            .iter()
            .take_while(|&&b| legal(b))
            .count(),
        None => s
            .iter()
            .take_while(|b| !sep.contains(b))
            .count(),
    };
    if n == 0 {
        return None;
    }
    let (token, rest) = s.split_at(n);
    let rest = match rest.split_first() {
        Some((_, after)) => skip(after, sep),
        None => rest,
    };
    Some((token, rest))
}

/// `token(&r, SPACE, legal, strip)` as `parse_media` reads its fields.
pub(crate) fn media_token<'a>(
    s: &'a [u8],
    legal: fn(u8) -> bool,
    strip: &[u8],
) -> Option<(&'a [u8], &'a [u8])> {
    token(s, SP, Some(legal), strip)
}

/// `parse_ul(p, &r, &value, max)`: `strtoul` after SP/TAB, refused when it reads no digit or, with
/// a nonzero `max`, reaches `max`; SP/TAB after the number is skipped.
pub(crate) fn parse_ul(s: &[u8], max: u64) -> Option<(u64, &[u8])> {
    let (value, rest) = strtoul(skip(s, SP_TAB))?;
    if max != 0 && max <= value {
        return None;
    }
    Some((value, skip(rest, SP_TAB)))
}

/// C's `strtoul(s, &end, 10)` on a 64-bit `unsigned long`: leading `isspace`, an optional sign, a
/// `-` negating modulo 2^64, and saturation on overflow. `None` where it reads no digit.
fn strtoul(s: &[u8]) -> Option<(u64, &[u8])> {
    let s = skip(s, b" \t\n\x0b\x0c\r");
    let (negative, s) = match s.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, s),
    };
    let digits = s
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    if digits == 0 {
        return None;
    }
    let value = s[..digits]
        .iter()
        .try_fold(0u64, |value, &d| {
            value
                .checked_mul(10)?
                .checked_add(u64::from(d - b'0'))
        });
    let value = match value {
        None => u64::MAX,
        Some(value) if negative => value.wrapping_neg(),
        Some(value) => value,
    };
    Some((value, &s[digits..]))
}

/// One line of the input: what `next()` makes a record of, from where its line starts.
pub(crate) struct Line<'a> {
    /// The byte offset where the line starts, ahead of any leading SP/TAB.
    pub(crate) offset: usize,
    /// The line past its leading SP/TAB, up to the CR, LF or NUL ending it.
    pub(crate) text: &'a [u8],
}

/// Every line of `input`, split on CR, LF and NUL. A run of those bytes ends one line, so the empty
/// lines `next()` steps over are not yielded; a line of only SP/TAB is, with empty `text`.
pub(crate) fn lines(input: &[u8]) -> impl Iterator<Item = Line<'_>> {
    let is_break = |b: &u8| matches!(b, b'\r' | b'\n' | 0);
    let mut at = 0;
    std::iter::from_fn(move || {
        at += input[at..]
            .iter()
            .take_while(|b| is_break(b))
            .count();
        if at == input.len() {
            return None;
        }
        let offset = at;
        let len = input[at..]
            .iter()
            .take_while(|b| !is_break(b))
            .count();
        at += len;
        Some(Line {
            offset,
            text: skip(&input[offset..at], SP_TAB),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_consumes_the_byte_that_ends_it_then_a_separator_run() {
        assert_eq!(
            token(b"rtpmap::96 x", b":", Some(is_token), SP_TAB),
            Some((&b"rtpmap"[..], &b"96 x"[..]))
        );
        assert_eq!(
            token(b"rtpmap 96", b":", Some(is_token), SP_TAB),
            Some((&b"rtpmap"[..], &b"96"[..]))
        );
        assert_eq!(token(b":x", b":", Some(is_token), SP_TAB), None);
        assert_eq!(
            token(b"sendonly", b":", Some(is_token), SP_TAB),
            Some((&b"sendonly"[..], &b""[..]))
        );
    }

    #[test]
    fn parse_ul_reads_as_strtoul_does() {
        assert_eq!(parse_ul(b" 96 x", 128), Some((96, &b"x"[..])));
        assert_eq!(parse_ul(b"128", 128), None);
        assert_eq!(parse_ul(b"+5", 0), Some((5, &b""[..])));
        assert_eq!(parse_ul(b"-1", 0), Some((u64::MAX, &b""[..])));
        assert_eq!(
            parse_ul(b"99999999999999999999999", 0),
            Some((u64::MAX, &b""[..]))
        );
        assert_eq!(parse_ul(b"x", 0), None);
        assert_eq!(parse_ul(b"96x", 128), Some((96, &b"x"[..])));
    }

    #[test]
    fn lines_split_on_cr_lf_and_nul_and_keep_whitespace_only_lines() {
        let found: Vec<(usize, &[u8])> = lines(b" v=0\r\n\r\n  \ra\0b\n")
            .map(|l| (l.offset, l.text))
            .collect();
        assert_eq!(found, [(0, &b"v=0"[..]), (8, b""), (11, b"a"), (13, b"b")]);
    }
}
