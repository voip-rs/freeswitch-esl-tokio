//! Command string builders for [`api()`] and [`bgapi()`].
//!
//! [`api()`]: https://docs.rs/freeswitch-esl-tokio/latest/freeswitch_esl_tokio/connection/struct.EslClient.html#method.api
//! [`bgapi()`]: https://docs.rs/freeswitch-esl-tokio/latest/freeswitch_esl_tokio/connection/struct.EslClient.html#method.bgapi
//!
//! Each builder implements [`Display`](std::fmt::Display), producing the argument
//! string for the corresponding FreeSWITCH API command.  The builders perform
//! escaping and validation so callers don't need to worry about wire-format
//! details.

pub mod bridge;
pub mod channel;
pub mod conference;
pub mod endpoint;
pub mod execute_on;
pub mod flattened;
pub mod originate;
pub mod variables;

#[cfg(all(test, feature = "serde"))]
mod proptests;

pub use bridge::{BridgeDialString, BridgeDialStringDisplay};
pub use channel::{
    UuidAnswer, UuidBridge, UuidDeflect, UuidGetVar, UuidHold, UuidKill, UuidSendDtmf, UuidSetVar,
    UuidTransfer,
};
pub use conference::{
    ConferenceDtmf, ConferenceHold, ConferenceMute, HoldAction, MuteAction, ParseHoldActionError,
    ParseMuteActionError,
};
pub use endpoint::{
    AudioEndpoint, DestinationEncoding, DialString, EndpointDisplay, EndpointFieldFault,
    ErrorEndpoint, GroupCall, GroupCallOrder, LoopbackEndpoint, ParseGroupCallOrderError,
    SofiaContact, SofiaEndpoint, SofiaGateway, UserEndpoint,
};
pub use execute_on::{ExecuteOn, ExecuteOnFault};
pub use flattened::{
    CauseReading, ErrorLeg, FlattenedDialString, FlattenedDialStringDisplay,
    FlattenedDialStringError, FlattenedGroup, FlattenedLeg, FlattenedThread, LegTarget, LegWarning,
    ListWarning, UnparsedLeg,
};
pub use originate::{
    Application, DialplanType, Endpoint, Originate, OriginateDisplay, OriginateError,
    OriginateTarget, ParseDialplanTypeError, Variables, VariablesType,
};
pub use variables::{
    BlockParse, DialStringCarrier, DialStringTarget, InvalidArgvSeparator, ParseBlockParseError,
    UnvouchedVersion, VariablesDisplay,
};

use crate::switch_passes::api_argument::{self, clean_argument, split_line};
use crate::switch_passes::originate_function;

/// Wrap a token in single quotes for originate command strings.
///
/// A token that is empty or carries a space, single quote, backslash, or a tab, vertical tab, CR
/// or newline, which the API strips from the edges of its argument line, is escaped and wrapped
/// as [`quote_for_uuid_setvar`] does, since that command splits on the same blank tokenizer;
/// any other token is returned as-is.
pub fn originate_quote(token: &str) -> String {
    api_argument::originate_quote(token)
}

/// Escape and single-quote a value for the argument string of `uuid_setvar`.
///
/// That command splits its arguments on spaces through `cleanup_separated_string`,
/// which honours `'` grouping and processes `\` escapes inside the quoted region,
/// so an unquoted value is silently truncated at its first space and a bare `'`
/// or `\` inside the quotes ends or eats a character. The inline originate
/// `{var=…}` block is a different carrier with different escaping.
pub fn quote_for_uuid_setvar(value: &str) -> String {
    api_argument::quote_for_uuid_setvar(value)
}

/// What the switch delivers of one token of the blank split: its quotes stripped and its
/// escapes read, the exact inverse of [`originate_quote`].
pub fn originate_unquote(token: &str) -> String {
    clean_argument(token, None)
}

/// Split a command line the way the `originate` API splits its arguments.
///
/// `split_at` is the default a leading `^^X` overrides, as `switch_separate_string`
/// in `switch_utils.c` reads one: an ASCII `X` followed by at least one byte, the
/// prefix itself dropped. The switch takes a non-ASCII `X` as its first UTF-8 byte,
/// which no char delimiter mirrors, so such a line splits on `split_at` whole.
///
/// On a space this is `separate_string_blank_delim`, on any other delimiter
/// `separate_string_char_delim`. Tokens keep their quotes and escapes:
/// [`originate_unquote`] and the variable-block parser consume those.
///
/// A quote left open is an error. The switch runs the rest of the line into one
/// argument instead, a shape nothing this crate renders produces.
pub fn originate_split(line: &str, split_at: char) -> Result<Vec<String>, OriginateError> {
    Ok(split_line(line, split_at)?
        .arguments
        .into_iter()
        .map(|argument| line[argument.raw].to_string())
        .collect())
}

/// Parse the target argument of an originate command.
///
/// Determines whether the target is a dialplan extension or application(s), in the order
/// `originate_function` decides it:
///
/// - If string is `&` and more: parse as XML app → `Application`, whatever the dialplan
/// - If dialplan is `Inline`: parse as inline apps → `InlineApplications`
/// - Otherwise: bare string → `Extension`
pub fn parse_originate_target(
    s: &str,
    dialplan: Option<&DialplanType>,
) -> Result<OriginateTarget, OriginateError> {
    originate_function::parse_originate_target(s, dialplan)
}
