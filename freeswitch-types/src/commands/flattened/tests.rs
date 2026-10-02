use super::*;
use crate::commands::variables::{DialStringCarrier, DialStringTarget};
use crate::switch_passes::originate_legs::MAX_PEERS;
use crate::variables::ChannelVariable;

const API: DialStringCarrier = DialStringCarrier::EslApi;
const DIALPLAN: DialStringCarrier = DialStringCarrier::Dialplan;

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("../../../tests/fixtures/flattened/", $name, ".txt"))
    };
}

fn parse(input: &str, carrier: DialStringCarrier) -> FlattenedDialString {
    FlattenedDialString::parse_for(input, carrier)
        .unwrap_or_else(|e| panic!("{input:?} at {carrier:?}: {e:?}"))
}

fn presence(leg: &FlattenedLeg) -> Option<&str> {
    leg.variable(ChannelVariable::PresenceId)
}

fn is_error(leg: &FlattenedLeg) -> bool {
    matches!(leg.target(), LegTarget::Error(_))
}

#[test]
fn raw_render_is_the_input_for_every_fixture() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/flattened");
    let mut parsed = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry
            .unwrap()
            .path();
        if path
            .extension()
            .is_none_or(|ext| ext != "txt")
        {
            continue;
        }
        let input = std::fs::read_to_string(&path).unwrap();
        for carrier in [API, DIALPLAN] {
            if let Ok(list) = FlattenedDialString::parse_for(&input, carrier) {
                parsed += 1;
                assert_eq!(
                    list.display_raw()
                        .to_string(),
                    input,
                    "{path:?} at {carrier:?}"
                );
            }
        }
    }
    assert!(parsed > 100, "{parsed}");
}

#[test]
fn plain_fixtures_render_back_to_the_input() {
    for input in [
        fixture!("g-fp-static.A"),
        fixture!("g-fp-reg.A"),
        fixture!("pbx-calltakers.A"),
    ] {
        assert_eq!(
            parse(input, API)
                .display_for(API)
                .to_string(),
            input
        );
    }
}

#[test]
fn head_blocks_render_ahead_and_only_set_pairs_are_carried() {
    let list = parse(
        "<e=1>{g=2}[k,a=1]loopback/9199/test:_:{h=3}loopback/9199/x|[b=2]null/a",
        API,
    );
    assert_eq!(
        list.display_for(API)
            .to_string(),
        "<e=1>{g=2}[a=1]loopback/9199/test:_:{h=3}loopback/9199/x|[b=2]null/a"
    );
}

#[test]
fn dropping_error_legs_rejoins_the_rest() {
    for input in [fixture!("flattened-probe.A"), fixture!("pbx-calltakers.A")] {
        let mut list = parse(input, API);
        assert!(list
            .legs()
            .any(is_error));
        list.retain(|leg| !is_error(leg));
        let want = input
            .split(',')
            .filter(|leg| !leg.contains("]error/"))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            list.display_raw()
                .to_string(),
            want
        );
        assert!(!list
            .legs()
            .any(is_error));
    }
}

#[test]
fn a_leg_ending_in_an_escaped_char_keeps_it_after_retain() {
    let cases = [
        (API, r"loopback/9199/test\\s"),
        (API, r"loopback/9199/test\\\\\\\\"),
        (DIALPLAN, r"loopback/9199/test\\\\\\\\"),
        (DIALPLAN, r"loopback/9199/test\s"),
    ];
    for (carrier, kept) in cases {
        let input = format!("{kept},error/USER_BUSY");
        let mut list = parse(&input, carrier);
        list.retain(|leg| !is_error(leg));
        assert_eq!(
            list.display_raw()
                .to_string(),
            kept,
            "{input:?} at {carrier:?}"
        );
    }
}

#[test]
fn a_dropped_quote_or_escape_stays_with_its_leg_after_retain() {
    let cases = [
        (
            API,
            "loopback/9199/a,'loopback/9199/b c',error/USER_BUSY",
            "loopback/9199/a,'loopback/9199/b c'",
        ),
        (
            API,
            "error/USER_BUSY,'loopback/9199/b c',loopback/9199/a",
            "'loopback/9199/b c',loopback/9199/a",
        ),
        (
            DIALPLAN,
            "[v='x y']loopback/9199/a,error/USER_BUSY",
            "[v='x y']loopback/9199/a",
        ),
        (
            DIALPLAN,
            "error/USER_BUSY,[v='x,y']loopback/9199/a",
            "[v='x,y']loopback/9199/a",
        ),
        (
            DIALPLAN,
            "'loopback/9199/a b',error/USER_BUSY",
            "'loopback/9199/a b'",
        ),
        (
            DIALPLAN,
            r"loopback/9199/a\',error/USER_BUSY",
            r"loopback/9199/a\'",
        ),
    ];
    for (carrier, input, kept) in cases {
        let mut list = parse(input, carrier);
        assert_eq!(
            list.display_raw()
                .to_string(),
            input
        );
        list.retain(|leg| !is_error(leg));
        assert_eq!(
            list.display_raw()
                .to_string(),
            kept,
            "{input:?} at {carrier:?}"
        );
    }
}

#[test]
fn a_non_ascii_block_separator_warns_and_carries_nothing() {
    let input = "[a=1][^^ék=secretév=2]loopback/9199/test";
    let list = parse(input, API);
    let leg = list
        .legs()
        .next()
        .unwrap();
    assert_eq!(
        leg.warnings(),
        [LegWarning::BlockSeparatorUnreadable { block: 1 }]
    );
    assert_eq!(
        list.display_for(API)
            .to_string(),
        "[a=1]loopback/9199/test"
    );
    let shown = leg.warnings()[0].to_string();
    assert!(shown.contains('1') && !shown.contains("secret") && !shown.contains('é'));
    assert_eq!(
        list.display_raw()
            .to_string(),
        input
    );
}

#[test]
fn a_block_rewriting_the_text_after_it_warns() {
    let list = parse("[a=1][^^]k=secret", API);
    let leg = list
        .legs()
        .next()
        .unwrap();
    assert_eq!(
        leg.warnings(),
        [LegWarning::BlockRewritesFollowingText { block: 1 }]
    );
    assert_eq!(leg.variable(Key("k")), Some("secret"));
    let shown = leg.warnings()[0].to_string();
    assert!(shown.contains('1') && !shown.contains("secret"), "{shown}");

    let list = parse(r"[a=1][k=secret\]loopback/9199/test", DIALPLAN);
    let leg = list
        .legs()
        .next()
        .unwrap();
    assert_eq!(leg.warnings(), []);
    assert_eq!(leg.variable(Key("k")), Some(r"secret\"));

    let list = parse("[a=1][ ^^ék=secret]loopback/9199/test", DIALPLAN);
    let leg = list
        .legs()
        .next()
        .unwrap();
    assert_eq!(leg.warnings(), [LegWarning::PairUnreadable { block: 1 }]);
    let shown = leg.warnings()[0].to_string();
    assert!(!shown.contains("secret") && !shown.contains('é'), "{shown}");

    for carrier in [API, DIALPLAN] {
        let list = parse("<^^>k=secret", carrier);
        assert_eq!(
            list.warnings(),
            [ListWarning::BlockRewritesFollowingText { block: 0 }]
        );
        let shown = list.warnings()[0].to_string();
        assert!(shown.contains('0') && !shown.contains("secret"), "{shown}");
    }
}

/// The switch splits on the first byte of a non-ASCII `^^` separator, which no char
/// delimiter mirrors, so the blank split runs over the whole argument.
#[test]
fn a_non_ascii_argument_separator_is_not_taken() {
    assert_eq!(
        FlattenedDialString::parse_for("^^é{v=a b}loopback/9199/test", API),
        Err(FlattenedDialStringError::ArgvSplit)
    );
}

struct Key(&'static str);

impl VariableName for Key {
    fn as_str(&self) -> &str {
        self.0
    }
}

fn argv_targets() -> [DialStringTarget; 2] {
    [
        DialStringTarget::new(API)
            .with_argv_separator('~')
            .expect("'~' separates originate's arguments"),
        DialStringTarget::new(API).with_unchecked_argv_separator('|'),
    ]
}

/// A capture's name, its body, and the values its first channel received.
type Capture = (
    &'static str,
    &'static str,
    &'static [(&'static str, Option<&'static str>)],
);

/// Values each capture's channel reported when originated under `^^~` and `^^|`.
#[test]
fn argv_separator_captures_read_as_their_channel_received_them() {
    let cases: &[Capture] = &[
        (
            "apos2",
            fixture!("g-fp-argv-apos2.A"),
            &[("v", Some("its ok")), ("sentinel", Some("s"))],
        ),
        (
            "bsbs",
            fixture!("g-fp-argv-bsbs.A"),
            &[("v", Some(r"a\b")), ("sentinel", Some("s"))],
        ),
        (
            "bsn",
            fixture!("g-fp-argv-bsn.A"),
            &[("v", Some("a\nb")), ("sentinel", Some("s"))],
        ),
        (
            "bsq",
            fixture!("g-fp-argv-bsq.A"),
            &[("v", Some("ab")), ("sentinel", Some("s"))],
        ),
        (
            "bss",
            fixture!("g-fp-argv-bss.A"),
            &[("v", Some("a b")), ("sentinel", Some("s"))],
        ),
        (
            "bstilde",
            fixture!("g-fp-argv-bstilde.A"),
            &[("v", Some(r"a\~b")), ("sentinel", Some("s"))],
        ),
        (
            "caret",
            fixture!("g-fp-argv-caret.A"),
            &[
                ("v", None),
                ("sentinel", Some("s")),
                ("k", Some("a")),
                ("j", Some("b")),
            ],
        ),
        (
            "lastesc",
            fixture!("g-fp-argv-lastesc.A"),
            &[("v", Some(r"x\~")), ("sentinel", Some("s"))],
        ),
        (
            "leg2",
            fixture!("g-fp-argv-leg2.A"),
            &[("v", Some(r"a b~c\d")), ("sentinel", Some("s"))],
        ),
        (
            "space",
            fixture!("g-fp-argv-space.A"),
            &[("v", Some("a b")), ("sentinel", Some("s"))],
        ),
        (
            "tilde",
            fixture!("g-fp-argv-tilde.A"),
            &[("v", Some("x~y")), ("sentinel", Some("s"))],
        ),
        (
            "trail-last",
            fixture!("g-fp-argv-trail-last.A"),
            &[("v", Some(r"end\")), ("sentinel", Some("s"))],
        ),
        (
            "trail-mid",
            fixture!("g-fp-argv-trail-mid.A"),
            &[("v", Some("end,sentinel=s")), ("sentinel", None)],
        ),
    ];
    for target in argv_targets() {
        for &(name, body, values) in cases {
            let escaped = target
                .escape_argument(body)
                .expect("a separator target escapes");
            let context = format!("{name} at {target:?}: {escaped}");
            let list = FlattenedDialString::parse_for(&escaped, target)
                .unwrap_or_else(|e| panic!("{context}: {e:?}"));
            assert_eq!(
                list.display_raw()
                    .to_string(),
                escaped,
                "{context}"
            );
            let rendered = list
                .display_for(target)
                .to_string();
            let reread = FlattenedDialString::parse_for(&rendered, target)
                .unwrap_or_else(|e| panic!("{context} rendered {rendered}: {e:?}"));
            for list in [&list, &reread] {
                let leg = list
                    .legs()
                    .next()
                    .expect("a capture has a leg");
                let presence = format!("fp-argv-{name}@pbx.example.com");
                assert_eq!(
                    leg.variable(ChannelVariable::PresenceId),
                    Some(presence.as_str()),
                    "{context}"
                );
                for &(key, want) in values {
                    assert_eq!(leg.variable(Key(key)), want, "{key} in {context}");
                }
            }
        }
    }
}

/// The blank split and every argv separator.
fn split_targets() -> [DialStringTarget; 3] {
    let [tilde, pipe] = argv_targets();
    [DialStringTarget::new(API), tilde, pipe]
}

#[test]
fn a_spaced_capture_is_one_argument_once_escaped() {
    for body in [fixture!("g-fp-argv-space.A"), fixture!("g-fp-argv-leg2.A")] {
        assert_eq!(
            FlattenedDialString::parse_for(body, API),
            Err(FlattenedDialStringError::ArgvSplit)
        );
        for target in split_targets() {
            let escaped = target
                .escape_argument(body)
                .expect("a separator target escapes");
            assert!(
                FlattenedDialString::parse_for(&escaped, target).is_ok(),
                "{escaped} at {target:?}"
            );
        }
    }
}

#[test]
fn argv_separator_captures_keep_their_legs() {
    for target in argv_targets() {
        for (body, sentinels) in [
            (fixture!("g-fp-argv-lastesc.A"), &["s", "s2"][..]),
            (fixture!("g-fp-argv-leg2.A"), &["s", "s2"][..]),
            (fixture!("g-fp-argv-epbs.A"), &["s"][..]),
        ] {
            let escaped = target
                .escape_argument(body)
                .expect("a separator target escapes");
            let list = FlattenedDialString::parse_for(&escaped, target)
                .unwrap_or_else(|e| panic!("{escaped} at {target:?}: {e:?}"));
            assert_eq!(
                list.legs()
                    .map(|leg| leg.variable(Key("sentinel")))
                    .collect::<Vec<_>>(),
                sentinels
                    .iter()
                    .map(|&s| Some(s))
                    .collect::<Vec<_>>(),
                "{escaped} at {target:?}"
            );
        }
    }
}

/// What `retain` forwards is still one argument escaped for the same split.
#[test]
fn retain_at_an_argv_separator_forwards_the_kept_legs_escaped() {
    for target in split_targets() {
        for body in [
            fixture!("g-fp-argv-lastesc.A"),
            fixture!("g-fp-argv-leg2.A"),
        ] {
            let (first, second) = body
                .split_once("test,")
                .map(|(first, second)| (format!("{first}test"), second))
                .expect("two legs");
            let escaped = target
                .escape_argument(body)
                .expect("a separator target escapes");
            for (dropped, kept) in [(1, first.as_str()), (0, second)] {
                let mut list = FlattenedDialString::parse_for(&escaped, target)
                    .unwrap_or_else(|e| panic!("{escaped} at {target:?}: {e:?}"));
                let mut index = 0;
                list.retain(|_| {
                    index += 1;
                    index - 1 != dropped
                });
                let forwarded = list
                    .display_raw()
                    .to_string();
                assert_eq!(
                    Some(forwarded.as_str()),
                    target
                        .escape_argument(kept)
                        .as_deref(),
                    "{escaped} without leg {dropped} at {target:?}"
                );
                let reread = FlattenedDialString::parse_for(&forwarded, target)
                    .unwrap_or_else(|e| panic!("{forwarded} at {target:?}: {e:?}"));
                assert_eq!(
                    reread
                        .legs()
                        .count(),
                    1
                );
            }
        }
    }
}

/// A trailing separator adds no argument; any text past one is a second.
#[test]
fn the_argv_separator_split_must_leave_one_argument() {
    let [tilde, _] = argv_targets();
    let list = FlattenedDialString::parse_for("loopback/9199/test~", tilde)
        .expect("a trailing separator adds no argument");
    assert_eq!(
        list.display_raw()
            .to_string(),
        "loopback/9199/test~"
    );
    for input in [
        "loopback/9199/a~loopback/9199/b",
        "loopback/9199/test~~",
        "~loopback/9199/test",
        "[v='x~y']loopback/9199/test",
    ] {
        assert_eq!(
            FlattenedDialString::parse_for(input, tilde),
            Err(FlattenedDialStringError::ArgvSplit),
            "{input}"
        );
    }
}

/// The raw render of `input` with the legs at `dropped` removed.
fn without_legs(input: &str, carrier: DialStringCarrier, dropped: &[usize]) -> String {
    let mut list = parse(input, carrier);
    let mut index = 0;
    list.retain(|_| {
        index += 1;
        !dropped.contains(&(index - 1))
    });
    list.display_raw()
        .to_string()
}

#[test]
fn a_leg_owns_every_byte_between_its_separators() {
    let cases: &[(&str, &[usize], &str)] = &[
        (
            r"loopback/9199/a,\'loopback/9199/b",
            &[0],
            r"\'loopback/9199/b",
        ),
        (
            r"loopback/9199/a,\'loopback/9199/b",
            &[1],
            "loopback/9199/a",
        ),
        (
            "loopback/9199/a,'loopback/9199/b c'",
            &[0],
            "'loopback/9199/b c'",
        ),
        (
            r"loopback/9199/a\',loopback/9199/b",
            &[1],
            r"loopback/9199/a\'",
        ),
        (
            r"loopback/9199/a\\\\\\\\,loopback/9199/b",
            &[1],
            r"loopback/9199/a\\\\\\\\",
        ),
        (
            "'loopback/9199/a b',loopback/9199/b",
            &[1],
            "'loopback/9199/a b'",
        ),
        (
            "loopback/9199/a,'',loopback/9199/b",
            &[1],
            "loopback/9199/a,loopback/9199/b",
        ),
        ("loopback/9199/a,'',loopback/9199/b", &[0, 2], "''"),
        (
            "loopback/9199/a,'',loopback/9199/b",
            &[0],
            "'',loopback/9199/b",
        ),
        (
            "loopback/9199/a,'',loopback/9199/b",
            &[2],
            "loopback/9199/a,''",
        ),
        (" loopback/9199/a,loopback/9199/b", &[0], "loopback/9199/b"),
        (" loopback/9199/a,loopback/9199/b", &[1], " loopback/9199/a"),
        (
            r"loopback/9199/a:_:\'loopback/9199/b",
            &[0],
            r"\'loopback/9199/b",
        ),
    ];
    for &(input, dropped, kept) in cases {
        for carrier in [API, DIALPLAN] {
            assert_eq!(without_legs(input, carrier, &[]), input, "{carrier:?}");
            assert_eq!(
                without_legs(input, carrier, dropped),
                kept,
                "{input:?} without {dropped:?} at {carrier:?}"
            );
        }
    }
}

#[test]
fn a_non_ascii_separator_ahead_of_the_legs_warns_and_carries_nothing() {
    for (input, block, carried) in [
        (
            "{g=1}{^^ék=secretév=2}loopback/9199/test",
            1,
            "{g=1}loopback/9199/test",
        ),
        (
            "<e=1>{g=2}loopback/9199/a:_:<^^ék=secret>loopback/9199/b",
            2,
            "<e=1>{g=2}loopback/9199/a:_:loopback/9199/b",
        ),
    ] {
        for carrier in [API, DIALPLAN] {
            let list = parse(input, carrier);
            assert_eq!(
                list.warnings(),
                [ListWarning::BlockSeparatorUnreadable { block }],
                "{input:?} at {carrier:?}"
            );
            assert_eq!(
                list.display_for(carrier)
                    .to_string(),
                carried
            );
            let shown = list.warnings()[0].to_string();
            assert!(
                shown.contains(&block.to_string())
                    && !shown.contains("secret")
                    && !shown.contains('é'),
                "{shown}"
            );
            assert_eq!(
                list.display_raw()
                    .to_string(),
                input
            );
        }
    }
}

#[test]
fn a_thread_ending_in_an_escaped_quote_keeps_it_after_retain() {
    let kept = r"loopback/9199/test\'";
    let input = format!("{kept}:_:error/USER_BUSY");
    let mut list = parse(&input, API);
    list.retain(|leg| !is_error(leg));
    assert_eq!(
        list.display_raw()
            .to_string(),
        kept
    );
}

#[test]
fn an_edge_leg_takes_its_one_separator() {
    let input = fixture!("g-fp-reg.A");
    let (first, last) = input
        .split_once(',')
        .unwrap();

    let mut list = parse(input, API);
    list.retain(|leg| {
        !leg.raw()
            .contains("gw=fp-reg-b")
    });
    assert_eq!(
        list.display_raw()
            .to_string(),
        last
    );

    let mut list = parse(input, API);
    list.retain(|leg| {
        !leg.raw()
            .contains("gw=fp-reg-a")
    });
    assert_eq!(
        list.display_raw()
            .to_string(),
        first
    );
    assert_eq!(
        list.legs()
            .map(FlattenedLeg::raw)
            .collect::<Vec<_>>(),
        [first]
    );
}

#[test]
fn an_emptied_group_goes_with_its_separator() {
    let input = fixture!("g-fp-pipe.A");
    let (first, rest) = input
        .split_once('|')
        .unwrap();
    let mut list = parse(input, API);
    let mut seen = 0;
    list.retain(|_| {
        seen += 1;
        seen != 1
    });
    assert_eq!(
        list.threads()
            .flat_map(FlattenedThread::groups)
            .count(),
        1
    );
    assert_eq!(
        list.display_raw()
            .to_string(),
        rest
    );

    let mut list = parse(input, API);
    let mut seen = 0;
    list.retain(|_| {
        seen += 1;
        seen != 2
    });
    let (_, last) = rest
        .split_once(',')
        .unwrap();
    assert_eq!(
        list.display_raw()
            .to_string(),
        format!("{first}|{last}")
    );
}

#[test]
fn an_emptied_thread_goes_with_its_separator() {
    let input = fixture!("g-fp-ent.none");
    let (first, rest) = input
        .split_once(":_:")
        .unwrap();

    let mut list = parse(input, API);
    list.retain(|leg| presence(leg) != Some("fp-ent@pbx.example.com"));
    assert_eq!(
        list.threads()
            .count(),
        1
    );
    assert_eq!(
        list.display_raw()
            .to_string(),
        rest
    );

    let mut list = parse(input, API);
    list.retain(|leg| presence(leg) == Some("fp-ent@pbx.example.com"));
    assert_eq!(
        list.display_raw()
            .to_string(),
        first
    );
}

#[test]
fn removing_every_leg_leaves_nothing_to_render() {
    let mut list = parse(fixture!("flattened-probe.A"), API);
    assert!(!list.is_empty());
    list.retain(|_| false);
    assert!(list.is_empty());
    assert_eq!(
        list.legs()
            .count(),
        0
    );
    assert_eq!(
        list.display_raw()
            .to_string(),
        ""
    );
    assert_eq!(
        list.display_for(API)
            .to_string(),
        ""
    );
}

#[test]
fn presence_id_is_read_through_every_scope() {
    let list = parse(fixture!("g-fp-static.A"), API);
    assert_eq!(
        list.legs()
            .map(presence)
            .collect::<Vec<_>>(),
        [Some("fp-static@pbx.example.com")]
    );

    let list = parse(fixture!("g-fp-nodial.A"), API);
    assert_eq!(
        list.legs()
            .map(presence)
            .collect::<Vec<_>>(),
        [None]
    );

    let list = parse(
        "<presence_id=e>loopback/9199/test:_:{presence_id=g}[presence_id=l]null/a",
        API,
    );
    assert_eq!(
        list.legs()
            .map(presence)
            .collect::<Vec<_>>(),
        [Some("e"), Some("g")]
    );
}

#[test]
fn leg_targets_are_typed() {
    let list = parse(fixture!("fp-one-empty.A"), API);
    let targets: Vec<&LegTarget> = list
        .legs()
        .map(FlattenedLeg::target)
        .collect();
    assert!(matches!(
        targets[0],
        LegTarget::Endpoint(Endpoint::Loopback(_))
    ));
    match targets[1] {
        LegTarget::Unparsed(unparsed) => assert_eq!(unparsed.endpoint(), ""),
        other => panic!("{other:?}"),
    }
    assert!(matches!(targets[2], LegTarget::Error(_)));

    let list = parse(fixture!("g-fp-nodial.A"), API);
    assert!(matches!(
        list.legs()
            .next()
            .map(FlattenedLeg::target),
        Some(LegTarget::Endpoint(Endpoint::User(_)))
    ));
}

fn error_leg(list: &FlattenedDialString) -> &ErrorLeg {
    match list
        .legs()
        .next()
        .map(FlattenedLeg::target)
    {
        Some(LegTarget::Error(error)) => error,
        other => panic!("{other:?}"),
    }
}

#[test]
fn every_cause_fixture_is_read_as_the_switch_reads_it() {
    for (input, written, reading, cause) in [
        (
            fixture!("g-fp-err-bogus.A"),
            "NOT_A_CAUSE",
            CauseReading::Unrecognized,
            None,
        ),
        (
            fixture!("g-fp-err-empty.A"),
            "",
            CauseReading::Unrecognized,
            None,
        ),
        (
            fixture!("g-fp-err-lower.A"),
            "user_busy",
            CauseReading::Name(HangupCause::UserBusy),
            Some(HangupCause::UserBusy),
        ),
        (
            fixture!("g-fp-err-upper.A"),
            "USER_BUSY",
            CauseReading::Name(HangupCause::UserBusy),
            Some(HangupCause::UserBusy),
        ),
        (
            fixture!("g-fp-err-num.A"),
            "17",
            CauseReading::Number(17),
            Some(HangupCause::UserBusy),
        ),
        (
            fixture!("g-fp-err-prefix.A"),
            "17abc",
            CauseReading::Number(17),
            Some(HangupCause::UserBusy),
        ),
        (
            fixture!("g-fp-err-zero.A"),
            "0",
            CauseReading::Number(0),
            Some(HangupCause::None),
        ),
        (
            fixture!("g-fp-unreg.A"),
            "user_not_registered",
            CauseReading::Name(HangupCause::UserNotRegistered),
            Some(HangupCause::UserNotRegistered),
        ),
        (
            fixture!("fp-none.A"),
            "NO_ROUTE_DESTINATION",
            CauseReading::Name(HangupCause::NoRouteDestination),
            Some(HangupCause::NoRouteDestination),
        ),
        ("error/70000", "70000", CauseReading::Number(70000), None),
        ("error/4", "4", CauseReading::Number(4), None),
    ] {
        for carrier in [API, DIALPLAN] {
            let list = parse(input, carrier);
            let error = error_leg(&list);
            assert_eq!(error.as_written(), written, "{input}");
            assert_eq!(error.reading(), reading, "{input}");
            assert_eq!(error.cause(), cause, "{input}");
        }
    }
}

#[test]
fn list_warnings_are_raised() {
    assert_eq!(parse(fixture!("g-fp-static.A"), DIALPLAN).warnings(), []);
    assert_eq!(
        parse("loopback/9199/'a|b'", DIALPLAN).warnings(),
        [ListWarning::QuoteSpansLegs]
    );
    assert_eq!(
        parse("[k=${x}]loopback/9199/test", DIALPLAN).warnings(),
        [ListWarning::CarrierExpands]
    );
}

fn nulls(count: usize, separator: &str) -> String {
    (0..count)
        .map(|n| format!("null/{n}"))
        .collect::<Vec<_>>()
        .join(separator)
}

/// Past `MAX_PEERS` the switch stops splitting: the last thread, group or leg keeps the rest of
/// the text, separators included, and nothing past it is dialled.
#[test]
fn a_split_past_the_limit_keeps_the_rest_in_its_last_token() {
    let cases = [
        (",", Some(0), Some(0)),
        ("|", Some(0), None),
        (ENTERPRISE_DELIM, None, None),
    ];
    for (separator, thread, group) in cases {
        let at = parse(&nulls(MAX_PEERS, separator), API);
        assert_eq!(at.warnings(), [], "{separator}");
        assert_eq!(
            at.legs()
                .count(),
            MAX_PEERS,
            "{separator}"
        );
        for extra in [1, 2] {
            let list = parse(&nulls(MAX_PEERS + extra, separator), API);
            assert_eq!(
                list.warnings(),
                [ListWarning::LegsPastLimit {
                    thread,
                    group,
                    excess: extra
                }],
                "{separator}"
            );
            let shown = list.warnings()[0].to_string();
            assert!(!shown.contains("null"), "{shown}");
            let legs: Vec<_> = list
                .legs()
                .collect();
            assert_eq!(legs.len(), MAX_PEERS, "{separator}");
            let rest = (MAX_PEERS - 1..MAX_PEERS + extra)
                .map(|n| format!("null/{n}"))
                .collect::<Vec<_>>()
                .join(separator);
            assert_eq!(
                legs[MAX_PEERS - 1]
                    .leg
                    .endpoint,
                rest,
                "{separator}"
            );
        }
    }
}

/// Only a token the switch would have dialled counts: an empty one between doubled separators, a
/// quoted empty one and a trailing separator do not.
#[test]
fn the_excess_counts_only_non_empty_tokens() {
    let excess = |tail: &str| {
        let input = format!("{}{tail}", nulls(MAX_PEERS, ","));
        match parse(&input, API).warnings() {
            [ListWarning::LegsPastLimit { excess, .. }] => Some(*excess),
            [] => None,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(excess(",null/x,,null/y"), Some(2));
    assert_eq!(excess(",null/x,'',null/y"), Some(2));
    assert_eq!(excess(","), Some(0));
    assert_eq!(excess(",,"), Some(0));
    assert_eq!(excess(""), None);
}

#[test]
fn a_split_past_the_limit_is_located_among_kept_groups() {
    let input = format!("null/a||{}", nulls(MAX_PEERS + 1, ","));
    assert_eq!(
        parse(&input, API).warnings(),
        [ListWarning::LegsPastLimit {
            thread: Some(0),
            group: Some(1),
            excess: 1
        }]
    );
}

fn leg_warnings(input: &str, carrier: DialStringCarrier) -> Vec<LegWarning> {
    parse(input, carrier)
        .legs()
        .flat_map(|leg| {
            leg.warnings()
                .to_vec()
        })
        .collect()
}

#[test]
fn leg_warnings_name_block_and_key() {
    assert_eq!(
        leg_warnings("[a=1][k]loopback/9199/test", API),
        [LegWarning::PairIgnored {
            block: 1,
            key: "k".into()
        }]
    );
    assert_eq!(
        leg_warnings(
            r"[k=1][sentinel=s,k=\\\\\\\\\\\\\\'\\\\\\\\\\\\\\']loopback/9199/test",
            DIALPLAN
        ),
        [LegWarning::PairCleared {
            block: 1,
            key: "k".into()
        }]
    );
    assert_eq!(
        leg_warnings(fixture!("g-fp-esc-nested.A"), API),
        [LegWarning::NestedVarsRefused {
            block: 0,
            key: "nv".into()
        }]
    );
    assert_eq!(
        leg_warnings("{origination_nested_vars=true}[nv=${x}]null/a", API),
        []
    );
    assert_eq!(leg_warnings(fixture!("g-fp-reg.A"), API), []);
}

#[test]
fn errors_name_their_kind_and_never_quote_input() {
    for (input, carrier, want) in [
        ("", API, FlattenedDialStringError::Empty),
        (
            fixture!("g-fp-esc-space.A"),
            API,
            FlattenedDialStringError::ArgvSplit,
        ),
        (
            fixture!("g-fp-esc-pipe.A"),
            API,
            FlattenedDialStringError::UnclosedBlock { leg: 0 },
        ),
        (
            "[presence_id=fp@pbx.example.com]loopback/9199/test\n",
            API,
            FlattenedDialStringError::TrailingNewline,
        ),
    ] {
        let err = FlattenedDialString::parse_for(input, carrier).unwrap_err();
        assert_eq!(err, want, "{input:?}");
        let shown = err.to_string();
        assert!(!shown.is_empty());
        for fragment in ["presence", "example", "loopback", "9199"] {
            assert!(!shown.contains(fragment), "{shown}");
        }
    }
}

#[test]
fn warnings_and_unparsed_legs_never_quote_values() {
    let list = parse("[nv=${secret_ref},k=]bogus/secret-endpoint", API);
    let leg = list
        .legs()
        .next()
        .unwrap();
    for warning in leg.warnings() {
        let shown = warning.to_string();
        assert!(!shown.contains("secret"), "{shown}");
    }
    assert!(leg
        .warnings()
        .iter()
        .any(|w| w
            .to_string()
            .contains("nv")));
    match leg.target() {
        LegTarget::Unparsed(unparsed) => {
            let shown = unparsed.to_string();
            assert!(!shown.contains("secret"), "{shown}");
            assert!(
                shown.contains(
                    &"bogus/secret-endpoint"
                        .len()
                        .to_string()
                ),
                "{shown}"
            );
        }
        other => panic!("{other:?}"),
    }
    for warning in [ListWarning::QuoteSpansLegs, ListWarning::CarrierExpands] {
        assert!(!warning
            .to_string()
            .is_empty());
    }
}

#[test]
fn a_cause_is_read_as_the_switch_reads_it() {
    for (text, want) in [
        ("USER_BUSY", CauseReading::Name(HangupCause::UserBusy)),
        ("user_busy", CauseReading::Name(HangupCause::UserBusy)),
        ("17", CauseReading::Number(17)),
        ("17abc", CauseReading::Number(17)),
        ("017", CauseReading::Number(17)),
        ("0", CauseReading::Number(0)),
        ("70000", CauseReading::Number(70000)),
        ("", CauseReading::Unrecognized),
        (" 17", CauseReading::Unrecognized),
        ("NOT_A_CAUSE", CauseReading::Unrecognized),
    ] {
        assert_eq!(str2cause(text), want, "{text:?}");
    }
}

#[test]
fn variable_str_reads_a_key_not_in_any_enum() {
    let list = parse("[sip_h_X-Seat=42]loopback/9199/test", API);
    let leg = list
        .legs()
        .next()
        .unwrap();
    assert_eq!(leg.variable_str("sip_h_X-Seat"), Some("42"));
    assert_eq!(leg.variable_str("SIP_H_X-SEAT"), Some("42"));
}
