//! Error types for FreeSWITCH ESL operations.
//!
//! All fallible operations in this crate return [`EslResult<T>`]. Every error
//! falls in exactly one of three classes, read off two predicates:
//!
//! - **Connection errors** ([`EslError::is_connection_error`]): there is no
//!   usable session, because it died, its framing broke, or it never came up.
//!   Reconnect.
//! - **Recoverable errors** ([`EslError::is_recoverable`]): the session is up and
//!   only this command failed (a command timeout, `-ERR`, a full queue). Retry
//!   the command, or move on. A timed-out command may still have run: its
//!   result follows as `LateReply`, so a retry of one that is not idempotent
//!   can run it twice.
//! - **Neither**: sending the same thing again fails the same way. A caller
//!   value the library refused before sending (`WireTerminatorInField`,
//!   `TransferArgument`, `Originate`, …), a credential the switch refused at
//!   connect (`AuthenticationFailed`), or a value the switch sent that did not
//!   parse (`HeaderParse`, an event or response body). Fix the input or the
//!   configuration; a reconnect loop on these only spins. `UnsolicitedReply`
//!   and `LateReply` sit here too: a reply nothing waited for, reported while
//!   the session carries on.

use crate::app::dptools::TransferArgumentError;
use crate::command::EslResponse;
use crate::commands::OriginateError;
use crate::connection::CommandTicket;
use crate::constants::{HEADER_CONTENT_TYPE, REPLY_PREFIX_ERR, REPLY_PREFIX_USAGE};
use crate::HeaderLookup;
use freeswitch_types::event::FilterHeaderFault;
use freeswitch_types::variables::ParseLoopbackLegError;
use freeswitch_types::UndeliverableVariable;
use freeswitch_types::{
    ParseAnswerStateError, ParseCallDirectionError, ParseCallStateError, ParseChannelStateError,
    ParseGatewayRegStateError, ParseHangupCauseError, ParseHeaderError, ParsePriorityError,
    ParseSipStatusCodeError, ParseTimetableError,
};
use thiserror::Error;

/// Result type alias for ESL operations
pub type EslResult<T> = Result<T, EslError>;

/// Payload of every `mod_event_socket` denial (`api`, `bgapi`, `log`, `event`);
/// the reply carries this and nothing more.
const PERMISSION_DENIED_PAYLOAD: &str = "permission denied";

/// Comprehensive error types for ESL operations
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum EslError {
    /// IO error from underlying TCP operations
    #[error("I/O error")]
    Io(#[source] std::io::Error),

    /// Connection is not established or lost
    #[error("Not connected to FreeSWITCH")]
    NotConnected,

    /// Authentication failed
    #[error("Authentication failed: {reason}")]
    AuthenticationFailed {
        /// Description from FreeSWITCH (e.g. "invalid" or "user not found").
        reason: String,
    },

    /// Protocol error - invalid message format
    #[error("Protocol error: {message}")]
    ProtocolError {
        /// What went wrong in the protocol exchange.
        message: String,
    },

    /// Command returned `-ERR` with an error message from FreeSWITCH
    #[error("Command failed: {reply_text}")]
    CommandFailed {
        /// The full `Reply-Text` value (e.g. `-ERR invalid command`).
        reply_text: String,
    },

    /// Reply-Text did not match the expected `+OK`/`-ERR` protocol format.
    ///
    /// Most ESL commands return `+OK ...` on success and `-ERR ...` on failure.
    /// A reply that matches neither indicates a protocol-level anomaly or a
    /// command with non-standard reply format (e.g. `getvar`).
    #[error("reply text of {} bytes is neither +OK nor -ERR", reply_text.len())]
    UnexpectedReply {
        /// The full `Reply-Text` value that didn't match `+OK` or `-ERR`.
        reply_text: String,
    },

    /// A command's reply did not arrive within the command timeout, on a
    /// connection that is still up.
    ///
    /// The connection remains usable and the command may still have run on the
    /// switch. Its reply, if it comes, arrives on the event stream as
    /// [`EslError::LateReply`] carrying the same `ticket`, and never reaches
    /// the next command.
    #[error("command {ticket} timed out after {timeout_ms}ms")]
    CommandTimeout {
        /// Elapsed time in milliseconds before the operation was abandoned.
        timeout_ms: u64,
        /// Pairs this timeout with its [`EslError::LateReply`].
        ticket: CommandTicket,
    },

    /// Invalid event format
    #[error("Invalid event format: {format}")]
    InvalidEventFormat {
        /// The unrecognized format string.
        format: String,
    },

    /// A JSON event body the decoder refused.
    #[error("JSON event body did not decode")]
    JsonError(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// An XML event body the decoder refused.
    #[error("XML event body did not decode")]
    XmlError(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Bytes the framing reads as text were not UTF-8: the stream is no longer
    /// trustworthy, so this is a connection error.
    #[error("invalid UTF-8 in {context}")]
    Utf8Error {
        /// What was being decoded (e.g. `"message headers"`).
        context: &'static str,
        /// Underlying decoder error.
        #[source]
        source: std::str::Utf8Error,
    },

    /// UTF-8 decoding failed for a specific header value during parsing.
    ///
    /// Preserves the source `Utf8Error` so callers walking `e.source()` see
    /// the exact decoder error (byte offset, invalid sequence) instead of a
    /// stringified message.
    #[error("invalid UTF-8 in {context} '{key}'")]
    InvalidUtf8InHeader {
        /// Where the bad byte appeared (e.g. `"header"`, `"event body"`).
        context: &'static str,
        /// Header key whose value failed to decode.
        key: String,
        /// Underlying decoder error.
        #[source]
        source: std::str::Utf8Error,
    },

    /// Buffer overflow - message too large
    #[error("Buffer overflow: message size {size} exceeds limit {limit}")]
    BufferOverflow {
        /// Actual message size in bytes.
        size: usize,
        /// Maximum allowed size in bytes.
        limit: usize,
    },

    /// A message envelope line without a `:`, or a `Content-Length` that is not
    /// a number. A body line is [`InvalidBodyLine`](Self::InvalidBodyLine).
    #[error("malformed header line ({} bytes)", header.len())]
    InvalidHeader {
        /// The malformed header line.
        header: String,
    },

    /// Connection rejected by FreeSWITCH ACL (text/rude-rejection)
    #[error("Access denied: {reason}")]
    AccessDenied {
        /// Message from the rejection notice.
        reason: String,
    },

    /// Connection closed by remote
    #[error("Connection closed by FreeSWITCH")]
    ConnectionClosed,

    /// Event queue full
    #[error("Event queue is full - dropping events")]
    QueueFull,

    /// Originate command builder error
    #[error("originate command refused")]
    Originate(#[from] OriginateError),

    /// Re-exec teardown refused before the reader was told to stop: a command
    /// in flight, or a second call. The connection stays usable.
    ///
    /// A failure after the stop signal is [`EslError::ReexecReaderLost`].
    #[cfg(unix)]
    #[error("Re-exec teardown failed: {reason}")]
    ReexecFailed {
        /// Why teardown refused.
        reason: String,
        /// The failure behind it, when one was reported.
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// A typed header accessor rejected a value that was present on the wire.
    #[error("header value did not parse")]
    HeaderParse(#[from] ParseHeaderError),

    /// `FilterDelete` named the header `all`, which mod_event_socket reads in
    /// any case as delete-all and drops the value; send `FilterDeleteAll`.
    #[error(
        "filter delete header ({} bytes) reads as all and deletes every filter; use FilterDeleteAll",
        header.len()
    )]
    FilterDeleteAllHeader {
        /// The header as given.
        header: String,
    },

    /// A caller-supplied field held a line break, which would inject a command,
    /// or a NUL, which would cut one short. Nothing was sent.
    #[error("{field} must not contain a line break or NUL")]
    WireTerminatorInField {
        /// Which field; the value is not kept, since it may be a credential.
        field: &'static str,
    },

    /// A `filter` or `filter delete` header mod_event_socket would read as
    /// some other header, or as a keyword.
    #[error("filter header {fault} ({} bytes)", header.len())]
    UnsplittableFilterHeader {
        /// The header as given.
        header: String,
        /// How the switch would misread it.
        fault: FilterHeaderFault,
    },

    /// An [`AppCommand`](crate::AppCommand) `transfer` argument the switch would
    /// misread.
    #[error("transfer argument refused")]
    TransferArgument(#[from] TransferArgumentError),

    /// A command lacked a field it cannot be written without. Nothing was sent.
    #[error("{field} is missing")]
    MissingField {
        /// Which field.
        field: &'static str,
    },

    /// The TCP connect did not complete within the connect timeout, so no
    /// session exists.
    #[error("TCP connect timed out after {timeout_ms}ms")]
    ConnectTimeout {
        /// The connect timeout in milliseconds.
        timeout_ms: u64,
    },

    /// The socket connected but the auth handshake did not complete within
    /// the connect timeout, which runs from the start of the TCP connect, so no
    /// session exists.
    #[error("ESL handshake did not complete within {timeout_ms}ms")]
    HandshakeTimeout {
        /// The connect timeout in milliseconds.
        timeout_ms: u64,
    },

    /// A `userauth` user holding a `:`, where mod_event_socket ends the user or
    /// the domain and starts the password. Nothing was sent.
    #[error(
        "userauth user ({} bytes) holds a ':', where the switch starts the password",
        user.len()
    )]
    ColonInAuthUser {
        /// The user as given.
        user: String,
    },

    /// A channel variable the switch would not deliver as written, from
    /// [`AppCommand::set_var`](crate::AppCommand::set_var) or
    /// [`UuidSetVar::new`](crate::UuidSetVar::new). Nothing was sent.
    #[error("channel variable refused")]
    UndeliverableVariable(#[from] UndeliverableVariable),

    /// A line without a `:` inside a serialized event or response body. The
    /// message's own framing held, so the connection stays usable.
    #[error("a line of {} bytes in the {context} has no colon", line.len())]
    InvalidBodyLine {
        /// Which body: `"event body"` or `"response body"`.
        context: &'static str,
        /// The line as received.
        line: String,
    },

    /// A `command/reply` or `api/response` that arrived with no command
    /// waiting for it. The session stays up and later replies still correlate,
    /// but the switch answered something this client did not send.
    #[error(
        "unsolicited {} with no command pending (reply text {} bytes, body {} bytes)",
        reply_content_type(response),
        response.reply_text().map_or(0, str::len),
        response.body().map_or(0, str::len)
    )]
    UnsolicitedReply {
        /// The reply as received.
        response: Box<EslResponse>,
    },

    /// The reply to a command that returned [`EslError::CommandTimeout`] with
    /// the same `ticket`, or whose future was dropped while it waited. The
    /// command ran on the switch and this is its result; later replies still
    /// correlate.
    ///
    /// A dropped future never received its ticket, so its reply pairs only by
    /// order. Like any stream item, it is dropped and counted when the event
    /// queue is full under `EventOverflow::DropIncoming`.
    #[error(
        "late {} for command {ticket} (reply text {} bytes, body {} bytes)",
        reply_content_type(response),
        response.reply_text().map_or(0, str::len),
        response.body().map_or(0, str::len)
    )]
    LateReply {
        /// The reply as received.
        response: Box<EslResponse>,
        /// The command this answers.
        ticket: CommandTicket,
    },

    /// A number of seconds past `i32::MAX`, which the switch reads with `atoi`
    /// into an `int` and wraps. Nothing was sent.
    #[error("{field} of {seconds} seconds is past what the switch reads")]
    SecondsOutOfRange {
        /// Which command field.
        field: &'static str,
        /// The value as given, in whole seconds.
        seconds: u64,
    },

    /// Re-exec teardown failed after the reader was told to stop, or found
    /// it already gone: the drain did not finish and the connection is over.
    ///
    /// The status reads `Disconnected` by the time this is returned, naming
    /// why. Refusals that leave the connection usable are
    /// [`EslError::ReexecFailed`].
    #[cfg(unix)]
    #[error("re-exec teardown lost the reader: {reason}")]
    ReexecReaderLost {
        /// What ended the drain.
        reason: String,
        /// The failure behind it, when one was reported.
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}

fn reply_content_type(response: &EslResponse) -> &str {
    response
        .headers()
        .header_str(HEADER_CONTENT_TYPE)
        .unwrap_or("reply")
}

/// Route each typed parser's error into [`EslError::HeaderParse`], so a reader
/// loop can `?` a header accessor without naming the parser that failed.
macro_rules! esl_error_from_header_parse {
    ($($(#[$attr:meta])* $Error:ty),+ $(,)?) => {
        $(
            $(#[$attr])*
            impl From<$Error> for EslError {
                fn from(e: $Error) -> Self {
                    Self::HeaderParse(ParseHeaderError::from(e))
                }
            }
        )+
    };
}

esl_error_from_header_parse! {
    ParseChannelStateError,
    ParseCallStateError,
    ParseAnswerStateError,
    ParseCallDirectionError,
    ParseHangupCauseError,
    ParseGatewayRegStateError,
    ParseLoopbackLegError,
    ParsePriorityError,
    ParseTimetableError,
    ParseSipStatusCodeError,
}

/// [`EslError::display_chain`]'s adapter.
struct DisplayChain<'a>(&'a EslError);

impl std::fmt::Display for DisplayChain<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)?;
        let mut source = std::error::Error::source(self.0);
        while let Some(cause) = source {
            write!(f, ": {cause}")?;
            source = cause.source();
        }
        Ok(())
    }
}

/// The failure text of an [`EslError::CommandFailed`], split by the prefix
/// FreeSWITCH wrote in front of it.
///
/// A sum rather than a `(kind, payload)` pair because the payload only means
/// something once the prefix is known: a `-USAGE` synopsis handed to
/// [`HangupCause::from_str`](freeswitch_types::HangupCause) is not a hangup
/// cause, and a product type lets a caller read one without the other.
///
/// A prefix counts as one only when no word character follows it: mod_commands
/// writes a bare `-ERROR`, which is [`Unprefixed`](Self::Unprefixed) rather than
/// an `-ERR` carrying `OR`. This is a finer question than the one
/// [`parse_api_body`](crate::parse_api_body) answers — whether the body is a
/// failure at all, which `-ERROR` is — so the two classify at different
/// granularities on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommandFailure<'a> {
    /// `-ERR <text>`, with the text after an optional `:` and one space.
    /// Empty for a bare `-ERR`.
    Err(&'a str),
    /// `-USAGE: <text>`. Only one space is consumed after the colon: the rest
    /// of a synopsis's indentation is wire content.
    Usage(&'a str),
    /// Neither prefix was found, so the whole reply text rides here rather
    /// than posing as a peeled payload.
    Unprefixed(&'a str),
}

impl<'a> CommandFailure<'a> {
    /// The text behind the prefix, or `None` when there was no prefix to peel.
    pub fn payload(self) -> Option<&'a str> {
        match self {
            CommandFailure::Err(text) | CommandFailure::Usage(text) => Some(text),
            CommandFailure::Unprefixed(_) => None,
        }
    }
}

/// Peel a reply prefix, or `None` when a word character follows it and the
/// match is therefore a longer word the switch wrote (`-ERROR`).
fn peel_prefix<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(prefix)?;
    if rest.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some(peel_separator(rest))
}

/// Drop the separator FreeSWITCH writes between a reply prefix and its text.
fn peel_separator(rest: &str) -> &str {
    let rest = rest
        .strip_prefix(':')
        .unwrap_or(rest);
    rest.strip_prefix(' ')
        .unwrap_or(rest)
}

impl EslError {
    /// The message and each source, joined by `: `, for a log line.
    ///
    /// `Display` stops at this error's own message, because a caller that walks
    /// `source()` itself (anyhow's `{:#}`, a tracing layer) would print the cause twice.
    pub fn display_chain(&self) -> impl std::fmt::Display + '_ {
        DisplayChain(self)
    }

    /// Construct a protocol error with a description.
    pub fn protocol_error(message: impl Into<String>) -> Self {
        Self::ProtocolError {
            message: message.into(),
        }
    }

    /// Construct an authentication failure with a reason.
    pub fn auth_failed(reason: impl Into<String>) -> Self {
        Self::AuthenticationFailed {
            reason: reason.into(),
        }
    }

    /// `true` if the session is still up and only this command failed, so the
    /// caller can retry it: `CommandTimeout`, `CommandFailed`, `UnexpectedReply`,
    /// `QueueFull`.
    ///
    /// `false` covers two classes [the module docs](crate::error) tell apart: a
    /// connection error, where [`Self::is_connection_error`] is `true` and the
    /// next step is a reconnect, and a refusal that a retry or a reconnect
    /// meets again (caller input, credentials), where both are `false`.
    // qual:allow(dry, duplicate) reason: "exhaustive per-variant classification, no catch-all by design"
    pub fn is_recoverable(&self) -> bool {
        match self {
            EslError::Io(_) => false,
            EslError::NotConnected => false,
            EslError::ConnectionClosed => false,
            EslError::AuthenticationFailed { .. } => false,
            EslError::AccessDenied { .. } => false,
            EslError::CommandTimeout { .. } => true,
            EslError::CommandFailed { .. } => true,
            EslError::UnexpectedReply { .. } => true,
            EslError::QueueFull => true,
            // Parse/protocol failures: correctness over recovery — never
            // hand back partial data, never paper over wire-format breaks.
            EslError::ProtocolError { .. } => false,
            EslError::InvalidEventFormat { .. } => false,
            EslError::JsonError(_) => false,
            EslError::XmlError(_) => false,
            EslError::Utf8Error { .. } => false,
            EslError::InvalidUtf8InHeader { .. } => false,
            EslError::InvalidHeader { .. } => false,
            EslError::HeaderParse(_) => false,
            // Stream resync impossible after BufferOverflow.
            EslError::BufferOverflow { .. } => false,
            // Per-call: don't retry the same command/originate as-is.
            EslError::Originate(_) => false,
            #[cfg(unix)]
            EslError::ReexecFailed { .. } => false,
            EslError::FilterDeleteAllHeader { .. } => false,
            EslError::WireTerminatorInField { .. } => false,
            EslError::UnsplittableFilterHeader { .. } => false,
            EslError::TransferArgument(_) => false,
            EslError::MissingField { .. } => false,
            EslError::ConnectTimeout { .. } => false,
            EslError::HandshakeTimeout { .. } => false,
            EslError::ColonInAuthUser { .. } => false,
            EslError::UndeliverableVariable(_) => false,
            EslError::InvalidBodyLine { .. } => false,
            EslError::UnsolicitedReply { .. } => false,
            EslError::LateReply { .. } => false,
            EslError::SecondsOutOfRange { .. } => false,
            #[cfg(unix)]
            EslError::ReexecReaderLost { .. } => false,
        }
    }

    /// `true` if there is no usable session and the next step is a reconnect:
    /// the socket failed or closed, the stream's framing broke (a header line,
    /// an oversized or non-UTF-8 envelope, a missing `Content-Type`), the ACL
    /// rejected the connection, or the connect or its handshake timed out.
    ///
    /// `AuthenticationFailed` is `false` here and in [`Self::is_recoverable`]:
    /// a reconnect sends the same credential and is refused again, so a
    /// [`EslClient::connect`] retry loop breaks on it.
    ///
    /// [`EslClient::connect`]: crate::EslClient::connect
    // qual:allow(dry, duplicate) reason: "exhaustive per-variant classification, no catch-all by design"
    pub fn is_connection_error(&self) -> bool {
        match self {
            EslError::Io(_) => true,
            EslError::NotConnected => true,
            EslError::ConnectionClosed => true,
            EslError::AccessDenied { .. } => true,
            EslError::ProtocolError { .. } => true,
            // Stream resync impossible after BufferOverflow — the next
            // bytes have no recoverable framing.
            EslError::BufferOverflow { .. } => true,
            // Auth is reported synchronously by connect(); see the
            // method-level docs for the rationale.
            EslError::AuthenticationFailed { .. } => false,
            EslError::CommandFailed { .. } => false,
            EslError::UnexpectedReply { .. } => false,
            EslError::CommandTimeout { .. } => false,
            EslError::QueueFull => false,
            // An event body that did not decode leaves the framing intact.
            EslError::InvalidEventFormat { .. } => false,
            EslError::JsonError(_) => false,
            EslError::XmlError(_) => false,
            // Only built from bytes the framing reads as text.
            EslError::Utf8Error { .. } => true,
            EslError::InvalidUtf8InHeader { .. } => false,
            // A colonless envelope line or a non-numeric Content-Length: the
            // framing is lost, as with a missing Content-Type.
            EslError::InvalidHeader { .. } => true,
            EslError::HeaderParse(_) => false,
            EslError::Originate(_) => false,
            // Refused before the stop signal: the reader still runs.
            #[cfg(unix)]
            EslError::ReexecFailed { .. } => false,
            EslError::FilterDeleteAllHeader { .. } => false,
            EslError::WireTerminatorInField { .. } => false,
            EslError::UnsplittableFilterHeader { .. } => false,
            EslError::TransferArgument(_) => false,
            EslError::MissingField { .. } => false,
            EslError::ConnectTimeout { .. } => true,
            EslError::HandshakeTimeout { .. } => true,
            EslError::ColonInAuthUser { .. } => false,
            EslError::UndeliverableVariable(_) => false,
            EslError::InvalidBodyLine { .. } => false,
            EslError::UnsolicitedReply { .. } => false,
            EslError::LateReply { .. } => false,
            EslError::SecondsOutOfRange { .. } => false,
            #[cfg(unix)]
            EslError::ReexecReaderLost { .. } => true,
        }
    }

    /// `true` if a command was rejected for lack of permission
    /// (`-ERR permission denied`).
    ///
    /// FreeSWITCH returns this when a restricted ESL user (e.g.
    /// `esl-allowed-events` without `HEARTBEAT`, or an `esl-allowed-api`
    /// gate) issues a command it is not authorized for. The denial is
    /// recoverable: the connection stays usable, only the command failed.
    /// Lets the caller distinguish "you may not do this" from a generic
    /// command failure without matching `reply_text` by hand.
    ///
    /// Matched as a prefix, not a substring: the denial is `mod_event_socket`'s
    /// own whole reply, so a different failure that merely mentions the phrase
    /// is not one.
    pub fn is_permission_denied(&self) -> bool {
        matches!(self.command_failure(), Some(CommandFailure::Err(payload))
            if payload
                .get(..PERMISSION_DENIED_PAYLOAD.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(PERMISSION_DENIED_PAYLOAD)))
    }

    /// Split the failure text of a [`CommandFailed`](Self::CommandFailed) by
    /// its wire prefix, so a caller can feed the payload to a typed parser
    /// (`-ERR USER_BUSY` to
    /// [`HangupCause`](freeswitch_types::HangupCause)) without stripping it
    /// by hand.
    ///
    /// `None` for every other variant, including
    /// [`UnexpectedReply`](Self::UnexpectedReply) — whose documented normal
    /// case is a `getvar` value, which is not a failure at all.
    pub fn command_failure(&self) -> Option<CommandFailure<'_>> {
        let EslError::CommandFailed { reply_text } = self else {
            return None;
        };
        let text = reply_text.trim_start();
        if let Some(payload) = peel_prefix(text, REPLY_PREFIX_ERR) {
            Some(CommandFailure::Err(payload))
        } else if let Some(payload) = peel_prefix(text, REPLY_PREFIX_USAGE) {
            Some(CommandFailure::Usage(payload))
        } else {
            Some(CommandFailure::Unprefixed(text))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use freeswitch_types::{HangupCause, ParseChannelStateError, ParseHeaderError};

    // A typed accessor's error must reach EslError through the union, so a
    // reader loop can `?` it without naming each parser's error type.
    #[test]
    fn header_parse_error_routes_through_the_union() {
        let err = EslError::from(ParseChannelStateError("CS_NONSENSE".into()));
        assert!(matches!(
            err,
            EslError::HeaderParse(ParseHeaderError::ChannelState(_))
        ));
        assert!(!err.is_recoverable());
        assert!(!err.is_connection_error());
    }

    #[test]
    fn a_sip_status_code_fault_routes_through_the_union() {
        let err = EslError::from(ParseSipStatusCodeError("notanumber".into()));
        assert!(matches!(
            err,
            EslError::HeaderParse(ParseHeaderError::SipStatusCode(_))
        ));
        assert!(!err.is_recoverable());
    }

    #[test]
    fn access_denied_not_recoverable() {
        let err = EslError::AccessDenied {
            reason: "ACL".into(),
        };
        assert!(!err.is_recoverable());
    }

    #[test]
    fn queue_full_is_recoverable() {
        assert!(EslError::QueueFull.is_recoverable());
    }

    #[test]
    fn command_failed_is_recoverable() {
        let err = EslError::CommandFailed {
            reply_text: "-ERR no such command".into(),
        };
        assert!(err.is_recoverable());
    }

    #[test]
    fn unexpected_reply_is_recoverable() {
        let err = EslError::UnexpectedReply {
            reply_text: "garbage".into(),
        };
        assert!(err.is_recoverable());
    }

    #[test]
    fn permission_denied_detected() {
        for reply in [
            "-ERR permission denied",
            "-ERR Permission Denied",
            "-ERR PERMISSION DENIED",
        ] {
            let err = EslError::CommandFailed {
                reply_text: reply.into(),
            };
            assert!(err.is_permission_denied(), "should detect: {reply}");
            assert!(err.is_recoverable());
            assert!(!err.is_connection_error());
        }
    }

    #[test]
    fn other_command_failure_not_permission_denied() {
        let err = EslError::CommandFailed {
            reply_text: "-ERR no such command".into(),
        };
        assert!(!err.is_permission_denied());
    }

    // The ESL denial is its own reply, not a phrase inside someone else's
    // output. An api body that merely mentions it must not be classified as
    // a permanent configuration fault.
    #[test]
    fn phrase_inside_another_failure_is_not_permission_denied() {
        for reply in [
            "-ERR error/permission denied while opening /var/lib/freeswitch/x.wav",
            "-USAGE: sched_api [+@]<time> <group_name> <command string> (permission denied)",
        ] {
            let err = EslError::CommandFailed {
                reply_text: reply.into(),
            };
            assert!(!err.is_permission_denied(), "should not detect: {reply}");
        }
    }

    #[test]
    fn permission_denied_tolerates_trailing_wire_whitespace() {
        let err = EslError::CommandFailed {
            reply_text: "-ERR permission denied\n".into(),
        };
        assert!(err.is_permission_denied());
    }

    #[test]
    fn non_command_failure_not_permission_denied() {
        assert!(!EslError::QueueFull.is_permission_denied());
        assert!(!EslError::AccessDenied {
            reason: "ACL".into()
        }
        .is_permission_denied());
    }

    fn failed(reply_text: &str) -> EslError {
        EslError::CommandFailed {
            reply_text: reply_text.into(),
        }
    }

    // The consumer case: a hangup cause arrives as an -ERR payload and has to
    // reach HangupCause::from_str without the caller peeling the prefix.
    #[test]
    fn err_payload_round_trips_through_hangup_cause() {
        let err = failed("-ERR USER_BUSY");
        let failure = err
            .command_failure()
            .expect("CommandFailed must classify");
        assert_eq!(failure, CommandFailure::Err("USER_BUSY"));
        assert_eq!(
            failure
                .payload()
                .expect("Err carries a payload")
                .parse::<HangupCause>(),
            Ok(HangupCause::UserBusy)
        );
    }

    #[test]
    fn usage_reply_classifies_as_usage() {
        let err = failed("-USAGE: originate <call_url> <exten>");
        assert_eq!(
            err.command_failure(),
            Some(CommandFailure::Usage("originate <call_url> <exten>"))
        );
    }

    // A usage synopsis's leading indentation is wire content: exactly one
    // space is consumed after the optional colon, never a trim.
    #[test]
    fn usage_payload_keeps_synopsis_indentation() {
        let err = failed("-USAGE:   sched_api [+@]<time>");
        assert_eq!(
            err.command_failure(),
            Some(CommandFailure::Usage("  sched_api [+@]<time>"))
        );
    }

    // mod_commands writes a bare `-ERROR` (uuid_answer with no argument, and
    // the three neighbouring uuid_* APIs). `-ERR` is a token, not a string
    // prefix, so it must not match inside a longer word.
    #[test]
    fn bare_error_reply_is_not_an_err_payload() {
        assert_eq!(
            failed("-ERROR").command_failure(),
            Some(CommandFailure::Unprefixed("-ERROR"))
        );
    }

    #[test]
    fn a_longer_word_behind_either_prefix_stays_unprefixed() {
        assert_eq!(
            failed("-ERRORS: 3").command_failure(),
            Some(CommandFailure::Unprefixed("-ERRORS: 3"))
        );
        assert_eq!(
            failed("-USAGEX y").command_failure(),
            Some(CommandFailure::Unprefixed("-USAGEX y"))
        );
    }

    // A bgapi BACKGROUND_JOB body reaches the caller without parse_api_body's
    // trailing-newline strip, so the terminator rides in the payload.
    #[test]
    fn a_line_terminator_behind_a_bare_prefix_still_peels() {
        assert_eq!(
            failed("-ERR\n").command_failure(),
            Some(CommandFailure::Err("\n"))
        );
        assert_eq!(
            failed("-ERR\r\n").command_failure(),
            Some(CommandFailure::Err("\r\n"))
        );
    }

    // mod_avmd writes `-ERR, <message>`. The comma is not a separator this
    // crate peels, so it stays on the payload.
    #[test]
    fn a_comma_separated_failure_keeps_its_comma() {
        assert_eq!(
            failed("-ERR, bad command!").command_failure(),
            Some(CommandFailure::Err(", bad command!"))
        );
    }

    #[test]
    fn bare_err_yields_empty_payload() {
        assert_eq!(
            failed("-ERR").command_failure(),
            Some(CommandFailure::Err(""))
        );
    }

    // Neither prefix found: the whole text rides along under a discriminant
    // that says so, rather than masquerading as a peeled -ERR payload.
    #[test]
    fn unprefixed_reply_carries_whole_text_and_no_payload() {
        let err = failed("sip_from_user");
        let failure = err
            .command_failure()
            .expect("CommandFailed must classify");
        assert_eq!(failure, CommandFailure::Unprefixed("sip_from_user"));
        assert_eq!(failure.payload(), None);
    }

    #[test]
    fn leading_wire_whitespace_is_tolerated() {
        assert_eq!(
            failed("  -ERR USER_BUSY").command_failure(),
            Some(CommandFailure::Err("USER_BUSY"))
        );
    }

    #[test]
    fn non_command_failed_variants_do_not_classify() {
        assert_eq!(EslError::QueueFull.command_failure(), None);
        assert_eq!(
            EslError::CommandTimeout {
                timeout_ms: 10,
                ticket: CommandTicket(0),
            }
            .command_failure(),
            None
        );
        assert_eq!(
            EslError::UnexpectedReply {
                reply_text: "-ERR nope".into()
            }
            .command_failure(),
            None
        );
    }

    // A -USAGE synopsis merely quoting the phrase is not the denial reply,
    // and the Err-only pattern is what excludes it.
    #[test]
    fn usage_mentioning_the_phrase_is_not_permission_denied() {
        let err = failed("-USAGE: sched_api [+@]<time> <group_name> <command> (permission denied)");
        assert!(matches!(
            err.command_failure(),
            Some(CommandFailure::Usage(_))
        ));
        assert!(!err.is_permission_denied());
    }

    #[test]
    fn classification_of_transport_and_setup_errors() {
        use std::io::ErrorKind;
        let io = |kind| EslError::Io(std::io::Error::new(kind, "test"));
        let cases: &[(EslError, bool, bool)] = &[
            (io(ErrorKind::ConnectionRefused), true, false),
            (io(ErrorKind::ConnectionReset), true, false),
            (io(ErrorKind::ConnectionAborted), true, false),
            (io(ErrorKind::BrokenPipe), true, false),
            (io(ErrorKind::UnexpectedEof), true, false),
            (EslError::ConnectionClosed, true, false),
            (EslError::NotConnected, true, false),
            (
                EslError::CommandTimeout {
                    timeout_ms: 5000,
                    ticket: CommandTicket(0),
                },
                false,
                true,
            ),
            (EslError::ConnectTimeout { timeout_ms: 5000 }, true, false),
            (EslError::HandshakeTimeout { timeout_ms: 5000 }, true, false),
            (
                EslError::ColonInAuthUser {
                    user: "a:b@c".into(),
                },
                false,
                false,
            ),
            (
                EslError::InvalidBodyLine {
                    context: "response body",
                    line: "NoColonHere".into(),
                },
                false,
                false,
            ),
            (EslError::protocol_error("bad framing"), true, false),
            (
                EslError::InvalidHeader {
                    header: "NoColonHere".into(),
                },
                true,
                false,
            ),
            (EslError::auth_failed("bad password"), false, false),
            (
                EslError::from(ParseChannelStateError("CS_NONSENSE".into())),
                false,
                false,
            ),
            (
                EslError::WireTerminatorInField {
                    field: "api command",
                },
                false,
                false,
            ),
            (
                EslError::MissingField {
                    field: "sendevent event name",
                },
                false,
                false,
            ),
            (json_error(), false, false),
            (EslError::XmlError("bad xml".into()), false, false),
            (utf8_error(), true, false),
            #[cfg(unix)]
            (
                EslError::ReexecFailed {
                    reason: "teardown already called".into(),
                    source: None,
                },
                false,
                false,
            ),
            #[cfg(unix)]
            (
                EslError::ReexecReaderLost {
                    reason: "connection closed during drain".into(),
                    source: None,
                },
                true,
                false,
            ),
        ];
        for (err, connection, recoverable) in cases {
            assert_eq!(err.is_connection_error(), *connection, "{err:?}");
            assert_eq!(err.is_recoverable(), *recoverable, "{err:?}");
        }
    }

    fn json_error() -> EslError {
        let decode = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        EslError::JsonError(Box::new(decode))
    }

    #[allow(invalid_from_utf8)]
    fn utf8_error() -> EslError {
        let bytes = [0xffu8];
        EslError::Utf8Error {
            context: "message headers",
            source: std::str::from_utf8(&bytes).unwrap_err(),
        }
    }

    #[test]
    fn decoder_errors_keep_their_source() {
        use std::error::Error;
        for err in [json_error(), utf8_error()] {
            assert!(
                err.source()
                    .is_some(),
                "{err:?}"
            );
        }
        let json = json_error();
        assert!(json
            .source()
            .unwrap()
            .downcast_ref::<serde_json::Error>()
            .is_some());
    }

    /// Display states the variant's own message; the cause is reached through
    /// `source()`, so a chain printer does not show it twice.
    #[test]
    fn display_leaves_the_cause_to_source() {
        use std::error::Error;
        let cases = vec![
            EslError::Io(std::io::Error::other("inner-cause")),
            EslError::JsonError("inner-cause".into()),
            EslError::XmlError("inner-cause".into()),
            utf8_error(),
            EslError::Originate(OriginateError::ParseError("inner-cause".into())),
            EslError::from(ParseChannelStateError("CS_INNER_CAUSE".into())),
            EslError::from(
                crate::AppCommand::transfer("inner cause").expect_err("a space is refused"),
            ),
            EslError::from(
                crate::AppCommand::set_var("sip_h_X-Inner", "a\u{1}b")
                    .expect_err("a control character is refused"),
            ),
            #[cfg(unix)]
            EslError::ReexecFailed {
                reason: "teardown already called".into(),
                source: Some("inner-cause".into()),
            },
        ];
        for err in cases {
            let cause = err
                .source()
                .unwrap_or_else(|| panic!("{err:?} has no source"))
                .to_string();
            let shown = err.to_string();
            assert!(!shown.contains(&cause), "{shown:?} repeats {cause:?}");
        }
    }

    /// A caller or wire value stays on its field; `{e}` names the field and its length.
    #[test]
    fn display_never_quotes_the_value_it_refused() {
        let secret = "s3cr3t-Value";
        let cases = [
            EslError::InvalidHeader {
                header: secret.into(),
            },
            EslError::FilterDeleteAllHeader {
                header: "aLL".into(),
            },
            EslError::UnsplittableFilterHeader {
                header: format!("{secret} x"),
                fault: FilterHeaderFault::Space,
            },
            EslError::UnexpectedReply {
                reply_text: secret.into(),
            },
            EslError::ColonInAuthUser {
                user: format!("{secret}:x@example.com"),
            },
            EslError::InvalidBodyLine {
                context: "event body",
                line: secret.into(),
            },
        ];
        for err in cases {
            let shown = err
                .display_chain()
                .to_string();
            assert!(!shown.contains(secret), "{shown:?}");
            assert!(!shown.contains("aLL"), "{shown:?}");
            assert!(shown.contains("bytes"), "{shown:?} omits the length");
        }
        let transfer = [
            crate::AppCommand::transfer(&format!("{secret} x")).expect_err("a space is refused"),
            crate::AppCommand::transfer("-bOTh").expect_err("a leg selector is refused"),
        ];
        for err in transfer {
            let shown = err.to_string();
            assert!(
                !shown.contains(secret) && !shown.contains("-bOTh"),
                "{shown:?}"
            );
        }
    }

    /// `set_var(..)?` reaches an `EslResult` as `transfer(..)?` does, as a
    /// refusal of caller input.
    #[test]
    fn an_undeliverable_variable_converts_with_question_mark() {
        fn set(value: &str) -> EslResult<crate::EslCommand> {
            Ok(crate::AppCommand::set_var("sip_h_X-Test", value)?)
        }
        let err = set("a\u{1}b").expect_err("a control character is refused");
        assert!(matches!(err, EslError::UndeliverableVariable(_)), "{err:?}");
        assert!(!err.is_connection_error());
        assert!(!err.is_recoverable());
        assert!(std::error::Error::source(&err).is_some());
    }

    /// The reader keeps only text for a disconnect, so it writes the chain.
    #[test]
    fn display_chain_carries_every_cause() {
        let err = utf8_error();
        let cause = std::error::Error::source(&err)
            .unwrap()
            .to_string();
        let chain = err
            .display_chain()
            .to_string();
        assert!(chain.starts_with(&err.to_string()), "{chain}");
        assert!(chain.ends_with(&cause), "{chain}");
    }

    #[test]
    fn display_chain_prints_a_header_parse_cause_once() {
        let inner = freeswitch_types::channel::ParseHangupCauseError("BOGUS".into());
        let cause = inner.to_string();
        let chain = EslError::HeaderParse(inner.into())
            .display_chain()
            .to_string();
        assert_eq!(
            chain
                .matches(&cause)
                .count(),
            1,
            "{chain}"
        );
    }
}
