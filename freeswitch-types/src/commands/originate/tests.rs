use super::*;
use crate::commands::endpoint::{LoopbackEndpoint, SofiaEndpoint, SofiaGateway};
use crate::commands::originate_split;
use crate::commands::variables::InvalidArgvSeparator;

/// `switch_strip_whitespace` strips tab, newline, vertical tab, CR and space only.
#[test]
fn parse_strips_only_the_switch_whitespace() {
    let extension = |line: &str| match Originate::from_str(line)
        .unwrap()
        .target()
    {
        OriginateTarget::Extension(ext) => ext.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        extension("originate \t\x0bloopback/9199/test 9199 \x0b\r\n"),
        "9199"
    );
    assert_eq!(
        extension("originate loopback/9199/test 9199\x0c"),
        "9199\x0c"
    );
    assert_eq!(
        extension("originate loopback/9199/test 9199\u{a0}"),
        "9199\u{a0}"
    );
}

// --- Endpoint ---

#[test]
fn endpoint_uri_only() {
    let ep = Endpoint::Sofia(SofiaEndpoint {
        profile: "internal".into(),
        destination: "123@example.com".into(),
        variables: None,
    });
    assert_eq!(ep.to_string(), "sofia/internal/123@example.com");
}

#[test]
fn endpoint_uri_with_variable() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("one_variable", "1");
    let ep = Endpoint::Sofia(SofiaEndpoint {
        profile: "internal".into(),
        destination: "123@example.com".into(),
        variables: Some(vars),
    });
    assert_eq!(
        ep.to_string(),
        "{one_variable=1}sofia/internal/123@example.com"
    );
}

#[test]
fn endpoint_variable_with_quote() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("one_variable", "one'quote");
    let ep = Endpoint::Sofia(SofiaEndpoint {
        profile: "internal".into(),
        destination: "123@example.com".into(),
        variables: Some(vars),
    });
    // An endpoint renders for the ESL API carrier, which is the deeper of
    // the two escapings.
    assert_eq!(
        ep.to_string(),
        r"{one_variable=one\\\\\\\'quote}sofia/internal/123@example.com"
    );
}

/// `Display` and `FromStr` are the default revision, not a second render path.
#[test]
fn display_and_from_str_are_the_default_revision() {
    use crate::commands::variables::BlockParse;

    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("cid", "it's");
    let orig = Originate::application(
        Endpoint::SofiaGateway(SofiaGateway::new("gw", "1234").with_variables(vars)),
        Application::simple("park"),
    );

    let rendered = orig
        .display_with(BlockParse::PairSplitCleans)
        .to_string();
    assert_eq!(rendered, orig.to_string());
    assert_eq!(
        Originate::parse_with(&rendered, BlockParse::PairSplitCleans)
            .unwrap_or_else(|e| panic!("{rendered} failed to parse: {e}")),
        orig
    );
}

#[test]
fn loopback_endpoint_display() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("one_variable", "1");
    let ep = Endpoint::Loopback(
        LoopbackEndpoint::new("aUri")
            .with_context("aContext")
            .with_variables(vars),
    );
    assert_eq!(ep.to_string(), "{one_variable=1}loopback/aUri/aContext");
}

#[test]
fn sofia_gateway_endpoint_display() {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("one_variable", "1");
    let ep = Endpoint::SofiaGateway(SofiaGateway {
        destination: "aUri".into(),
        profile: None,
        gateway: "internal".into(),
        variables: Some(vars),
    });
    assert_eq!(
        ep.to_string(),
        "{one_variable=1}sofia/gateway/internal/aUri"
    );
}

// --- Application ---

#[test]
fn application_xml_format() {
    let app = Application::new("testApp", Some("testArg"));
    assert_eq!(
        app.to_string_with_dialplan(&DialplanType::XML),
        "&testApp(testArg)"
    );
}

#[test]
fn application_inline_format() {
    let app = Application::new("testApp", Some("testArg"));
    assert_eq!(
        app.to_string_with_dialplan(&DialplanType::INLINE),
        "testApp:testArg"
    );
}

#[test]
fn application_inline_no_args() {
    let app = Application::simple("park");
    assert_eq!(app.to_string_with_dialplan(&DialplanType::INLINE), "park");
}

// --- Originate ---

#[test]
fn originate_xml_display() {
    let ep = Endpoint::Sofia(SofiaEndpoint {
        profile: "internal".into(),
        destination: "123@example.com".into(),
        variables: None,
    });
    let orig = Originate::application(ep, Application::new("conference", Some("1")))
        .dialplan(DialplanType::XML)
        .unwrap();
    assert_eq!(
        orig.to_string(),
        "originate sofia/internal/123@example.com &conference(1) XML"
    );
}

#[test]
fn originate_inline_display() {
    let ep = Endpoint::Sofia(SofiaEndpoint {
        profile: "internal".into(),
        destination: "123@example.com".into(),
        variables: None,
    });
    let orig = Originate::inline(ep, vec![Application::new("conference", Some("1"))])
        .unwrap()
        .dialplan(DialplanType::INLINE)
        .unwrap();
    assert_eq!(
        orig.to_string(),
        "originate sofia/internal/123@example.com conference:1 inline"
    );
}

#[test]
fn originate_extension_display() {
    let ep = Endpoint::Sofia(SofiaEndpoint {
        profile: "internal".into(),
        destination: "123@example.com".into(),
        variables: None,
    });
    let orig = Originate::extension(ep, "1000")
        .dialplan(DialplanType::XML)
        .unwrap()
        .context("default");
    assert_eq!(
        orig.to_string(),
        "originate sofia/internal/123@example.com 1000 XML default"
    );
}

#[test]
fn originate_extension_round_trip() {
    let input = "originate sofia/internal/test@example.com 1000 XML default";
    let parsed: Originate = input
        .parse()
        .unwrap();
    assert_eq!(parsed.to_string(), input);
    assert!(matches!(parsed.target(), OriginateTarget::Extension(ref e) if e == "1000"));
}

#[test]
fn originate_extension_no_dialplan() {
    let input = "originate sofia/internal/test@example.com 1000";
    let parsed: Originate = input
        .parse()
        .unwrap();
    assert!(matches!(parsed.target(), OriginateTarget::Extension(ref e) if e == "1000"));
    assert_eq!(parsed.to_string(), input);
}

#[test]
fn originate_extension_with_inline_errors() {
    let ep = Endpoint::Sofia(SofiaEndpoint {
        profile: "internal".into(),
        destination: "123@example.com".into(),
        variables: None,
    });
    let result = Originate::extension(ep, "1000").dialplan(DialplanType::INLINE);
    assert!(result.is_err());
}

#[test]
fn originate_empty_inline_errors() {
    let ep = Endpoint::Sofia(SofiaEndpoint {
        profile: "internal".into(),
        destination: "123@example.com".into(),
        variables: None,
    });
    let result = Originate::inline(ep, vec![]);
    assert!(result.is_err());
}

#[test]
fn originate_from_string_round_trip() {
    let input = "originate {test='variable with quote'}sofia/internal/test@example.com 123";
    let orig: Originate = input
        .parse()
        .unwrap();
    assert!(matches!(orig.target(), OriginateTarget::Extension(ref e) if e == "123"));
    assert_eq!(orig.to_string(), input);
}

#[test]
fn originate_socket_app_quoted() {
    let ep = Endpoint::Loopback(LoopbackEndpoint::new("9199").with_context("test"));
    let orig = Originate::application(
        ep,
        Application::new("socket", Some("127.0.0.1:8040 async full")),
    );
    assert_eq!(
        orig.to_string(),
        "originate loopback/9199/test '&socket(127.0.0.1:8040 async full)'"
    );
}

#[test]
fn originate_socket_round_trip() {
    let input = "originate loopback/9199/test '&socket(127.0.0.1:8040 async full)'";
    let parsed: Originate = input
        .parse()
        .unwrap();
    assert_eq!(parsed.to_string(), input);
    if let OriginateTarget::Application(ref app) = parsed.target() {
        assert_eq!(app.args(), Some("127.0.0.1:8040 async full"));
    } else {
        panic!("expected Application target");
    }
}

#[test]
fn originate_display_round_trip() {
    let ep = Endpoint::Sofia(SofiaEndpoint {
        profile: "internal".into(),
        destination: "123@example.com".into(),
        variables: None,
    });
    let orig = Originate::application(ep, Application::new("conference", Some("1")))
        .dialplan(DialplanType::XML)
        .unwrap();
    let s = orig.to_string();
    let parsed: Originate = s
        .parse()
        .unwrap();
    assert_eq!(parsed.to_string(), s);
}

#[test]
fn originate_inline_no_args_round_trip() {
    let input = "originate sofia/internal/123@example.com park inline";
    let parsed: Originate = input
        .parse()
        .unwrap();
    assert_eq!(parsed.to_string(), input);
    if let OriginateTarget::InlineApplications(ref apps) = parsed.target() {
        assert!(apps[0]
            .args()
            .is_none());
    } else {
        panic!("expected InlineApplications target");
    }
}

#[test]
fn originate_inline_multi_app_round_trip() {
    let input =
            "originate sofia/internal/123@example.com playback:/tmp/test.wav,hangup:NORMAL_CLEARING inline";
    let parsed: Originate = input
        .parse()
        .unwrap();
    assert_eq!(parsed.to_string(), input);
}

#[test]
fn originate_inline_auto_dialplan() {
    let ep = Endpoint::Sofia(SofiaEndpoint {
        profile: "internal".into(),
        destination: "123@example.com".into(),
        variables: None,
    });
    let orig = Originate::inline(ep, vec![Application::simple("park")]).unwrap();
    assert!(orig
        .to_string()
        .contains("inline"));
}

// --- DialplanType ---

#[test]
fn dialplan_type_display() {
    assert_eq!(DialplanType::INLINE.to_string(), "inline");
    assert_eq!(DialplanType::XML.to_string(), "XML");
}

#[test]
fn dialplan_type_from_str() {
    assert_eq!(
        "inline"
            .parse::<DialplanType>()
            .unwrap(),
        DialplanType::INLINE
    );
    assert_eq!(
        "XML"
            .parse::<DialplanType>()
            .unwrap(),
        DialplanType::XML
    );
}

#[test]
fn dialplan_type_from_str_case_insensitive() {
    assert_eq!(
        "xml"
            .parse::<DialplanType>()
            .unwrap(),
        DialplanType::XML
    );
    assert_eq!(
        "Xml"
            .parse::<DialplanType>()
            .unwrap(),
        DialplanType::XML
    );
    assert_eq!(
        "INLINE"
            .parse::<DialplanType>()
            .unwrap(),
        DialplanType::INLINE
    );
    assert_eq!(
        "Inline"
            .parse::<DialplanType>()
            .unwrap(),
        DialplanType::INLINE
    );
}

/// A known name in any case is the typed value, so equality and the inline checks see it.
#[test]
fn dialplan_type_named_resolves_known_names() {
    for (name, known) in [
        ("inline", DialplanType::INLINE),
        ("Inline", DialplanType::INLINE),
        ("xml", DialplanType::XML),
        ("XML", DialplanType::XML),
    ] {
        assert_eq!(DialplanType::named(name).unwrap(), known, "{name}");
    }
    let custom = DialplanType::named("Custom").unwrap();
    assert_ne!(custom, DialplanType::XML);
    assert_eq!(custom, DialplanType::named("CUSTOM").unwrap());
    assert_eq!(custom.name(), "Custom");
    assert_eq!(custom.to_string(), "Custom");
    assert_eq!(
        "Custom"
            .parse::<DialplanType>()
            .unwrap(),
        custom
    );
    let json = serde_json::to_string(&custom).unwrap();
    assert_eq!(json, "\"Custom\"");
    assert_eq!(serde_json::from_str::<DialplanType>(&json).unwrap(), custom);
    assert_eq!(
        serde_json::from_str::<DialplanType>("\"XML\"").unwrap(),
        DialplanType::XML
    );
}

/// `originate_function` reads `undef` in any case as an absent argument, so it names no
/// dialplan.
#[test]
fn dialplan_type_refuses_undef() {
    for name in ["undef", "UNDEF", "Undef"] {
        assert_eq!(
            DialplanType::named(name),
            Err(ParseDialplanTypeError(name.into()))
        );
        assert!(name
            .parse::<DialplanType>()
            .is_err());
        let msg = serde_json::from_str::<DialplanType>(&format!("\"{name}\""))
            .unwrap_err()
            .to_string();
        assert!(msg.contains("undef"), "{msg}");
    }
}

// --- Serde ---

#[test]
fn serde_dialplan_type_xml() {
    let json = serde_json::to_string(&DialplanType::XML).unwrap();
    assert_eq!(json, "\"xml\"");
    let parsed: DialplanType = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, DialplanType::XML);
}

#[test]
fn serde_dialplan_type_inline() {
    let json = serde_json::to_string(&DialplanType::INLINE).unwrap();
    assert_eq!(json, "\"inline\"");
    let parsed: DialplanType = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, DialplanType::INLINE);
}

#[test]
fn serde_application() {
    let app = Application::new("park", None::<&str>);
    let json = serde_json::to_string(&app).unwrap();
    let parsed: Application = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, app);
}

#[test]
fn serde_application_with_args() {
    let app = Application::new("conference", Some("1"));
    let json = serde_json::to_string(&app).unwrap();
    let parsed: Application = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, app);
}

#[test]
fn serde_application_skips_none_args() {
    let app = Application::new("park", None::<&str>);
    let json = serde_json::to_string(&app).unwrap();
    assert!(!json.contains("args"));
}

#[test]
fn serde_originate_application_round_trip() {
    let ep = Endpoint::Sofia(SofiaEndpoint {
        profile: "internal".into(),
        destination: "123@example.com".into(),
        variables: None,
    });
    let orig = Originate::application(ep, Application::new("park", None::<&str>))
        .dialplan(DialplanType::XML)
        .unwrap()
        .context("default")
        .cid_name("Test")
        .cid_num("5551234")
        .timeout(Duration::from_secs(30));
    let json = serde_json::to_string(&orig).unwrap();
    assert!(json.contains("\"application\""));
    let parsed: Originate = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, orig);
}

#[test]
fn serde_originate_extension() {
    let json = r#"{
            "endpoint": {"sofia": {"profile": "internal", "destination": "123@example.com"}},
            "extension": "1000",
            "dialplan": "xml",
            "context": "default"
        }"#;
    let orig: Originate = serde_json::from_str(json).unwrap();
    assert!(matches!(orig.target(), OriginateTarget::Extension(ref e) if e == "1000"));
    assert_eq!(
        orig.to_string(),
        "originate sofia/internal/123@example.com 1000 XML default"
    );
}

#[test]
fn serde_originate_extension_with_inline_rejected() {
    let json = r#"{
            "endpoint": {"sofia": {"profile": "internal", "destination": "123@example.com"}},
            "extension": "1000",
            "dialplan": "inline"
        }"#;
    let result = serde_json::from_str::<Originate>(json);
    assert!(result.is_err());
}

#[test]
fn serde_originate_empty_inline_rejected() {
    let json = r#"{
            "endpoint": {"sofia": {"profile": "internal", "destination": "123@example.com"}},
            "inline_applications": []
        }"#;
    let result = serde_json::from_str::<Originate>(json);
    assert!(result.is_err());
}

#[test]
fn serde_originate_inline_applications() {
    let json = r#"{
            "endpoint": {"sofia": {"profile": "internal", "destination": "123@example.com"}},
            "inline_applications": [
                {"name": "playback", "args": "/tmp/test.wav"},
                {"name": "hangup", "args": "NORMAL_CLEARING"}
            ]
        }"#;
    let orig: Originate = serde_json::from_str(json).unwrap();
    if let OriginateTarget::InlineApplications(ref apps) = orig.target() {
        assert_eq!(apps.len(), 2);
    } else {
        panic!("expected InlineApplications");
    }
    assert!(orig
        .to_string()
        .contains("inline"));
}

/// A config file states applications as structured data, so it carries no
/// separator to get wrong — and must not have to grow one.
#[test]
fn serde_inline_gets_a_separator_without_naming_one() {
    let json = r#"{
            "endpoint": {"sofia": {"profile": "internal", "destination": "123@example.com"}},
            "inline_applications": [
                {"name": "playback", "args": "tone_stream://%(500,0,800)"},
                {"name": "park"}
            ]
        }"#;
    let orig: Originate = serde_json::from_str(json).unwrap();

    assert_eq!(orig.inline_delimiter(), None);
    assert!(orig
        .to_string()
        .contains(r"playback:tone_stream://%(500\\,0\\,800),park"));

    // Serializing back must not introduce a separator field the source
    // never had.
    let round_tripped = serde_json::to_string(&orig).unwrap();
    assert!(!round_tripped.contains("delimiter"));
    let reparsed: Originate = serde_json::from_str(&round_tripped).unwrap();
    assert_eq!(reparsed.to_string(), orig.to_string());
}

#[test]
fn serde_originate_skips_none_fields() {
    let ep = Endpoint::Sofia(SofiaEndpoint {
        profile: "internal".into(),
        destination: "123@example.com".into(),
        variables: None,
    });
    let orig = Originate::application(ep, Application::new("park", None::<&str>));
    let json = serde_json::to_string(&orig).unwrap();
    assert!(!json.contains("dialplan"));
    assert!(!json.contains("context"));
    assert!(!json.contains("cid_name"));
    assert!(!json.contains("cid_num"));
    assert!(!json.contains("timeout"));
}

#[test]
fn serde_originate_to_wire_format() {
    let json = r#"{
            "endpoint": {"sofia": {"profile": "internal", "destination": "123@example.com"}},
            "application": {"name": "park"},
            "dialplan": "xml",
            "context": "default"
        }"#;
    let orig: Originate = serde_json::from_str(json).unwrap();
    let wire = orig.to_string();
    assert!(wire.starts_with("originate"));
    assert!(wire.contains("sofia/internal/123@example.com"));
    assert!(wire.contains("&park()"));
    assert!(wire.contains("XML"));
}

// --- Application::simple ---

#[test]
fn application_simple_no_args() {
    let app = Application::simple("park");
    assert_eq!(app.name(), "park");
    assert!(app
        .args()
        .is_none());
}

#[test]
fn application_simple_xml_format() {
    let app = Application::simple("park");
    assert_eq!(app.to_string_with_dialplan(&DialplanType::XML), "&park()");
}

// --- OriginateTarget From impls ---

#[test]
fn originate_target_from_application() {
    let target: OriginateTarget = Application::simple("park").into();
    assert!(matches!(target, OriginateTarget::Application(_)));
}

#[test]
fn originate_target_from_vec() {
    let target: OriginateTarget = vec![
        Application::new("conference", Some("1")),
        Application::new("hangup", Some("NORMAL_CLEARING")),
    ]
    .into();
    if let OriginateTarget::InlineApplications(apps) = target {
        assert_eq!(apps.len(), 2);
    } else {
        panic!("expected InlineApplications");
    }
}

#[test]
fn originate_target_application_wire_format() {
    let ep = Endpoint::Sofia(SofiaEndpoint {
        profile: "internal".into(),
        destination: "123@example.com".into(),
        variables: None,
    });
    let orig = Originate::application(ep, Application::simple("park"));
    assert_eq!(
        orig.to_string(),
        "originate sofia/internal/123@example.com &park()"
    );
}

#[test]
fn originate_timeout_only_fills_positional_gaps() {
    let ep = Endpoint::Loopback(LoopbackEndpoint::new("9199").with_context("test"));
    let cmd =
        Originate::application(ep, Application::simple("park")).timeout(Duration::from_secs(30));
    assert_eq!(
        cmd.to_string(),
        "originate loopback/9199/test &park() undef undef undef undef 30"
    );
}

#[test]
fn originate_cid_num_only_fills_preceding_gaps() {
    let ep = Endpoint::Loopback(LoopbackEndpoint::new("9199").with_context("test"));
    let cmd = Originate::application(ep, Application::simple("park")).cid_num("5551234");
    assert_eq!(
        cmd.to_string(),
        "originate loopback/9199/test &park() undef undef undef 5551234"
    );
}

#[test]
fn originate_context_only_fills_dialplan() {
    let ep = Endpoint::Loopback(LoopbackEndpoint::new("9199").with_context("test"));
    let cmd = Originate::extension(ep, "1000").context("myctx");
    assert_eq!(
        cmd.to_string(),
        "originate loopback/9199/test 1000 undef myctx"
    );
}

/// A slot forced present by a later one is `undef`, which reads back as absent.
#[test]
fn originate_gap_filler_round_trips() {
    let ep = Endpoint::Loopback(LoopbackEndpoint::new("9199").with_context("test"));
    let cmd = Originate::application(ep, Application::simple("park")).cid_name("Alice");
    let wire = cmd.to_string();
    let parsed: Originate = wire
        .parse()
        .unwrap();
    assert_eq!(parsed, cmd);
    assert_eq!(parsed.to_string(), wire);
}

#[test]
fn serde_originate_full_round_trip_with_variables() {
    let mut ep_vars = Variables::new(VariablesType::Default);
    ep_vars.insert("originate_timeout", "30");
    ep_vars.insert("sip_h_X-Custom", "value with spaces");
    let ep = Endpoint::SofiaGateway(SofiaGateway {
        gateway: "my_provider".into(),
        destination: "18005551234".into(),
        profile: Some("external".into()),
        variables: Some(ep_vars),
    });
    let orig = Originate::application(ep, Application::new("park", None::<&str>))
        .dialplan(DialplanType::XML)
        .unwrap()
        .context("public")
        .cid_name("Test Caller")
        .cid_num("5551234")
        .timeout(Duration::from_secs(60));
    let json = serde_json::to_string(&orig).unwrap();
    let parsed: Originate = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, orig);
    assert_eq!(parsed.to_string(), orig.to_string());
}

#[test]
fn serde_originate_inline_round_trip_with_all_fields() {
    let ep = Endpoint::Loopback(LoopbackEndpoint::new("9199").with_context("default"));
    let orig = Originate::inline(
        ep,
        vec![
            Application::new("playback", Some("/tmp/test.wav")),
            Application::new("hangup", Some("NORMAL_CLEARING")),
        ],
    )
    .unwrap()
    .dialplan(DialplanType::INLINE)
    .unwrap()
    .context("default")
    .cid_name("IVR")
    .cid_num("0000")
    .timeout(Duration::from_secs(45));
    let json = serde_json::to_string(&orig).unwrap();
    let parsed: Originate = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, orig);
    assert_eq!(parsed.to_string(), orig.to_string());
}

#[test]
fn originate_context_named_inline() {
    let ep = Endpoint::Sofia(SofiaEndpoint {
        profile: "internal".into(),
        destination: "123@example.com".into(),
        variables: None,
    });
    let orig = Originate::extension(ep, "1000")
        .dialplan(DialplanType::XML)
        .unwrap()
        .context("inline");
    let wire = orig.to_string();
    assert!(wire.contains("XML inline"), "wire: {}", wire);
    let parsed: Originate = wire
        .parse()
        .unwrap();
    // "inline" is consumed as the dialplan type, not the context
    // This is an accepted limitation of positional parsing
    assert_eq!(parsed.to_string(), wire);
}

#[test]
fn originate_context_named_xml() {
    let ep = Endpoint::Sofia(SofiaEndpoint {
        profile: "internal".into(),
        destination: "123@example.com".into(),
        variables: None,
    });
    let orig = Originate::extension(ep, "1000")
        .dialplan(DialplanType::XML)
        .unwrap()
        .context("XML");
    let wire = orig.to_string();
    // "XML XML" - first is dialplan, second is context
    assert!(wire.contains("XML XML"), "wire: {}", wire);
    let parsed: Originate = wire
        .parse()
        .unwrap();
    assert_eq!(parsed.to_string(), wire);
}

/// A dial string carries caller-id and `sip_h_*` values, so a rejection
/// names the field it failed on and leaves the bytes on the error.
#[test]
fn errors_name_the_field_and_never_quote_the_input() {
    let cases: [(&str, &str); 4] = [
        (
            "originate sofia/a/b 1000 XML default undef undef 30s",
            "30s",
        ),
        ("originate error/NO_SUCH_CAUSE 1000", "NO_SUCH_CAUSE"),
        ("originate ${group_call(support@example.com+Z)} 1000", "+Z"),
        ("originate verto/15551234567 1000", "15551234567"),
    ];
    for (input, secret) in cases {
        let msg = input
            .parse::<Originate>()
            .expect_err(input)
            .to_string();
        assert!(!msg.contains(secret), "{input} quoted its input: {msg}");
    }

    let msg = originate_split("originate 'never closed", ' ')
        .expect_err("unclosed quote")
        .to_string();
    assert!(!msg.contains("never closed"), "quoted its input: {msg}");
}

/// A rejected timeout, cause or order has a cause of its own; stringifying
/// it into a message drops the chain a caller would match on.
#[test]
fn errors_keep_their_source() {
    use std::error::Error;

    for input in [
        "originate sofia/a/b 1000 XML default undef undef 30s",
        "originate error/NO_SUCH_CAUSE 1000",
        "originate ${group_call(support@example.com+Z)} 1000",
    ] {
        assert!(
            input
                .parse::<Originate>()
                .expect_err(input)
                .source()
                .is_some(),
            "{input} has no source"
        );
    }
}

/// A caller-id with a space is quoted on the way out, so the quotes are
/// this crate's own framing and have to come back off on the way in. Left
/// on, they are re-quoted at every hop and the switch dials the quotes.
#[test]
fn quoted_caller_id_round_trips_without_gaining_quotes() {
    let ep = Endpoint::Loopback(LoopbackEndpoint::new("9199").with_context("test"));
    let cmd = Originate::application(ep, Application::simple("park"))
        .cid_name("Outbound Call")
        .cid_num("555 1234");
    let wire = cmd.to_string();
    assert_eq!(
        wire,
        "originate loopback/9199/test &park() undef undef 'Outbound Call' '555 1234'"
    );

    let parsed: Originate = wire
        .parse()
        .unwrap();
    assert_eq!(parsed.caller_id_name(), Some("Outbound Call"));
    assert_eq!(parsed.caller_id_number(), Some("555 1234"));
    assert_eq!(parsed.to_string(), wire);
}

/// The mutators are the config-driven path: deserialize a template, then
/// override per call. Clearing a field is half of that and had no coverage.
#[test]
fn originate_mutators_set_and_clear_every_optional_field() {
    let ep = Endpoint::Loopback(LoopbackEndpoint::new("9199").with_context("default"));
    let mut cmd = Originate::extension(ep, "1000");

    cmd.set_dialplan(Some(DialplanType::XML));
    cmd.set_context(Some("public"));
    cmd.set_cid_name(Some("Alice"));
    cmd.set_cid_num(Some("5551234"));
    cmd.set_timeout(Some(Duration::from_secs(30)));
    assert_eq!(
        cmd.to_string(),
        "originate loopback/9199/default 1000 XML public Alice 5551234 30"
    );
    assert_eq!(cmd.timeout_duration(), Some(Duration::from_secs(30)));

    // The turbofish is not decoration: `Option<impl Into<String>>` leaves a
    // bare `None` with no type to infer, so clearing a field needs one.
    cmd.set_dialplan(None);
    cmd.set_context(None::<String>);
    cmd.set_cid_name(None::<String>);
    cmd.set_cid_num(None::<String>);
    cmd.set_timeout(None);
    assert_eq!(cmd.to_string(), "originate loopback/9199/default 1000");
}

#[test]
fn originate_mut_accessors_reach_the_endpoint_and_the_target() {
    let ep = Endpoint::Sofia(SofiaEndpoint::new("internal", "1000@example.com"));
    let mut cmd = Originate::application(ep, Application::new("socket", Some("old")));

    *cmd.endpoint_mut() = Endpoint::Loopback(LoopbackEndpoint::new("9199"));
    if let OriginateTarget::Application(app) = cmd.target_mut() {
        *app.name_mut() = "playback".into();
        *app.args_mut() = Some("/tmp/test.wav".into());
    } else {
        panic!("expected Application target");
    }

    assert_eq!(
        cmd.to_string(),
        "originate loopback/9199 &playback(/tmp/test.wav)"
    );
}

#[test]
fn originate_accessors() {
    let ep = Endpoint::Loopback(LoopbackEndpoint::new("9199").with_context("default"));
    let cmd = Originate::extension(ep, "1000")
        .dialplan(DialplanType::XML)
        .unwrap()
        .context("default")
        .cid_name("Alice")
        .cid_num("5551234")
        .timeout(Duration::from_secs(30));

    assert!(matches!(cmd.target(), OriginateTarget::Extension(ref e) if e == "1000"));
    assert_eq!(cmd.dialplan_type(), Some(&DialplanType::XML));
    assert_eq!(cmd.context_str(), Some("default"));
    assert_eq!(cmd.caller_id_name(), Some("Alice"));
    assert_eq!(cmd.caller_id_number(), Some("5551234"));
    assert_eq!(cmd.timeout_seconds(), Some(30));
}

fn null_endpoint() -> Endpoint {
    Endpoint::Loopback(LoopbackEndpoint::new("9199"))
}

#[test]
fn inline_without_commas_keeps_the_default_separator() {
    let cmd = Originate::inline(
        null_endpoint(),
        [
            Application::simple("answer"),
            Application::new("playback", Some("silence_stream://300")),
        ],
    )
    .unwrap();

    assert_eq!(
        cmd.to_string(),
        "originate loopback/9199 answer,playback:silence_stream://300 inline"
    );
    assert_eq!(cmd.inline_delimiter(), None);
}

/// A bare comma inside an argument is read as an action separator by
/// `inline_dialplan_hunt`, producing applications the caller never wrote
/// and no error anywhere. Escaping it is what the switch's own
/// `cleanup_separated_string` undoes on the far side.
#[test]
fn inline_escapes_a_separator_inside_an_argument() {
    let cmd = Originate::inline(
        null_endpoint(),
        [
            Application::new("playback", Some("tone_stream://%(500,0,800)")),
            Application::simple("park"),
        ],
    )
    .unwrap();

    assert_eq!(
        cmd.to_string(),
        r"originate loopback/9199 'playback:tone_stream://%(500\\,0\\,800),park' inline"
    );
}

/// The whole point of escaping over separator juggling: an argument
/// rewritten after construction cannot invalidate anything, because
/// nothing was decided at construction.
#[test]
fn escaping_survives_arguments_rewritten_after_construction() {
    let mut cmd = Originate::inline(
        null_endpoint(),
        [Application::new("bridge", Some("${codecs}sofia/gw/1"))],
    )
    .unwrap();

    let OriginateTarget::InlineApplications(apps) = cmd.target_mut() else {
        panic!("expected InlineApplications");
    };
    *apps[0].args_mut() = Some("{absolute_codec_string=G722,PCMU}sofia/gw/1".to_string());

    assert!(cmd
        .to_string()
        .contains(r"G722\\,PCMU"));
}

#[test]
fn inline_escaped_separator_round_trips() {
    let cmd = Originate::inline(
        null_endpoint(),
        [
            Application::new("playback", Some("tone_stream://%(500,0,800)")),
            Application::new("bridge", Some("{a=1,b=2}null/farend")),
        ],
    )
    .unwrap();

    let rendered = cmd.to_string();
    let parsed: Originate = rendered
        .parse()
        .unwrap();
    assert_eq!(parsed.to_string(), rendered);
    assert_eq!(parsed.target(), cmd.target());
}

#[test]
fn inline_with_delimiter_overrides_the_choice() {
    let cmd = Originate::inline_with_delimiter(
        null_endpoint(),
        [Application::simple("answer"), Application::simple("park")],
        ';',
    )
    .unwrap();

    assert_eq!(cmd.inline_delimiter(), Some(';'));
    assert!(cmd
        .to_string()
        .contains("m:;:answer;park"));
}

#[test]
fn inline_with_delimiter_rejects_colon() {
    // `inline_dialplan_hunt` splits application from data on the first
    // colon, so a colon separator cannot be recovered.
    let err = Originate::inline_with_delimiter(null_endpoint(), [Application::simple("park")], ':')
        .unwrap_err();
    assert!(matches!(err, OriginateError::InvalidInlineDelimiter(':')));
}

/// The hunt's split pairs quotes, reads `\n r t s` as escapes and takes a non-ASCII
/// separator as one byte, so such a separator never carries every argument.
#[test]
fn inline_delimiter_refuses_what_breaks_the_hunt_split() {
    let parsed = [
        ' ', '\'', '\\', ':', '\t', '\n', '\u{1}', '\u{7f}', 'n', 'r', 't', 's',
    ];
    for delimiter in parsed
        .into_iter()
        .chain(['é'])
    {
        assert_eq!(
            Originate::inline_with_delimiter(null_endpoint(), [Application::park()], delimiter),
            Err(OriginateError::InvalidInlineDelimiter(delimiter)),
            "{delimiter:?}"
        );
    }
    for delimiter in parsed {
        let list = originate_quote(&format!("m:{delimiter}:park"));
        let line = format!("originate loopback/9199 {list} inline");
        assert_eq!(
            line.parse::<Originate>(),
            Err(OriginateError::InvalidInlineDelimiter(delimiter)),
            "{line:?}"
        );
    }
}

#[test]
fn invalid_inline_delimiter_display_omits_it() {
    let shown = OriginateError::InvalidInlineDelimiter('~').to_string();
    assert!(!shown.contains('~'), "{shown}");
}

/// An explicit separator that appears in an argument is escaped like any
/// other, so naming one never has to be conditional on the data.
#[test]
fn inline_with_delimiter_escapes_its_own_separator() {
    let cmd = Originate::inline_with_delimiter(
        null_endpoint(),
        [Application::new("playback", Some("a|b"))],
        '|',
    )
    .unwrap();

    assert_eq!(
        cmd.to_string(),
        r"originate loopback/9199 'm:|:playback:a\\|b' inline"
    );
}

#[test]
/// No argument can make a list unrenderable, so there is no separator
/// exhaustion to report.
fn inline_renders_an_argument_made_only_of_separators() {
    let cmd = Originate::inline(
        null_endpoint(),
        [Application::new("playback", Some(",|;~^!"))],
    )
    .unwrap();

    assert_eq!(
        cmd.to_string(),
        r"originate loopback/9199 'playback:\\,|;~^!' inline"
    );
}

/// Escaped for the hunt's split and again for the line's, a pair of quotes reaches the
/// application as written; measured on a live switch.
#[test]
fn inline_delivers_an_argument_carrying_two_quotes() {
    let cases = [
        (
            Originate::inline(
                null_endpoint(),
                [Application::new(
                    "set",
                    Some("c=${cond('${v}' != '' ? red : black)}"),
                )],
            )
            .unwrap(),
            r"originate loopback/9199 'set:c=${cond(\\\'${v}\\\' != \\\'\\\' ? red : black)}' inline",
        ),
        (
            Originate::inline_with_delimiter(
                null_endpoint(),
                [Application::new("playback", Some("a'b'c"))],
                '|',
            )
            .unwrap(),
            r"originate loopback/9199 'm:|:playback:a\\\'b\\\'c' inline",
        ),
    ];
    for (cmd, wire) in cases {
        assert_eq!(cmd.to_string(), wire);
        let parsed: Originate = wire
            .parse()
            .unwrap_or_else(|e| panic!("{wire:?} failed to parse: {e}"));
        assert_eq!(parsed.target(), cmd.target(), "{wire:?}");
    }
}

#[test]
fn parses_a_prefixed_inline_target() {
    let parsed: Originate = "originate loopback/9199 'm:|:answer|playback:a,b' inline"
        .parse()
        .unwrap();

    assert_eq!(parsed.inline_delimiter(), Some('|'));
    let OriginateTarget::InlineApplications(ref apps) = parsed.target() else {
        panic!("expected InlineApplications");
    };
    assert_eq!(apps.len(), 2);
    assert_eq!(apps[1].args(), Some("a,b"));
}

fn test_endpoint() -> Endpoint {
    Endpoint::Loopback(LoopbackEndpoint::new("9199").with_context("test"))
}

fn spaced_endpoint() -> Endpoint {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("k", "a b");
    Endpoint::Loopback(
        LoopbackEndpoint::new("9199")
            .with_context("test")
            .with_variables(vars),
    )
}

fn separated_commands() -> Vec<(Originate, &'static str)> {
    vec![
        (
            Originate::application(
                spaced_endpoint(),
                Application::new("socket", Some("127.0.0.1:8040 async full")),
            )
            .cid_name("it's ~ here")
            .with_argv_separator('~')
            .unwrap(),
            r"originate ^^~{k=\'a b\'}loopback/9199/test~&socket(127.0.0.1:8040 async full)~undef~undef~it\'s \~ here",
        ),
        (
            Originate::extension(test_endpoint(), "1000")
                .context("")
                .cid_name(" Lead ")
                .cid_num("")
                .with_argv_separator('~')
                .unwrap(),
            r"originate ^^~loopback/9199/test~1000~undef~~\sLead\s~''",
        ),
        (
            Originate::application(test_endpoint(), Application::simple("park"))
                .timeout(Duration::from_secs(30))
                .with_argv_separator('~')
                .unwrap(),
            "originate ^^~loopback/9199/test~&park()~undef~undef~undef~undef~30",
        ),
        (
            Originate::inline(
                null_endpoint(),
                [
                    Application::new("playback", Some("tone_stream://%(500,0,800)")),
                    Application::simple("park"),
                ],
            )
            .unwrap()
            .dialplan(DialplanType::INLINE)
            .unwrap()
            .with_argv_separator('~')
            .unwrap(),
            r"originate ^^~loopback/9199~playback:tone_stream://%(500\\,0\\,800),park~inline",
        ),
        (
            Originate::extension(test_endpoint(), "1000")
                .dialplan(DialplanType::XML)
                .unwrap()
                .context("ctx with space")
                .cid_num("555 1234")
                .with_argv_separator('!')
                .unwrap(),
            "originate ^^!loopback/9199/test!1000!XML!ctx with space!undef!555 1234",
        ),
    ]
}

/// Each argument is escaped once for the split on the separator, an absent slot a later
/// one forces reads `undef`, and an empty value stays an argument.
#[test]
fn argv_separator_renders_each_argument_escaped_once() {
    for (cmd, wire) in separated_commands() {
        assert_eq!(cmd.to_string(), wire);
        assert_eq!(
            cmd.display_with(BlockParse::PairSplitCleans)
                .to_string(),
            wire
        );
    }
}

#[test]
fn argv_separator_round_trips() {
    for (cmd, wire) in separated_commands() {
        let parsed: Originate = wire
            .parse()
            .unwrap_or_else(|e| panic!("{wire} failed to parse: {e}"));
        assert_eq!(parsed, cmd, "{wire}");
        assert_eq!(parsed.to_string(), wire);
    }
}

#[test]
fn argv_separator_is_absent_by_default() {
    assert_eq!(
        Originate::extension(test_endpoint(), "1000").argv_separator(),
        None
    );
    assert_eq!(
        "originate loopback/9199/test 1000"
            .parse::<Originate>()
            .unwrap()
            .argv_separator(),
        None
    );
}

/// Measured: `undef` in any case and quoting reads as absent, `\undef` keeps its
/// backslash, an empty token is an empty value.
#[test]
fn argv_separator_parse_reads_the_switch_spellings() {
    let parsed: Originate = r"originate ^^~loopback/9199/test~&park()~XML~~'UNDEF'~\undef"
        .parse()
        .unwrap();
    assert_eq!(parsed.argv_separator(), Some('~'));
    assert_eq!(parsed.dialplan_type(), Some(&DialplanType::XML));
    assert_eq!(parsed.context_str(), Some(""));
    assert_eq!(parsed.caller_id_name(), None);
    assert_eq!(parsed.caller_id_number(), Some(r"\undef"));

    let parsed: Originate =
        r"originate ^^~loopback/9199/test~&park()~undef~undef~  Lead Trail  ~\s\sx"
            .parse()
            .unwrap();
    assert_eq!(parsed.dialplan_type(), None);
    assert_eq!(parsed.context_str(), None);
    assert_eq!(parsed.caller_id_name(), Some("Lead Trail"));
    assert_eq!(parsed.caller_id_number(), Some("  x"));
}

#[test]
fn argv_separator_parse_takes_the_endpoint_at_the_separator_target() {
    let parsed: Originate = r"originate ^^~{k=\'a b\'}loopback/9199/test~&park()"
        .parse()
        .unwrap();
    assert_eq!(parsed.endpoint(), &spaced_endpoint());
}

#[test]
fn an_unusable_argv_separator_is_refused() {
    let err = Originate::application(test_endpoint(), Application::simple("park"))
        .with_argv_separator('\'')
        .unwrap_err();
    assert_eq!(
        err,
        OriginateError::InvalidArgvSeparator(InvalidArgvSeparator::Unusable('\''))
    );
    assert!(std::error::Error::source(&err).is_some());

    assert_eq!(
        "originate ^^|loopback/9199/test|&park()"
            .parse::<Originate>()
            .unwrap_err(),
        OriginateError::InvalidArgvSeparator(InvalidArgvSeparator::Unusable('|'))
    );
}

/// `originate_function` answers usage past seven arguments.
#[test]
fn argv_separator_parse_refuses_what_the_switch_refuses() {
    assert!(
        "originate ^^~loopback/9199/test~&park()~XML~default~a~b~30~extra"
            .parse::<Originate>()
            .is_err()
    );
}

#[test]
fn serde_argv_separator_round_trips_and_is_checked() {
    let cmd = Originate::application(test_endpoint(), Application::simple("park"))
        .with_argv_separator('~')
        .unwrap();
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains(r#""argv_separator":"~""#), "{json}");
    assert_eq!(serde_json::from_str::<Originate>(&json).unwrap(), cmd);

    let plain = serde_json::to_string(&Originate::application(
        test_endpoint(),
        Application::simple("park"),
    ))
    .unwrap();
    assert!(!plain.contains("argv_separator"), "{plain}");

    let refused = r#"{
            "endpoint": {"loopback": {"extension": "9199"}},
            "application": {"name": "park"},
            "argv_separator": "|"
        }"#;
    assert!(serde_json::from_str::<Originate>(refused).is_err());
}

/// `originate_function` reads `undef` in any case as an absent argument, so no spelling
/// delivers it; a config naming one fails at load.
#[test]
fn serde_refuses_an_undef_positional() {
    for (field, value) in [
        ("context", "undef"),
        ("cid_name", "UNDEF"),
        ("cid_num", "Undef"),
    ] {
        let json = format!(
            r#"{{"endpoint": {{"loopback": {{"extension": "9199"}}}},
                "application": {{"name": "park"}}, "{field}": "{value}"}}"#
        );
        let msg = serde_json::from_str::<Originate>(&json)
            .expect_err(&json)
            .to_string();
        assert!(msg.contains(field), "does not name {field}: {msg}");
        assert!(
            value == "undef" || !msg.contains(value),
            "quoted its input: {msg}"
        );
    }
    let json = r#"{"endpoint": {"loopback": {"extension": "9199"}},
            "application": {"name": "park"}, "cid_name": "undefined"}"#;
    assert!(serde_json::from_str::<Originate>(json).is_ok());
}

/// `originate_function` NULLs an `undef` target and then asserts it is set, which aborts
/// the switch.
#[test]
fn an_undef_target_is_refused() {
    for line in [
        "originate loopback/9199/test undef",
        "originate loopback/9199/test 'UNDEF'",
        "originate loopback/9199/test Undef inline",
        "originate ^^~loopback/9199/test~undef",
        "originate ^^~loopback/9199/test~UNDEF~inline",
    ] {
        assert_eq!(
            line.parse::<Originate>(),
            Err(OriginateError::UndefPositional("target")),
            "{line}"
        );
    }
    for target in [
        r#""extension": "undef""#,
        r#""extension": "UNDEF", "argv_separator": "~""#,
        r#""inline_applications": [{"name": "Undef"}]"#,
    ] {
        let json = format!(r#"{{"endpoint": {{"loopback": {{"extension": "9199"}}}}, {target}}}"#);
        let msg = serde_json::from_str::<Originate>(&json)
            .expect_err(&json)
            .to_string();
        assert!(msg.contains("target"), "does not name the target: {msg}");
    }
    assert!(Originate::extension(test_endpoint(), "undefined")
        .to_string()
        .parse::<Originate>()
        .is_ok());
}

/// The blank split reads its positionals by the same rules as a separator split: the
/// third argument is strictly the dialplan, `undef` is absent, and an eighth is refused.
#[test]
fn blank_positionals_read_as_the_separator_split_does() {
    assert!(
        "originate loopback/9199/test &park() XML default a b 30 extra"
            .parse::<Originate>()
            .is_err()
    );

    let parsed: Originate = "originate loopback/9199/test 1000 ctx"
        .parse()
        .unwrap();
    assert_eq!(
        parsed
            .dialplan_type()
            .map(DialplanType::name),
        Some("ctx")
    );
    assert_eq!(parsed.context_str(), None);

    let parsed: Originate = "originate loopback/9199/test &park() undef UNDEF Alice"
        .parse()
        .unwrap();
    assert_eq!(parsed.dialplan_type(), None);
    assert_eq!(parsed.context_str(), None);
    assert_eq!(parsed.caller_id_name(), Some("Alice"));

    let parsed: Originate = "originate loopback/9199/test &park() undef ctx"
        .parse()
        .unwrap();
    assert_eq!(parsed.dialplan_type(), None);
    assert_eq!(parsed.context_str(), Some("ctx"));
}

/// The blank split collapses a run of spaces and splits a bare space, so an empty or
/// spaced positional is quoted to arrive as one argument.
#[test]
fn blank_split_quotes_empty_and_spaced_positionals() {
    let cases = [
        (
            Originate::extension(test_endpoint(), "1000")
                .context("")
                .cid_name("Alice"),
            "originate loopback/9199/test 1000 undef '' Alice",
        ),
        (
            Originate::extension(test_endpoint(), "1000")
                .context("ctx with space")
                .cid_name("Alice"),
            "originate loopback/9199/test 1000 undef 'ctx with space' Alice",
        ),
        (
            Originate::extension(test_endpoint(), "1000")
                .context("test")
                .cid_name("Alice")
                .cid_num(""),
            "originate loopback/9199/test 1000 undef test Alice ''",
        ),
    ];
    for (cmd, wire) in cases {
        assert_eq!(cmd.to_string(), wire);
        let parsed: Originate = wire
            .parse()
            .unwrap_or_else(|e| panic!("{wire} failed to parse: {e}"));
        assert_eq!(parsed.context_str(), cmd.context_str(), "{wire}");
        assert_eq!(parsed.caller_id_name(), cmd.caller_id_name(), "{wire}");
        assert_eq!(parsed.caller_id_number(), cmd.caller_id_number(), "{wire}");
        assert_eq!(parsed.to_string(), wire);
    }
}

/// A lone quote on the blank split is wrapped and escaped, so the split closes the region
/// it opens and its cleanup delivers the quote.
#[test]
fn blank_split_delivers_a_single_quote_in_any_argument() {
    let cases = [
        (
            Originate::application(test_endpoint(), Application::simple("park"))
                .dialplan(DialplanType::XML)
                .unwrap()
                .context("default")
                .cid_name("it's"),
            r"originate loopback/9199/test &park() XML default 'it\'s'",
        ),
        (
            Originate::application(
                test_endpoint(),
                Application::new("set", Some(r"quoted=it's\n")),
            ),
            r"originate loopback/9199/test '&set(quoted=it\'s\\n)'",
        ),
        (
            Originate::inline(null_endpoint(), [Application::new("set", Some("a=it's,b"))])
                .unwrap()
                .dialplan(DialplanType::INLINE)
                .unwrap(),
            r"originate loopback/9199 'set:a=it\\\'s\\,b' inline",
        ),
        (
            Originate::application(
                test_endpoint(),
                Application::new("set", Some(r"lit=a\nb\\c\sd")),
            )
            .cid_name(r"x\ny")
            .cid_num(r"\t"),
            r"originate loopback/9199/test '&set(lit=a\\nb\\\\c\\sd)' undef undef 'x\\ny' '\\t'",
        ),
    ];
    for (cmd, wire) in cases {
        assert_eq!(cmd.to_string(), wire);
        let parsed: Originate = wire
            .parse()
            .unwrap_or_else(|e| panic!("{wire} failed to parse: {e}"));
        assert_eq!(parsed, cmd, "{wire}");
    }

    let parsed: Originate = r"originate loopback/9199/test &park() XML default it\'s"
        .parse()
        .unwrap();
    assert_eq!(parsed.caller_id_name(), Some("it's"));
}

/// The hunt's split cleans each action up, reading `\\`, `\'`, `\n`, `\s` and the rest and
/// trimming a space at the action's end, so an argument escapes for it once.
#[test]
fn an_inline_argument_escapes_for_the_hunt_split() {
    let cases = [
        (
            Some("v= "),
            None,
            r"originate loopback/9199 'set:v=\\s,park' inline",
        ),
        (
            Some(r"v=a\nb"),
            None,
            r"originate loopback/9199 'set:v=a\\\\nb,park' inline",
        ),
        (
            Some("v=a\tb "),
            Some('~'),
            r"originate ^^~loopback/9199~set:v=a\\tb\\s,park~inline",
        ),
    ];
    for (args, sep, wire) in cases {
        let cmd = Originate::inline(
            null_endpoint(),
            [Application::new("set", args), Application::park()],
        )
        .unwrap();
        let cmd = match sep {
            Some(sep) => cmd
                .with_argv_separator(sep)
                .unwrap(),
            None => cmd,
        };
        assert_eq!(cmd.to_string(), wire);
        let parsed: Originate = wire
            .parse()
            .unwrap_or_else(|e| panic!("{wire:?} failed to parse: {e}"));
        assert_eq!(parsed.target(), cmd.target(), "{wire:?}");
    }
}

/// `originate_function` hands the target to the inline hunt only under the inline dialplan;
/// under any other it transfers the action list as an extension.
#[test]
fn inline_applications_refuse_another_dialplan() {
    let inline = || Originate::inline(null_endpoint(), [Application::simple("park")]).unwrap();
    for refused in [
        inline().dialplan(DialplanType::XML),
        inline().dialplan(DialplanType::named("").unwrap()),
        inline().dialplan(DialplanType::named("nosuchdp").unwrap()),
    ] {
        assert_eq!(refused, Err(OriginateError::InlineApplicationsWithDialplan));
    }
    assert!(inline()
        .dialplan(DialplanType::INLINE)
        .is_ok());
    assert!(inline()
        .dialplan(DialplanType::named("INLINE").unwrap())
        .is_ok());
    let json = r#"{"endpoint": {"loopback": {"extension": "9199"}},
            "inline_applications": [{"name": "park"}], "dialplan": "xml"}"#;
    assert_eq!(
        serde_json::from_str::<Originate>(json)
            .unwrap_err()
            .to_string(),
        OriginateError::InlineApplicationsWithDialplan.to_string()
    );
}

/// `originate_function` runs a target opening `&` and more as an application and ends its
/// arguments at the first `)`.
#[test]
fn a_target_originate_reads_as_something_else_is_refused() {
    assert_eq!(
        "originate loopback/9199/test &park(a)b)".parse::<Originate>(),
        Err(OriginateError::ParenthesisInApplication {
            application: "park".into()
        })
    );
    for target in [
        r#""application": {"name": "park", "args": "a)b"}"#,
        r#""application": {"name": "pa(rk"}"#,
        r#""extension": "&park()""#,
        r#""extension": "&x""#,
    ] {
        let json = format!(r#"{{"endpoint": {{"loopback": {{"extension": "9199"}}}}, {target}}}"#);
        let msg = serde_json::from_str::<Originate>(&json)
            .expect_err(&json)
            .to_string();
        assert!(!msg.contains("park") && !msg.contains("a)b"), "{msg}");
    }
    let json = r#"{"endpoint": {"loopback": {"extension": "9199"}}, "extension": "&"}"#;
    assert!(serde_json::from_str::<Originate>(json).is_ok());
}

/// An inline action list reads an argument up to its separator, not its first `)`, so the
/// refusal names the form that delivers a tone stream's parenthesised spec.
#[test]
fn a_parenthesis_refusal_points_at_the_inline_action_list() {
    let json = r#"{"endpoint": {"loopback": {"extension": "9199"}},
            "application": {"name": "playback", "args": "tone_stream://%(500,0,800)"}}"#;
    let msg = serde_json::from_str::<Originate>(json)
        .expect_err(json)
        .to_string();
    assert!(msg.contains("Originate::inline"), "{msg}");

    let inline = r#"{"endpoint": {"loopback": {"extension": "9199"}},
            "inline_applications": [{"name": "playback", "args": "tone_stream://%(500,0,800)"}]}"#;
    let originate = serde_json::from_str::<Originate>(inline).expect(inline);
    assert_eq!(
        originate.to_string(),
        r"originate loopback/9199 'playback:tone_stream://%(500\\,0\\,800)' inline"
    );
}

#[test]
fn an_application_under_the_inline_dialplan_and_a_lone_ampersand_round_trip() {
    let cases = [
        (
            Originate::application(test_endpoint(), Application::new("park", Some(":,")))
                .dialplan(DialplanType::INLINE)
                .unwrap(),
            "originate loopback/9199/test &park(:,) inline",
        ),
        (
            Originate::extension(test_endpoint(), "&"),
            "originate loopback/9199/test &",
        ),
    ];
    for (cmd, wire) in cases {
        assert_eq!(cmd.to_string(), wire);
        let parsed: Originate = wire
            .parse()
            .unwrap_or_else(|e| panic!("{wire:?} failed to parse: {e}"));
        assert_eq!(parsed, cmd, "{wire:?}");
    }
}

/// `switch_api_execute` strips tab, vertical tab, CR, newline and space from the edges of
/// the argument line, so a last positional ending in one is kept inside quotes.
#[test]
fn a_last_positional_ending_in_stripped_whitespace_arrives() {
    let cases = [
        (
            Originate::extension(test_endpoint(), "1000").cid_num("\t"),
            "originate loopback/9199/test 1000 undef undef undef '\t'",
        ),
        (
            Originate::extension(test_endpoint(), "1000").cid_num("x\u{b}"),
            "originate loopback/9199/test 1000 undef undef undef 'x\u{b}'",
        ),
        (
            Originate::extension(test_endpoint(), "1000")
                .cid_num("\u{b}")
                .with_argv_separator('~')
                .unwrap(),
            "originate ^^~loopback/9199/test~1000~undef~undef~undef~''\u{b}''",
        ),
    ];
    for (cmd, wire) in cases {
        assert_eq!(cmd.to_string(), wire);
        let parsed: Originate = wire
            .parse()
            .unwrap_or_else(|e| panic!("{wire:?} failed to parse: {e}"));
        assert_eq!(parsed, cmd, "{wire:?}");
    }
}

/// `originate_function` hands any word in the dialplan slot to the transfer, which looks
/// up a dialplan module by that name.
#[test]
fn any_dialplan_name_is_kept() {
    for (line, name) in [
        ("originate loopback/9199/test 1000 nosuchdp ctx", "nosuchdp"),
        (
            "originate ^^~loopback/9199/test~1000~nosuchdp~ctx",
            "nosuchdp",
        ),
        ("originate loopback/9199/test 1000 '' ctx", ""),
    ] {
        let parsed: Originate = line
            .parse()
            .unwrap_or_else(|e| panic!("{line}: {e}"));
        assert_eq!(
            parsed.dialplan_type(),
            Some(&DialplanType::named(name).unwrap()),
            "{line}"
        );
        assert_eq!(parsed.context_str(), Some("ctx"), "{line}");
        assert_eq!(parsed.to_string(), line);
    }

    let typed: Originate = "originate loopback/9199/test 1000 xml ctx"
        .parse()
        .unwrap();
    assert_eq!(typed.dialplan_type(), Some(&DialplanType::XML));
    assert_eq!(DialplanType::XML.name(), "XML");

    let named = Originate::extension(test_endpoint(), "1000")
        .dialplan(DialplanType::named("nosuchdp").unwrap())
        .unwrap();
    assert_eq!(
        named.to_string(),
        "originate loopback/9199/test 1000 nosuchdp"
    );
    assert_eq!(
        Originate::extension(test_endpoint(), "1000")
            .dialplan(DialplanType::named("INLINE").unwrap()),
        Err(OriginateError::ExtensionWithInlineDialplan)
    );
    let json = serde_json::to_string(&named).unwrap();
    assert!(json.contains(r#""dialplan":"nosuchdp""#), "{json}");
    assert_eq!(serde_json::from_str::<Originate>(&json).unwrap(), named);
}

#[test]
fn serde_dialplan_keeps_existing_configs_and_refuses_undef() {
    let config = |dialplan: &str| {
        format!(
            r#"{{"endpoint": {{"loopback": {{"extension": "9199"}}}},
                "extension": "1000", "dialplan": "{dialplan}"}}"#
        )
    };
    for (dialplan, typed) in [("xml", DialplanType::XML), ("XML", DialplanType::XML)] {
        let cmd: Originate = serde_json::from_str(&config(dialplan)).unwrap();
        assert_eq!(cmd.dialplan_type(), Some(&typed));
        assert!(serde_json::to_string(&cmd)
            .unwrap()
            .contains(r#""dialplan":"xml""#));
    }
    let msg = serde_json::from_str::<Originate>(&config("Undef"))
        .expect_err("undef dialplan")
        .to_string();
    assert!(msg.contains("dialplan"), "{msg}");
}

/// `switch_separate_string` takes `^^ ` as picking the blank split itself.
#[test]
fn a_blank_argv_separator_picks_the_blank_split() {
    let parsed: Originate = "originate ^^ loopback/9199/test &park() XML ctx"
        .parse()
        .unwrap();
    assert_eq!(parsed.argv_separator(), None);
    assert_eq!(parsed.endpoint(), &test_endpoint());
    assert_eq!(parsed.context_str(), Some("ctx"));
    assert_eq!(
        parsed.to_string(),
        "originate loopback/9199/test &park() XML ctx"
    );
}

/// The switch splits any dial string holding `:_:` into enterprise threads, quoted or not.
#[test]
fn an_enterprise_separator_in_a_variable_value_is_refused() {
    assert!("originate {k=x:_:y}loopback/9199/test &park()"
        .parse::<Originate>()
        .is_err());
}
