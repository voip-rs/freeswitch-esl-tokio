use super::*;

/// `originate_function` runs `&name(args)` before it reads the dialplan, and takes a lone
/// `&` as an extension.
#[test]
fn parse_target_reads_an_application_before_the_dialplan() {
    for dialplan in [None, Some(&DialplanType::INLINE), Some(&DialplanType::XML)] {
        let target = parse_originate_target("&park(:,)", dialplan).unwrap();
        assert_eq!(
            target,
            OriginateTarget::Application(Application::new("park", Some(":,"))),
            "{dialplan:?}"
        );
    }
    assert_eq!(
        parse_originate_target("&", None).unwrap(),
        OriginateTarget::Extension("&".into())
    );
}

#[test]
fn parse_target_bare_extension() {
    let target = parse_originate_target("123", None).unwrap();
    assert!(matches!(target, OriginateTarget::Extension(ref e) if e == "123"));
}

#[test]
fn parse_target_xml_no_args() {
    let target = parse_originate_target("&conference()", None).unwrap();
    if let OriginateTarget::Application(app) = target {
        assert_eq!(app.name(), "conference");
        assert!(app
            .args()
            .is_none());
    } else {
        panic!("expected Application");
    }
}

#[test]
fn parse_target_xml_with_args() {
    let target = parse_originate_target("&conference(1)", None).unwrap();
    if let OriginateTarget::Application(app) = target {
        assert_eq!(app.name(), "conference");
        assert_eq!(app.args(), Some("1"));
    } else {
        panic!("expected Application");
    }
}

#[test]
fn parse_target_two_inline_apps() {
    let target = parse_originate_target(
        "conference:1,hangup:NORMAL_CLEARING",
        Some(&DialplanType::INLINE),
    )
    .unwrap();
    if let OriginateTarget::InlineApplications(apps) = target {
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].name(), "conference");
        assert_eq!(apps[0].args(), Some("1"));
        assert_eq!(apps[1].name(), "hangup");
        assert_eq!(apps[1].args(), Some("NORMAL_CLEARING"));
    } else {
        panic!("expected InlineApplications");
    }
}

#[test]
fn parse_target_inline_bare_name() {
    let target = parse_originate_target("hangup", Some(&DialplanType::INLINE)).unwrap();
    if let OriginateTarget::InlineApplications(apps) = target {
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name(), "hangup");
        assert!(apps[0]
            .args()
            .is_none());
    } else {
        panic!("expected InlineApplications");
    }
}

#[test]
fn parse_target_inline_mixed_bare_and_args() {
    let target =
        parse_originate_target("park,hangup:NORMAL_CLEARING", Some(&DialplanType::INLINE)).unwrap();
    if let OriginateTarget::InlineApplications(apps) = target {
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].name(), "park");
        assert!(apps[0]
            .args()
            .is_none());
        assert_eq!(apps[1].name(), "hangup");
        assert_eq!(apps[1].args(), Some("NORMAL_CLEARING"));
    } else {
        panic!("expected InlineApplications");
    }
}

#[test]
fn parse_target_inline_trailing_colon_collapses_to_none() {
    let target = parse_originate_target("park:", Some(&DialplanType::INLINE)).unwrap();
    if let OriginateTarget::InlineApplications(apps) = target {
        assert!(apps[0]
            .args()
            .is_none());
    } else {
        panic!("expected InlineApplications");
    }
}
