//! `switch_event_create_brackets` against the switch's own C on every built tree.

use freeswitch_c_oracle::Pair;
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::select;

use super::{install, parse_block, Block, PairEffect};
use crate::commands::variables::BlockParse;
use crate::switch_passes::originate_legs::UNQUOTED_ESC_COMMA;
use crate::switch_passes::separate::CBuffer;
use crate::switch_passes::{against_the_c_at_its_revision, trace, tree_block_parse, untrace};
use crate::test_text::text;

/// The headers an event carrying `EF_UNIQ_HEADERS` holds once `blocks` install into it.
pub(crate) fn installed<'a>(blocks: impl IntoIterator<Item = &'a Block>) -> Vec<Pair> {
    install(blocks)
        .into_iter()
        .map(|(key, value)| {
            (
                key.as_bytes()
                    .to_vec(),
                value
                    .as_bytes()
                    .to_vec(),
            )
        })
        .collect()
}

/// A block the port flags rather than reads: a non-ASCII separator, or a pair opening one.
pub(crate) fn unmodelled(block: &Block) -> bool {
    block.separator_unreadable()
        || block
            .pairs
            .iter()
            .any(|pair| pair.effect == PairEffect::Unreadable)
}

const HEADS: &[&str] = &[
    "", "", "", "^^", "^^,", "^^:", "^^~", "^^'", "^^\\", "^^=", "^^é",
];

/// Block content: text, the separators the parse reads, and brackets of any kind.
fn content() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        3 => text(),
        2 => select(&["=", ",", ":", "~", "\u{2}", "{", "}", "[", "]", "<", ">", "k=v", "'", "^^"][..])
            .prop_map(str::to_owned),
    ];
    vec(piece, 0..6).prop_map(|pieces| pieces.concat())
}

/// A block of `open` and `close`, its close sometimes missing.
pub(crate) fn block_text(open: char, close: char) -> impl Strategy<Value = String> {
    (
        select(HEADS),
        content(),
        prop_oneof![6 => Just(true), 1 => Just(false)],
    )
        .prop_map(move |(head, content, closed)| {
            let close = if closed {
                close.to_string()
            } else {
                String::new()
            };
            format!("{open}{head}{content}{close}")
        })
}

#[test]
fn blocks_match_the_switch() {
    let kind = select(
        &[
            ('<', '>', ','),
            ('{', '}', ','),
            ('[', ']', ','),
            ('[', ']', UNQUOTED_ESC_COMMA),
        ][..],
    );
    let case = kind.prop_flat_map(|(open, close, comma)| {
        (Just((open, close, comma)), block_text(open, close), text())
    });
    against_the_c_at_its_revision(
        file!(),
        "blocks_match_the_switch",
        case,
        |c, block_parse, ((open, close, comma), block, tail)| {
            let input = format!("{block}{tail}");
            let text = trace(&input);
            let mut buffer = CBuffer::new(&text);
            let port = parse_block(&mut buffer, 0, open, close, comma, block_parse);
            if port
                .as_ref()
                .is_some_and(|parsed| unmodelled(&parsed.block))
            {
                return Ok(());
            }
            let port = port.map(|parsed| {
                (
                    installed([&parsed.block]),
                    parsed
                        .next
                        .min(input.len()),
                    untrace(buffer.c_str(parsed.next)).into_bytes(),
                )
            });
            let switch = c
                .brackets(input.as_bytes(), open as u8, close as u8, comma as u8)
                .map(|read| (read.pairs, read.rest, read.following));
            prop_assert_eq!(port, switch, "{:?} split on {:?}", input, comma);
            Ok(())
        },
    );
}

/// The block's event replaces a header by name ignoring case, as the port's event store does;
/// `GONE=` splits into one field, so it installs and deletes nothing.
#[test]
fn a_block_event_folds_names_by_case() {
    let block = "{k=1,K=2,gone=x,GONE=,kept=y}";
    let text = trace(block);
    let port = |block_parse| {
        let parsed = parse_block(&mut CBuffer::new(&text), 0, '{', '}', ',', block_parse)
            .expect("the block closes");
        installed([&parsed.block])
    };
    let pair = |key: &[u8], value: &[u8]| -> Pair { (key.to_vec(), value.to_vec()) };
    let want = vec![pair(b"K", b"2"), pair(b"gone", b"x"), pair(b"kept", b"y")];
    assert_eq!(port(BlockParse::MEASURED), want);
    for (tree, c) in freeswitch_c_oracle::oracles() {
        assert_eq!(port(tree_block_parse(tree)), want, "tree {tree}");
        let read = c
            .brackets(block.as_bytes(), b'{', b'}', b',')
            .expect("the block closes");
        assert_eq!(read.pairs, want, "tree {tree}");
    }
}
