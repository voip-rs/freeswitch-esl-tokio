//! Internal wire-safety checks shared across the crate.
//!
//! ESL is a line-delimited text protocol the switch reads as C strings: an
//! embedded `\n`/`\r` in a user-supplied string injects a command, and a NUL
//! cuts one short. The predicates leave the error type and message to each call
//! site; [`check_header_value`] returns the variable refusal every path that sets
//! a channel variable shares, `uuid_setvar` and the `set` application included.
//!
//! This module is `#[doc(hidden)]` and not part of the stable API surface.
//! It is exposed publicly only so the `freeswitch-esl-tokio` crate (same
//! workspace) can call these checks without depending on its own copy. Do not
//! rely on it from external crates.

#[cfg(feature = "esl")]
use crate::commands::variables::{UndeliverableVariable, VariableFault};
#[cfg(feature = "esl")]
use crate::variables::names_outgoing_header;

/// `true` if `s` contains `\n`, `\r` or NUL, the bytes that end an ESL command
/// early on the switch's side.
#[doc(hidden)]
pub fn contains_wire_terminator(s: &str) -> bool {
    s.contains(['\n', '\r', '\0'])
}

/// `true` if `s` carries a control character other than tab, which no SIP
/// header field value may hold.
#[doc(hidden)]
pub fn contains_header_control(s: &str) -> bool {
    s.chars()
        .any(|c| c.is_ascii_control() && c != '\t')
}

/// Refuse a value the switch copies into an outgoing SIP header, by the name it
/// is set under, when it carries a control character other than tab.
#[doc(hidden)]
#[cfg(feature = "esl")]
pub fn check_header_value(name: &str, value: &str) -> Result<(), UndeliverableVariable> {
    match names_outgoing_header(name) && contains_header_control(value) {
        true => Err(UndeliverableVariable::in_value(
            name,
            VariableFault::HeaderControl,
        )),
        false => Ok(()),
    }
}
