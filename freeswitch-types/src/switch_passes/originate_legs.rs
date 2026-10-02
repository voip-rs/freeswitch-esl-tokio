//! Port of the passes `switch_ivr_originate` and `switch_ivr_enterprise_originate` run over a dial
//! string up to each leg's endpoint: the `:_:` thread split, the `|` group and `,` leg splits in
//! place, and the blocks ahead of each.

use std::ops::Range;

use super::brackets::{install, parse_block, same_header, Block};
use super::expansion::{enterprise_nests, names_a_variable};
use super::separate::{find, find_end_paren, separate_string_string, skip_spaces, CBuffer, Split};
use super::{byte_range, untrace, PipelineError, Traced};
use crate::commands::variables::BlockParse;

#[cfg(test)]
mod c_oracle;

/// `MAX_PEERS` in `switch_ivr_originate.c`: the most threads, groups or legs a
/// split keeps.
pub(crate) const MAX_PEERS: usize = 128;

/// `SWITCH_ENT_ORIGINATE_DELIM`.
pub(crate) const ENTERPRISE_DELIM: &str = ":_:";
/// What `switch_ivr_originate` turns a `[]` block's comma into before the leg split.
const QUOTED_ESC_COMMA: char = '\u{1}';
pub(crate) const UNQUOTED_ESC_COMMA: char = '\u{2}';

/// One leg: its blocks, the endpoint text after them, and where it sits in the
/// input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Leg {
    pub(crate) raw: Range<usize>,
    pub(crate) blocks: Vec<Block>,
    pub(crate) endpoint: String,
}

/// One `:_:` thread, or the whole dial string when there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Thread {
    pub(crate) raw: Range<usize>,
    pub(crate) blocks: Vec<Block>,
    pub(crate) groups: Vec<Vec<Leg>>,
    /// `origination_nested_vars=true` appears in the thread's text, or the `<>` event ahead of an
    /// enterprise split sets it true; either lets a value holding `${` reach the channel.
    pub(crate) nested_vars: bool,
}

/// A dial string after every pass up to the channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DialList {
    /// `<>` blocks ahead of an enterprise split, installed on every thread.
    pub(crate) blocks: Vec<Block>,
    pub(crate) threads: Vec<Thread>,
    /// A quote kept a leg separator from splitting.
    pub(crate) quote_spans_legs: bool,
    /// The dialplan carrier substitutes a `${}` or `$${}` reference with a value
    /// only the switch knows; the reference text is kept as written.
    pub(crate) carrier_expands: bool,
    /// Splits that stopped at [`MAX_PEERS`] with text left, in reading order.
    pub(crate) past_limit: Vec<PastLimit>,
}

/// A split that stopped at [`MAX_PEERS`], its last token keeping the rest of the text. The split
/// is the `:_:` one with no thread, a thread's `|` one with no group, else a group's `,` one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PastLimit {
    /// Index among the list's threads.
    pub(crate) thread: Option<usize>,
    /// Index among the thread's groups.
    pub(crate) group: Option<usize>,
    /// Non-empty tokens the last one keeps past its own, which the switch never splits off.
    pub(crate) excess: usize,
}

/// `switch_ivr_originate`'s passes over `text`, what the carrier's pass left of the input bytes
/// `raw`.
pub(crate) fn dial_list(
    text: &[Traced],
    raw: Range<usize>,
    carrier_expands: bool,
    block_parse: BlockParse,
) -> Result<DialList, PipelineError> {
    let mut reader = Reader {
        block_parse,
        legs: 0,
        quote_spans_legs: false,
        past_limit: Vec::new(),
    };
    let (blocks, threads) = if find(text, ENTERPRISE_DELIM).is_some() {
        let head = head_blocks(text, &[('<', '>')], 0, block_parse)?;
        let scanned = &head.text[..head.data_end];
        let split = separate_string_string(&scanned[head.data..], ENTERPRISE_DELIM, MAX_PEERS);
        reader.record(None, None, split.unsplit, split.excess);
        let mut threads: Vec<Thread> = split
            .spans
            .into_iter()
            .enumerate()
            .map(|(k, span)| {
                let span = head.span(k, span);
                reader.thread(
                    k,
                    &scanned[span.clone()],
                    byte_range(scanned, raw.clone(), span),
                )
            })
            .collect::<Result<_, _>>()?;
        let inherited = enterprise_nests(&head.blocks, switch_true);
        for thread in &mut threads {
            thread.nested_vars |= inherited;
        }
        (head.blocks, threads)
    } else {
        (Vec::new(), vec![reader.thread(0, text, raw)?])
    };
    Ok(DialList {
        blocks,
        threads,
        quote_spans_legs: reader.quote_spans_legs,
        carrier_expands,
        past_limit: reader.past_limit,
    })
}

/// Whether `switch_ivr_originate` takes `text` down the enterprise path, whatever quoting surrounds
/// the delimiter.
pub(crate) fn splits_into_threads(text: &str) -> bool {
    text.contains(ENTERPRISE_DELIM)
}

/// `switch_stristr` for the opt-in `switch_ivr_originate` looks for in the text it dials.
fn opts_into_nested_vars(text: &[Traced]) -> bool {
    untrace(text)
        .to_ascii_lowercase()
        .contains("origination_nested_vars=true")
}

struct Reader {
    block_parse: BlockParse,
    legs: usize,
    quote_spans_legs: bool,
    past_limit: Vec<PastLimit>,
}

impl Reader {
    /// A split at `thread` and `group` whose last token keeps `unsplit` delimiters, `excess` of
    /// them opening a non-empty token.
    fn record(
        &mut self,
        thread: Option<usize>,
        group: Option<usize>,
        unsplit: usize,
        excess: usize,
    ) {
        if unsplit > 0 {
            self.past_limit
                .push(PastLimit {
                    thread,
                    group,
                    excess,
                });
        }
    }

    /// The thread `text` at `index`, which covers the input bytes `raw`. Its groups and legs are
    /// split in one copy of the data, as `switch_ivr_originate` splits them in its own.
    fn thread(
        &mut self,
        index: usize,
        text: &[Traced],
        raw: Range<usize>,
    ) -> Result<Thread, PipelineError> {
        let head = head_blocks(text, &[('<', '>'), ('{', '}')], self.legs, self.block_parse)?;
        let scanned = &head.text[..head.data_end];
        let data = &scanned[head.data..];
        if data.is_empty() {
            return Err(PipelineError::Empty);
        }
        let mut buffer = CBuffer::new(data);
        let split = split_groups(&mut buffer);
        if split.unreadable_head {
            return Err(PipelineError::SplitSeparatorUnreadable);
        }
        self.quote_spans_legs |= split.held_delimiter;
        self.record(Some(index), None, split.unsplit, split.excess);
        let groups = split
            .tokens
            .into_iter()
            .enumerate()
            .map(|(k, group)| {
                let span = head.span(k, group.raw);
                self.group(
                    &mut buffer,
                    (index, k),
                    group.start,
                    byte_range(scanned, raw.clone(), span),
                )
            })
            .collect::<Result<_, _>>()?;
        Ok(Thread {
            raw,
            blocks: head.blocks,
            groups,
            nested_vars: opts_into_nested_vars(text),
        })
    }

    /// The group at `start` of `buffer`, `(thread, group)` in the list, which covers the input
    /// bytes `raw`. A leg the split reads past the group's terminator covers none.
    fn group(
        &mut self,
        buffer: &mut CBuffer,
        (thread, group): (usize, usize),
        start: usize,
        raw: Range<usize>,
    ) -> Result<Vec<Leg>, PipelineError> {
        scan_group(buffer, start);
        let scanned = buffer
            .c_str(start)
            .to_vec();
        let split = buffer.separate(start, ',', MAX_PEERS);
        if split.unreadable_head {
            return Err(PipelineError::SplitSeparatorUnreadable);
        }
        self.quote_spans_legs |= split.held_delimiter;
        self.record(Some(thread), Some(group), split.unsplit, split.excess);
        split
            .tokens
            .into_iter()
            .map(|leg| {
                let within = |at: usize| (at - start).min(scanned.len());
                let span = within(
                    leg.raw
                        .start,
                )
                    ..within(
                        leg.raw
                            .end,
                    );
                self.leg(buffer, leg.start, byte_range(&scanned, raw.clone(), span))
            })
            .collect()
    }

    fn leg(
        &mut self,
        buffer: &mut CBuffer,
        start: usize,
        raw: Range<usize>,
    ) -> Result<Leg, PipelineError> {
        let unclosed = PipelineError::UnclosedBlock { leg: self.legs };
        self.legs += 1;
        let mut pos = start + leading_spaces(buffer.c_str(start));
        let mut blocks = Vec::new();
        while buffer.at(pos) == '[' {
            if let Some(bend) = find_end_paren(buffer.c_str(pos), '[', ']') {
                for (c, ..) in &mut buffer.c_str_mut(pos)[1..bend] {
                    if *c == QUOTED_ESC_COMMA {
                        *c = ',';
                    }
                }
            }
            let (block, next) =
                parse_block(buffer, pos, '[', ']', UNQUOTED_ESC_COMMA, self.block_parse)
                    .ok_or(unclosed)?
                    .read()?;
            blocks.push(block);
            pos = next;
        }
        Ok(Leg {
            raw,
            blocks,
            endpoint: untrace(skip_spaces(buffer.c_str(pos))),
        })
    }
}

fn leading_spaces(text: &[Traced]) -> usize {
    text.len() - skip_spaces(text).len()
}

/// The blocks ahead of a list or thread, and where in its text they end.
struct HeadBlocks {
    blocks: Vec<Block>,
    /// The text read, as the blocks' parse left it.
    text: Vec<Traced>,
    /// The index after the last block, or 0 with none.
    end: usize,
    /// The index the data starts at, past the spaces around the blocks.
    data: usize,
    /// The index of the data's terminator.
    data_end: usize,
}

impl HeadBlocks {
    /// Token `k` of a split of the data, as indices into the whole text. The first token
    /// reaches back to the blocks, so the spaces the switch skips go with it.
    fn span(&self, k: usize, span: Range<usize>) -> Range<usize> {
        let start = match (k, span.start) {
            (0, 0) => self.end,
            (_, start) => start + self.data,
        };
        start..span.end + self.data
    }
}

/// Each kind of block in turn, as many as follow one another, after the spaces ahead of
/// `text`.
fn head_blocks(
    text: &[Traced],
    kinds: &[(char, char)],
    leg: usize,
    block_parse: BlockParse,
) -> Result<HeadBlocks, PipelineError> {
    let mut buffer = CBuffer::new(text);
    let mut pos = leading_spaces(text);
    let mut end = 0;
    let mut blocks = Vec::new();
    for &(open, close) in kinds {
        while buffer.at(pos) == open {
            let (block, next) = parse_block(&mut buffer, pos, open, close, ',', block_parse)
                .ok_or(PipelineError::UnclosedBlock { leg })?
                .read()?;
            blocks.push(block);
            pos = next;
            end = pos;
        }
    }
    let data = pos + leading_spaces(buffer.c_str(pos));
    let data_end = data
        + buffer
            .c_str(data)
            .len();
    Ok(HeadBlocks {
        blocks,
        text: buffer.into_chars(),
        end,
        data,
        data_end,
    })
}

/// The `|` split `switch_ivr_originate` cuts a thread's data into, in the buffer holding it.
fn split_groups(buffer: &mut CBuffer) -> Split {
    buffer.separate(0, '|', MAX_PEERS)
}

/// The comma pre-scan `switch_ivr_originate` runs over the group at `start` before its leg split,
/// in place.
fn scan_group(buffer: &mut CBuffer, start: usize) {
    escape_block_commas(buffer.c_str_mut(start));
}

fn escape_block_commas(group: &mut [Traced]) {
    let mut end = None;
    let mut quoted = false;
    let mut alt = false;
    for p in 0..group.len() {
        let c = group[p].0;
        if end.is_none() && c == '[' {
            end = find_end_paren(&group[p..], '[', ']').map(|e| p + e);
            alt = matches!(group.get(p + 1..p + 3), Some([('^', ..), ('^', ..)]));
            quoted = false;
        }
        if c == '\'' {
            quoted = !quoted;
        }
        let escaped = p
            .checked_sub(1)
            .is_some_and(|before| group[before].0 == '\\');
        if c == ',' && !escaped && end.is_some_and(|e| p < e) {
            group[p].0 = if quoted || alt {
                QUOTED_ESC_COMMA
            } else {
                UNQUOTED_ESC_COMMA
            };
        }
        if end == Some(p) {
            end = None;
        }
    }
}

/// The value `key` ends with on a leg's channel: `inherited` (enterprise then thread) installs after
/// the leg's own unless `local_var_clobber` says otherwise, refusing `${` values unless `nested_vars`.
pub(crate) fn resolve<'a>(
    inherited: impl IntoIterator<Item = &'a Block>,
    leg: &'a Leg,
    key: &str,
    nested_vars: bool,
) -> Option<&'a str> {
    let global = install(inherited);
    let local = install(&leg.blocks);
    let clobber = global
        .iter()
        .find(|(name, _)| same_header(name, "local_var_clobber"))
        .is_some_and(|&(_, value)| switch_true(value));
    let (first, last) = if clobber {
        (global, local)
    } else {
        (local, global)
    };
    first
        .iter()
        .chain(&last)
        .filter(|(name, value)| same_header(name, key) && (nested_vars || !names_a_variable(value)))
        .map(|&(_, value)| value)
        .next_back()
}

/// `switch_true`.
pub(crate) fn switch_true(value: &str) -> bool {
    let word = ["yes", "on", "true", "t", "enabled", "active", "allow"]
        .iter()
        .any(|word| value.eq_ignore_ascii_case(word));
    let unsigned = value
        .strip_prefix(['-', '+'])
        .unwrap_or(value);
    let number = unsigned
        .chars()
        .all(|c| c == '.' || c.is_ascii_digit());
    let nonzero = unsigned
        .chars()
        .take_while(char::is_ascii_digit)
        .any(|c| c != '0');
    word || (number && nonzero)
}
