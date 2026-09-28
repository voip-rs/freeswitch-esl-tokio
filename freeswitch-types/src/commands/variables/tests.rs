use super::*;
use crate::switch_passes::escape::escape_value;
use crate::version::FreeswitchVersion;

/// Measured on a live switch: an empty value is discarded under every
/// encoding, silently, so nothing can be emitted for one and the refusal
/// has to name a remedy the caller could not guess.
#[test]
fn an_empty_value_is_refused_at_the_boundary() {
    let err = "{k=,after=sentinel}"
        .parse::<Variables>()
        .unwrap_err()
        .to_string();
    assert!(err.contains('k'), "error does not name the variable: {err}");
    assert!(
        err.contains("remove it"),
        "error does not name a remedy: {err}"
    );
    // Prescribing removal alone is wrong for a caller whose *presence* test
    // is the signal: dropping the key silently reverses that decision.
    assert!(
        err.contains("presence"),
        "error prescribes a fix without allowing that presence meant something: {err}"
    );
}

/// sofia copies a header-bound variable into the outgoing message verbatim, and
/// its parser ends a field only at CR/LF, so any other control character ships.
#[test]
fn a_control_character_in_a_header_bound_value_is_refused() {
    for block in [
        "{sip_h_X-Tag=a\u{1}b}",
        "{sip_rh_X-Tag=a\u{1b}b}",
        "{sip_ph_X-Tag=a\u{7f}b}",
        "{sip_bye_h_X-Tag=a\u{1}b}",
        "{SIP_H_X-Tag=a\u{1}b}",
    ] {
        assert!(
            block
                .parse::<Variables>()
                .is_err(),
            "{block:?} parsed"
        );
    }
    assert!(serde_json::from_str::<Variables>("{\"sip_h_X-Tag\":\"a\\u0001b\"}").is_err());
}

#[test]
fn a_tab_or_a_control_character_outside_a_header_passes() {
    for block in [
        "{sip_h_X-Tag=a\tb}",
        "{probe=a\u{1}b}",
        "{sip_i_x_tag=a\u{1}b}",
    ] {
        assert!(
            block
                .parse::<Variables>()
                .is_ok(),
            "{block:?} refused"
        );
    }
}

/// The switch finds a block's end by counting bracket depth and honours no
/// escape while doing so, so a value closing a bracket it never opened ends
/// the block early and the rest becomes dial-string text.
#[test]
fn an_unbalanced_bracket_is_refused_while_a_balanced_one_is_not() {
    assert!("{k=oops}extra}"
        .parse::<Variables>()
        .is_err());

    let balanced: Variables = "{k=${some_var}}"
        .parse()
        .expect("a balanced ${...} is ordinary and must parse");
    assert_eq!(balanced.get("k"), Some("${some_var}"));
}

/// A chosen separator is what carries a comma through a value that was
/// expanded before the block was parsed, so the comma must survive as
/// ordinary text rather than picking up an escape.
#[test]
fn chosen_separator_leaves_commas_alone() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("codecs", "PCMA,PCMU,G729");
    let vars = vars
        .with_separator(':')
        .unwrap();

    assert_eq!(vars.to_string(), "{^^:codecs=PCMA,PCMU,G729}");
    assert_eq!(vars.separator(), Some(':'));
}

#[test]
fn separator_that_cannot_delimit_the_block_is_refused() {
    let vars = Variables::new(VariablesType::Channel);
    for sep in ['[', ']', '=', '^', 'é', '§'] {
        assert!(
            vars.clone()
                .with_separator(sep)
                .is_err(),
            "accepted {sep:?}"
        );
    }
}

/// `separate_string_char_delim` skips the byte after a backslash, a quote pairs with the next
/// one, a space or control is trimmed or cuts the argument, `n r t s` name escapes, and dialplan
/// expansion reads `$` then `{` across a pair boundary as a reference.
#[test]
fn a_separator_breaking_the_switch_split_is_refused_everywhere() {
    for sep in [
        '\\', '\'', ' ', '\t', '\n', '\u{b}', '\0', '\u{7f}', 'n', 'r', 't', 's', '$', '{',
    ] {
        for scope in [
            VariablesType::Default,
            VariablesType::Enterprise,
            VariablesType::Channel,
        ] {
            let mut vars = Variables::new(scope);
            vars.insert("a", "1");
            assert!(
                vars.with_separator(sep)
                    .is_err(),
                "builder accepted {sep:?} in {scope:?}"
            );
        }
        assert!(
            Variables::parse_for(
                &format!("{{^^{sep}a=1{sep}b=2}}"),
                DialStringCarrier::Dialplan
            )
            .is_err(),
            "parser accepted {sep:?}"
        );
    }
    for sep in ['~', ';', '!', '#', 'N', '0', '"', ','] {
        let mut vars = Variables::new(VariablesType::Default);
        vars.insert("a", "1");
        assert!(
            vars.with_separator(sep)
                .is_ok(),
            "refused {sep:?}"
        );
    }
}

/// The parser has to refuse what the builder refuses, `^` included:
/// accepting a block no render of this crate can reproduce hands the caller
/// a value that changes when it is written back out.
#[test]
fn the_parser_refuses_every_separator_the_builder_does() {
    for sep in ['[', ']', '=', '^', 'é', '§'] {
        assert!(
            format!("[^^{sep}a=1{sep}b=2]")
                .parse::<Variables>()
                .is_err(),
            "parser accepted {sep:?}"
        );
    }
}

/// Refusing here is the whole point: a value carrying the separator would
/// split into a pair that was never written, and the switch reports nothing.
#[test]
fn separator_already_present_in_a_value_is_refused() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("uri", "sip:bob@example.com");
    let err = vars
        .with_separator(':')
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("uri"),
        "error does not name the variable: {err}"
    );
}

/// Measured against a live switch, with two awkward values in one block —
/// a block carrying only one is more forgiving and hides the failure.
/// Sweeping the backslash count, each carrier succeeds at counts the other
/// fails, so these forms are the wire contract and not a preference.
#[test]
fn escaping_pins_the_measured_wire_forms() {
    let cases = [
        (DialStringCarrier::Dialplan, "it's", r"it\\\\\\'s"),
        (DialStringCarrier::EslApi, "it's", r"it\\\\\\\'s"),
        (DialStringCarrier::Dialplan, "l'a'b", r"l\\\\\\'a\\\\\\'b"),
        (DialStringCarrier::EslApi, "l'a'b", r"l\\\\\\\'a\\\\\\\'b"),
        (DialStringCarrier::Dialplan, r"a\nb", r"a\\\\\\\\nb"),
        (DialStringCarrier::EslApi, r"a\nb", r"a\\\\\\\\nb"),
        (DialStringCarrier::Dialplan, "a,b", r"a\,b"),
        (DialStringCarrier::EslApi, "a,b", r"a\,b"),
        (DialStringCarrier::Dialplan, "a|b", "a|b"),
        (DialStringCarrier::EslApi, "a|b", "a|b"),
        (DialStringCarrier::Dialplan, "pa$$word", r"\'pa\$\$word"),
        (DialStringCarrier::Dialplan, "a$b", "a$b"),
        (DialStringCarrier::EslApi, "pa$$word", "pa$$word"),
    ];
    for (carrier, value, want) in cases {
        assert_eq!(
            escape_value(value, carrier, true, VariablesType::Default),
            want,
            "{value:?} for {carrier:?}"
        );
    }
}

/// A `[]` block rides through the leg splits before it is parsed: two more
/// backslash-consuming passes, and a pipe the first of them would read.
#[test]
fn channel_scope_escapes_for_the_leg_splits() {
    let cases = [
        (
            DialStringCarrier::Dialplan,
            r"a\nb",
            r"a\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\nb",
        ),
        (
            DialStringCarrier::EslApi,
            r"a\nb",
            r"a\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\nb",
        ),
        (DialStringCarrier::Dialplan, "a|b", r"a\|b"),
        (DialStringCarrier::EslApi, "a|b", r"a\|b"),
        (DialStringCarrier::EslApi, "a,b", r"a\,b"),
    ];
    for (carrier, value, want) in cases {
        assert_eq!(
            escape_value(value, carrier, true, VariablesType::Channel),
            want,
            "{value:?} for {carrier:?}"
        );
    }
}

/// A `^^` block reaches the switch's tokenizer as often as the comma form,
/// so every rule above still holds there; only the comma stops being the
/// separator and so stops being escaped.
#[test]
fn a_chosen_separator_changes_only_the_comma() {
    let cases = [
        (DialStringCarrier::Dialplan, "it's", r"it\\\\\\'s"),
        (DialStringCarrier::EslApi, "it's", r"it\\\\\\\'s"),
        (DialStringCarrier::Dialplan, "l'a'b", r"l\\\\\\'a\\\\\\'b"),
        (DialStringCarrier::EslApi, "l'a'b", r"l\\\\\\\'a\\\\\\\'b"),
        (DialStringCarrier::Dialplan, r"a\nb", r"a\\\\\\\\nb"),
        (DialStringCarrier::EslApi, r"a\nb", r"a\\\\\\\\nb"),
        (DialStringCarrier::Dialplan, "a,b", "a,b"),
        (DialStringCarrier::EslApi, "a,b", "a,b"),
    ];
    for (carrier, value, want) in cases {
        assert_eq!(
            escape_value(value, carrier, false, VariablesType::Default),
            want,
            "{value:?} for {carrier:?}"
        );
    }
}

/// The crate exists to drive `api originate`, so a block rendered with no
/// carrier named is rendered for that one.
#[test]
fn display_defaults_to_the_api_carrier() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("k", "it's");
    assert_eq!(vars.to_string(), r"{k=it\\\\\\\'s}");
    assert_eq!(
        vars.display_for(DialStringCarrier::Dialplan)
            .to_string(),
        r"{k=it\\\\\\'s}"
    );
}

#[test]
fn round_trips_at_either_carrier() {
    for carrier in [DialStringCarrier::EslApi, DialStringCarrier::Dialplan] {
        for value in ["it's", r"a\,b", r"C:\path", "a,b", "with space"] {
            let mut vars = Variables::new(VariablesType::Default);
            vars.insert("k", value);
            vars.insert("after", "sentinel");
            let rendered = vars
                .display_for(carrier)
                .to_string();

            let back = Variables::parse_for(&rendered, carrier)
                .unwrap_or_else(|e| panic!("{value:?} for {carrier:?} rendered {rendered}: {e}"));
            assert_eq!(back.get("k"), Some(value), "rendered {rendered}");
            assert_eq!(back.get("after"), Some("sentinel"), "rendered {rendered}");
        }
    }
}

/// A chosen separator changes which character needs escaping, not whether
/// the block is escape-processed: the switch runs the same tokenizer over
/// it, so a value still comes back through the same undoing.
#[test]
fn separated_block_round_trips_at_either_carrier() {
    for carrier in [DialStringCarrier::EslApi, DialStringCarrier::Dialplan] {
        for value in ["it's", "a,b", r"C:\path", r"a\nb", "with space"] {
            let mut vars = Variables::new(VariablesType::Default);
            vars.insert("k", value);
            vars.insert("after", "sentinel");
            let vars = vars
                .with_separator('~')
                .expect("'~' appears in none of these values");
            let rendered = vars
                .display_for(carrier)
                .to_string();

            let back = Variables::parse_for(&rendered, carrier)
                .unwrap_or_else(|e| panic!("{value:?} for {carrier:?} rendered {rendered}: {e}"));
            assert_eq!(back.get("k"), Some(value), "rendered {rendered}");
            assert_eq!(back.get("after"), Some("sentinel"), "rendered {rendered}");
        }
    }
}

/// `split_unescaped_commas` reads a comma behind an even number of
/// backslashes as a real separator, so a value ending in a backslash has to
/// be written with its own backslash escaped or the writer contradicts the
/// reader and the block no longer parses.
#[test]
fn value_with_backslash_round_trips() {
    for value in [r"a\,b", r"C:\path", r"trailing\", r"\\", r"a\nb"] {
        let mut vars = Variables::new(VariablesType::Default);
        vars.insert("k", value);
        vars.insert("after", "sentinel");
        let rendered = vars.to_string();

        let back: Variables = rendered
            .parse()
            .unwrap_or_else(|e| panic!("{value:?} rendered {rendered} and failed to parse: {e}"));
        assert_eq!(back.get("k"), Some(value), "rendered {rendered}");
        assert_eq!(
            back.get("after"),
            Some("sentinel"),
            "value {value:?} ate the next variable: {rendered}"
        );
    }
}

/// A variable block holds dialled numbers and passthrough header values, so a
/// malformed part is reported by position.
#[test]
fn missing_equals_error_omits_the_fragment() {
    let msg = "{origination_caller_id_number=15551234567,15550009999}"
        .parse::<Variables>()
        .unwrap_err()
        .to_string();
    assert!(
        !msg.contains("15550009999"),
        "error quoted its input: {msg}"
    );
    assert!(
        msg.contains("variable 1"),
        "error does not name the part: {msg}"
    );
}

#[test]
fn unknown_delimiters_error_omits_the_block() {
    let msg = "(origination_caller_id_number=15551234567)"
        .parse::<Variables>()
        .unwrap_err()
        .to_string();
    assert!(
        !msg.contains("15551234567"),
        "error quoted its input: {msg}"
    );
}

#[test]
fn variables_standard_chars() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("test_key", "this_value");
    vars.insert("second", "2");
    assert_eq!(vars.to_string(), "{test_key=this_value,second=2}");
}

#[test]
fn variables_comma_escaped() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("test_key", "this,is,a,value");
    let result = vars.to_string();
    assert!(result.contains("\\,"));
}

#[test]
fn variables_spaces_quoted() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("test_key", "this is a value");
    let result = vars.to_string();
    assert_eq!(
        result
            .matches('\'')
            .count(),
        2
    );
}

#[test]
fn variables_single_quote_escaped() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("test_key", "let's_this_be_a_value");
    let result = vars.to_string();
    assert!(result.contains("\\'"));
}

#[test]
fn variables_enterprise_delimiters() {
    let mut vars = Variables::new(VariablesType::Enterprise);
    vars.insert("k", "v");
    let result = vars.to_string();
    assert!(result.starts_with('<'));
    assert!(result.ends_with('>'));
}

#[test]
fn variables_channel_delimiters() {
    let mut vars = Variables::new(VariablesType::Channel);
    vars.insert("k", "v");
    let result = vars.to_string();
    assert!(result.starts_with('['));
    assert!(result.ends_with(']'));
}

#[test]
fn variables_default_delimiters() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("k", "v");
    let result = vars.to_string();
    assert!(result.starts_with('{'));
    assert!(result.ends_with('}'));
}

#[test]
fn variables_parse_round_trip() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("origination_caller_id_number", "9005551212");
    vars.insert("sip_h_Call-Info", "<url>;meta=123,<uri>");
    let s = vars.to_string();
    let parsed: Variables = s
        .parse()
        .unwrap();
    assert_eq!(
        parsed.get("origination_caller_id_number"),
        Some("9005551212")
    );
    assert_eq!(parsed.get("sip_h_Call-Info"), Some("<url>;meta=123,<uri>"));
}

#[test]
fn variables_caret_caret_separator() {
    let vars: Variables = "[^^:sip_invite_domain=pbx.example.com:presence_id=1211@pbx.example.com]"
        .parse()
        .unwrap();
    assert_eq!(vars.scope(), VariablesType::Channel);
    assert_eq!(vars.get("sip_invite_domain"), Some("pbx.example.com"));
    assert_eq!(vars.get("presence_id"), Some("1211@pbx.example.com"));
}

#[test]
fn variables_caret_caret_display_uses_canonical_comma() {
    let vars: Variables = "[^^:a=1:b=2]"
        .parse()
        .unwrap();
    assert_eq!(vars.to_string(), "[a=1,b=2]");
}

#[test]
fn variables_caret_caret_default_scope() {
    let vars: Variables = "{^^|x=1|y=2}"
        .parse()
        .unwrap();
    assert_eq!(vars.scope(), VariablesType::Default);
    assert_eq!(vars.get("x"), Some("1"));
    assert_eq!(vars.get("y"), Some("2"));
}

#[test]
fn variables_caret_caret_enterprise_scope() {
    let vars: Variables = "<^^;a=1;b=2>"
        .parse()
        .unwrap();
    assert_eq!(vars.scope(), VariablesType::Enterprise);
    assert_eq!(vars.get("a"), Some("1"));
}

/// A cleanup consumes a backslash only before a quote, another backslash, its own delimiter
/// or an escape letter, so a block separated on something else keeps `\,`; a `[]` block meets
/// the `,` leg split first, which reads it as a comma.
#[test]
fn variables_caret_caret_keeps_an_escaped_comma_outside_channel_scope() {
    for (block, want) in [
        (r"{^^:key=val\,ue:other=x}", r"val\,ue"),
        (r"[^^:key=val\,ue:other=x]", "val,ue"),
    ] {
        let vars: Variables = block
            .parse()
            .unwrap_or_else(|e| panic!("{block}: {e}"));
        assert_eq!(vars.get("key"), Some(want), "{block}");
    }
}

#[test]
fn variables_caret_caret_values_with_commas() {
    let vars: Variables = "{^^|sip_h_X-Call-Info=<urn:foo>;purpose=bar,<urn:baz>|other=val}"
        .parse()
        .unwrap();
    assert_eq!(
        vars.get("sip_h_X-Call-Info"),
        Some("<urn:foo>;purpose=bar,<urn:baz>")
    );
    assert_eq!(vars.get("other"), Some("val"));
}

#[test]
fn variables_caret_caret_empty_vars() {
    let vars: Variables = "[^^:]"
        .parse()
        .unwrap();
    assert!(vars.is_empty());
    assert_eq!(vars.scope(), VariablesType::Channel);
}

#[test]
fn variables_caret_caret_missing_separator() {
    assert!("[^^]"
        .parse::<Variables>()
        .is_err());
}

#[test]
fn variables_caret_caret_closing_bracket_as_sep() {
    assert!("[^^]]"
        .parse::<Variables>()
        .is_err());
}

#[test]
fn variables_caret_caret_equals_as_sep() {
    assert!("[^^=a=1]"
        .parse::<Variables>()
        .is_err());
}

#[test]
fn variables_from_str_empty_block() {
    let result = "{}".parse::<Variables>();
    assert!(
        result.is_ok(),
        "empty variable block should parse successfully"
    );
    let vars = result.unwrap();
    assert!(
        vars.is_empty(),
        "parsed empty block should have no variables"
    );
}

#[test]
fn variables_from_str_empty_channel_block() {
    let result = "[]".parse::<Variables>();
    assert!(result.is_ok());
    let vars = result.unwrap();
    assert!(vars.is_empty());
    assert_eq!(vars.scope(), VariablesType::Channel);
}

#[test]
fn variables_from_str_empty_enterprise_block() {
    let result = "<>".parse::<Variables>();
    assert!(result.is_ok());
    let vars = result.unwrap();
    assert!(vars.is_empty());
    assert_eq!(vars.scope(), VariablesType::Enterprise);
}

#[test]
fn serde_variables_type() {
    let json = serde_json::to_string(&VariablesType::Enterprise).unwrap();
    assert_eq!(json, "\"enterprise\"");
    let parsed: VariablesType = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, VariablesType::Enterprise);
}

#[test]
fn serde_variables_flat_default() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("key1", "val1");
    vars.insert("key2", "val2");
    let json = serde_json::to_string(&vars).unwrap();
    let parsed: Variables = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.scope(), VariablesType::Default);
    assert_eq!(parsed.get("key1"), Some("val1"));
    assert_eq!(parsed.get("key2"), Some("val2"));
}

#[test]
fn serde_variables_scoped_enterprise() {
    let mut vars = Variables::new(VariablesType::Enterprise);
    vars.insert("key1", "val1");
    let json = serde_json::to_string(&vars).unwrap();
    assert!(json.contains("\"enterprise\""));
    let parsed: Variables = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.scope(), VariablesType::Enterprise);
    assert_eq!(parsed.get("key1"), Some("val1"));
}

#[test]
fn serde_variables_flat_map_deserializes_as_default() {
    let json = r#"{"key1":"val1","key2":"val2"}"#;
    let vars: Variables = serde_json::from_str(json).unwrap();
    assert_eq!(vars.scope(), VariablesType::Default);
    assert_eq!(vars.get("key1"), Some("val1"));
    assert_eq!(vars.get("key2"), Some("val2"));
}

#[test]
fn serde_variables_scoped_deserializes() {
    let json = r#"{"scope":"channel","vars":{"k":"v"}}"#;
    let vars: Variables = serde_json::from_str(json).unwrap();
    assert_eq!(vars.scope(), VariablesType::Channel);
    assert_eq!(vars.get("k"), Some("v"));
}

/// A block whose separator is dropped by a round trip renders as a comma
/// block, which splits the very value the separator was chosen to carry.
#[test]
fn serde_variables_separator_round_trips() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("codecs", "PCMA,PCMU");
    let vars = vars
        .with_separator('|')
        .unwrap();

    let json = serde_json::to_string(&vars).unwrap();
    let parsed: Variables = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.separator(), Some('|'));
    assert_eq!(parsed, vars);
    assert_eq!(parsed.to_string(), "{^^|codecs=PCMA,PCMU}");
}

/// The default separator has no key, so a config written before the key
/// existed serializes back byte-identical.
#[test]
fn serde_variables_default_separator_is_absent() {
    let mut flat = Variables::new(VariablesType::Default);
    flat.insert("k", "v");
    assert_eq!(serde_json::to_string(&flat).unwrap(), r#"{"k":"v"}"#);

    let mut scoped = Variables::new(VariablesType::Channel);
    scoped.insert("k", "v");
    assert_eq!(
        serde_json::to_string(&scoped).unwrap(),
        r#"{"scope":"channel","vars":{"k":"v"}}"#
    );
}

#[test]
fn serde_variables_separator_deserializes_from_config() {
    let json = r#"{"scope":"channel","vars":{"codecs":"PCMA,PCMU"},"separator":":"}"#;
    let vars: Variables = serde_json::from_str(json).unwrap();
    assert_eq!(vars.scope(), VariablesType::Channel);
    assert_eq!(vars.separator(), Some(':'));
    assert_eq!(vars.to_string(), "[^^:codecs=PCMA,PCMU]");
}

/// The leg split reads a `|` before a `[]` block is parsed, so it can
/// separate nothing there; in `{}` it is ordinary.
#[test]
fn pipe_separator_is_refused_in_channel_scope_only() {
    let mut vars = Variables::new(VariablesType::Channel);
    vars.insert("k", "v");
    assert!(vars
        .clone()
        .with_separator('|')
        .is_err());
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("k", "v");
    assert!(vars
        .with_separator('|')
        .is_ok());
}

/// The builder's two refusals have to hold at the config boundary too, or a
/// YAML file produces a block the same crate would not build.
#[test]
fn serde_variables_unusable_separator_is_refused() {
    let undelimitable = r#"{"scope":"default","vars":{"k":"v"},"separator":"="}"#;
    assert!(serde_json::from_str::<Variables>(undelimitable).is_err());

    let in_a_value = r#"{"scope":"default","vars":{"uri":"sip:bob@example.com"},"separator":":"}"#;
    let err = serde_json::from_str::<Variables>(in_a_value)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("uri"),
        "error does not name the variable: {err}"
    );
}

/// Measured: `[p1=it's,p2=don't,p3=x]` reaches the channel as
/// `p1=its,p2=dont` with no `p2`, at every escaping depth, because the scan
/// ahead of the peer split pairs quotes across values. The same value in
/// default scope is ordinary.
#[test]
fn a_quote_in_channel_scope_is_refused_at_every_boundary() {
    let err = r"[cid=it\\\\\\\'s]"
        .parse::<Variables>()
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("cid") && err.contains("channel scope"),
        "error does not name the variable and scope: {err}"
    );

    let err = serde_json::from_str::<Variables>(r#"{"scope":"channel","vars":{"cid":"it's"}}"#)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("cid") && err.contains("channel scope"),
        "error does not name the variable and scope: {err}"
    );

    assert!(r"{cid=it\\\\\\\'s}"
        .parse::<Variables>()
        .is_ok());
    assert!(
        serde_json::from_str::<Variables>(r#"{"scope":"default","vars":{"cid":"it's"}}"#).is_ok()
    );
}

/// Every revision a block can be rendered for; a new variant joins this list.
const REVISIONS: &[BlockParse] = &[BlockParse::PairSplitCleans];

#[test]
fn a_bare_carrier_is_a_target_at_the_default_revision() {
    let target = DialStringTarget::from(DialStringCarrier::Dialplan);
    assert_eq!(target.carrier(), DialStringCarrier::Dialplan);
    assert_eq!(target.block_parse(), BlockParse::default());
    assert_eq!(BlockParse::default(), BlockParse::PairSplitCleans);
    assert_eq!(
        DialStringTarget::new(DialStringCarrier::EslApi)
            .with_block_parse(BlockParse::PairSplitCleans)
            .block_parse(),
        BlockParse::PairSplitCleans
    );
}

/// An enterprise block is parsed at the same depth as a default one; nothing
/// else pins its forms.
#[test]
fn enterprise_scope_escapes_like_default_scope() {
    let cases = [
        (DialStringCarrier::Dialplan, "it's", r"it\\\\\\'s"),
        (DialStringCarrier::EslApi, "it's", r"it\\\\\\\'s"),
        (DialStringCarrier::EslApi, r"a\nb", r"a\\\\\\\\nb"),
        (DialStringCarrier::EslApi, "a,b", r"a\,b"),
    ];
    for (carrier, value, want) in cases {
        let target = DialStringTarget::new(carrier).with_block_parse(BlockParse::PairSplitCleans);
        assert_eq!(
            escape_value(value, target, true, VariablesType::Enterprise),
            want,
            "{value:?} for {carrier:?}"
        );
    }
}

/// Measured: the `=` split trims both edges of a value, quoted or not, so an edge space
/// must still read `\s` entering it.
#[test]
fn an_edge_space_is_escaped_for_the_pass_that_trims_it() {
    let api = DialStringTarget::new(DialStringCarrier::EslApi);
    let dialplan = DialStringTarget::new(DialStringCarrier::Dialplan);
    let cases = [
        (api, VariablesType::Default, " a b ", r"'\\\\sa b\\\\s'"),
        (
            dialplan,
            VariablesType::Default,
            " a b ",
            r"'\\\\sa b\\\\s'",
        ),
        (api, VariablesType::Enterprise, " a", r"\\\\sa"),
        (api, VariablesType::Default, "a  ", r"'a \\\\s'"),
        (api, VariablesType::Default, " ", r"\\\\s"),
        (api, VariablesType::Default, "  ", r"\\\\s\\\\s"),
        (api, VariablesType::Default, r"a\ ", r"a\\\\\\\\\\\\s"),
        (
            api,
            VariablesType::Channel,
            " a ",
            &format!("{0}sa{0}s", "\\".repeat(16)),
        ),
    ];
    for (target, scope, value, want) in cases {
        assert_eq!(
            escape_value(value, target, true, scope),
            want,
            "{value:?} in {scope:?} at {target:?}"
        );
    }

    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("k", " a b ");
    assert_eq!(
        vars.display_for(tilde())
            .to_string(),
        r"{k=\'\\\\sa b\\\\s\'}"
    );
}

/// The scan ahead of the leg split protects a `[]` comma only when the byte before it is no
/// backslash, so a value ending in one is followed by an empty `''` reaching that scan bare.
#[test]
fn a_channel_value_ending_in_a_backslash_guards_the_next_comma() {
    let run = "\\".repeat(32);
    for (target, guard) in [
        (
            DialStringTarget::new(DialStringCarrier::EslApi),
            r"\\\'\\\'",
        ),
        (
            DialStringTarget::new(DialStringCarrier::Dialplan),
            r"\\'\\'",
        ),
    ] {
        assert_eq!(
            escape_value(r"a\", target, true, VariablesType::Channel),
            format!("a{run}{guard}"),
            "{target:?}"
        );
        assert_eq!(
            escape_value(r"a\", target, false, VariablesType::Channel),
            format!("a{run}"),
            "{target:?}"
        );
        assert_eq!(
            escape_value(r"a\,b", target, false, VariablesType::Channel),
            format!("a{run}{guard},b"),
            "{target:?}"
        );
    }
    assert_eq!(
        escape_value(
            r"a\",
            DialStringCarrier::EslApi,
            true,
            VariablesType::Default
        ),
        r"a\\\\\\\\"
    );
}

#[test]
fn round_trips_at_every_target() {
    const EDGES: [&str; 6] = [" lead and trail ", " a", "a  ", " ", r"a\ ", r"a\s"];
    let cases = [
        (
            VariablesType::Default,
            &["it's", r"C:\path", "a,b", r"a\nb", "with space", "x~y"][..],
        ),
        (
            VariablesType::Enterprise,
            &["it's", r"C:\path", "a,b", "with space", "x~y"][..],
        ),
        (
            VariablesType::Channel,
            &[r"C:\path", "a,b", "a|b", "with space", "x~y"][..],
        ),
    ];
    let cases = cases.map(|(scope, values)| {
        let values: Vec<&str> = values
            .iter()
            .chain(&EDGES)
            .copied()
            .collect();
        (scope, values)
    });
    for &block_parse in REVISIONS {
        for target in [
            DialStringTarget::new(DialStringCarrier::EslApi),
            DialStringTarget::new(DialStringCarrier::Dialplan),
            tilde(),
        ] {
            let target = target.with_block_parse(block_parse);
            for (scope, values) in &cases {
                for &value in values {
                    let mut vars = Variables::new(*scope);
                    vars.insert("k", value);
                    vars.insert("after", "sentinel");
                    let rendered = vars
                        .display_for(target)
                        .to_string();

                    let back = Variables::parse_for(&rendered, target).unwrap_or_else(|e| {
                        panic!("{value:?} for {target:?} rendered {rendered}: {e}")
                    });
                    assert_eq!(back.get("k"), Some(value), "rendered {rendered}");
                    assert_eq!(back.get("after"), Some("sentinel"), "rendered {rendered}");
                }
            }
        }
    }
}

/// What the switch cannot deliver is decided before any block parse runs, so
/// no revision makes one of these representable.
#[test]
fn refusals_do_not_depend_on_the_revision() {
    for &block_parse in REVISIONS {
        for (carrier, quoted) in [
            (DialStringCarrier::EslApi, r"[cid=it\\\\\\\'s]"),
            (DialStringCarrier::Dialplan, r"[cid=it\\\\\\'s]"),
        ] {
            let target = DialStringTarget::new(carrier).with_block_parse(block_parse);
            for block in [
                quoted,
                "{k=,after=sentinel}",
                "{k=oops}extra}",
                "{^^=a=1=b=2}",
                "[^^|a=1|b=2]",
            ] {
                assert!(
                    Variables::parse_for(block, target).is_err(),
                    "{block} accepted at {target:?}"
                );
            }
        }
    }
}

#[test]
fn block_parse_reads_config_spellings_and_writes_the_canonical_one() {
    for spelling in [
        "pair_split_cleans",
        "PAIR_SPLIT_CLEANS",
        "Pair_Split_Cleans",
    ] {
        assert_eq!(
            spelling
                .parse::<BlockParse>()
                .ok(),
            Some(BlockParse::PairSplitCleans),
            "{spelling}"
        );
    }
    assert_eq!(BlockParse::PairSplitCleans.to_string(), "pair_split_cleans");

    let err = "pair_split_whatever"
        .parse::<BlockParse>()
        .unwrap_err()
        .to_string();
    assert!(!err.contains("whatever"), "error quoted its input: {err}");
}

#[test]
fn serde_block_parse_uses_the_config_spelling() {
    assert_eq!(
        serde_json::to_string(&BlockParse::PairSplitCleans).unwrap(),
        r#""pair_split_cleans""#
    );
    let parsed: BlockParse = serde_json::from_str(r#""pair_split_cleans""#).unwrap();
    assert_eq!(parsed, BlockParse::PairSplitCleans);
}

#[test]
fn for_version_maps_a_vouched_release() {
    for version in [
        FreeswitchVersion::new(1, 10, 0),
        FreeswitchVersion::new(1, 10, 7),
        FreeswitchVersion::new(1, 10, 12),
        FreeswitchVersion::new(1, 11, 0),
        FreeswitchVersion::new(1, 11, 1),
        FreeswitchVersion::new(1, 11, 3),
    ] {
        assert_eq!(
            BlockParse::for_version(&version).ok(),
            Some(BlockParse::PairSplitCleans),
            "{version}"
        );
    }
}

/// A dev build reports one version across every commit, so even one below
/// the vouched range's last release is refused.
#[test]
fn for_version_refuses_what_it_cannot_vouch_for() {
    for version in [
        FreeswitchVersion::new(1, 10, 12).dev(),
        FreeswitchVersion::new(1, 10, 5).dev(),
    ] {
        let err = BlockParse::for_version(&version).expect_err(&version.to_string());
        assert!(
            matches!(err, UnvouchedVersion::Dev { .. }),
            "{version}: {err:?}"
        );
        assert_eq!(err.version(), version);
    }
    for version in [
        FreeswitchVersion::new(1, 11, 4),
        FreeswitchVersion::new(1, 12, 0),
    ] {
        let err = BlockParse::for_version(&version).expect_err(&version.to_string());
        assert!(
            matches!(err, UnvouchedVersion::NewerThanVouched { .. }),
            "{version}: {err:?}"
        );
    }
    let version = FreeswitchVersion::new(1, 8, 7);
    let err = BlockParse::for_version(&version).expect_err(&version.to_string());
    assert!(
        matches!(err, UnvouchedVersion::OlderThanVouched { .. }),
        "{version}: {err:?}"
    );
}

fn tilde() -> DialStringTarget {
    DialStringTarget::new(DialStringCarrier::EslApi)
        .with_argv_separator('~')
        .expect("'~' separates originate's arguments")
}

#[test]
fn an_argv_separator_is_part_of_the_target() {
    let blank = DialStringTarget::new(DialStringCarrier::EslApi);
    assert_eq!(blank.argv_separator(), None);
    assert_eq!(tilde().argv_separator(), Some('~'));
    assert_eq!(tilde().carrier(), DialStringCarrier::EslApi);
    assert_eq!(
        tilde()
            .with_block_parse(BlockParse::PairSplitCleans)
            .argv_separator(),
        Some('~')
    );
    assert_ne!(tilde(), blank);
}

#[test]
fn the_dialplan_carrier_takes_no_argv_separator() {
    assert_eq!(
        DialStringTarget::new(DialStringCarrier::Dialplan).with_argv_separator('~'),
        Err(InvalidArgvSeparator::WrongCarrier(
            DialStringCarrier::Dialplan
        ))
    );
}

/// Space, `\`, `'`, lowercase `n r t s`, controls and non-ASCII break the switch's split
/// or its escapes; the rest read as grammar, quoting, an escape letter or a word.
#[test]
fn unusable_argv_separators_are_refused() {
    for sep in [
        ' ', '\\', '\'', 'é', '\n', '\r', '\t', '\0', '\u{b}', 'n', 'r', 't', 's', 'N', 'R', 'T',
        'S', 'a', '0', '^', '"', ',', '|', '[', ']', '{', '}', '<', '>', '=', ':',
    ] {
        let err = DialStringTarget::new(DialStringCarrier::EslApi)
            .with_argv_separator(sep)
            .expect_err(&format!("accepted {sep:?}"));
        assert_eq!(err, InvalidArgvSeparator::Unusable(sep));
        assert!(!err
            .to_string()
            .is_empty());
    }
    for sep in ['~', ';', '!', '#'] {
        assert!(
            DialStringTarget::new(DialStringCarrier::EslApi)
                .with_argv_separator(sep)
                .is_ok(),
            "refused {sep:?}"
        );
    }
}

/// Inside the argument a value meets the same passes as at the blank split, so the
/// argument escape is the whole difference between the two renders.
#[test]
fn a_block_at_an_argv_separator_is_escaped_once_at_its_edge() {
    let cases = [
        (
            VariablesType::Default,
            "it's",
            r"{k=it\\\\\\\'s}".to_owned(),
        ),
        (
            VariablesType::Default,
            r"a\nb",
            r"{k=a\\\\\\\\nb}".to_owned(),
        ),
        (VariablesType::Default, "x~y", r"{k=x\~y}".to_owned()),
        (VariablesType::Default, "a,b", r"{k=a\\,b}".to_owned()),
        (VariablesType::Default, "a b", r"{k=\'a b\'}".to_owned()),
        (VariablesType::Channel, "a|b", r"[k=a\\|b]".to_owned()),
        (
            VariablesType::Channel,
            r"a\nb",
            format!("[k=a{}nb]", "\\".repeat(32)),
        ),
    ];
    for (scope, value, want) in cases {
        let mut vars = Variables::new(scope);
        vars.insert("k", value);
        assert_eq!(
            vars.display_for(tilde())
                .to_string(),
            want,
            "{value:?} in {scope:?}"
        );
    }
}

#[test]
fn a_block_cut_by_its_argv_separator_is_refused() {
    for block in ["{k=a}~{j=b}", "{k=a~j=b}", "{k='a~b'}"] {
        let err = Variables::parse_for(block, tilde()).expect_err(block);
        assert!(!err
            .to_string()
            .contains("k="));
    }
    assert_eq!(
        Variables::parse_for("{k=a}~", tilde())
            .expect("a trailing separator adds no argument")
            .get("k"),
        Some("a")
    );
}

#[test]
fn a_block_cut_by_the_blank_split_is_refused() {
    for block in ["{k=a b}", r"{k=x\\'y}", "{k=a} {j=b}"] {
        let err = Variables::parse_for(block, DialStringCarrier::EslApi).expect_err(block);
        assert!(!err
            .to_string()
            .contains("k="));
    }
    for (block, want) in [
        ("{k='a b'}", "a b"),
        (r"{k=\\\\sa\\\\s}", " a "),
        (" {k=v} ", "v"),
    ] {
        assert_eq!(
            Variables::parse_for(block, DialStringCarrier::EslApi)
                .unwrap_or_else(|e| panic!("{block}: {e}"))
                .get("k"),
            Some(want),
            "{block}"
        );
    }
    assert!(Variables::parse_for("{k=a b}", DialStringCarrier::Dialplan).is_ok());
}

/// `switch_ivr_originate` takes the enterprise path on any `:_:` in the dial string, and
/// that split honours no quote or escape.
#[test]
fn a_value_carrying_the_enterprise_separator_is_refused() {
    for block in ["{k=x:_:y}", "<k=x:_:y>", "[k=x:_:y]", "{k='a :_: b'}"] {
        let err = Variables::parse_for(block, DialStringCarrier::EslApi).expect_err(block);
        assert!(!err
            .to_string()
            .contains("x:_:y"));
    }
    assert!(serde_json::from_str::<Variables>(r#"{"k":"x:_:y"}"#).is_err());
}

/// The `=` split skips a byte after a backslash and its cleanup reads `\=`, and a pair
/// opening `^^` names that split's separator unless a quote pair the cleanup strips leads it.
#[test]
fn a_key_the_switch_splits_arrives_escaped_and_round_trips() {
    use crate::switch_passes::brackets::PairEffect;
    use crate::switch_passes::pipeline;

    let keys = [
        "a=b", "a,b", "a b", "^^ab", "^^", "x^^", " edge ", r"C:\p", "it's", "pa$$", "a|b",
        r"a\=b", r"end\",
    ];
    for target in [
        DialStringTarget::new(DialStringCarrier::EslApi),
        DialStringTarget::new(DialStringCarrier::Dialplan),
        tilde(),
    ] {
        for scope in [
            VariablesType::Default,
            VariablesType::Enterprise,
            VariablesType::Channel,
        ] {
            for key in keys {
                if scope == VariablesType::Channel && key.contains('\'') {
                    continue;
                }
                let mut vars = Variables::new(scope);
                vars.insert(key, "v");
                vars.insert("after", "sentinel");
                let rendered = vars
                    .display_for(target)
                    .to_string();
                let list = pipeline::read(&format!("{rendered}null/x"), target)
                    .unwrap_or_else(|e| panic!("{rendered:?} at {target:?}: {e:?}"));
                let installed: Vec<(String, PairEffect)> = list
                    .blocks
                    .iter()
                    .chain(&list.threads[0].blocks)
                    .chain(&list.threads[0].groups[0][0].blocks)
                    .flat_map(|block| &block.pairs)
                    .map(|pair| {
                        (
                            pair.key
                                .clone(),
                            pair.effect
                                .clone(),
                        )
                    })
                    .collect();
                assert_eq!(
                    installed,
                    [
                        (key.to_owned(), PairEffect::Set("v".to_owned())),
                        ("after".to_owned(), PairEffect::Set("sentinel".to_owned()))
                    ],
                    "{key:?} in {scope:?} at {target:?}: {rendered:?}"
                );
                let back = Variables::parse_for(&rendered, target)
                    .unwrap_or_else(|e| panic!("{rendered:?} at {target:?}: {e}"));
                assert_eq!(back, vars, "{rendered:?} at {target:?}");
            }
        }
    }
}

/// Each key carries `SECRET`, which no refusal may quote.
#[test]
fn a_key_no_escaping_delivers_is_refused_at_parse_and_config_load() {
    for block in [
        "{SECRET:_:x=v}",
        "{SECRET}=v}",
        "{=v,after=sentinel}",
        r"[SECRET\\\\\\'s=v]",
    ] {
        let msg = Variables::parse_for(block, DialStringCarrier::Dialplan)
            .expect_err(block)
            .to_string();
        assert!(!msg.contains("SECRET"), "{block}: {msg}");
    }
    for json in [
        r#"{"SECRET:_:x":"v"}"#,
        r#"{"SECRET}":"v"}"#,
        r#"{"":"v"}"#,
        r#"{"scope":"channel","vars":{"SECRET's":"v"}}"#,
        r#"{"scope":"channel","vars":{"SECRET]":"v"}}"#,
    ] {
        let msg = serde_json::from_str::<Variables>(json)
            .expect_err(json)
            .to_string();
        assert!(!msg.contains("SECRET"), "{json}: {msg}");
    }
    assert!(serde_json::from_str::<Variables>(r#"{"a=b, c":"v"}"#).is_ok());
}

/// `switch_event_base_add_header` reads a name carrying `[` as an array index and installs the
/// value under the text before it, in every scope.
#[test]
fn a_key_carrying_an_array_index_is_refused_at_parse_and_config_load() {
    for block in [
        "{SECRET[1]=v,after=sentinel}",
        "<SECRET[x=v>",
        "[SECRET[1]=v]",
        r"{^^;SECRET[0]=v;after=a,b}",
    ] {
        let msg = Variables::parse_for(block, DialStringCarrier::Dialplan)
            .expect_err(block)
            .to_string();
        assert!(!msg.contains("SECRET"), "{block}: {msg}");
    }
    for json in [
        r#"{"SECRET[1]":"v"}"#,
        r#"{"scope":"channel","vars":{"SECRET[0]":"v"}}"#,
    ] {
        let msg = serde_json::from_str::<Variables>(json)
            .expect_err(json)
            .to_string();
        assert!(!msg.contains("SECRET"), "{json}: {msg}");
    }
    assert!(serde_json::from_str::<Variables>(r#"{"SECRET]":"v"}"#).is_ok());
}

/// A block's variables land in one `EF_UNIQ_HEADERS` event, whose add deletes every header of
/// the name by `strcasecmp`, so two names differing only in case install as one.
#[test]
fn two_keys_differing_only_in_case_are_refused_at_parse_and_config_load() {
    for block in [
        "{Secret=1,SECRET=2}",
        "<secret=1,after=a,SECRET=2>",
        "[SeCrEt=1,secret=2]",
        "{^^;secret=1;SECRET=a,b}",
    ] {
        let msg = Variables::parse_for(block, DialStringCarrier::EslApi)
            .expect_err(block)
            .to_string();
        assert!(
            !msg.to_ascii_lowercase()
                .contains("secret"),
            "{block}: {msg}"
        );
    }
    for json in [
        r#"{"Secret":"1","SECRET":"2"}"#,
        r#"{"scope":"enterprise","vars":{"secret":"1","Secret":"2"}}"#,
    ] {
        let msg = serde_json::from_str::<Variables>(json)
            .expect_err(json)
            .to_string();
        assert!(
            !msg.to_ascii_lowercase()
                .contains("secret"),
            "{json}: {msg}"
        );
    }
    let same = Variables::parse_for("{k=1,k=2}", DialStringCarrier::EslApi)
        .expect("a repeated name is the last one written, on the switch and here");
    assert_eq!(same.get("k"), Some("2"));
}

/// Every split ahead of the `=` split reads `\\` as one backslash and leaves `\b` alone, and in
/// `[]` the `,` leg split reads `\,` as a comma whatever the block's own separator.
#[test]
fn parse_reads_a_value_as_the_switch_installs_it() {
    for (block, target, want) in [
        (
            r"{k=a\\\\\\b}",
            DialStringTarget::new(DialStringCarrier::EslApi),
            r"a\b",
        ),
        (
            r"{k=a\\\\b}",
            DialStringTarget::new(DialStringCarrier::Dialplan),
            r"a\b",
        ),
        (
            r"[^^:k=val\,ue:other=x]",
            DialStringTarget::new(DialStringCarrier::EslApi),
            "val,ue",
        ),
    ] {
        let parsed = Variables::parse_for(block, target).unwrap_or_else(|e| panic!("{block}: {e}"));
        assert_eq!(parsed.get("k"), Some(want), "{block} at {target:?}");
    }
}

/// A block `parse_for` accepts reads back the pairs the port of the switch's passes installs
/// from it, whatever wrote the block.
#[test]
fn parse_agrees_with_the_switch_port() {
    use crate::switch_passes::brackets::PairEffect;
    use crate::switch_passes::pipeline;

    let blocks = [
        r"{k=a\\\\\\b}",
        r"{k=a\\\\b}",
        r"{k=a\b,j=c\\d}",
        "<k='a b'>",
        r"{k=\'x\'}",
        r"[k=a\,b]",
        r"[^^:k=val\,ue:other=x]",
        r"{^^;k=a,b;j=\\\\s}",
        r"{k=\\\\sx\\\\s}",
        r"[k=a\\\\\\\\\\\\\\\\b]",
        r"{k=it\\\\\\\'s}",
        r"{k=it\\\\\\'s}",
        "{k=v}",
        r"{k\\=x=v}",
    ];
    let targets = [
        DialStringTarget::new(DialStringCarrier::EslApi),
        DialStringTarget::new(DialStringCarrier::Dialplan),
        tilde(),
    ];
    for target in targets {
        for block in blocks {
            let Ok(parsed) = Variables::parse_for(block, target) else {
                continue;
            };
            let list = pipeline::read(&format!("{block}null/x"), target)
                .unwrap_or_else(|e| panic!("{block:?} at {target:?}: {e:?}"));
            let installed: Vec<(&str, &str)> = list
                .threads
                .iter()
                .flat_map(|thread| {
                    thread
                        .blocks
                        .iter()
                        .chain(
                            thread
                                .groups
                                .iter()
                                .flatten()
                                .flat_map(|leg| &leg.blocks),
                        )
                })
                .flat_map(|block| &block.pairs)
                .filter_map(|pair| match &pair.effect {
                    PairEffect::Set(value) => Some((
                        pair.key
                            .as_str(),
                        value.as_str(),
                    )),
                    PairEffect::Ignored
                    | PairEffect::Cleared
                    | PairEffect::Unreadable
                    | PairEffect::Valueless => None,
                })
                .collect();
            assert_eq!(
                parsed
                    .iter()
                    .collect::<Vec<_>>(),
                installed,
                "{block:?} at {target:?}"
            );
        }
    }
}

/// The caller has to decide what to name instead, so the refusal says what
/// would have been accepted.
#[test]
fn an_unvouched_version_names_the_vouched_range() {
    let msg = BlockParse::for_version(&FreeswitchVersion::new(1, 11, 4))
        .unwrap_err()
        .to_string();
    assert!(
        msg.contains("1.10.0") && msg.contains("1.11.3"),
        "does not name the vouched range: {msg}"
    );
}
