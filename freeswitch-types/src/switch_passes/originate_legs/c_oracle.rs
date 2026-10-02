//! `switch_ivr_originate`'s passes over a dial string, and `switch_true`, against the switch's own
//! C on every built tree.

use freeswitch_c_oracle::{against_the_c, Dial, Pair};
use proptest::collection::vec;
use proptest::option;
use proptest::prelude::*;
use proptest::sample::select;

use super::{dial_list, switch_true, DialList, MAX_PEERS};
use crate::commands::variables::BlockParse;
use crate::switch_passes::brackets::c_oracle::{block_text, installed, unmodelled};
use crate::switch_passes::brackets::{self, Block, PairEffect};
use crate::switch_passes::expansion::enterprise_nests;
use crate::switch_passes::{against_the_c_at_its_revision, trace, tree_block_parse, PipelineError};
use crate::test_text::text;

fn spaces() -> impl Strategy<Value = &'static str> {
    select(&["", "", " ", "  "][..])
}

/// One thread's text: its `<>` and `{}` blocks, then `|` groups of `,` legs under `[]` blocks.
fn thread_text() -> impl Strategy<Value = String> {
    let leg = (spaces(), vec(block_text('[', ']'), 0..3), text())
        .prop_map(|(lead, blocks, endpoint)| format!("{lead}{}{endpoint}", blocks.concat()));
    let group = vec(leg, 1..4).prop_map(|legs| legs.join(","));
    (
        spaces(),
        option::of(block_text('<', '>')),
        vec(block_text('{', '}'), 0..3),
        vec(group, 1..4),
    )
        .prop_map(|(lead, ultra, global, groups)| {
            format!(
                "{lead}{}{}{}",
                ultra.unwrap_or_default(),
                global.concat(),
                groups.join("|")
            )
        })
}

const NESTED_VARS: &[&str] = &[
    "",
    "",
    "",
    "{origination_nested_vars=true}",
    "[ORIGINATION_NESTED_VARS=TRUE]",
    "<origination_nested_vars=yes>",
];

/// A dial string built from the grammar the passes read, spaces and stray text included.
fn dial_text() -> impl Strategy<Value = String> {
    let enterprise = (
        spaces(),
        select(NESTED_VARS),
        option::of(block_text('<', '>')),
        vec(thread_text(), 2..4),
    )
        .prop_map(|(lead, nested, ultra, threads)| {
            format!(
                "{lead}{nested}{}{}",
                ultra.unwrap_or_default(),
                threads.join(":_:")
            )
        });
    prop_oneof![
        4 => (select(NESTED_VARS), thread_text()).prop_map(|(nested, thread)| format!("{nested}{thread}")),
        2 => enterprise,
        1 => text(),
    ]
}

/// Every block of the list, the port flagging at least one it does not read.
fn flags_a_block(list: &DialList) -> bool {
    list.blocks
        .iter()
        .chain(
            list.threads
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
                }),
        )
        .any(unmodelled)
}

/// How a thread reads, the same shape whichever side produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ThreadView {
    pairs: Vec<Pair>,
    groups: Vec<Vec<(Vec<Pair>, Vec<u8>)>>,
}

/// What stopped a read: nothing to dial, or a block that never closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Empty,
    Unclosed,
}

fn port_view(list: &DialList) -> (Vec<Pair>, Vec<ThreadView>) {
    let threads = list
        .threads
        .iter()
        .map(|thread| ThreadView {
            pairs: installed(&thread.blocks),
            groups: thread
                .groups
                .iter()
                .map(|group| {
                    group
                        .iter()
                        .map(|leg| {
                            (
                                installed(&leg.blocks),
                                leg.endpoint
                                    .clone()
                                    .into_bytes(),
                            )
                        })
                        .collect()
                })
                .collect(),
        })
        .collect();
    (installed(&list.blocks), threads)
}

fn switch_stop(failure: &[u8]) -> Stop {
    match failure {
        b"No origination URL specified!" => Stop::Empty,
        b"Parse Error!" => Stop::Unclosed,
        other => panic!("no port reading for {:?}", String::from_utf8_lossy(other)),
    }
}

/// The switch's reading, or the first thing that stopped it.
fn switch_view(dial: &Dial) -> Result<(Vec<Pair>, Vec<ThreadView>), Stop> {
    if let Some(failure) = &dial.failure {
        return Err(switch_stop(failure));
    }
    let threads = dial
        .threads
        .iter()
        .map(|thread| {
            if let Some(failure) = &thread.failure {
                return Err(switch_stop(failure));
            }
            Ok(ThreadView {
                pairs: thread
                    .pairs
                    .clone(),
                groups: thread
                    .groups
                    .iter()
                    .map(|group| {
                        group
                            .iter()
                            .map(|leg| {
                                (
                                    leg.pairs
                                        .clone(),
                                    leg.endpoint
                                        .clone()
                                        .unwrap_or_default(),
                                )
                            })
                            .collect()
                    })
                    .collect(),
            })
        })
        .collect::<Result<_, _>>()?;
    Ok((
        dial.enterprise
            .clone(),
        threads,
    ))
}

/// The `<>` event `switch_ivr_enterprise_originate` hands every thread, as a block of the pairs
/// the switch installed; the stub stores no headers, so the port's event store reads it.
fn enterprise_block(enterprise: &[Pair]) -> Block {
    let utf8 = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    Block {
        open: '<',
        separator: ',',
        pairs: enterprise
            .iter()
            .map(|(key, value)| brackets::Pair {
                key: utf8(key),
                effect: match &value[..] {
                    [] => PairEffect::Cleared,
                    value => PairEffect::Set(utf8(value)),
                },
            })
            .collect(),
        rewrites_following_text: false,
    }
}

#[test]
fn dial_lists_match_the_switch() {
    against_the_c_at_its_revision(
        file!(),
        "dial_lists_match_the_switch",
        dial_text(),
        |c, block_parse, input| {
            let text = trace(&input);
            let port = dial_list(&text, 0..input.len(), false, block_parse);
            let refused = matches!(port, Err(PipelineError::SplitSeparatorUnreadable));
            if refused
                || port
                    .as_ref()
                    .is_ok_and(flags_a_block)
            {
                return Ok(());
            }
            let dial = c.dial(input.as_bytes());
            let port_read = port
                .as_ref()
                .map(port_view)
                .map_err(|e| match e {
                    PipelineError::Empty => Stop::Empty,
                    PipelineError::UnclosedBlock { .. } => Stop::Unclosed,
                    PipelineError::ArgvSplit | PipelineError::SplitSeparatorUnreadable => {
                        unreachable!("no carrier pass runs, and a refused split returned early")
                    }
                });
            prop_assert_eq!(&port_read, &switch_view(&dial), "{:?}", input);
            if let Ok(list) = &port {
                let inherited = enterprise_nests([&enterprise_block(&dial.enterprise)], |value| {
                    c.switch_true(value.as_bytes())
                });
                let switch: Vec<bool> = dial
                    .threads
                    .iter()
                    .map(|thread| thread.nested_vars || inherited)
                    .collect();
                let port: Vec<bool> = list
                    .threads
                    .iter()
                    .map(|thread| thread.nested_vars)
                    .collect();
                prop_assert_eq!(port, switch, "nested vars per thread in {:?}", input);
            }
            Ok(())
        },
    );
}

/// A non-ASCII `^^` block chaining a second block splits that block on the separator's first byte,
/// which here cuts the text holding `:_:`; the port refuses rather than read it by char.
#[test]
fn a_chained_block_split_by_a_non_ascii_byte_is_refused() {
    let input = "<^^é><<>:_:[>],é|[[]";
    let refused = |block_parse| {
        matches!(
            dial_list(&trace(input), 0..input.len(), false, block_parse),
            Err(PipelineError::SplitSeparatorUnreadable)
        )
    };
    assert!(refused(BlockParse::default()));
    for (tree, c) in freeswitch_c_oracle::oracles() {
        assert!(refused(tree_block_parse(tree)), "tree {tree}");
        let dial = c.dial(input.as_bytes());
        let endpoints: Vec<_> = dial
            .threads
            .iter()
            .flat_map(|thread| {
                thread
                    .groups
                    .iter()
                    .flatten()
            })
            .map(|leg| {
                leg.endpoint
                    .clone()
            })
            .collect();
        assert_eq!(endpoints, [Some(b"]".to_vec())], "tree {tree}: {dial:?}");
    }
}

/// A `[` range from the first leg to the second's endpoint decides how the comma scan rewrites the
/// commas of the second leg's block: a `^^,` head splits alike, a `^^:` value keeps the placeholder.
#[test]
fn a_range_from_an_earlier_leg_rewrites_a_later_legs_block() {
    let second_leg = |input: &str, block_parse| {
        let list = dial_list(&trace(input), 0..input.len(), false, block_parse).unwrap();
        port_view(&list).1[0].groups[0][1].clone()
    };
    let cases = [
        (r"portaudio/[\\\\,[^^,v0=undef]sofia/gateway/]/", "undef"),
        (r"portaudio/[\\\\,[^^:v0=a,b]sofia/gateway/]/", "a\u{2}b"),
    ];
    for (input, value) in cases {
        let want = (
            vec![(
                b"v0".to_vec(),
                value
                    .as_bytes()
                    .to_vec(),
            )],
            b"sofia/gateway/]/".to_vec(),
        );
        assert_eq!(second_leg(input, BlockParse::default()), want, "{input}");
        for (tree, c) in freeswitch_c_oracle::oracles() {
            let dial = c.dial(input.as_bytes());
            let [thread] = &dial.threads[..] else {
                panic!("tree {tree}: one thread: {dial:?}");
            };
            let leg = &thread.groups[0][1];
            let switch = (
                leg.pairs
                    .clone(),
                leg.endpoint
                    .clone()
                    .unwrap_or_default(),
            );
            assert_eq!(switch, want, "tree {tree}: {input}");
            assert_eq!(
                second_leg(input, tree_block_parse(tree)),
                want,
                "tree {tree}"
            );
        }
    }
}

/// Past `MAX_PEERS` the last thread, group or leg keeps the rest of the text, as in the switch.
#[test]
fn a_split_past_the_limit_keeps_the_rest_as_the_switch_does() {
    for separator in [",", "|", ":_:"] {
        let input = (0..=MAX_PEERS)
            .map(|n| format!("null/{n}"))
            .collect::<Vec<_>>()
            .join(separator);
        let read = |block_parse| {
            dial_list(&trace(&input), 0..input.len(), false, block_parse)
                .map(|list| port_view(&list))
        };
        let (_, threads) = read(BlockParse::default()).unwrap();
        let last = threads
            .iter()
            .flat_map(|thread| {
                thread
                    .groups
                    .iter()
                    .flatten()
            })
            .last()
            .map(|(_, endpoint)| endpoint.clone());
        let rest = format!("null/{}{separator}null/{MAX_PEERS}", MAX_PEERS - 1);
        assert_eq!(last, Some(rest.into_bytes()), "{separator}");
        for (tree, c) in freeswitch_c_oracle::oracles() {
            assert_eq!(
                Ok(read(tree_block_parse(tree)).unwrap()),
                switch_view(&c.dial(input.as_bytes())),
                "tree {tree} on {separator}"
            );
        }
    }
}

#[test]
fn switch_true_matches_the_switch() {
    let word = select(
        &[
            "yes", "YES", "On", "true", "t", "T", "enabled", "active", "allow", "no", "false", "0",
            "1", "-1", "+2", "00", "0.5", ".", "", "-", "+", "1a", "１", "10.", "-0",
        ][..],
    );
    let value = prop_oneof![
        2 => word.prop_map(str::to_owned),
        1 => text(),
    ];
    against_the_c(
        file!(),
        "switch_true_matches_the_switch",
        value,
        |c, value| {
            prop_assert_eq!(
                switch_true(&value),
                c.switch_true(value.as_bytes()),
                "{:?}",
                value
            );
            Ok(())
        },
    );
}

#[test]
fn the_oracle_reads_one_originate() {
    let Some(c) = freeswitch_c_oracle::trees()
        .iter()
        .find_map(|tree| {
            tree.oracle()
                .ok()
        })
    else {
        return;
    };
    let dial = c.dial(b"<e=1>{g=2}[l=3]loopback/9199,error/USER_BUSY|null/a");
    let pair = |key: &[u8], value: &[u8]| -> Pair { (key.to_vec(), value.to_vec()) };
    assert_eq!(dial.enterprise, []);
    let [thread] = &dial.threads[..] else {
        panic!("one thread: {dial:?}");
    };
    assert_eq!(thread.pairs, [pair(b"e", b"1"), pair(b"g", b"2")]);
    assert_eq!(
        thread
            .groups
            .len(),
        2
    );
    assert_eq!(thread.groups[0][0].pairs, [pair(b"l", b"3")]);
    assert_eq!(
        thread.groups[0][0]
            .endpoint
            .as_deref(),
        Some(&b"loopback/9199"[..])
    );
    assert_eq!(
        thread.groups[1][0]
            .endpoint
            .as_deref(),
        Some(&b"null/a"[..])
    );
}
