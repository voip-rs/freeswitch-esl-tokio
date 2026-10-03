//! Expected values are what the live switch measured for each capture, carrier
//! and escape depth; `tests/fixtures/flattened/README.md` names the captures.

use super::read;
use crate::commands::variables::{
    BlockParse, DialStringCarrier, DialStringTarget, Variables, VariablesType,
};
use crate::switch_passes::brackets::{parse_block, same_header, Pair, PairEffect, Parsed};
use crate::switch_passes::expansion::names_a_variable;
use crate::switch_passes::originate_legs::{
    channel_variables, dial_list, DialList, Leg, Thread, ENTERPRISE_DELIM,
};
use crate::switch_passes::separate::CBuffer;
use crate::switch_passes::{trace, untrace, PipelineError};

const API: DialStringCarrier = DialStringCarrier::EslApi;
const DIALPLAN: DialStringCarrier = DialStringCarrier::Dialplan;

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("../../../tests/fixtures/flattened/", $name, ".txt"))
    };
}

fn read_at(input: &str, carrier: DialStringCarrier) -> DialList {
    read(input, carrier.into()).unwrap_or_else(|e| panic!("{input:?} at {carrier:?}: {e:?}"))
}

fn legs(list: &DialList) -> Vec<(&Thread, &Leg)> {
    list.threads
        .iter()
        .flat_map(|t| {
            t.groups
                .iter()
                .flatten()
                .map(move |l| (t, l))
        })
        .collect()
}

fn value<'a>(list: &'a DialList, leg: usize, key: &str) -> Option<&'a str> {
    let (thread, leg) = legs(list)[leg];
    resolve_on(list, thread, leg, key)
}

fn resolve_on<'a>(
    list: &'a DialList,
    thread: &'a Thread,
    leg: &'a Leg,
    key: &str,
) -> Option<&'a str> {
    channel_variables(
        list.blocks
            .iter()
            .chain(&thread.blocks),
        leg,
        thread.nested_vars,
    )
    .into_iter()
    .find(|(name, _)| same_header(name, key))
    .map(|(_, value)| value)
}

fn effect(list: &DialList, leg: usize, key: &str) -> Vec<PairEffect> {
    let (thread, leg) = legs(list)[leg];
    list.blocks
        .iter()
        .chain(&thread.blocks)
        .chain(&leg.blocks)
        .flat_map(|b| &b.pairs)
        .filter(|p| p.key == key)
        .map(|p| {
            p.effect
                .clone()
        })
        .collect()
}

#[test]
fn a_seat_with_a_separator_block_is_one_leg() {
    let input = fixture!("g-fp-static.A");
    for carrier in [API, DIALPLAN] {
        let list = read_at(input, carrier);
        let legs = legs(&list);
        assert_eq!(legs.len(), 1);
        let leg = legs[0].1;
        assert_eq!(leg.endpoint, "loopback/9199/test");
        assert_eq!(leg.blocks[0].separator, ':');
        assert_eq!(
            &input[leg
                .raw
                .clone()],
            input
        );
        assert_eq!(
            value(&list, 0, "presence_id"),
            Some("fp-static@pbx.example.com")
        );
    }
}

#[test]
fn registered_contacts_keep_their_uri_and_their_seat() {
    let input = fixture!("pbx-calltakers.A");
    let list = read_at(input, API);
    let legs = legs(&list);
    let seat_1436: Vec<&str> = legs
        .iter()
        .filter(|(t, l)| resolve_on(&list, t, l, "presence_id") == Some("1436@pbx.example.com"))
        .map(|(_, l)| {
            l.endpoint
                .as_str()
        })
        .collect();
    assert_eq!(seat_1436.len(), 2, "{seat_1436:?}");
    assert!(seat_1436
        .iter()
        .any(|e| e.contains("@[2001:db8:2220:33:b17a:8818:d0f7:212d]:43620;transport=tcp")));
    let rejoined = legs
        .iter()
        .map(|(_, l)| {
            &input[l
                .raw
                .clone()]
        })
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(rejoined, input);
}

#[test]
fn an_empty_member_is_an_empty_leg_between_the_others() {
    let list = read_at(fixture!("fp-one-empty.A"), API);
    let endpoints: Vec<&str> = legs(&list)
        .iter()
        .map(|(_, l)| {
            l.endpoint
                .as_str()
        })
        .collect();
    assert_eq!(endpoints, ["loopback/9199/test", "", "error/USER_BUSY"]);

    let all_empty = read_at(fixture!("fp-all-empty.A"), API);
    assert_eq!(legs(&all_empty).len(), 1);
    assert_eq!(
        legs(&all_empty)[0]
            .1
            .endpoint,
        ""
    );
}

#[test]
fn a_member_separator_is_read_as_originate_reads_it() {
    let piped = read_at(fixture!("g-fp-pipe.A"), API);
    assert_eq!(
        piped
            .threads
            .len(),
        1
    );
    let sizes: Vec<usize> = piped.threads[0]
        .groups
        .iter()
        .map(Vec::len)
        .collect();
    assert_eq!(sizes, [1, 2]);

    let enterprise = read_at(fixture!("g-fp-ent.none"), API);
    assert_eq!(
        enterprise
            .threads
            .len(),
        2
    );
    assert_eq!(
        value(&enterprise, 0, "presence_id"),
        Some("fp-ent@pbx.example.com")
    );
    assert_eq!(value(&enterprise, 1, "presence_id"), None);
    assert_eq!(
        value(&enterprise, 2, "presence_id"),
        Some("fp-static@pbx.example.com")
    );
}

#[test]
fn the_last_block_on_a_leg_wins() {
    for (name, want) in [
        ("g-fp-scopes-none.A", "a"),
        ("g-fp-scopes-all.A", "a"),
        ("g-fp-scopes-ent.A", "e"),
        ("g-fp-scopes-both.A", "e"),
        ("g-fp-scopes-noleg.A", "e"),
    ] {
        let input = std::fs::read_to_string(format!(
            "{}/tests/fixtures/flattened/{name}.txt",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        for carrier in [API, DIALPLAN] {
            assert_eq!(
                value(&read_at(&input, carrier), 0, "k"),
                Some(want),
                "{name} {carrier:?}"
            );
        }
    }
}

#[test]
fn a_global_block_beats_the_leg_unless_it_clobbers() {
    for (input, want) in [
        ("{k=g}[k=l]loopback/9199/test", "g"),
        ("{local_var_clobber=true,k=g}[k=l]loopback/9199/test", "l"),
        ("<k=e>[k=l]loopback/9199/test", "e"),
        ("<k=e>{k=g}loopback/9199/test", "g"),
        ("<k=e>{k=g}[k=l]loopback/9199/test", "g"),
    ] {
        assert_eq!(value(&read_at(input, API), 0, "k"), Some(want), "{input}");
    }
}

#[test]
fn values_the_switch_escaped_arrive_as_measured() {
    let bs = read_at(fixture!("g-fp-esc-bs-exp.A"), API);
    assert_eq!(value(&bs, 0, "path"), Some(r"a\b"));

    let comma = read_at(fixture!("g-fp-esc-comma.A"), API);
    assert_eq!(value(&comma, 0, "codecs"), Some("PCMA,PCMU"));

    let member = read_at(fixture!("g-fp-quote-member.A"), API);
    assert_eq!(legs(&member).len(), 2);
    assert_eq!(value(&member, 0, "q"), Some("its"));
    assert_eq!(value(&member, 1, "q"), Some("its"));
    assert!(!member.quote_spans_legs);

    let first = read_at(fixture!("g-fp-quote-first.A"), DIALPLAN);
    assert_eq!(value(&first, 0, "q"), Some("it's"));
    assert_eq!(value(&first, 1, "sentinel"), Some("s"));

    let space = read_at(fixture!("g-fp-esc-space.A"), DIALPLAN);
    assert_eq!(value(&space, 0, "greet"), Some("a b"));
}

#[test]
fn the_api_carrier_refuses_what_its_argument_split_cuts() {
    for name in [fixture!("g-fp-esc-space.A"), fixture!("g-fp-quote-first.A")] {
        assert_eq!(
            read(name, API.into()),
            Err(PipelineError::ArgvSplit),
            "{name}"
        );
    }
}

#[test]
fn a_block_cut_open_aborts_the_whole_list() {
    assert_eq!(
        read(fixture!("g-fp-esc-pipe.A"), API.into()),
        Err(PipelineError::UnclosedBlock { leg: 0 })
    );
    let bracket = read_at(fixture!("g-fp-esc-bracket.A"), API);
    assert_eq!(
        legs(&bracket)[0]
            .1
            .endpoint,
        "y]loopback/9199/test"
    );
}

#[test]
fn an_empty_pair_is_ignored_or_clears_by_the_depth_it_arrives_at() {
    let api_braces = read_at(r"{k=1}{sentinel=s,k=\\\'\\\'}loopback/9199/test", API);
    assert_eq!(value(&api_braces, 0, "k"), None);
    assert_eq!(value(&api_braces, 0, "sentinel"), Some("s"));

    for (input, carrier, cleared) in [
        (r"{k=1}{sentinel=s,k=''}loopback/9199/test", API, false),
        (r"{k=1}{sentinel=s,k=}loopback/9199/test", API, false),
        (
            r"[k=1][sentinel=s,k=\\\\\\\\\\\\\\\'\\\\\\\\\\\\\\\']loopback/9199/test",
            API,
            true,
        ),
        (
            r"[k=1][sentinel=s,k=\\\\\\\'\\\\\\\']loopback/9199/test",
            API,
            false,
        ),
        (
            r"{k=1}{sentinel=s,k=\\'\\'}loopback/9199/test",
            DIALPLAN,
            true,
        ),
        (r"{k=1}{sentinel=s,k=''}loopback/9199/test", DIALPLAN, false),
        (
            r"[k=1][sentinel=s,k=\\\\\\\\\\\\\\'\\\\\\\\\\\\\\']loopback/9199/test",
            DIALPLAN,
            true,
        ),
        (
            r"[k=1][sentinel=s,k=\\\\\\'\\\\\\']loopback/9199/test",
            DIALPLAN,
            false,
        ),
    ] {
        let list = read_at(input, carrier);
        let want = if cleared {
            PairEffect::Cleared
        } else {
            PairEffect::Valueless
        };
        assert_eq!(
            effect(&list, 0, "k").last(),
            Some(&want),
            "{input} {carrier:?}"
        );
        assert_eq!(
            value(&list, 0, "k"),
            (!cleared).then_some("1"),
            "{input} {carrier:?}"
        );
        assert_eq!(
            value(&list, 0, "sentinel"),
            Some("s"),
            "{input} {carrier:?}"
        );
    }

    let fixture = read_at(fixture!("g-fp-esc-quotedempty.A"), API);
    assert_eq!(value(&fixture, 0, "k"), Some("1"));
}

#[test]
fn a_nested_variable_is_named_and_the_opt_in_is_seen_anywhere() {
    let list = read_at(fixture!("g-fp-esc-nested.A"), API);
    assert!(!list.threads[0].nested_vars);
    assert!(matches!(
        effect(&list, 0, "nv").last(),
        Some(PairEffect::Set(v)) if names_a_variable(v)
    ));
    assert_eq!(value(&list, 0, "nv"), None);
    assert_eq!(value(&list, 0, "sentinel"), Some("s"));
    let opted_in = read_at("{origination_nested_vars=true}[nv=${x}]null/a", API);
    assert_eq!(value(&opted_in, 0, "nv"), Some("${x}"));
    assert!(read_at("{origination_nested_vars=true}[nv=${x}]null/a", API).threads[0].nested_vars);

    assert!(names_a_variable("${x}"));
    assert!(names_a_variable(r"\${x}"));
    assert!(names_a_variable(r"$\{x}"));
    assert!(!names_a_variable("$x"));
    assert!(!names_a_variable("a$"));
}

#[test]
fn nothing_to_dial_is_empty() {
    assert_eq!(read("", API.into()), Err(PipelineError::Empty));
}

/// Everything this crate renders has to read back to the values it was given,
/// or the port and the renderer disagree about a pass.
#[test]
fn the_port_reads_back_every_value_the_renderer_writes() {
    let values = [
        r"C:\path",
        r"a\nb",
        "a,b",
        "it's",
        "don't,x",
        "a b",
        "a|b",
        "p1:p2",
        "x~y",
        " lead and trail ",
        " a",
        "a  ",
        " ",
        r"a\ ",
        "$$",
        "pa$$ word",
        r"ends\",
        r"back\,comma",
    ];
    let targets = [
        API.into(),
        DIALPLAN.into(),
        DialStringTarget::new(API)
            .with_argv_separator('~')
            .expect("'~' separates originate's arguments"),
        DialStringTarget::new(API).with_unchecked_argv_separator('|'),
    ];
    for scope in [
        VariablesType::Default,
        VariablesType::Enterprise,
        VariablesType::Channel,
    ] {
        for separator in [None, Some('~')] {
            for carrier in targets {
                let mut vars = Variables::new(scope);
                for (i, v) in values
                    .iter()
                    .enumerate()
                {
                    if scope == VariablesType::Channel && v.contains('\'') {
                        continue;
                    }
                    if separator.is_some_and(|sep| v.contains(sep)) {
                        continue;
                    }
                    vars.insert(format!("v{i}"), *v);
                }
                vars.insert("sentinel", "s");
                let vars = match separator {
                    Some(sep) => vars
                        .with_separator(sep)
                        .unwrap(),
                    None => vars,
                };
                let dial = format!("{}null/drift", vars.display_for(carrier));
                let list = read(&dial, carrier)
                    .unwrap_or_else(|e| panic!("{dial:?} at {carrier:?}: {e:?}"));
                for (key, want) in vars.iter() {
                    assert_eq!(value(&list, 0, key), Some(want), "{dial} at {carrier:?}");
                }
            }
        }
    }
}

#[test]
fn legs_and_threads_meet_only_at_their_separators() {
    let is_separator = |gap: &str| {
        !gap.is_empty()
            && gap
                .chars()
                .all(|c| matches!(c, ',' | '|'))
    };
    for input in crate::test_text::TILING_INPUTS
        .iter()
        .copied()
        .chain([
            r"a\'",
            r"\'",
            r"x\\\'y",
            r"\$$",
            "$${a}$${b}",
            r"a:_:\'b:_:'c d'",
            r"  a,\'b|'c',''",
            "a,'',b",
        ])
    {
        for carrier in [API, DIALPLAN] {
            let Ok(list) = read(input, carrier.into()) else {
                continue;
            };
            let context = format!("{input:?} at {carrier:?}: {list:?}");
            for pair in list
                .threads
                .windows(2)
            {
                assert_eq!(
                    &input[pair[0]
                        .raw
                        .end
                        ..pair[1]
                            .raw
                            .start],
                    ENTERPRISE_DELIM,
                    "{context}"
                );
            }
            for thread in list
                .threads
                .iter()
                .filter(|thread| {
                    thread
                        .groups
                        .iter()
                        .any(|group| !group.is_empty())
                })
            {
                let mut at = thread
                    .raw
                    .start;
                for (k, leg) in thread
                    .groups
                    .iter()
                    .flatten()
                    .enumerate()
                {
                    let gap = &input[at..leg
                        .raw
                        .start];
                    assert!((k == 0 && gap.is_empty()) || is_separator(gap), "{context}");
                    at = leg
                        .raw
                        .end;
                }
                let rest = &input[at..thread
                    .raw
                    .end];
                // A trailing empty leg yields no token, so its bytes trail the separator.
                assert!(rest.is_empty() || is_separator(&rest[..1]), "{context}");
            }
        }
    }
}

#[test]
fn a_non_ascii_block_separator_yields_no_pairs() {
    let list = read_at("[a=1][^^éb=2éc=3]loopback/9199/test", API);
    let (_, leg) = legs(&list)[0];
    assert_eq!(leg.blocks[1].separator, 'é');
    assert!(leg.blocks[1]
        .pairs
        .is_empty());
    assert_eq!(value(&list, 0, "a"), Some("1"));
    assert_eq!(leg.endpoint, "loopback/9199/test");
}

/// Each enterprise thread looks for the opt-in in its own text; the `<>` event reaches them all.
#[test]
fn nested_vars_are_opted_into_per_thread() {
    let list = read_at(
        "{origination_nested_vars=true}[nv=${x}]null/a:_:[nv=${x}]null/b",
        API,
    );
    assert_eq!(
        list.threads
            .iter()
            .map(|thread| thread.nested_vars)
            .collect::<Vec<_>>(),
        [true, false]
    );
    assert_eq!(value(&list, 0, "nv"), Some("${x}"));
    assert_eq!(value(&list, 1, "nv"), None);

    let list = read_at(
        "<origination_nested_vars=yes>[nv=${x}]null/a:_:[nv=${x}]null/b",
        API,
    );
    assert_eq!(value(&list, 0, "nv"), Some("${x}"));
    assert_eq!(value(&list, 1, "nv"), Some("${x}"));
}

/// The splits run in place, so a backslash escaping a token's terminator reads on into
/// whatever the buffer holds after it.
#[test]
fn a_block_parse_reads_the_buffer_as_the_switch_leaves_it() {
    let mut buffer = CBuffer::new(&trace(r"{\}k=v}"));
    let Parsed { block, next, .. } =
        parse_block(&mut buffer, 0, '{', '}', ',', BlockParse::default()).unwrap();
    assert_eq!(next, 3);
    assert_eq!(untrace(buffer.c_str(next)), "k");
    assert_eq!(
        block.pairs,
        [Pair {
            key: r"\".into(),
            effect: PairEffect::Set("v}".into())
        }]
    );
    assert!(block.rewrites_following_text);

    let mut buffer = CBuffer::new(&trace(r"{x\\,k=v}"));
    let Parsed { block, .. } =
        parse_block(&mut buffer, 0, '{', '}', ',', BlockParse::default()).unwrap();
    assert_eq!(
        block
            .pairs
            .iter()
            .map(|pair| &pair.effect)
            .collect::<Vec<_>>(),
        [&PairEffect::Ignored, &PairEffect::Set("v".into())]
    );
    assert!(!block.rewrites_following_text);

    let mut buffer = CBuffer::new(&trace("<^^>=>"));
    let Parsed { block, next, .. } =
        parse_block(&mut buffer, 0, '<', '>', ',', BlockParse::default()).unwrap();
    assert_eq!(untrace(buffer.c_str(next)), "");
    assert!(block.rewrites_following_text);

    let bare = |input: &str| {
        dial_list(&trace(input), 0..input.len(), false, BlockParse::default())
            .unwrap_or_else(|e| panic!("{input:?}: {e:?}"))
    };
    let list = bare("<\\>\t>,[[]");
    let (_, leg) = legs(&list)[0];
    assert_eq!(legs(&list).len(), 1);
    assert_eq!(leg.endpoint, "\t>");

    for refused in ["^^é]", "<^^éa\\>x:_:é"] {
        assert_eq!(
            dial_list(
                &trace(refused),
                0..refused.len(),
                false,
                BlockParse::default()
            ),
            Err(PipelineError::SplitSeparatorUnreadable),
            "{refused:?}"
        );
    }

    let list = bare("['\\n]|\\n',\\");
    assert_eq!(
        legs(&list)
            .iter()
            .map(|(_, leg)| leg
                .endpoint
                .as_str())
            .collect::<Vec<_>>(),
        ["|\n", "\\", "\\"]
    );
    assert_eq!(
        block.pairs,
        [Pair {
            key: String::new(),
            effect: PairEffect::Set(">".into())
        }]
    );
}
