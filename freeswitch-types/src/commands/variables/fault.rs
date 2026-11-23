//! Why a channel variable cannot be delivered as written.
//!
//! One shape serves every path that refuses a variable: a block's builder, its parser and config
//! load through [`OriginateError::UndeliverableVariable`](crate::OriginateError::UndeliverableVariable),
//! and `uuid_setvar` directly. The [`Display`](fmt::Display) impls name a variable only when the
//! fault lies in its value, never quote a value, and never quote a name the fault lies in.

use std::fmt;

use crate::commands::originate::OriginateError;
use crate::switch_passes::originate_legs::ENTERPRISE_DELIM;
use crate::variables::names_outgoing_header;
use crate::wire_safety::contains_header_control;

/// Which half of a `name=value` pair a [`VariableFault`] lies in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum VariablePart {
    /// The variable's name.
    Name,
    /// The variable's value.
    Value,
}

/// What the switch does with a variable name or value, whatever the escaping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum VariableFault {
    /// Empty. The switch installs a value under no name, and drops an empty value without
    /// logging it, so the pair cannot be told from an absent one.
    Empty,
    /// Carries `:_:`, on which the switch splits the dial string into threads.
    EnterpriseSeparator,
    /// Carries a single quote in a `[]` block, which the switch pairs with the next quote in the
    /// dial string before it parses the block.
    QuoteInChannelScope,
    /// Closes the block's bracket, carried here, without opening it, ending the block early.
    ClosesUnopenedBracket(char),
    /// Opens the block's bracket, carried here, without closing it, swallowing the block's end.
    UnclosedBracket(char),
    /// Carries the block's `^^` separator, carried here, which splits the pair.
    BlockSeparator(char),
    /// A name carrying `[`, which the switch reads as an array index.
    ArrayIndex,
    /// A name differing only in ASCII case from another in the block: the switch installs both
    /// as one variable.
    CaseCollision,
    /// A value carrying a control character other than tab, under a name the switch copies into
    /// an outgoing SIP header, whose field value cannot hold one.
    HeaderControl,
}

impl fmt::Display for VariableFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("is empty"),
            Self::EnterpriseSeparator => write!(
                f,
                "carries the enterprise separator {ENTERPRISE_DELIM}, on which the switch splits \
                 the dial string into threads whatever quoting or escaping surrounds it"
            ),
            Self::QuoteInChannelScope => f.write_str(
                "carries a single quote in channel scope, which the switch pairs with the next \
                 quote in the dial string before it parses the block, whatever escaping \
                 precedes either. Use default scope, or keep the quote out of the variable",
            ),
            Self::ClosesUnopenedBracket(open) => write!(
                f,
                "closes a '{open}' it never opened, ending the block early"
            ),
            Self::UnclosedBracket(open) => write!(
                f,
                "opens a '{open}' it never closes, swallowing the block's end"
            ),
            Self::BlockSeparator(sep) => write!(
                f,
                "contains the block's '{sep}' separator, which splits the pair"
            ),
            Self::ArrayIndex => f.write_str(
                "carries '[', which the switch reads as an array index, installing the value \
                 under the text before it",
            ),
            Self::CaseCollision => f.write_str(
                "differs only in case from another name, which the switch installs as the same \
                 variable",
            ),
            Self::HeaderControl => f.write_str(
                "carries a control character, which the switch copies verbatim into an outgoing \
                 SIP header whose field value cannot hold one",
            ),
        }
    }
}

/// A channel variable no command delivers as written.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UndeliverableVariable {
    /// The variable's name.
    pub name: String,
    /// The half of the pair at fault.
    pub part: VariablePart,
    /// What the switch does with it.
    pub fault: VariableFault,
}

impl UndeliverableVariable {
    pub(crate) fn in_name(name: &str, fault: VariableFault) -> Self {
        Self {
            name: name.to_owned(),
            part: VariablePart::Name,
            fault,
        }
    }

    pub(crate) fn in_value(name: &str, fault: VariableFault) -> Self {
        Self {
            name: name.to_owned(),
            part: VariablePart::Value,
            fault,
        }
    }

    /// This refusal for the pair at `index` of its block.
    pub(crate) fn at(self, index: usize) -> OriginateError {
        OriginateError::UndeliverableVariable {
            index,
            variable: self,
        }
    }
}

/// Refuse `value` when `name` opens with a [`SipHeaderPrefix`](crate::SipHeaderPrefix) mod_sofia
/// copies into an outgoing SIP header, in any case, and `value` carries a control character other
/// than tab, which no header field value may hold.
///
/// Every builder that sets a channel variable runs it, freeswitch-esl-tokio's
/// `AppCommand::set_var` included. Run it on a `set`, `export` or `uuid_setvar` line built by
/// hand, on the value the channel should hold rather than its quoted form.
pub fn check_header_value(name: &str, value: &str) -> Result<(), UndeliverableVariable> {
    match names_outgoing_header(name) && contains_header_control(value) {
        true => Err(UndeliverableVariable::in_value(
            name,
            VariableFault::HeaderControl,
        )),
        false => Ok(()),
    }
}

impl fmt::Display for UndeliverableVariable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.part, self.fault) {
            (VariablePart::Name, VariableFault::Empty) => {
                f.write_str("a variable name is empty, which the switch installs under no name")
            }
            (VariablePart::Name, fault) => write!(f, "a variable name {fault}"),
            (VariablePart::Value, VariableFault::Empty) => write!(
                f,
                "variable {} has an empty value: the switch discards such a pair without \
                 logging it, so it cannot be told from an absent variable on the wire. Give it \
                 a value or remove it -- and if its presence was itself the signal, that signal \
                 needs a home outside the dial string",
                self.name
            ),
            (VariablePart::Value, fault) => write!(f, "variable {} {fault}", self.name),
        }
    }
}

impl std::error::Error for UndeliverableVariable {}
