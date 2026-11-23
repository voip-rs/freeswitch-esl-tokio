use super::*;
use crate::commands::originate::{Application, Originate};
use crate::commands::variables::{BlockParse, VariablesType};

/// `switch_find_end_paren` counts depth, so a balanced bracket in a value leaves the block
/// closing at its own bracket.
#[test]
fn a_leading_block_closes_at_its_matching_bracket() {
    for (input, module_text) in [
        ("<sip_h_Call-Info=<url>>sofia/gw/x", "sofia/gw/x"),
        ("{a={b}}sofia/internal/1000", "sofia/internal/1000"),
    ] {
        let ep = Endpoint::parse_for(input, DialStringCarrier::EslApi.measured())
            .unwrap_or_else(|e| panic!("{input}: {e}"));
        assert!(
            ep.variables()
                .is_some(),
            "{input}"
        );
        assert_eq!(ep.module_text(), module_text, "{input}");
    }
    assert!(Endpoint::parse_for("{a=b", DialStringCarrier::EslApi.measured()).is_err());
}

// --- Endpoint::parse_for dispatch ---

#[test]
fn endpoint_from_str_sofia() {
    let ep = Endpoint::parse_for(
        "sofia/internal/1000@example.com",
        DialStringCarrier::EslApi.measured(),
    )
    .unwrap();
    assert!(matches!(ep, Endpoint::Sofia(_)));
}

#[test]
fn endpoint_from_str_sofia_gateway() {
    let ep = Endpoint::parse_for(
        "sofia/gateway/my_gw/1234",
        DialStringCarrier::EslApi.measured(),
    )
    .unwrap();
    assert!(matches!(ep, Endpoint::SofiaGateway(_)));
}

#[test]
fn endpoint_from_str_loopback() {
    let ep =
        Endpoint::parse_for("loopback/9199/test", DialStringCarrier::EslApi.measured()).unwrap();
    assert!(matches!(ep, Endpoint::Loopback(_)));
}

#[test]
fn endpoint_from_str_user() {
    let ep = Endpoint::parse_for(
        "user/1000@example.com",
        DialStringCarrier::EslApi.measured(),
    )
    .unwrap();
    assert!(matches!(ep, Endpoint::User(_)));
}

#[test]
fn endpoint_parse_for_dialplan_sofia_contact() {
    let ep = Endpoint::parse_for(
        "${sofia_contact(1000@example.com)}",
        DialStringCarrier::Dialplan.measured(),
    )
    .unwrap();
    assert!(matches!(ep, Endpoint::SofiaContact(_)));
}

#[test]
fn endpoint_parse_for_dialplan_group_call() {
    let ep = Endpoint::parse_for(
        "${group_call(support@example.com+A)}",
        DialStringCarrier::Dialplan.measured(),
    )
    .unwrap();
    assert!(matches!(ep, Endpoint::GroupCall(_)));
}

/// Neither `originate` over the API nor a script's originate expands, so an expression there dials
/// as an unknown endpoint.
#[test]
fn an_expression_is_refused_where_nothing_expands_it() {
    let unexpanded = |kind| OriginateError::UnexpandedExpression { endpoint: kind };
    for (text, kind) in [
        ("${sofia_contact(SECRET@example.com)}", "sofia_contact"),
        ("${group_call(SECRET@example.com+A)}", "group_call"),
    ] {
        for target in [
            DialStringTarget::new(DialStringCarrier::EslApi, BlockParse::MEASURED),
            tilde(),
            DialStringTarget::new(DialStringCarrier::Direct, BlockParse::MEASURED),
        ] {
            let refused = Endpoint::parse_for(text, target);
            assert_eq!(refused, Err(unexpanded(kind)), "{text} at {target:?}");
        }
        assert_eq!(
            Endpoint::parse_for(text, DialStringCarrier::EslApi.measured()),
            Err(unexpanded(kind)),
            "{text}"
        );
        let msg = unexpanded(kind).to_string();
        assert!(msg.contains(kind) && !msg.contains("SECRET"), "{msg}");
        let line = format!("originate {text} &park()");
        assert_eq!(
            Originate::parse_for(&line, BlockParse::MEASURED),
            Err(unexpanded(kind)),
            "{line}"
        );
    }
    let config = r#"{"endpoint":{"group_call":{"group":"support","domain":"example.com"}},"application":{"name":"park"}}"#;
    let refused = serde_json::from_str::<Originate>(config)
        .expect_err("an Originate config dials over the API")
        .to_string();
    assert!(refused.contains("group_call"), "{refused}");
    assert_eq!(
        Originate::application(
            GroupCall::new("support", "example.com")
                .unwrap()
                .into(),
            Application::simple("park"),
        ),
        Err(unexpanded("group_call"))
    );
}

#[test]
fn endpoint_from_str_error() {
    let ep = Endpoint::parse_for("error/USER_BUSY", DialStringCarrier::EslApi.measured()).unwrap();
    assert!(matches!(ep, Endpoint::Error(_)));
}

/// `ErrorEndpoint` has nowhere to keep a block, so accepting one loses
/// every variable it named without a word to the caller.
#[test]
fn a_block_on_an_endpoint_that_cannot_hold_one_is_refused() {
    for carrier in [DialStringCarrier::EslApi, DialStringCarrier::Dialplan] {
        let msg = Endpoint::parse_for("{a=b}error/USER_BUSY", carrier.measured())
            .expect_err(&format!("accepted at {carrier:?}"))
            .to_string();
        assert!(msg.contains("error"), "does not name the type: {msg}");
    }
    assert!(
        Endpoint::parse_for("{a=b}error/USER_BUSY", DialStringCarrier::EslApi.measured()).is_err()
    );
}

/// The two entry points have to agree: `from_str` is `parse_for` at the
/// default carrier, not a second dispatch with its own rules.
#[test]
fn from_str_matches_parse_for_at_the_default_carrier() {
    for input in [
        "sofia/internal/1000@example.com",
        "{a=b}sofia/internal/1000@example.com",
        "<a=b>loopback/9199/default",
        "[a=b]user/bob@example.com",
        "{a=b}error/USER_BUSY",
        "verto/1234",
    ] {
        assert_eq!(
            Endpoint::parse_for(input, DialStringCarrier::EslApi.measured()).is_ok(),
            Endpoint::parse_for(input, DialStringCarrier::EslApi.measured()).is_ok(),
            "{input}"
        );
    }
}

/// A target naming only a carrier is that carrier: both spellings render and
/// parse identically.
#[test]
fn a_target_and_its_bare_carrier_agree() {
    use crate::commands::variables::{BlockParse, DialStringTarget};

    let input = r"{a=it\\\\\\'s}sofia/internal/1000@example.com";
    let target = DialStringTarget::new(DialStringCarrier::Dialplan, BlockParse::MEASURED)
        .with_block_parse(BlockParse::PairSplitCleans);
    let by_target = Endpoint::parse_for(input, target).unwrap();
    let by_carrier = Endpoint::parse_for(input, DialStringCarrier::Dialplan.measured()).unwrap();
    assert_eq!(by_target, by_carrier);
    assert_eq!(
        by_target
            .display_for(target)
            .to_string(),
        input
    );
}

/// The leg split and the block parse skip and trim spaces only, so any other whitespace
/// the switch keeps is endpoint text.
#[test]
fn whitespace_other_than_a_space_is_kept() {
    let ep =
        Endpoint::parse_for("<v0==>loopback/\u{b}", DialStringCarrier::EslApi.measured()).unwrap();
    let Endpoint::Loopback(loopback) = &ep else {
        panic!("expected Loopback: {ep:?}");
    };
    assert_eq!(loopback.extension(), "\u{b}");

    assert!(Variables::parse_for("\u{b}{k=v}", DialStringCarrier::Dialplan.measured()).is_err());

    let bridge = crate::commands::BridgeDialString::parse_for(
        "loopback/9199/test\t",
        crate::commands::BlockParse::PairSplitCleans,
    )
    .unwrap();
    assert_eq!(bridge.render(BlockParse::MEASURED), "loopback/9199/test\t");
}

#[test]
fn endpoint_from_str_unknown_errors() {
    let result = Endpoint::parse_for("verto/1234", DialStringCarrier::EslApi.measured());
    assert!(result.is_err());
}

#[test]
fn endpoint_from_str_with_variables() {
    let ep = Endpoint::parse_for(
        "{timeout=30}sofia/internal/1000@example.com",
        DialStringCarrier::EslApi.measured(),
    )
    .unwrap();
    if let Endpoint::Sofia(inner) = &ep {
        assert_eq!(inner.profile(), "internal");
        assert!(inner
            .variables()
            .is_some());
    } else {
        panic!("expected Sofia variant");
    }
}

// --- Display delegation ---

#[test]
fn endpoint_display_delegates_to_inner() {
    let ep = Endpoint::Sofia(SofiaEndpoint::new("internal", "1000@example.com").unwrap());
    assert_eq!(
        ep.render(DialStringCarrier::EslApi.measured()),
        "sofia/internal/1000@example.com"
    );
}

// --- DialString trait ---

#[test]
fn dial_string_variables_returns_some() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("k", "v")
        .unwrap();
    let ep = SofiaEndpoint::new("internal", "1000")
        .unwrap()
        .with_variables(vars);
    assert!(ep
        .variables()
        .is_some());
    assert_eq!(
        ep.variables()
            .unwrap()
            .get("k"),
        Some("v")
    );
}

#[test]
fn dial_string_variables_returns_none() {
    let ep = SofiaEndpoint::new("internal", "1000").unwrap();
    assert!(ep
        .variables()
        .is_none());
}

#[test]
fn dial_string_set_variables() {
    let mut ep = SofiaEndpoint::new("internal", "1000").unwrap();
    let mut vars = Variables::new(VariablesType::Channel);
    vars.insert("k", "v")
        .unwrap();
    ep.set_variables(Some(vars))
        .unwrap();
    assert!(ep
        .variables()
        .is_some());
}

#[test]
fn dial_string_error_endpoint_no_variables() {
    let ep = ErrorEndpoint::new(crate::channel::HangupCause::UserBusy);
    assert!(ep
        .variables()
        .is_none());
}

/// `error/` renders no block, so a block handed to it would vanish from the wire.
#[test]
fn error_endpoint_refuses_a_variable_block() {
    let mut vars = Variables::new(VariablesType::Channel);
    vars.insert("k", "v")
        .unwrap();
    let mut ep = ErrorEndpoint::new(crate::channel::HangupCause::UserBusy);
    assert_eq!(
        ep.set_variables(Some(vars.clone())),
        Err(OriginateError::VariablesNotSupported("error"))
    );
    assert_eq!(
        Endpoint::from(ep).with_variables(vars),
        Err(OriginateError::VariablesNotSupported("error"))
    );
    assert_eq!(ep.set_variables(None), Ok(()));
}

/// Parse hands the block to the endpoint, which refuses it on its own.
#[test]
fn parse_refuses_a_block_on_an_error_endpoint() {
    assert_eq!(
        Endpoint::parse_for("{k=v}error/USER_BUSY", DialStringCarrier::EslApi.measured()),
        Err(OriginateError::VariablesNotSupported("error"))
    );
    assert_eq!(
        crate::commands::BridgeDialString::parse_for(
            "[k=v]error/USER_BUSY",
            crate::commands::variables::BlockParse::MEASURED
        ),
        Err(OriginateError::VariablesNotSupported("error"))
    );
}

#[test]
fn dial_string_on_endpoint_enum() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("k", "v")
        .unwrap();
    let ep = Endpoint::Sofia(
        SofiaEndpoint::new("internal", "1000")
            .unwrap()
            .with_variables(vars),
    );
    assert!(ep
        .variables()
        .is_some());
}

// --- Serde: Endpoint enum ---

/// One case per variant: the externally tagged name is the config key a
/// deployment writes, so a rename is a break and has to show up here.
#[test]
fn serde_endpoint_enum_tags_round_trip() {
    let cases: [(&str, Endpoint); 7] = [
        (
            "sofia",
            SofiaEndpoint::new("internal", "1000@example.com")
                .unwrap()
                .into(),
        ),
        (
            "sofia_gateway",
            SofiaGateway::new("gw1", "1234")
                .unwrap()
                .into(),
        ),
        (
            "loopback",
            LoopbackEndpoint::new("9199")
                .unwrap()
                .with_context("default")
                .unwrap()
                .into(),
        ),
        (
            "user",
            UserEndpoint::new("bob")
                .unwrap()
                .with_domain("example.com")
                .unwrap()
                .into(),
        ),
        (
            "sofia_contact",
            SofiaContact::new("1000", "example.com")
                .unwrap()
                .into(),
        ),
        (
            "group_call",
            GroupCall::new("support", "example.com")
                .unwrap()
                .with_order(GroupCallOrder::All)
                .into(),
        ),
        (
            "error",
            ErrorEndpoint::new(crate::channel::HangupCause::UserBusy).into(),
        ),
    ];
    for (tag, ep) in cases {
        let json = serde_json::to_string(&ep).unwrap();
        assert!(json.contains(&format!("\"{tag}\"")), "{tag}: {json}");
        assert_eq!(serde_json::from_str::<Endpoint>(&json).unwrap(), ep);
    }
}

#[test]
fn serde_endpoint_skips_none_variables() {
    let ep = SofiaEndpoint::new("internal", "1000").unwrap();
    let json = serde_json::to_string(&ep).unwrap();
    assert!(!json.contains("variables"));
}

#[test]
fn serde_endpoint_skips_none_profile() {
    let ep = SofiaGateway::new("gw", "1234").unwrap();
    let json = serde_json::to_string(&ep).unwrap();
    assert!(!json.contains("profile"));
}

// --- Audio endpoints through Endpoint enum ---

/// The three audio modules share one struct and differ only in the prefix
/// their variant supplies, so each row has to name its own module.
#[test]
fn audio_endpoints_render_and_parse_per_module() {
    type Variant = fn(AudioEndpoint) -> Endpoint;
    let cases: [(Variant, &str, &str); 6] = [
        (Endpoint::PortAudio, "portaudio", "portaudio/auto_answer"),
        (Endpoint::PortAudio, "portaudio", "portaudio"),
        (Endpoint::PulseAudio, "pulseaudio", "pulseaudio/auto_answer"),
        (Endpoint::PulseAudio, "pulseaudio", "pulseaudio"),
        (Endpoint::Alsa, "alsa", "alsa/auto_answer"),
        (Endpoint::Alsa, "alsa", "alsa"),
    ];
    for (variant, module, wire) in cases {
        let destination = wire
            .strip_prefix(module)
            .and_then(|rest| rest.strip_prefix('/'))
            .map(str::to_string);
        let ep = variant(match destination {
            Some(destination) => AudioEndpoint::new()
                .with_destination(destination)
                .unwrap(),
            None => AudioEndpoint::new(),
        });
        assert_eq!(ep.render(DialStringCarrier::EslApi.measured()), wire);

        let parsed = Endpoint::parse_for(wire, DialStringCarrier::EslApi.measured()).unwrap();
        assert_eq!(parsed, ep, "{wire}");
        assert!(
            parsed
                .variables()
                .is_none(),
            "{wire}"
        );

        let json = serde_json::to_string(&ep).unwrap();
        assert!(json.contains(&format!("\"{module}\"")), "{wire}: {json}");
        assert_eq!(serde_json::from_str::<Endpoint>(&json).unwrap(), ep);
    }
}

#[test]
fn audio_endpoint_carries_variables() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("codec", "PCMU")
        .unwrap();
    let ep = Endpoint::PortAudio(
        AudioEndpoint::new()
            .with_destination("auto_answer")
            .unwrap()
            .with_variables(vars),
    );
    assert_eq!(
        ep.render(DialStringCarrier::EslApi.measured()),
        "{codec=PCMU}portaudio/auto_answer"
    );
    assert_eq!(
        Endpoint::parse_for(
            &ep.render(DialStringCarrier::EslApi.measured()),
            DialStringCarrier::EslApi.measured()
        )
        .unwrap(),
        ep
    );
}

/// A renderer and a parser that disagree on the carrier hand back a
/// different value than was put in — silently, for every endpoint type.
#[test]
fn every_endpoint_type_round_trips_at_the_dialplan_carrier() {
    let mut vars = Variables::new(VariablesType::Channel);
    vars.insert("path", r"C:\path")
        .unwrap();
    vars.insert("other", "a,b")
        .unwrap();

    let with_vars = |mut ep: Endpoint| {
        ep.set_variables(Some(vars.clone()))
            .unwrap();
        ep
    };
    let cases: [Endpoint; 10] = [
        with_vars(
            SofiaEndpoint::new("internal", "1000@example.com")
                .unwrap()
                .into(),
        ),
        with_vars(
            SofiaGateway::new("gw", "1234")
                .unwrap()
                .with_profile("external")
                .unwrap()
                .into(),
        ),
        with_vars(
            LoopbackEndpoint::new("9199")
                .unwrap()
                .with_context("default")
                .unwrap()
                .into(),
        ),
        with_vars(
            UserEndpoint::new("bob")
                .unwrap()
                .with_domain("example.com")
                .unwrap()
                .into(),
        ),
        with_vars(
            SofiaContact::new("1000", "example.com")
                .unwrap()
                .with_profile("*")
                .unwrap()
                .into(),
        ),
        with_vars(
            GroupCall::new("support", "example.com")
                .unwrap()
                .with_order(GroupCallOrder::All)
                .into(),
        ),
        ErrorEndpoint::new(crate::channel::HangupCause::UserBusy).into(),
        with_vars(Endpoint::PortAudio(
            AudioEndpoint::new()
                .with_destination("auto_answer")
                .unwrap(),
        )),
        with_vars(Endpoint::PulseAudio(AudioEndpoint::new())),
        with_vars(Endpoint::Alsa(
            AudioEndpoint::new()
                .with_destination("auto_answer")
                .unwrap(),
        )),
    ];

    for ep in cases {
        let rendered = ep.render(DialStringCarrier::Dialplan.measured());
        let back = Endpoint::parse_for(&rendered, DialStringCarrier::Dialplan.measured())
            .unwrap_or_else(|e| panic!("{rendered} failed to parse: {e}"));
        assert_eq!(back, ep, "rendered {rendered}");
    }
}

#[test]
fn every_endpoint_type_round_trips_at_an_argv_separator() {
    let target = DialStringTarget::new(DialStringCarrier::EslApi, BlockParse::MEASURED)
        .with_argv_separator('~')
        .expect("'~' separates originate's arguments");
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("path", r"C:\path")
        .unwrap();
    vars.insert("tilde", "x~y")
        .unwrap();
    vars.insert("spaced", "a b")
        .unwrap();
    vars.insert("quote", "it's")
        .unwrap();

    let with_vars = |mut ep: Endpoint| {
        ep.set_variables(Some(vars.clone()))
            .unwrap();
        ep
    };
    let cases: [Endpoint; 8] = [
        with_vars(
            SofiaEndpoint::new("internal", "1000@example.com")
                .unwrap()
                .into(),
        ),
        with_vars(
            SofiaGateway::new("gw", "1234")
                .unwrap()
                .with_profile("external")
                .unwrap()
                .into(),
        ),
        with_vars(
            LoopbackEndpoint::new("9199")
                .unwrap()
                .with_context("default")
                .unwrap()
                .into(),
        ),
        with_vars(
            UserEndpoint::new("bob")
                .unwrap()
                .with_domain("example.com")
                .unwrap()
                .into(),
        ),
        with_vars(
            SofiaContact::new("1000", "example.com")
                .unwrap()
                .into(),
        ),
        with_vars(
            GroupCall::new("support", "example.com")
                .unwrap()
                .into(),
        ),
        ErrorEndpoint::new(crate::channel::HangupCause::UserBusy).into(),
        with_vars(Endpoint::Alsa(
            AudioEndpoint::new()
                .with_destination("auto_answer")
                .unwrap(),
        )),
    ];

    for ep in cases {
        let rendered = ep
            .display_for(target)
            .to_string();
        let back = Endpoint::parse_for(&rendered, target);
        if matches!(ep, Endpoint::SofiaContact(_) | Endpoint::GroupCall(_)) {
            assert!(
                matches!(back, Err(OriginateError::UnexpandedExpression { .. })),
                "rendered {rendered}: {back:?}"
            );
            continue;
        }
        let back = back.unwrap_or_else(|e| panic!("{rendered} failed to parse: {e}"));
        assert_eq!(back, ep, "rendered {rendered}");
    }
}

#[test]
fn an_endpoint_cut_by_its_argv_separator_is_refused() {
    let target = DialStringTarget::new(DialStringCarrier::EslApi, BlockParse::MEASURED)
        .with_argv_separator('~')
        .expect("'~' separates originate's arguments");
    assert!(Endpoint::parse_for("loopback/9199/test~error/USER_BUSY", target).is_err());
    assert!(Endpoint::parse_for(r"{k=x\~y}loopback/9199/test", target).is_ok());
}

#[test]
fn an_endpoint_cut_by_the_blank_split_is_refused() {
    for input in [
        "{v=a b}loopback/9199/test",
        "loopback/9199/test error/USER_BUSY",
        r"{v=x\\'y}loopback/9199/test",
    ] {
        assert!(
            Endpoint::parse_for(input, DialStringCarrier::EslApi.measured()).is_err(),
            "{input}"
        );
    }
    assert!(Endpoint::parse_for(
        "{v='a b'}loopback/9199/test",
        DialStringCarrier::EslApi.measured()
    )
    .is_ok());
    assert!(Endpoint::parse_for(
        "{v=a b}loopback/9199/test",
        DialStringCarrier::Dialplan.measured()
    )
    .is_ok());
}

// --- From impls ---

#[test]
fn from_sofia_endpoint() {
    let inner = SofiaEndpoint::new("internal", "1000@example.com").unwrap();
    let ep: Endpoint = inner
        .clone()
        .into();
    assert_eq!(ep, Endpoint::Sofia(inner));
}

#[test]
fn from_sofia_gateway() {
    let inner = SofiaGateway::new("gw1", "1234").unwrap();
    let ep: Endpoint = inner
        .clone()
        .into();
    assert_eq!(ep, Endpoint::SofiaGateway(inner));
}

#[test]
fn from_loopback_endpoint() {
    let inner = LoopbackEndpoint::new("9199")
        .unwrap()
        .with_context("default")
        .unwrap();
    let ep: Endpoint = inner
        .clone()
        .into();
    assert_eq!(ep, Endpoint::Loopback(inner));
}

#[test]
fn from_user_endpoint() {
    let inner = UserEndpoint::new("bob")
        .unwrap()
        .with_domain("example.com")
        .unwrap();
    let ep: Endpoint = inner
        .clone()
        .into();
    assert_eq!(ep, Endpoint::User(inner));
}

#[test]
fn from_sofia_contact() {
    let inner = SofiaContact::new("1000", "example.com").unwrap();
    let ep: Endpoint = inner
        .clone()
        .into();
    assert_eq!(ep, Endpoint::SofiaContact(inner));
}

#[test]
fn from_group_call() {
    let inner = GroupCall::new("support", "example.com")
        .unwrap()
        .with_order(GroupCallOrder::All);
    let ep: Endpoint = inner
        .clone()
        .into();
    assert_eq!(ep, Endpoint::GroupCall(inner));
}

#[test]
fn from_error_endpoint() {
    let inner = ErrorEndpoint::new(crate::channel::HangupCause::UserBusy);
    let ep: Endpoint = inner.into();
    assert_eq!(ep, Endpoint::Error(inner));
}

// --- Endpoint text through the leg splits ---

fn tilde() -> DialStringTarget {
    DialStringTarget::new(DialStringCarrier::EslApi, BlockParse::MEASURED)
        .with_argv_separator('~')
        .expect("'~' separates originate's arguments")
}

/// Endpoint text meets the carrier's pass and both leg splits, so it escapes like a `{}`
/// value with the leg separators added.
#[test]
fn endpoint_text_is_escaped_for_the_leg_splits() {
    let api = DialStringTarget::new(DialStringCarrier::EslApi, BlockParse::MEASURED);
    let dialplan = DialStringTarget::new(DialStringCarrier::Dialplan, BlockParse::MEASURED);
    let separators: Endpoint = LoopbackEndpoint::new("a,b")
        .unwrap()
        .with_context("c|d")
        .unwrap()
        .into();
    let cases: [(DialStringTarget, Endpoint, &str); 10] = [
        (api, separators.clone(), r"loopback/a\,b/c\|d"),
        (dialplan, separators, r"loopback/a\,b/c\|d"),
        (
            api,
            LoopbackEndpoint::new(r"C:\x")
                .unwrap()
                .into(),
            r"loopback/C:\\\\\\\\x",
        ),
        (
            api,
            LoopbackEndpoint::new("it's")
                .unwrap()
                .into(),
            r"loopback/it\\\\\\\'s",
        ),
        (
            dialplan,
            LoopbackEndpoint::new("it's")
                .unwrap()
                .into(),
            r"loopback/it\\\\\\'s",
        ),
        (
            api,
            SofiaEndpoint::new("internal", "a b")
                .unwrap()
                .into(),
            "'sofia/internal/a b'",
        ),
        (
            api,
            UserEndpoint::new("bob")
                .unwrap()
                .with_domain("end ")
                .unwrap()
                .into(),
            r"user/bob@end\\\\s",
        ),
        (
            dialplan,
            SofiaEndpoint::new("internal", "pa$$")
                .unwrap()
                .into(),
            r"\'sofia/internal/pa\$\$",
        ),
        (
            dialplan,
            SofiaEndpoint::new("internal", "${v}")
                .unwrap()
                .into(),
            "sofia/internal/${v}",
        ),
        (
            dialplan,
            LoopbackEndpoint::new("pa$$${v}")
                .unwrap()
                .into(),
            "loopback/pa$$${v}",
        ),
    ];
    for (target, ep, want) in cases {
        assert_eq!(
            ep.display_for(target)
                .to_string(),
            want,
            "{ep:?} at {target:?}"
        );
    }
}

const HOSTILE_FIELDS: &[&str] = &[
    "a b", "it's", r"C:\p", "x,y", "p|q", " edge ", "pa$$", "${v}", "x~y", "q\"r", "[b]", "tab\t",
    r"a\,b",
];

fn hostile_endpoints(field: &str) -> [Endpoint; 5] {
    [
        SofiaEndpoint::new("internal", field)
            .unwrap()
            .into(),
        SofiaGateway::new("gw", field)
            .unwrap()
            .with_profile("external")
            .unwrap()
            .into(),
        LoopbackEndpoint::new(field)
            .unwrap()
            .with_context(field)
            .unwrap()
            .into(),
        UserEndpoint::new(field)
            .unwrap()
            .with_domain(field)
            .unwrap()
            .into(),
        Endpoint::Alsa(
            AudioEndpoint::new()
                .with_destination(field)
                .unwrap(),
        ),
    ]
}

/// The port of the switch's passes reads back the module text, and the parser the endpoint.
#[test]
fn hostile_fields_arrive_and_round_trip_at_every_target() {
    use crate::switch_passes::pipeline;

    let targets = [
        DialStringTarget::new(DialStringCarrier::EslApi, BlockParse::MEASURED),
        DialStringTarget::new(DialStringCarrier::Dialplan, BlockParse::MEASURED),
        tilde(),
    ];
    for field in HOSTILE_FIELDS {
        for ep in hostile_endpoints(field) {
            for target in targets {
                let rendered = ep
                    .display_for(target)
                    .to_string();
                let list = pipeline::read(&rendered, target)
                    .unwrap_or_else(|e| panic!("{rendered:?} at {target:?}: {e:?}"));
                let legs: Vec<&str> = list
                    .threads
                    .iter()
                    .flat_map(|thread| &thread.groups)
                    .flatten()
                    .map(|leg| {
                        leg.endpoint
                            .as_str()
                    })
                    .collect();
                assert_eq!(legs, [ep.module_text()], "{rendered:?} at {target:?}");
                assert_eq!(
                    Endpoint::parse_for(&rendered, target)
                        .unwrap_or_else(|e| panic!("{rendered:?} at {target:?}: {e}")),
                    ep,
                    "{rendered:?} at {target:?}"
                );
            }
        }
    }
}

/// Each value carries `SECRET`, which no refusal may quote.
#[test]
fn fields_the_switch_cannot_receive_are_refused_at_parse() {
    for input in [
        "sofia/GATEWAY/SECRET/1000",
        "sofia/SECRET^x/1000",
        "sofia/gateway/SECRET^x/1000",
        "loopback/SECRET//xml",
        "loopback/SECRET/test/",
        "loopback/SECRET:_:x/test",
        "user/SECRET:_:x@example.com",
    ] {
        let msg = Endpoint::parse_for(input, DialStringCarrier::Dialplan.measured())
            .expect_err(input)
            .to_string();
        assert!(
            !msg.contains("SECRET"),
            "{input}: error quoted its input: {msg}"
        );
    }
}

#[test]
fn fields_the_switch_cannot_receive_are_refused_at_config_load() {
    for json in [
        r#"{"sofia":{"profile":"SECRET/x","destination":"1"}}"#,
        r#"{"sofia":{"profile":"GateWay","destination":"SECRET"}}"#,
        r#"{"sofia":{"profile":"SECRET^x","destination":"1"}}"#,
        r#"{"sofia":{"profile":"internal","destination":"SECRET:_:x"}}"#,
        r#"{"sofia_gateway":{"gateway":"SECRET::x","destination":"1"}}"#,
        r#"{"sofia_gateway":{"gateway":"g","profile":"SECRET::x","destination":"1"}}"#,
        r#"{"sofia_gateway":{"gateway":"g","profile":"SECRET:","destination":"1"}}"#,
        r#"{"sofia_gateway":{"gateway":"SECRET/x","destination":"1"}}"#,
        r#"{"sofia_gateway":{"gateway":"SECRET^x","destination":"1"}}"#,
        r#"{"loopback":{"extension":"SECRET/x"}}"#,
        r#"{"loopback":{"extension":"SECRET","context":""}}"#,
        r#"{"loopback":{"extension":"9199","context":"SECRET/x"}}"#,
        r#"{"loopback":{"extension":"SECRET","dialplan":""}}"#,
        r#"{"loopback":{"extension":"app=bridge:SECRET","context":"test"}}"#,
        r#"{"loopback":{"extension":"APP=SECRET/x:y"}}"#,
        r#"{"user":{"name":"SECRET@x","domain":"example.com"}}"#,
        r#"{"user":{"name":"bob","domain":"SECRET:_:x"}}"#,
        r#"{"portaudio":{"destination":"SECRET:_:x"}}"#,
    ] {
        let msg = serde_json::from_str::<Endpoint>(json)
            .expect_err(json)
            .to_string();
        assert!(
            !msg.contains("SECRET"),
            "{json}: error quoted its input: {msg}"
        );
    }
    for json in [
        r#"{"loopback":{"extension":"app=bridge:null/farend"}}"#,
        r#"{"sofia":{"profile":"internal","destination":"sip:a/b^c@example.com"}}"#,
        r#"{"sofia":{"profile":"a::b","destination":"1"}}"#,
        r#"{"sofia_gateway":{"gateway":":g","profile":"p","destination":"1"}}"#,
        r#"{"sofia_gateway":{"gateway":"g::h","profile":"p","destination":"1"}}"#,
        r#"{"loopback":{"extension":"9199","dialplan":"a/b"}}"#,
        r#"{"user":{"name":"bob","domain":"a@b"}}"#,
    ] {
        assert!(serde_json::from_str::<Endpoint>(json).is_ok(), "{json}");
    }
    assert!(
        serde_json::from_str::<LoopbackEndpoint>(r#"{"extension":"SECRET","context":""}"#).is_err()
    );
}

/// mod_loopback runs `app=<name>[:<args>]` and reads no context or dialplan after it, so the
/// whole text is the extension.
#[test]
fn a_loopback_application_is_one_extension() {
    let ep = LoopbackEndpoint::parse_for(
        "loopback/app=bridge:null/farend",
        DialStringCarrier::EslApi.measured(),
    )
    .unwrap();
    assert_eq!(ep.extension(), "app=bridge:null/farend");
    assert_eq!(ep.context(), None);
    assert_eq!(
        ep.render(DialStringCarrier::EslApi.measured()),
        "loopback/app=bridge:null/farend"
    );
}

/// `sofia_contact_function` cuts at the first `~`, then `/`, then `@`, then a `/` after the
/// domain, and `group_call_function` at the first `+`, then `@`; the argument split, the leg
/// splits and the reference parse read the rest ahead of either. Each value carries `SECRET`.
#[test]
fn expression_and_audio_fields_the_switch_misreads_are_refused() {
    for json in [
        r#"{"sofia_contact":{"user":"SECRET~x","domain":"example.com"}}"#,
        r#"{"sofia_contact":{"user":"SECRET@x","domain":"example.com"}}"#,
        r#"{"sofia_contact":{"user":"SECRET/x","domain":"example.com"}}"#,
        r#"{"sofia_contact":{"user":"u","domain":"SECRET/x"}}"#,
        r#"{"sofia_contact":{"user":"u","domain":"SECRET~x"}}"#,
        r#"{"sofia_contact":{"user":"SECRET","domain":""}}"#,
        r#"{"sofia_contact":{"user":"SECRET","domain":"example.com","profile":""}}"#,
        r#"{"sofia_contact":{"user":"u","domain":"example.com","profile":"SECRET/x"}}"#,
        r#"{"sofia_contact":{"user":"SECRET:_:x","domain":"example.com"}}"#,
        r#"{"sofia_contact":{"user":"SECRET x","domain":"example.com"}}"#,
        r#"{"sofia_contact":{"user":"SECRET,x","domain":"example.com"}}"#,
        r#"{"sofia_contact":{"user":"SECRET's","domain":"example.com"}}"#,
        r#"{"sofia_contact":{"user":"SECRET\\n","domain":"example.com"}}"#,
        r#"{"sofia_contact":{"user":"SECRET)","domain":"example.com"}}"#,
        r#"{"group_call":{"group":"SECRET+x","domain":"example.com"}}"#,
        r#"{"group_call":{"group":"SECRET@x","domain":"example.com"}}"#,
        r#"{"group_call":{"group":"g","domain":"SECRET+x"}}"#,
        r#"{"group_call":{"group":"SECRET|x","domain":"example.com"}}"#,
        r#"{"group_call":{"group":"g","domain":"SECRET}"}}"#,
        r#"{"group_call":{"group":"g","domain":"SECRET:_:x"}}"#,
        r#"{"portaudio":{"destination":""}}"#,
        r#"{"alsa":{"destination":""}}"#,
    ] {
        let msg = serde_json::from_str::<Endpoint>(json)
            .expect_err(json)
            .to_string();
        assert!(!msg.contains("SECRET"), "{json}: {msg}");
    }
    for input in [
        "${sofia_contact(SECRET~x@example.com)}",
        "${sofia_contact(/SECRET@example.com)}",
        "${sofia_contact(SECRET@)}",
        "${sofia_contact(SECRET@example.com/x)}",
        "${group_call(g@SECRET:_:x)}",
    ] {
        let msg = Endpoint::parse_for(input, DialStringCarrier::Dialplan.measured())
            .expect_err(input)
            .to_string();
        assert!(!msg.contains("SECRET"), "{input}: {msg}");
    }
}

/// The module reads each of these empty fields as written rather than as a default.
#[test]
fn empty_fields_the_module_reads_as_written_round_trip() {
    let cases: [Endpoint; 8] = [
        LoopbackEndpoint::new("")
            .unwrap()
            .into(),
        SofiaEndpoint::new("", "1000")
            .unwrap()
            .into(),
        SofiaEndpoint::new("internal", "")
            .unwrap()
            .into(),
        SofiaGateway::new("gw", "1")
            .unwrap()
            .with_profile("")
            .unwrap()
            .into(),
        UserEndpoint::new("bob")
            .unwrap()
            .with_domain("")
            .unwrap()
            .into(),
        GroupCall::new("", "example.com")
            .unwrap()
            .into(),
        GroupCall::new("support", "")
            .unwrap()
            .into(),
        SofiaContact::new("", "example.com")
            .unwrap()
            .into(),
    ];
    for ep in cases {
        for target in [
            DialStringTarget::new(DialStringCarrier::EslApi, BlockParse::MEASURED),
            DialStringTarget::new(DialStringCarrier::Dialplan, BlockParse::MEASURED),
            tilde(),
            DialStringTarget::new(DialStringCarrier::Direct, BlockParse::MEASURED),
        ] {
            let rendered = ep
                .display_for(target)
                .to_string();
            let parsed = Endpoint::parse_for(&rendered, target);
            if ep
                .check_expanded(target.carrier())
                .is_err()
            {
                assert!(parsed.is_err(), "{rendered:?} at {target:?}");
                continue;
            }
            assert_eq!(
                parsed.unwrap_or_else(|e| panic!("{rendered:?} at {target:?}: {e}")),
                ep,
                "{rendered:?} at {target:?}"
            );
        }
        let json = serde_json::to_string(&ep).unwrap();
        assert_eq!(
            serde_json::from_str::<Endpoint>(&json).unwrap(),
            ep,
            "{json}"
        );
    }
}

/// `group_call_function` takes the first `+` for the order before it looks for `@`, and
/// `sofia_contact_function` reads a `/` after the profile's as part of the user.
#[test]
fn expression_fields_split_where_the_functions_split() {
    let group = GroupCall::parse_bare("${group_call(g@d@e+F)}").unwrap();
    assert_eq!(
        (group.group(), group.domain(), group.order()),
        ("g", "d@e", Some(GroupCallOrder::First))
    );
    let contact = SofiaContact::on_profile("p", "u/x", "example.com").unwrap();
    assert_eq!(
        SofiaContact::parse_bare(&contact.render(DialStringCarrier::EslApi.measured())).unwrap(),
        contact
    );
    assert!(serde_json::from_str::<Endpoint>(
        r#"{"sofia_contact":{"user":"u/x","domain":"example.com","profile":"p"}}"#
    )
    .is_ok());
}

/// The block ahead of an endpoint meets the same passes as the rest of its leg, each reading
/// `\\` as one backslash.
#[test]
fn parse_reads_the_block_as_the_switch_installs_it() {
    let ep = Endpoint::parse_for(
        r"{k=a\\\\\\b}loopback/9199",
        DialStringCarrier::EslApi.measured(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        ep.variables()
            .and_then(|vars| vars.get("k")),
        Some(r"a\b")
    );
}

/// `:_:` splits the dial string into threads, so no endpoint carries it.
#[test]
fn the_enterprise_separator_is_refused_in_any_field() {
    for input in ["loopback/9199/SECRET:_:x", "{k=v}sofia/internal/SECRET:_:x"] {
        let msg = Endpoint::parse_for(input, DialStringCarrier::EslApi.measured())
            .expect_err(input)
            .to_string();
        assert!(!msg.contains("SECRET"), "{input}: {msg}");
    }
}

/// Each constructor and `with_*` builder refuses the field parse and config load refuse, naming
/// the same endpoint, field and fault.
#[test]
fn builders_refuse_what_parse_refuses() {
    use EndpointFieldFault::*;
    let cases: Vec<(
        Result<Endpoint, OriginateError>,
        &str,
        &str,
        EndpointFieldFault,
    )> = vec![
        (
            SofiaEndpoint::new("gateway", "1000").map(Endpoint::from),
            "sofia",
            "profile",
            ReadsAsGateway,
        ),
        (
            SofiaEndpoint::new("a/b", "1000").map(Endpoint::from),
            "sofia",
            "profile",
            ModuleSeparator("/"),
        ),
        (
            SofiaEndpoint::new("internal", "a:_:b").map(Endpoint::from),
            "sofia",
            "destination",
            EnterpriseSeparator,
        ),
        (
            SofiaGateway::new("gw@x", "1").map(Endpoint::from),
            "sofia gateway",
            "gateway",
            ModuleSeparator("@"),
        ),
        (
            SofiaGateway::new("a::b", "1").map(Endpoint::from),
            "sofia gateway",
            "gateway",
            ModuleSeparator("::"),
        ),
        (
            SofiaGateway::new("gw", "1")
                .and_then(|ep| ep.with_profile("p^"))
                .map(Endpoint::from),
            "sofia gateway",
            "profile",
            ModuleSeparator("^"),
        ),
        (
            SofiaContact::new("u~", "d").map(Endpoint::from),
            "sofia_contact",
            "user",
            FunctionSeparator("~"),
        ),
        (
            SofiaContact::new("u", "").map(Endpoint::from),
            "sofia_contact",
            "domain",
            EmptyReadsAsDefault,
        ),
        (
            SofiaContact::new("u", "d")
                .and_then(|ep| ep.with_profile(""))
                .map(Endpoint::from),
            "sofia_contact",
            "profile",
            EmptyReadsAsDefault,
        ),
        (
            LoopbackEndpoint::new("9/9").map(Endpoint::from),
            "loopback",
            "extension",
            ModuleSeparator("/"),
        ),
        (
            LoopbackEndpoint::new("9199")
                .and_then(|ep| ep.with_context(""))
                .map(Endpoint::from),
            "loopback",
            "context",
            EmptyReadsAsDefault,
        ),
        (
            LoopbackEndpoint::new("9199")
                .and_then(|ep| ep.with_dialplan(""))
                .map(Endpoint::from),
            "loopback",
            "dialplan",
            EmptyReadsAsDefault,
        ),
        (
            LoopbackEndpoint::new("app=park")
                .and_then(|ep| ep.with_context("x"))
                .map(Endpoint::from),
            "loopback",
            "context",
            FollowsAnApplication,
        ),
        (
            UserEndpoint::new("a@b").map(Endpoint::from),
            "user",
            "name",
            ModuleSeparator("@"),
        ),
        (
            UserEndpoint::new("").map(Endpoint::from),
            "user",
            "name",
            EmptyDialsNothing,
        ),
        (
            UserEndpoint::new("bob")
                .and_then(|ep| ep.with_domain("a:_:b"))
                .map(Endpoint::from),
            "user",
            "domain",
            EnterpriseSeparator,
        ),
        (
            AudioEndpoint::new()
                .with_destination("")
                .map(Endpoint::Alsa),
            "audio",
            "destination",
            EmptyReadsAsDefault,
        ),
        (
            GroupCall::new("g+", "d").map(Endpoint::from),
            "group_call",
            "group",
            FunctionSeparator("+"),
        ),
        (
            GroupCall::new("g", "d d").map(Endpoint::from),
            "group_call",
            "domain",
            ReadBeforeTheFunction,
        ),
    ];
    for (built, endpoint, field, fault) in cases {
        assert_eq!(
            built,
            Err(OriginateError::UndeliverableEndpointField {
                endpoint,
                field,
                fault
            }),
            "{endpoint} {field}"
        );
    }
}

/// A field legal only beside its qualifier builds through the constructor taking both, and
/// renders the text whose parse holds the same value.
#[test]
fn qualified_constructors_build_what_parse_builds() {
    use DialStringCarrier::{Dialplan, EslApi};
    let cases: [(Endpoint, &str, DialStringCarrier); 3] = [
        (
            SofiaGateway::on_profile("external", "a::b", "1000")
                .unwrap()
                .into(),
            "sofia/gateway/external::a::b/1000",
            EslApi,
        ),
        (
            SofiaContact::on_profile("internal", "u/x", "example.com")
                .unwrap()
                .into(),
            "${sofia_contact(internal/u/x@example.com)}",
            Dialplan,
        ),
        (
            UserEndpoint::in_domain("", "example.com")
                .unwrap()
                .into(),
            "user/@example.com",
            EslApi,
        ),
    ];
    for (built, text, carrier) in cases {
        assert_eq!(built.render(carrier.measured()), text);
        assert_eq!(
            Endpoint::parse_for(text, carrier.measured()),
            Ok(built),
            "{text}"
        );
    }
    assert!(SofiaGateway::on_profile("p^", "gw", "1").is_err());
    assert!(SofiaContact::on_profile("", "u", "d").is_err());
    assert!(UserEndpoint::in_domain("a@b", "d").is_err());
}
