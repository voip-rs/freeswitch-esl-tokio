//! Internal wire-safety predicates shared across the crate.
//!
//! ESL is a line-delimited text protocol the switch reads as C strings: an
//! embedded `\n`/`\r` in a user-supplied string injects a command, and a NUL
//! cuts one short. Each call site keeps its own error type and message — this
//! module only provides the predicate.
//!
//! This module is `#[doc(hidden)]` and not part of the stable API surface.
//! It is exposed publicly only so the `freeswitch-esl-tokio` crate (same
//! workspace) can re-export the predicate as `pub(crate)` without depending
//! on its own copy. Do not rely on it from external crates.

/// `true` if `s` contains `\n`, `\r` or NUL, the bytes that end an ESL command
/// early on the switch's side.
#[doc(hidden)]
pub fn contains_wire_terminator(s: &str) -> bool {
    s.contains(['\n', '\r', '\0'])
}
