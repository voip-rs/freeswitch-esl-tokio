//! Port of `switch_event_create_brackets`, which reads the pairs of a `<>`, `{}` or `[]` block out
//! of the buffer it is handed, and of the originate event those pairs install into.

use super::api_argument::breaks_a_split;
use super::separate::{find_end_paren, CBuffer, Split};
use super::{untrace, PipelineError, Traced};
use crate::commands::originate::OriginateError;
use crate::commands::variables::{BlockParse, VariablesType};

#[cfg(test)]
pub(super) mod c_oracle;

/// Why `open` and `close` in `text` move the end the switch counts its way to.
pub(crate) fn unbalanced(text: &str, (open, close): (char, char)) -> Option<String> {
    let mut depth = 0i32;
    for ch in text.chars() {
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth < 0 {
                return Some(format!(
                    "closes a '{open}' it never opened, ending the block early"
                ));
            }
        }
    }
    (depth != 0).then(|| format!("opens a '{open}' it never closes, swallowing the block's end"))
}

/// Reject a separator that cannot delimit the block it was chosen for.
///
/// Beyond what [`breaks_a_split`], either bracket moves the end the switch counts its way to,
/// `=` splits the pair instead, `^` leaves the `^^` prefix reading as its own separator, and
/// `|` in a `[]` block is read by the leg split before the block is parsed. Dialplan expansion
/// reads `$` then `{` across a pair boundary as a reference, so neither separates.
pub(crate) fn check_separator(sep: char, vars_type: VariablesType) -> Result<(), OriginateError> {
    let (open, close) = vars_type.delimiters();
    if breaks_a_split(sep)
        || sep == open
        || sep == close
        || matches!(sep, '=' | '^' | '$' | '{')
        || (sep == '|' && vars_type == VariablesType::Channel)
    {
        return Err(OriginateError::ParseError(format!(
            "invalid ^^ separator: '{sep}'"
        )));
    }
    Ok(())
}

/// `var_array` in `switch_event_create_brackets`.
const BLOCK_PAIRS: usize = 1024;

/// What installing one pair of a block does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PairEffect {
    /// A value is set.
    Set(String),
    /// The pair has no value by the `=` split, so nothing is installed.
    Ignored,
    /// An empty value is installed, which deletes an earlier value.
    Cleared,
    /// A `^^` head names a non-ASCII separator, which the split takes as its first byte; whatever
    /// it installs no string carries.
    Unreadable,
    /// The pair ends at its `=`, whose empty field the split drops, so nothing is installed.
    Valueless,
}

/// One pair, keyed by the name the switch installs it under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pair {
    pub(crate) key: String,
    pub(crate) effect: PairEffect,
}

/// One `<>`, `{}` or `[]` block as the switch parsed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Block {
    pub(crate) open: char,
    pub(crate) separator: char,
    pub(crate) pairs: Vec<Pair>,
    /// The parse wrote into the text after the close, which the switch then reads rewritten.
    pub(crate) rewrites_following_text: bool,
}

/// What `switch_event_create_brackets` read of a block.
pub(crate) struct Parsed {
    pub(crate) block: Block,
    /// The index after the close.
    pub(crate) next: usize,
    /// A non-ASCII separator's split runs past the close, splitting what follows by byte.
    splits_past_close_by_byte: bool,
}

impl Parsed {
    /// The block, or the refusal of a split no string carries.
    pub(crate) fn read(self) -> Result<(Block, usize), PipelineError> {
        if self.splits_past_close_by_byte {
            return Err(PipelineError::SplitSeparatorUnreadable);
        }
        Ok((self.block, self.next))
    }
}

/// `switch_event_create_brackets` of `block_parse` on the block opening at `at` of `buffer`,
/// rewriting the buffer as the switch does.
pub(crate) fn parse_block(
    buffer: &mut CBuffer,
    at: usize,
    open: char,
    close: char,
    comma: char,
    block_parse: BlockParse,
) -> Option<Parsed> {
    let end = at + find_end_paren(buffer.c_str(at), open, close)?;
    let separator = match buffer
        .c_str(at)
        .get(1..end - at)?
    {
        // `switch_ivr_originate`'s comma scan can turn a `^^,` head into the `[]` default.
        [('^', ..), ('^', ..), (picked, ..), ..] if *picked == comma => ',',
        [('^', ..), ('^', ..), (picked, ..), ..] => *picked,
        [('^', ..), ('^', ..)] => '\0',
        _ => ',',
    };
    let mut block = Block {
        open,
        separator,
        pairs: Vec::new(),
        rewrites_following_text: false,
    };
    let next = end + 1;
    if block.separator_unreadable() {
        let escaped = escapes_its_end(
            buffer
                .c_str(at)
                .get(4..end - at)
                .unwrap_or_default(),
        );
        let chains_a_block = following_block(buffer, next, open, close).is_some();
        return Some(Parsed {
            block,
            next,
            splits_past_close_by_byte: escaped || chains_a_block,
        });
    }
    let following = chars(buffer.c_str(next));
    buffer.terminate(end);
    let (mut data, mut delim) = (at + 1, comma);
    let mut second = following_block(buffer, next, open, close);
    let mut reads_past_by_byte = false;
    loop {
        if buffer.at(data) == '^' && buffer.at(data + 1) == '^' {
            delim = buffer.at(data + 2);
            data += 3;
        }
        // A split on a non-ASCII head's first byte steps over an escaped terminator into the text
        // after the close, which the port's split on the given separator would cut instead.
        let escaped = escapes_its_end(buffer.c_str(data));
        let split = block_split(buffer, data, delim, BLOCK_PAIRS, block_parse);
        if split.unreadable_head {
            reads_past_by_byte |= escaped;
            block
                .pairs
                .push(Pair {
                    key: untrace(buffer.c_str(data)),
                    effect: PairEffect::Unreadable,
                });
        } else {
            for token in split.tokens {
                let escaped = escapes_its_end(buffer.c_str(token.start));
                let pair = pair(buffer, token.start, block_parse);
                reads_past_by_byte |= escaped && pair.effect == PairEffect::Unreadable;
                block
                    .pairs
                    .push(pair);
            }
        }
        match second.take() {
            Some(close) => data = close,
            None => break,
        }
    }
    block.rewrites_following_text = chars(buffer.c_str(next)) != following;
    Some(Parsed {
        block,
        next,
        splits_past_close_by_byte: reads_past_by_byte,
    })
}

/// `text` ends in an odd run of backslashes, the last escaping its terminator.
fn escapes_its_end(text: &[Traced]) -> bool {
    text.iter()
        .rev()
        .take_while(|&&(c, ..)| c == '\\')
        .count()
        % 2
        == 1
}

fn chars(text: &[Traced]) -> Vec<char> {
    text.iter()
        .map(|&(c, ..)| c)
        .collect()
}

/// Where `switch_event_create_brackets` reads more pairs after the first: the close of a block
/// opening right after an opener that follows the close past spaces.
fn following_block(buffer: &CBuffer, after: usize, open: char, close: char) -> Option<usize> {
    let rest = buffer.c_str(after);
    let opener = match rest
        .iter()
        .position(|&(c, ..)| c != ' ')
    {
        _ if rest.is_empty() => return None,
        Some(at) if rest[at].0 == open => after + at,
        Some(_) => return None,
        None => after + rest.len(),
    };
    find_end_paren(buffer.c_str(opener + 1), open, close).map(|at| opener + 1 + at)
}

impl Block {
    /// A non-ASCII `^^` separator, which the switch reads as its first UTF-8 byte and
    /// which no char split mirrors; such a block carries no pairs.
    pub(crate) fn separator_unreadable(&self) -> bool {
        !self
            .separator
            .is_ascii()
    }
}

/// One of the block's own splits, on its separator or on `=`, in place.
fn block_split(
    buffer: &mut CBuffer,
    index: usize,
    delim: char,
    limit: usize,
    block_parse: BlockParse,
) -> Split {
    match block_parse {
        BlockParse::PairSplitCleans => buffer.separate(index, delim, limit),
    }
}

/// The pair the `=` split reads of the token at `index`, in place.
fn pair(buffer: &mut CBuffer, index: usize, block_parse: BlockParse) -> Pair {
    let token = untrace(buffer.c_str(index));
    let end = index
        + buffer
            .c_str(index)
            .len();
    let split = block_split(buffer, index, '=', 2, block_parse);
    if split.unreadable_head {
        return Pair {
            key: token,
            effect: PairEffect::Unreadable,
        };
    }
    let fields: Vec<usize> = split
        .tokens
        .iter()
        .map(|field| field.start)
        .collect();
    let key = fields
        .first()
        .map_or_else(String::new, |&at| untrace(buffer.c_str(at)));
    let effect = match fields[..] {
        [_, value] => match untrace(buffer.c_str(value)) {
            value if value.is_empty() => PairEffect::Cleared,
            value => PairEffect::Set(value),
        },
        [_] if split
            .tokens
            .first()
            .is_some_and(|field| {
                field
                    .raw
                    .end
                    < end
            }) =>
        {
            PairEffect::Valueless
        }
        _ => PairEffect::Ignored,
    };
    Pair { key, effect }
}

/// Whether two names are one header of an originate event, which compares them by `strcasecmp`.
pub(crate) fn same_header(name: &str, other: &str) -> bool {
    name.eq_ignore_ascii_case(other)
}

/// The headers of an originate event, which carries `EF_UNIQ_HEADERS`: a set replaces every header
/// of that name ignoring case, an empty value deletes them.
pub(crate) fn install<'a>(blocks: impl IntoIterator<Item = &'a Block>) -> Vec<(&'a str, &'a str)> {
    let mut headers: Vec<(&str, &str)> = Vec::new();
    for pair in blocks
        .into_iter()
        .flat_map(|block| &block.pairs)
    {
        match &pair.effect {
            PairEffect::Set(value) => set_header(&mut headers, &pair.key, value),
            PairEffect::Cleared => headers.retain(|(name, _)| !same_header(name, &pair.key)),
            PairEffect::Ignored | PairEffect::Unreadable | PairEffect::Valueless => {}
        }
    }
    headers
}

/// `switch_event_add_header_string` at `SWITCH_STACK_BOTTOM` on an event carrying
/// `EF_UNIQ_HEADERS`, as originate events and a channel's variables do.
pub(crate) fn set_header<'a>(headers: &mut Vec<(&'a str, &'a str)>, name: &'a str, value: &'a str) {
    headers.retain(|(held, _)| !same_header(held, name));
    headers.push((name, value));
}
