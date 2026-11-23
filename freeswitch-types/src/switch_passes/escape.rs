//! The escapes a render writes so a text reaches the channel as written through every pass its
//! target applies ahead of it.

use super::expansion::{protects_dollars, quote_bare_after};
use crate::commands::variables::{DialStringTarget, VariablesType};

/// `switch_ivr_originate` cuts a thread into groups on `|` and a group into legs on `,`, each
/// through `cleanup_separated_string`.
const LEG_SPLIT_PASSES: u32 = 2;

/// Text escaped for the passes that read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EscapedField {
    /// A pair's value in a block of `scope`, whose pairs split on commas when `commas_separate`.
    Value {
        scope: VariablesType,
        commas_separate: bool,
    },
    /// A leg's text after its blocks, which the leg splits read and the endpoint module parses.
    Endpoint,
    /// A pair's key, which meets the value's passes and ends the `=` split.
    Key {
        scope: VariablesType,
        commas_separate: bool,
    },
}

impl EscapedField {
    fn escapes_comma(self) -> bool {
        match self {
            Self::Value {
                commas_separate, ..
            }
            | Self::Key {
                commas_separate, ..
            } => commas_separate,
            Self::Endpoint => true,
        }
    }

    fn escapes_pipe(self) -> bool {
        match self {
            Self::Value { scope, .. } | Self::Key { scope, .. } => scope == VariablesType::Channel,
            Self::Endpoint => true,
        }
    }
}

/// A `[]` block rides through the `|` and `,` leg splits before the block
/// parse, and both consume escapes.
fn leg_split_passes(scope: VariablesType) -> u32 {
    match scope {
        VariablesType::Enterprise | VariablesType::Default => 0,
        VariablesType::Channel => LEG_SPLIT_PASSES,
    }
}

/// The escape-consuming passes `field` meets at `target`.
fn passes(target: DialStringTarget, field: EscapedField) -> u32 {
    target.argument_passes()
        + match field {
            EscapedField::Value { scope, .. } | EscapedField::Key { scope, .. } => {
                target
                    .block_parse()
                    .cleanup_passes()
                    + leg_split_passes(scope)
            }
            EscapedField::Endpoint => LEG_SPLIT_PASSES,
        }
}

/// Every pass halves a run of backslashes, so a literal one needs 2^passes.
fn backslash_escape(target: DialStringTarget, field: EscapedField) -> String {
    "\\".repeat(1 << passes(target, field))
}

/// The last pass trims the text's edges, so an edge space must read `\s` entering it.
fn space_escape(target: DialStringTarget, field: EscapedField) -> String {
    format!("{}s", "\\".repeat(1 << (passes(target, field) - 1)))
}

/// A quote must still read `\'` entering the last pass, or bare after a
/// carrier pass that deletes `\'`.
fn quote_escape(target: DialStringTarget, field: EscapedField) -> String {
    quote_bare_after(target, passes(target, field))
}

/// An empty `''` reaching bare the scan `switch_ivr_originate` runs over a `[]` block after
/// the `|` leg split, which protects a comma only when the byte before it is no backslash.
fn channel_comma_guard(target: DialStringTarget) -> String {
    quote_bare_after(target, target.argument_passes() + 1).repeat(2)
}

/// An empty `''` reaching the `=` split bare ahead of a key opening `^^`, which would otherwise
/// name that split's separator, or the block's when the key is first.
fn caret_guard(target: DialStringTarget, field: EscapedField) -> String {
    quote_bare_after(target, passes(target, field) - 1).repeat(2)
}

/// Escape a value for the wire, escaping the comma only when `commas_separate`
/// says it is this block's separator; a `^^` block separates on something else
/// and refuses a value carrying it, so a comma there is ordinary text. A `[]`
/// block also escapes the pipe, which the leg split would otherwise read.
pub(crate) fn escape_value(
    value: &str,
    target: DialStringTarget,
    commas_separate: bool,
    vars_type: VariablesType,
) -> String {
    escape_text(
        value,
        target,
        EscapedField::Value {
            scope: vars_type,
            commas_separate,
        },
    )
}

/// Escape `text` for every pass `field` meets at `target`.
pub(crate) fn escape_text(text: &str, target: DialStringTarget, field: EscapedField) -> String {
    let value = text;
    // The backslash goes first, or the ones the other rules introduce get
    // escaped in turn.
    let escaped = value
        .replace('\\', &backslash_escape(target, field))
        .replace('\'', &quote_escape(target, field));
    let dollars = protects_dollars(value, target);
    let escaped = if dollars {
        escaped.replace('$', "\\$")
    } else {
        escaped
    };
    // `\,` and `\|` keep one backslash: a pass consumes `\x` only before a
    // quote, a backslash, a named escape or that pass's own delimiter.
    let escaped = if field.escapes_comma() {
        escaped.replace(',', "\\,")
    } else {
        escaped
    };
    let escaped = if field.escapes_pipe() {
        escaped.replace('|', "\\|")
    } else {
        escaped
    };
    let escaped = match field {
        EscapedField::Key { .. } => escaped.replace('=', "\\="),
        EscapedField::Value { .. } | EscapedField::Endpoint => escaped,
    };
    let escaped = match field {
        EscapedField::Value {
            scope: VariablesType::Channel,
            commas_separate,
        } => guard_channel_commas(
            escaped,
            commas_separate && value.ends_with('\\'),
            target,
            commas_separate,
        ),
        EscapedField::Key {
            scope: VariablesType::Channel,
            commas_separate,
        } => guard_channel_commas(escaped, false, target, commas_separate),
        EscapedField::Value { .. } | EscapedField::Key { .. } | EscapedField::Endpoint => escaped,
    };
    let space = space_escape(target, field);
    let escaped = match escaped.strip_prefix(' ') {
        Some(rest) => format!("{space}{rest}"),
        None => escaped,
    };
    let escaped = match escaped.strip_suffix(' ') {
        Some(rest) => format!("{rest}{space}"),
        None => escaped,
    };
    let escaped = match field {
        EscapedField::Key { .. } if value.starts_with("^^") => {
            format!("{}{escaped}", caret_guard(target, field))
        }
        EscapedField::Value { .. } | EscapedField::Key { .. } | EscapedField::Endpoint => escaped,
    };
    let escaped = if dollars {
        format!("\\'{escaped}")
    } else {
        escaped
    };
    if escaped.contains(' ') {
        format!("'{}'", escaped)
    } else {
        escaped
    }
}

/// Put the guard between a backslash and the comma after it in a `[]` field: the separator after
/// a value ending in one (`separator_follows`), or a literal comma in a `^^` block. Channel scope
/// refuses a quote.
fn guard_channel_commas(
    escaped: String,
    separator_follows: bool,
    target: DialStringTarget,
    commas_separate: bool,
) -> String {
    let guard = channel_comma_guard(target);
    if !commas_separate {
        let backslash = backslash_escape(
            target,
            EscapedField::Value {
                scope: VariablesType::Channel,
                commas_separate,
            },
        );
        escaped.replace(&format!("{backslash},"), &format!("{backslash}{guard},"))
    } else if separator_follows {
        escaped + &guard
    } else {
        escaped
    }
}
