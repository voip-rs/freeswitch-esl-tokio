//! The port against the switch's own tokenizer, compiled from every tracked tree, byte for byte.

use proptest::prelude::*;
use proptest::sample::select;

use super::{
    argument_head, cleanup, escape_inside_token, find_end_paren, separate, separate_on,
    separate_string_string, Head,
};
use crate::switch_passes::{trace, untrace};
use freeswitch_c_oracle::against_the_c;

use crate::test_text::{opens_with_a_non_ascii_head, text};

/// Text that may open with a `^^X` head, ASCII or not.
fn line() -> impl Strategy<Value = String> {
    let head = select(&["^^", "^^ ", "^^~", "^^,", "^^'", "^^\\", "^^é", "^^😀"][..]);
    prop_oneof![
        3 => text(),
        1 => (head, text()).prop_map(|(head, rest)| format!("{head}{rest}")),
    ]
}

fn owned(tokens: impl IntoIterator<Item = String>) -> Vec<Vec<u8>> {
    tokens
        .into_iter()
        .map(String::into_bytes)
        .collect()
}

const CLEANUP_DELIMS: &[u8] = &[0, b',', b'~', b' ', b'|', b'=', b'\'', b'\\', b'n', b':'];
const SPLIT_DELIMS: &[u8] = b" ,|=~:'";
const LIMITS: &[u32] = &[1, 2, 3, 10, 128, 1024];

#[test]
fn cleanup_matches_the_switch() {
    against_the_c(
        file!(),
        "cleanup_matches_the_switch",
        (line(), select(CLEANUP_DELIMS)),
        |c, (input, delim)| {
            let port = untrace(&cleanup(
                &trace(&input),
                (delim != 0).then_some(char::from(delim)),
            ));
            prop_assert_eq!(
                c.cleanup(input.as_bytes(), delim),
                port.into_bytes(),
                "{:?} cleaned up on {:?}",
                input,
                char::from(delim)
            );
            Ok(())
        },
    );
}

#[test]
fn separate_matches_the_switch() {
    against_the_c(
        file!(),
        "separate_matches_the_switch",
        (line(), select(SPLIT_DELIMS), select(LIMITS)),
        |c, (input, delim, limit)| {
            let text = trace(&input);
            let port = separate(&text, char::from(delim), limit as usize);
            if opens_with_a_non_ascii_head(&input) {
                prop_assert_eq!(argument_head(&text), Head::Unreadable);
                return Ok(());
            }
            let port = owned(
                port.tokens
                    .iter()
                    .map(|token| untrace(&token.text)),
            );
            prop_assert_eq!(
                c.separate_string(input.as_bytes(), delim, limit),
                port,
                "{:?} on {:?} keeping {}",
                input,
                char::from(delim),
                limit
            );
            Ok(())
        },
    );
}

#[test]
fn char_and_blank_splits_match_the_switch() {
    against_the_c(
        file!(),
        "char_and_blank_splits_match_the_switch",
        (line(), select(SPLIT_DELIMS), select(LIMITS)),
        |c, (input, delim, limit)| {
            let port = owned(
                separate_on(&trace(&input), char::from(delim), limit as usize)
                    .tokens
                    .iter()
                    .map(|token| untrace(&token.text)),
            );
            let split = match delim {
                b' ' => c.blank_delim(input.as_bytes(), limit),
                delim => c.char_delim(input.as_bytes(), delim, limit),
            };
            prop_assert_eq!(
                split,
                port,
                "{:?} on {:?} keeping {}",
                input,
                char::from(delim),
                limit
            );
            Ok(())
        },
    );
}

/// Text escaped inside a token reads back as written through the switch's char split and cleanup.
#[test]
fn escaped_text_reads_back_through_the_switch() {
    against_the_c(
        file!(),
        "escaped_text_reads_back_through_the_switch",
        (text(), select(CLEANUP_DELIMS)),
        |c, (input, delim)| {
            let delimiter = (delim != 0).then_some(char::from(delim));
            let token = format!("a{}b", escape_inside_token(&input, delimiter));
            let want = format!("a{input}b").into_bytes();
            match delim {
                0 => prop_assert_eq!(c.cleanup(token.as_bytes(), 0), want, "{:?}", token),
                delim => prop_assert_eq!(
                    c.char_delim(token.as_bytes(), delim, 1024),
                    vec![want],
                    "{:?} on {:?}",
                    token,
                    char::from(delim)
                ),
            }
            Ok(())
        },
    );
}

/// Every fmtp a codec string renders is what the switch's entry split leaves after the `~`.
#[cfg(feature = "sdp")]
#[test]
fn codec_string_fmtp_reaches_the_switch() {
    use crate::sdp::{CodecString, CodecStringEntry};
    use proptest::collection::vec;

    against_the_c(
        file!(),
        "codec_string_fmtp_reaches_the_switch",
        vec(text(), 1..4),
        |c, fmtps| {
            let codecs: CodecString = fmtps
                .iter()
                .filter_map(|fmtp| {
                    CodecStringEntry::new("PCMU")
                        .and_then(|entry| entry.with_module("mod_x"))
                        .and_then(|entry| entry.with_fmtp(fmtp.as_str()))
                        .ok()
                })
                .collect();
            let want: Vec<Vec<u8>> = codecs
                .iter()
                .map(|entry| {
                    format!(
                        "mod_x.PCMU~{}",
                        entry
                            .fmtp()
                            .unwrap_or_default()
                    )
                    .into_bytes()
                })
                .collect();
            let line = codecs.to_string();
            prop_assert_eq!(
                c.char_delim(line.as_bytes(), b',', 1024),
                want,
                "{:?}",
                line
            );
            Ok(())
        },
    );
}

#[test]
fn string_split_matches_the_switch() {
    against_the_c(
        file!(),
        "string_split_matches_the_switch",
        (
            line(),
            select(&[":_:", ",", "ab", "::"][..]),
            select(LIMITS),
        ),
        |c, (input, delim, limit)| {
            let text = trace(&input);
            let port = owned(
                separate_string_string(&text, delim, limit as usize)
                    .spans
                    .into_iter()
                    .map(|span| untrace(&text[span])),
            );
            prop_assert_eq!(
                c.separate_string_string(input.as_bytes(), delim.as_bytes(), limit),
                port,
                "{:?} on {:?} keeping {}",
                input,
                delim,
                limit
            );
            Ok(())
        },
    );
}

#[test]
fn end_paren_matches_the_switch() {
    against_the_c(
        file!(),
        "end_paren_matches_the_switch",
        (
            line(),
            select(&[(b'[', b']'), (b'{', b'}'), (b'<', b'>'), (b'\'', b'\'')][..]),
        ),
        |c, (input, (open, close))| {
            let text = trace(&input);
            let port =
                find_end_paren(&text, char::from(open), char::from(close)).map(|at| text[at].1);
            prop_assert_eq!(
                c.find_end_paren(input.as_bytes(), open, close),
                port,
                "{:?} from {:?} to {:?}",
                input,
                char::from(open),
                char::from(close)
            );
            Ok(())
        },
    );
}
