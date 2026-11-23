//! Port of the escape handling `switch_channel_expand_variables_check` runs over a dialplan
//! application's argument, and of the switch's test for a value naming a variable.

use std::ops::Range;

use super::brackets::{install, same_header, Block};
use super::Traced;
use crate::commands::variables::DialStringTarget;

/// A quote reading bare once `passes` passes at `target` have run: the dialplan carrier's expansion
/// deletes a `\'` outright, where every other pass unescapes it.
pub(crate) fn quote_bare_after(target: DialStringTarget, passes: u32) -> String {
    let consumed_by_carrier = if target
        .carrier()
        .expands()
    {
        2
    } else {
        1
    };
    let run = (1usize << passes) - consumed_by_carrier;
    format!("{}'", "\\".repeat(run))
}

/// Expansion drops the first `$` of a `$$` opening no reference and substitutes a reference.
/// `\$` keeps it only while expansion runs, which a leading `\'` guarantees and expansion then
/// deletes. Text naming a variable is left to the switch, whichever field carries it.
pub(crate) fn protects_dollars(value: &str, target: DialStringTarget) -> bool {
    target
        .carrier()
        .expands()
        && value.contains("$$")
        && !names_a_variable(value)
}

#[cfg(all(test, freeswitch_oracle))]
mod c_oracle;

/// The escape handling of `switch_channel_expand_variables_check`, and the spans of its output
/// holding a reference the switch substitutes.
pub(crate) fn expand_escapes(text: &[Traced]) -> (Vec<Traced>, Vec<Range<usize>>) {
    let has_escaped_data = text
        .windows(2)
        .any(|w| w[0].0 == '\\' && matches!(w[1].0, '\\' | 'n' | 's' | 't' | '\''));
    if !has_escaped_data
        && !var_check_const(
            text.iter()
                .map(|&(c, ..)| c),
        )
    {
        return (text.to_vec(), Vec::new());
    }
    let at = |i: usize| {
        text.get(i)
            .map(|&(c, ..)| c)
    };
    let mut out = Vec::with_capacity(text.len());
    let mut references = Vec::new();
    let mut p = 0;
    while let Some(c) = at(p) {
        match (c, at(p + 1)) {
            ('\\', Some('$')) => {
                p += 1;
                if at(p + 1) == Some('$') {
                    p += 1;
                }
            }
            ('\\', Some('\'')) => {
                p += 2;
                continue;
            }
            ('\\', Some('\\')) => {
                out.push((c, text[p].1, text[p + 1].2));
                p += 2;
                continue;
            }
            ('$', _) => {
                let reference = p;
                if at(p + 1) == Some('$') {
                    p += 1;
                }
                if at(p + 1) == Some('{') {
                    p = reference_end(text, p + 2);
                    let written = out.len();
                    out.extend_from_slice(&text[reference..p]);
                    references.push(written..out.len());
                    // The char after the reference is copied unescaped, unless it
                    // opens another reference.
                    if at(p).is_some_and(|next| next != '$') {
                        out.push(text[p]);
                        p += 1;
                    }
                    continue;
                }
            }
            _ => {}
        }
        out.push(text[p]);
        p += 1;
    }
    (out, references)
}

/// The index after the `}` closing a reference whose name starts at `start`.
fn reference_end(text: &[Traced], start: usize) -> usize {
    let mut depth = 1usize;
    for (e, &(c, ..)) in text
        .iter()
        .enumerate()
        .skip(start)
    {
        if depth == 1 && c == '}' {
            return e + 1;
        }
        if c == '{' && e != start {
            depth += 1;
        } else if depth > 1 && c == '}' {
            depth -= 1;
        }
    }
    text.len()
}

/// `switch_string_var_check_const`: whether a value names a variable, which the
/// switch refuses to install while `origination_nested_vars` is off.
pub(crate) fn names_a_variable(value: &str) -> bool {
    var_check_const(value.chars())
}

fn var_check_const(chars: impl IntoIterator<Item = char>) -> bool {
    let mut dollar = false;
    for c in chars {
        if c == '$' {
            dollar = true;
        } else if dollar && c == '{' {
            return true;
        } else if c != '\\' {
            dollar = false;
        }
    }
    false
}

/// The `<>` event `switch_ivr_enterprise_originate` hands every thread holds an
/// `origination_nested_vars` that `switch_true` reads as true, as `switch_event_get_header` finds it.
pub(crate) fn enterprise_nests<'a>(
    blocks: impl IntoIterator<Item = &'a Block>,
    switch_true: impl Fn(&str) -> bool,
) -> bool {
    install(blocks)
        .into_iter()
        .find(|(name, _)| same_header(name, "origination_nested_vars"))
        .is_some_and(|(_, value)| switch_true(value))
}
