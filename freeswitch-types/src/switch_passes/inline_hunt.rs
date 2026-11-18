//! Port of `inline_dialplan_hunt`, which splits an inline action list into applications, and the
//! render that escapes each action for it.

use super::api_argument::{breaks_a_split, write_escaped};
use super::separate::separate;
use super::{trace, untrace};
use crate::commands::originate::{Application, DialplanType, OriginateError};

#[cfg(test)]
mod c_oracle;

/// The separator an inline action list uses unless one is named.
pub(crate) const DEFAULT_INLINE_DELIMITER: char = ',';

/// Reject an inline separator the hunt's split breaks on, or `:`, where it splits an application
/// from its data.
pub(crate) fn check_inline_delimiter(delimiter: char) -> Result<(), OriginateError> {
    if delimiter == ':' || breaks_a_split(delimiter) {
        return Err(OriginateError::InvalidInlineDelimiter(delimiter));
    }
    Ok(())
}

/// Split an `m:<delim>:` prefix off an inline action list.
///
/// `inline_dialplan_hunt` reads exactly four bytes for this, so anything longer
/// or shorter is part of the first application rather than a prefix.
pub(crate) fn split_inline_prefix(s: &str) -> (Option<char>, &str) {
    let bytes = s.as_bytes();
    match bytes {
        [b'm', b':', delimiter, b':', ..] if delimiter.is_ascii() => {
            (Some(*delimiter as char), &s[4..])
        }
        _ => (None, s),
    }
}

/// `argv` in `inline_dialplan_hunt`: the most actions its split keeps.
const INLINE_ACTIONS: usize = 128;

/// The actions `inline_dialplan_hunt` splits an action list into, each through its cleanup.
pub(crate) fn split_inline_actions(s: &str, delimiter: char) -> Vec<String> {
    separate(&trace(s), delimiter, INLINE_ACTIONS)
        .tokens
        .iter()
        .map(|token| untrace(&token.text))
        .collect()
}

/// Render `apps` as one inline action list, each action escaped once for the split
/// `inline_dialplan_hunt` runs on `delimiter`, whose cleanup reads the escapes and trims
/// the action's edges; the originate line's own split escapes the whole list again.
pub(crate) fn render_inline(apps: &[Application], delimiter: char) -> String {
    let mut list = String::new();
    for (i, app) in apps
        .iter()
        .enumerate()
    {
        if i > 0 {
            list.push(delimiter);
        }
        // Writing to a String cannot fail.
        let _ = write_escaped(
            &mut list,
            delimiter,
            app.to_string_with_dialplan(&DialplanType::INLINE),
        );
    }
    list
}
