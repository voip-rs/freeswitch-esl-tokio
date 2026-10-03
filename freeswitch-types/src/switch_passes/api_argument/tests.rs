use super::*;
use crate::commands::variables::{DialStringCarrier, DialStringTarget};
use crate::commands::{originate_quote, originate_split, originate_unquote, quote_for_uuid_setvar};
use crate::switch_passes::{trace, untrace};

fn tilde() -> DialStringTarget {
    DialStringTarget::new(DialStringCarrier::EslApi)
        .with_argv_separator('~')
        .expect("'~' separates originate's arguments")
}

#[test]
fn escape_argument_is_none_where_no_argument_split_runs() {
    for carrier in [DialStringCarrier::Dialplan, DialStringCarrier::Direct] {
        assert_eq!(DialStringTarget::new(carrier).escape_argument("a b"), None);
    }
    for (text, want) in [
        ("a b", r"a\sb"),
        (" a  b ", r"\sa\s\sb\s"),
        ("x~y", "x~y"),
        ("it's", r"it\'s"),
        (r"a\b", r"a\\b"),
        ("a\nb\tc\rd", r"a\nb\tc\rd"),
        ("", "''"),
        ("^^~a b", r"''^^~a\sb"),
    ] {
        assert_eq!(
            DialStringTarget::new(DialStringCarrier::EslApi)
                .escape_argument(text)
                .as_deref(),
            Some(want),
            "{text:?}"
        );
    }
    assert!(matches!(
        tilde().escape_argument("loopback/9199/test"),
        Some(std::borrow::Cow::Borrowed("loopback/9199/test"))
    ));
}

/// The last case is a capture originated under `^^~`.
#[test]
fn escape_argument_escapes_for_the_separator_split() {
    for (text, want) in [
        ("a b", "a b"),
        ("x~y", r"x\~y"),
        ("it's o'k", r"it\'s o\'k"),
        (r"a\b", r"a\\b"),
        (r"a\~b", r"a\\\~b"),
        (r"end\", r"end\\"),
        ("a\nb", r"a\nb"),
        ("a\rb", r"a\rb"),
        ("a\tb", r"a\tb"),
        (
            r"[presence_id=fp-argv-leg2@pbx.example.com,v=a b~c\d,sentinel=s]loopback/9199/test,[presence_id=fp-argv-leg2@pbx.example.com,sentinel=s2]loopback/9199/test",
            r"[presence_id=fp-argv-leg2@pbx.example.com,v=a b\~c\\d,sentinel=s]loopback/9199/test,[presence_id=fp-argv-leg2@pbx.example.com,sentinel=s2]loopback/9199/test",
        ),
    ] {
        assert_eq!(
            tilde()
                .escape_argument(text)
                .as_deref(),
            Some(want),
            "{text:?}"
        );
    }
}

#[test]
fn escape_argument_keeps_edge_spaces() {
    for (text, want) in [
        (" a ", r"\sa\s"),
        ("a  ", r"a \s"),
        ("  a", r"\s a"),
        (" ", r"\s"),
        ("  ", r"\s\s"),
        ("", "''"),
        ("^^~", "^^\\~"),
    ] {
        assert_eq!(
            tilde()
                .escape_argument(text)
                .as_deref(),
            Some(want),
            "{text:?}"
        );
    }
}

/// `switch_api_execute` strips a vertical tab from the edges of the argument line before
/// `originate` splits it, and no escape letter names one.
#[test]
fn a_vertical_tab_at_an_edge_survives_the_line_strip() {
    use crate::switch_passes::separate::separate;

    for text in ["\u{b}", "\u{b}a", "a\u{b}", "\u{b} \u{b}"] {
        for target in [DialStringTarget::new(DialStringCarrier::EslApi), tilde()] {
            let escaped = target
                .escape_argument(text)
                .expect("an API target escapes");
            let (prefix, sep) = match target.argv_separator() {
                Some(sep) => (format!("^^{sep}"), sep),
                None => (String::new(), ' '),
            };
            for (line, want) in [
                (format!("{prefix}{escaped}{sep}y"), vec![text, "y"]),
                (format!("{prefix}x{sep}{escaped}"), vec!["x", text]),
            ] {
                let stripped = line.trim_matches(STRIPPED_WHITESPACE);
                let tokens: Vec<String> = separate(&trace(stripped), ' ', usize::MAX)
                    .tokens
                    .iter()
                    .map(|token| untrace(&token.text))
                    .collect();
                assert_eq!(tokens, want, "{line:?}");
            }
        }
    }
}

/// An empty argument vanished from either split and a text opening `^^` renamed the blank
/// split's separator when it opened the line.
#[test]
fn an_empty_or_caret_led_text_stays_one_argument_in_any_position() {
    use crate::switch_passes::separate::separate;

    for text in ["", "^^", "^^~a", "^^ y"] {
        for target in [DialStringTarget::new(DialStringCarrier::EslApi), tilde()] {
            let escaped = target
                .escape_argument(text)
                .expect("an API target escapes");
            let (prefix, sep) = match target.argv_separator() {
                Some(sep) => (format!("^^{sep}"), sep),
                None => (String::new(), ' '),
            };
            for (line, want) in [
                (format!("{prefix}{escaped}{sep}y"), vec![text, "y"]),
                (
                    format!("{prefix}x{sep}{escaped}{sep}y"),
                    vec!["x", text, "y"],
                ),
                (format!("{prefix}x{sep}{escaped}"), vec!["x", text]),
            ] {
                let tokens: Vec<String> = separate(&trace(&line), ' ', usize::MAX)
                    .tokens
                    .iter()
                    .map(|token| untrace(&token.text))
                    .collect();
                assert_eq!(tokens, want, "{line:?}");
            }
        }
    }
}

#[test]
fn escape_argument_is_undone_by_the_split_cleanup() {
    use crate::switch_passes::separate::cleanup;

    for text in [
        "a b",
        " a b ",
        r"it's \'q\' ''",
        r#"a"b\"c"#,
        r"\\\~~\n\s",
        "tab\tcr\rnl\n",
        "x~y~",
        "~",
        r"trailing\",
        "é ü",
    ] {
        let escaped = tilde()
            .escape_argument(text)
            .expect("a separator target escapes");
        assert_eq!(
            untrace(&cleanup(&trace(&escaped), Some('~'))),
            text,
            "escaped {escaped:?}"
        );
    }
}

#[test]
fn escape_argument_is_undone_by_the_blank_split() {
    let api = DialStringTarget::new(DialStringCarrier::EslApi);
    for text in [
        "a b",
        "  a  b  ",
        r"it's \'q\' ''",
        r#"a"b\"c"#,
        r"\\\~~\n\s",
        "tab\tcr\rnl\n",
        r"trailing\",
        "é ü",
        "'",
        "",
    ] {
        let escaped = api
            .escape_argument(text)
            .expect("the API carrier escapes");
        let (argument, _) = api
            .read_argument(&escaped)
            .unwrap_or_else(|e| panic!("{escaped:?}: {e}"));
        assert_eq!(argument, text, "escaped {escaped:?}");
    }
}

#[test]
fn split_with_quotes_ignores_spaces_inside() {
    let result =
        originate_split("originate {test='variable with quote'}sofia/test 123", ' ').unwrap();
    assert_eq!(result[0], "originate");
    assert_eq!(result[1], "{test='variable with quote'}sofia/test");
    assert_eq!(result[2], "123");
}

#[test]
fn split_missing_quote_returns_error() {
    let result = originate_split(
        "originate {test='variable with missing quote}sofia/test 123",
        ' ',
    );
    assert!(result.is_err());
}

#[test]
fn split_string_starting_ending_with_quote() {
    let result = originate_split("'this is test'", ' ').unwrap();
    assert_eq!(result[0], "'this is test'");
}

#[test]
fn split_comma_separated() {
    let result = originate_split("item1,item2", ',').unwrap();
    assert_eq!(result[0], "item1");
    assert_eq!(result[1], "item2");
}

#[test]
fn split_with_escaped_quotes() {
    let result = originate_split(
        "originate {test='variable with quote'}sofia/test let\\'s add a quote",
        ' ',
    )
    .unwrap();
    assert_eq!(result[0], "originate");
    assert_eq!(result[1], "{test='variable with quote'}sofia/test");
    assert_eq!(result[2], "let\\'s");
    assert_eq!(result[3], "add");
    assert_eq!(result[4], "a");
    assert_eq!(result[5], "quote");
}

#[test]
fn quote_without_spaces_returns_as_is() {
    assert_eq!(originate_quote("&park()"), "&park()");
}

#[test]
fn quote_with_spaces_wraps_in_single_quotes() {
    assert_eq!(
        originate_quote("&socket(127.0.0.1:8040 async full)"),
        "'&socket(127.0.0.1:8040 async full)'"
    );
}

#[test]
fn quote_with_single_quote_and_spaces_escapes_quote() {
    assert_eq!(
        originate_quote("&playback(it's a test file)"),
        "'&playback(it\\'s a test file)'"
    );
}

/// A bare quote opens a region the blank split never closes, and a backslash inside the
/// wrapping is consumed by that split's cleanup.
#[test]
fn quote_wraps_any_token_carrying_a_single_quote() {
    assert_eq!(originate_quote("it's"), r"'it\'s'");
    assert_eq!(originate_quote(r"a\b it's"), r"'a\\b it\'s'");
    assert_eq!(originate_quote("a b"), "'a b'");
    assert_eq!(originate_quote(r"a\b c"), r"'a\\b c'");
    assert_eq!(originate_quote(r"a\,b"), r"'a\\,b'");
    assert_eq!(originate_quote(r"x\ny"), r"'x\\ny'");
    assert_eq!(originate_quote(""), "''");
    assert_eq!(originate_quote("&park()"), "&park()");
}

/// Every string over the characters the blank split and its cleanup treat specially
/// survives quoting, the split and unquoting unchanged.
#[test]
fn unquote_inverts_quote_through_the_blank_split() {
    const ALPHABET: [char; 7] = ['a', ' ', '\'', '\\', 'n', 's', '"'];
    let mut strings = vec![String::new()];
    for _ in 0..5 {
        let longer: Vec<String> = strings
            .iter()
            .filter(|s| {
                s.len()
                    == strings
                        .last()
                        .map_or(0, String::len)
            })
            .flat_map(|s| {
                ALPHABET
                    .iter()
                    .map(move |c| {
                        let mut next = s.clone();
                        next.push(*c);
                        next
                    })
            })
            .collect();
        strings.extend(longer);
    }
    for value in strings {
        let quoted = originate_quote(&value);
        assert_eq!(
            originate_unquote(&quoted),
            value,
            "{value:?} via {quoted:?}"
        );
        let line = format!("x {quoted} y");
        let tokens = originate_split(&line, ' ').unwrap_or_else(|e| panic!("{line:?}: {e}"));
        assert_eq!(tokens.len(), 3, "{line:?} split into {tokens:?}");
        assert_eq!(originate_unquote(&tokens[1]), value, "{line:?}");
    }
}

#[test]
fn unquote_non_quoted_returns_as_is() {
    assert_eq!(originate_unquote("&park()"), "&park()");
}

#[test]
fn unquote_strips_outer_quotes() {
    assert_eq!(
        originate_unquote("'&socket(127.0.0.1:8040 async full)'"),
        "&socket(127.0.0.1:8040 async full)"
    );
}

#[test]
fn unquote_unescapes_inner_quotes() {
    assert_eq!(
        originate_unquote("'&playback(it\\'s a test file)'"),
        "&playback(it's a test file)"
    );
}

#[test]
fn quote_unquote_round_trip() {
    let original = "&socket(127.0.0.1:8040 async full)";
    assert_eq!(originate_unquote(&originate_quote(original)), original);
}

#[test]
fn quote_unquote_round_trip_with_inner_quote() {
    let original = "&playback(it's a test file)";
    assert_eq!(originate_unquote(&originate_quote(original)), original);
}

#[test]
fn split_multiple_consecutive_spaces() {
    let result = originate_split("originate  sofia/test  123", ' ').unwrap();
    // Multiple consecutive spaces produce empty tokens that are trimmed/skipped
    assert_eq!(result[0], "originate");
    assert_eq!(result[1], "sofia/test");
    assert_eq!(result[2], "123");
}

#[test]
fn split_leading_trailing_spaces() {
    let result = originate_split("  originate sofia/test  ", ' ').unwrap();
    assert_eq!(result[0], "originate");
    assert_eq!(result[1], "sofia/test");
}

/// Measured on a live switch: `\\'` escapes the backslash, so the quote opens
/// a region the rest of the line never closes and `originate` answers usage.
#[test]
fn split_quote_after_escaped_backslash_opens_a_region() {
    assert!(matches!(
        originate_split(r"originate {v=x\\'y z}loopback/9199/test &park()", ' '),
        Err(OriginateError::UnclosedQuote(_))
    ));
}

/// Measured on a live switch: the backslash escapes the space, one argument.
#[test]
fn split_escaped_space_does_not_split() {
    assert_eq!(
        originate_split(r"originate {v=a\ b}loopback/9199/test &park()", ' ').unwrap(),
        ["originate", r"{v=a\ b}loopback/9199/test", "&park()"]
    );
}

/// Only a space separates, and only a space is trimmed.
#[test]
fn split_keeps_a_tab() {
    assert_eq!(
        originate_split("originate x\t y", ' ').unwrap(),
        ["originate", "x\t", "y"]
    );
}

/// A comma split toggles on a quote only when another quote follows it, and
/// keeps the empty token between two separators.
#[test]
fn split_on_comma_follows_the_char_delimiter_rules() {
    assert_eq!(originate_split("a'b,c", ',').unwrap(), ["a'b", "c"]);
    assert_eq!(originate_split("a,,b", ',').unwrap(), ["a", "", "b"]);
}

#[test]
fn split_honours_a_leading_argument_separator() {
    assert_eq!(
        originate_split(r"^^~{v=a b}loopback/9199/test~&park()", ' ').unwrap(),
        ["{v=a b}loopback/9199/test", "&park()"]
    );
    assert_eq!(originate_split("^^~a b~c", ',').unwrap(), ["a b", "c"]);
    assert_eq!(originate_split("^^~é", ' ').unwrap(), ["é"]);
}

#[test]
fn split_takes_no_override_without_a_byte_after_it() {
    assert_eq!(originate_split("^^~", ' ').unwrap(), ["^^~"]);
    assert_eq!(originate_split("^^~", ',').unwrap(), ["^^~"]);
}

/// The switch splits on the first byte of a non-ASCII separator, which no
/// char delimiter can mirror, so the default split runs over the whole line.
#[test]
fn split_takes_no_override_on_a_non_ascii_separator() {
    assert_eq!(originate_split("^^éaéb c", ' ').unwrap(), ["^^éaéb", "c"]);
    assert_eq!(originate_split("^^éaé,b", ',').unwrap(), ["^^éaé", "b"]);
}

#[test]
fn setvar_quoting_escapes_for_the_setvar_tokenizer() {
    let cases: &[(&str, &str)] = &[
        ("PCMU,PCMA", "'PCMU,PCMA'"),
        ("mode-set=0; octet-align=1", "'mode-set=0; octet-align=1'"),
        ("a'b", "'a\\'b'"),
        ("a\\b", "'a\\\\b'"),
    ];
    for (value, expected) in cases {
        assert_eq!(&quote_for_uuid_setvar(value), expected);
    }
}
