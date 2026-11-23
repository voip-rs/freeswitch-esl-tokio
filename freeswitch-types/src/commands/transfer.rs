//! The argument check every transfer builder runs: the `transfer` dialplan application and the
//! `uuid_transfer` API both split their argument on blanks, honouring `'` quotes and `\`
//! escapes, and read a leading token as the leg to transfer.
//!
//! The two differ in that token. The application takes `-bleg` or `-both` in any case as its
//! first argument; `uuid_transfer` takes any destination opening `-`, then skips the `-`.
//! Only the application's extension opens its line, so only it can carry a `^^X` delimiter head.

use std::fmt;

use super::{originate_split, originate_unquote};

/// A `transfer` argument field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransferField {
    /// The destination extension, first.
    Extension,
    /// The dialplan name, second.
    Dialplan,
    /// The context, third.
    Context,
}

impl fmt::Display for TransferField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TransferField::Extension => "extension",
            TransferField::Dialplan => "dialplan",
            TransferField::Context => "context",
        })
    }
}

/// A transfer argument the switch would read as something else.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransferArgumentError {
    /// The switch splits the argument on blanks, honouring `'` quotes and `\` escapes, and this
    /// field would not come back as itself.
    Unsplittable {
        /// Which field.
        field: TransferField,
        /// The field as given.
        value: String,
    },
    /// An extension the switch reads as the leg to transfer: `-bleg` or `-both` in any case for
    /// the `transfer` application, any leading `-` for `uuid_transfer`.
    LegSelector {
        /// The extension as given.
        extension: String,
    },
}

impl fmt::Display for TransferArgumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsplittable { field, value } => write!(
                f,
                "transfer {field} {value:?} does not survive the switch's argument split"
            ),
            Self::LegSelector { extension } => {
                write!(
                    f,
                    "transfer extension {extension:?} is read as a leg selector"
                )
            }
        }
    }
}

impl std::error::Error for TransferArgumentError {}

/// Refuse `transfer` application arguments, in order, that the switch would read as something
/// else.
pub fn check_transfer_arguments(
    fields: &[(TransferField, &str)],
) -> Result<(), TransferArgumentError> {
    check_fields(fields, TransferLine::Application)
}

/// Which command line the fields follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferLine {
    /// The `transfer` application's argument, which the extension opens.
    Application,
    /// `uuid_transfer`'s argument, after the channel UUID.
    UuidTransfer,
}

/// First arguments the `transfer` application reads as the leg to transfer, in any case.
const LEG_SELECTORS: [&str; 2] = ["-bleg", "-both"];

impl TransferLine {
    fn reads_leg_selector(self, extension: &str) -> bool {
        match self {
            Self::Application => LEG_SELECTORS
                .iter()
                .any(|s| extension.eq_ignore_ascii_case(s)),
            Self::UuidTransfer => extension.starts_with('-'),
        }
    }
}

pub(crate) fn check_fields(
    fields: &[(TransferField, &str)],
    line: TransferLine,
) -> Result<(), TransferArgumentError> {
    if let Some((_, extension)) = fields.first() {
        if line.reads_leg_selector(extension) {
            return Err(TransferArgumentError::LegSelector {
                extension: extension.to_string(),
            });
        }
    }
    for (index, (field, value)) in fields
        .iter()
        .enumerate()
    {
        let opens_line = index == 0 && line == TransferLine::Application;
        if !survives_blank_split(value, opens_line) {
            return Err(TransferArgumentError::Unsplittable {
                field: *field,
                value: value.to_string(),
            });
        }
    }
    Ok(())
}

/// Whether `value` comes back from the switch's blank split as one token equal to itself. Only
/// the line's first token can open with a `^^X` delimiter head, so a later one is checked behind
/// a stand-in token.
fn survives_blank_split(value: &str, first: bool) -> bool {
    let line = if first {
        value.to_string()
    } else {
        format!("x {value}")
    };
    let Ok(tokens) = originate_split(&line, ' ') else {
        return false;
    };
    let read: Vec<String> = tokens
        .iter()
        .skip(usize::from(!first))
        .map(|t| originate_unquote(t))
        .collect();
    read == [value]
}
