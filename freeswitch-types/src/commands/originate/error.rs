//! What refuses an originate command, its endpoint or its target.

use std::num::ParseIntError;

use crate::channel::ParseHangupCauseError;
use crate::commands::endpoint::{EndpointFieldFault, ParseGroupCallOrderError};
use crate::commands::execute_on::ExecuteOnFault;
use crate::commands::variables::InvalidArgvSeparator;

/// Errors from originate command parsing or construction.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OriginateError {
    /// A single-quoted token was never closed.
    UnclosedQuote(String),
    /// General parse failure with a description.
    ParseError(String),
    /// Inline originate requires at least one application.
    EmptyInlineApplications,
    /// Extension target cannot use inline dialplan.
    ExtensionWithInlineDialplan,
    /// A dial string carried a variable block for an endpoint type that has
    /// nowhere to keep it, such as `error/`. Carries that type's name.
    VariablesNotSupported(&'static str),
    /// The timeout argument is not a whole number of seconds.
    InvalidTimeout {
        /// The rejected token.
        value: String,
        /// Why it is not a number.
        source: ParseIntError,
    },
    /// An `error/` endpoint named a cause this crate does not know.
    UnknownHangupCause {
        /// The rejected token.
        value: String,
        /// The hangup-cause parse failure.
        source: ParseHangupCauseError,
    },
    /// A `group_call` expression carried an unknown order suffix.
    UnknownGroupCallOrder {
        /// The rejected token.
        value: String,
        /// The order parse failure.
        source: ParseGroupCallOrderError,
    },
    /// A dial string whose leading path segment names no endpoint type.
    UnknownEndpointType(String),
    /// An inline separator the hunt's split breaks on, or `:`. Carries it.
    InvalidInlineDelimiter(char),
    /// A `^^` argument separator [`DialStringTarget::with_argv_separator`](crate::commands::DialStringTarget::with_argv_separator)
    /// refuses.
    InvalidArgvSeparator(InvalidArgvSeparator),
    /// A positional argument reads `undef`, which the switch takes as absent. Names the field.
    UndefPositional(&'static str),
    /// An extension opens with `&` and more, which `originate` runs as an application.
    ExtensionReadsAsApplication,
    /// An `&name(args)` application whose name carries a parenthesis or whose arguments carry
    /// `)`, where `originate` ends the arguments. [`Originate::inline`](super::Originate::inline)
    /// delivers such arguments.
    ParenthesisInApplication {
        /// The application name.
        application: String,
    },
    /// Inline applications under a dialplan other than `inline`, which transfers them as an
    /// extension.
    InlineApplicationsWithDialplan,
    /// An endpoint field the switch reads as something else, whatever the escaping.
    UndeliverableEndpointField {
        /// The endpoint type.
        endpoint: &'static str,
        /// The field.
        field: &'static str,
        /// What the switch does with it.
        fault: EndpointFieldFault,
    },
    /// A `[` in one leg's endpoint text closes in a later leg of the same group, so the switch
    /// merges the legs between or rewrites a later leg's block. Names the group.
    BracketSpansLegs {
        /// Index of the group.
        group: usize,
    },
    /// A `sofia_contact` or `group_call` expression at the API carrier, where nothing expands it:
    /// the switch dials the text as an unknown endpoint.
    UnexpandedExpression {
        /// The endpoint type.
        endpoint: &'static str,
    },
    /// An [`ExecuteOn`](crate::commands::ExecuteOn) name or argument the application does not
    /// receive as written.
    UndeliverableExecuteOn {
        /// The part of the value.
        part: &'static str,
        /// What the switch does with it.
        fault: ExecuteOnFault,
    },
    /// More legs in a group, or more groups, than the switch splits off: the last keeps the rest
    /// of the text, separators included, and nothing past it is dialled.
    TooManyLegs {
        /// Index of the group, or `None` when the groups are too many.
        group: Option<usize>,
        /// The most legs or groups the switch splits off.
        max: usize,
    },
}

impl std::fmt::Display for OriginateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnclosedQuote(s) => {
                write!(f, "unclosed quote in the final token ({} bytes)", s.len())
            }
            Self::ParseError(s) => write!(f, "parse error: {s}"),
            Self::EmptyInlineApplications => {
                f.write_str("inline originate requires at least one application")
            }
            Self::ExtensionWithInlineDialplan => {
                f.write_str("extension target is incompatible with inline dialplan")
            }
            Self::VariablesNotSupported(kind) => {
                write!(f, "a {kind} endpoint carries no variable block")
            }
            Self::InvalidTimeout { value, .. } => write!(
                f,
                "timeout is not a whole number of seconds ({} bytes)",
                value.len()
            ),
            Self::UnknownHangupCause { value, .. } => write!(
                f,
                "unknown hangup cause in an error endpoint ({} bytes)",
                value.len()
            ),
            Self::UnknownGroupCallOrder { value, .. } => {
                write!(f, "unknown group_call order suffix ({} bytes)", value.len())
            }
            Self::InvalidInlineDelimiter(_) => {
                f.write_str("the named separator cannot separate an inline action list")
            }
            Self::UnknownEndpointType(s) => {
                write!(f, "unknown endpoint type ({} bytes)", s.len())
            }
            Self::InvalidArgvSeparator(_) => f.write_str("unusable originate argument separator"),
            Self::UndefPositional(field) => write!(
                f,
                "{field} reads as the undef placeholder, which the switch takes as absent"
            ),
            Self::ExtensionReadsAsApplication => f.write_str(
                "the extension opens with & and more, which originate runs as an application",
            ),
            Self::ParenthesisInApplication { .. } => f.write_str(
                "an application's name carries a parenthesis or its arguments carry ), \
                 where originate ends the arguments; Originate::inline delivers such \
                 arguments in an inline action list",
            ),
            Self::InlineApplicationsWithDialplan => f.write_str(
                "inline applications run only under the inline dialplan; \
                 any other transfers them as an extension",
            ),
            Self::UndeliverableEndpointField {
                endpoint,
                field,
                fault,
            } => write!(f, "the {field} of a {endpoint} endpoint {fault}"),
            Self::BracketSpansLegs { group } => write!(
                f,
                "a bracket in group {group} closes in a later leg, which the switch reads across the legs"
            ),
            Self::UnexpandedExpression { endpoint } => write!(
                f,
                "a {endpoint} expression reaches originate unexpanded at the API carrier; \
                 only a dialplan application or the expand API expands it"
            ),
            Self::UndeliverableExecuteOn { part, fault } => {
                write!(f, "the {part} of an execute_on value {fault}")
            }
            Self::TooManyLegs {
                group: Some(group),
                max,
            } => write!(
                f,
                "group {group} has more legs than the {max} the switch splits off; \
                 it dials the rest as the last leg's endpoint"
            ),
            Self::TooManyLegs { group: None, max } => write!(
                f,
                "more groups than the {max} the switch splits off; \
                 it dials the rest as the last group"
            ),
        }
    }
}

impl std::error::Error for OriginateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidTimeout { source, .. } => Some(source),
            Self::UnknownHangupCause { source, .. } => Some(source),
            Self::UnknownGroupCallOrder { source, .. } => Some(source),
            Self::InvalidArgvSeparator(source) => Some(source),
            Self::UnclosedQuote(_)
            | Self::ParseError(_)
            | Self::EmptyInlineApplications
            | Self::ExtensionWithInlineDialplan
            | Self::VariablesNotSupported(_)
            | Self::UnknownEndpointType(_)
            | Self::InvalidInlineDelimiter(_)
            | Self::UndefPositional(_)
            | Self::ExtensionReadsAsApplication
            | Self::ParenthesisInApplication { .. }
            | Self::InlineApplicationsWithDialplan
            | Self::UndeliverableEndpointField { .. }
            | Self::BracketSpansLegs { .. }
            | Self::UnexpandedExpression { .. }
            | Self::UndeliverableExecuteOn { .. }
            | Self::TooManyLegs { .. } => None,
        }
    }
}
