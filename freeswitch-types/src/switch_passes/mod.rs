//! Ports of the switch's own string passes, one module per pass, each held to the switch's C.
//!
//! Every pass runs over traced text: each char paired with the byte range in the
//! original input it came from, so a cleaned token still points back at its source.

#[cfg(feature = "esl")]
use std::ops::Range;

#[cfg(feature = "esl")]
pub(crate) mod api_argument;
#[cfg(feature = "esl")]
pub(crate) mod brackets;
#[cfg(feature = "esl")]
pub(crate) mod escape;
#[cfg(feature = "esl")]
pub(crate) mod execute_on;
#[cfg(feature = "esl")]
pub(crate) mod expansion;
#[cfg(feature = "esl")]
pub(crate) mod inline_hunt;
#[cfg(feature = "esl")]
pub(crate) mod originate_function;
#[cfg(feature = "esl")]
pub(crate) mod originate_legs;
#[cfg(feature = "esl")]
pub(crate) mod pipeline;
pub(crate) mod separate;

/// The block-parse revision the oracle index names for `tree`.
#[cfg(all(test, feature = "esl", freeswitch_oracle))]
pub(crate) fn tree_block_parse(tree: &str) -> crate::commands::BlockParse {
    let revision = freeswitch_c_oracle::tree(tree)
        .unwrap_or_else(|| panic!("the oracle index names no tree {tree}"))
        .block_parse();
    revision
        .parse()
        .unwrap_or_else(|e| panic!("tree {tree} block_parse: {e}"))
}

/// `against_the_c` handing the property each tree's block-parse revision.
#[cfg(all(test, feature = "esl", freeswitch_oracle))]
pub(crate) fn against_the_c_at_its_revision<S: proptest::strategy::Strategy>(
    source: &'static str,
    name: &str,
    strategy: S,
    property: impl Fn(
        freeswitch_c_oracle::Oracle,
        crate::commands::BlockParse,
        S::Value,
    ) -> proptest::test_runner::TestCaseResult,
) {
    freeswitch_c_oracle::on_every_tree(source, name, strategy, |tree, c, value| {
        property(c, tree_block_parse(tree), value)
    });
}

#[cfg(all(test, feature = "esl", freeswitch_oracle))]
#[test]
fn every_oracle_tree_names_a_known_block_parse() {
    for tree in freeswitch_c_oracle::trees() {
        tree_block_parse(tree.name());
    }
}

/// What stops the switch from reading a dial string at all.
#[cfg(feature = "esl")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PipelineError {
    /// Nothing to dial.
    Empty,
    /// The API carrier's argument split cuts the dial string into more than one
    /// argument, leaves a quote open, or finds a quote holding its separator.
    ArgvSplit,
    /// A block never closes, which aborts the whole originate.
    UnclosedBlock { leg: usize },
    /// A split the switch runs on a non-ASCII `^^` separator's first byte reaches text no string
    /// carries: a group or leg opening such a head, or a block with one ending in a backslash.
    SplitSeparatorUnreadable,
}

/// A char and the start and end of the input bytes it stands for; an unescaped char
/// spans its whole escape.
pub(crate) type Traced = (char, usize, usize);

pub(crate) fn trace(s: &str) -> Vec<Traced> {
    s.char_indices()
        .map(|(at, c)| (c, at, at + c.len_utf8()))
        .collect()
}

pub(crate) fn untrace(text: &[Traced]) -> String {
    text.iter()
        .filter(|entry| !separate::is_continuation(entry))
        .map(|&(c, ..)| c)
        .collect()
}

/// The bytes of `extent` token `span` of `text` covers, from the end of the char before it to the
/// start of the char after it, so chars a pass dropped beside the token belong to it.
#[cfg(feature = "esl")]
pub(crate) fn byte_range(
    text: &[Traced],
    extent: Range<usize>,
    span: Range<usize>,
) -> Range<usize> {
    let start = span
        .start
        .checked_sub(1)
        .and_then(|before| text.get(before))
        .map_or(extent.start, |&(.., end)| end);
    let end = text
        .get(span.end)
        .map_or(extent.end, |&(_, start, _)| start);
    start..end
}

/// The input bytes an untouched trace covers.
#[cfg(feature = "esl")]
pub(crate) fn extent(text: &[Traced]) -> Range<usize> {
    let start = text
        .first()
        .map_or(0, |&(_, start, _)| start);
    let end = text
        .last()
        .map_or(start, |&(.., end)| end);
    start..end
}
