//! FreeSWITCH dptools application commands (`answer`, `hangup`, `playback`, etc.).

use std::fmt;

use crate::channel::HangupCause;
use crate::command::{EslCommand, ExecuteOptions};
use crate::commands::originate::DialplanType;
use crate::commands::{
    originate_split, originate_unquote, BlockParse, BridgeDialString, UndeliverableVariable,
};
use freeswitch_types::wire_safety::check_header_value;

/// Constructors for common dptools application commands.
///
/// Each method returns a `sendmsg`/`execute` command ready for
/// [`EslClient::send_command()`](crate::EslClient::send_command).
/// The `uuid` field is `None` -- set it on the command or pass it to `sendmsg()`.
pub struct AppCommand;

fn execute(app: &str, args: Option<String>) -> EslCommand {
    EslCommand::Execute {
        app: app.to_string(),
        args,
        uuid: None,
        options: ExecuteOptions::default(),
    }
}

impl AppCommand {
    /// Answer the channel. In outbound mode, answers the incoming call.
    pub fn answer() -> EslCommand {
        execute("answer", None)
    }

    /// Hang up the channel with an optional cause code.
    pub fn hangup(cause: Option<HangupCause>) -> EslCommand {
        execute("hangup", cause.map(|c| c.to_string()))
    }

    /// `file`: path, `tone_stream://`, or any FreeSWITCH file-like URI.
    pub fn playback(file: &str) -> EslCommand {
        execute("playback", Some(file.to_string()))
    }

    /// Bridge to `destination`, rendered for a switch whose dialplan parser is
    /// `block_parse`.
    pub fn bridge(destination: &BridgeDialString, block_parse: BlockParse) -> EslCommand {
        Self::bridge_raw(&destination.render(block_parse))
    }

    /// Bridge to a dial string sent verbatim, such as one
    /// [`Endpoint::render`](crate::Endpoint::render) wrote for
    /// [`DialStringCarrier::Dialplan`](crate::DialStringCarrier::Dialplan).
    pub fn bridge_raw(destination: &str) -> EslCommand {
        execute("bridge", Some(destination.to_string()))
    }

    /// Set a channel variable (`set` application), refusing a value with a control character
    /// other than tab under a name mod_sofia sends as an outgoing SIP header.
    pub fn set_var(name: &str, value: &str) -> Result<EslCommand, UndeliverableVariable> {
        check_header_value(name, value)?;
        Ok(execute("set", Some(format!("{}={}", name, value))))
    }

    /// Park the channel (suspends dialplan execution until another command picks it up).
    pub fn park() -> EslCommand {
        execute("park", None)
    }

    /// Transfer the channel to `extension` in its own dialplan and context.
    ///
    /// The switch takes `force_transfer_dialplan` and `force_transfer_context`
    /// first, then the caller profile's, with `inline` falling back to `XML`.
    ///
    /// Every `transfer` constructor refuses a field the switch's blank split
    /// would read as something else; see [`TransferArgumentError`].
    pub fn transfer(extension: &str) -> Result<EslCommand, TransferArgumentError> {
        transfer_command(&[(TransferField::Extension, extension)])
    }

    /// Transfer the channel to `extension` under `dialplan`, keeping its context.
    pub fn transfer_to_dialplan(
        extension: &str,
        dialplan: DialplanType,
    ) -> Result<EslCommand, TransferArgumentError> {
        transfer_command(&[
            (TransferField::Extension, extension),
            (TransferField::Dialplan, dialplan.name()),
        ])
    }

    /// Transfer the channel to `extension` in `context` under `dialplan`.
    ///
    /// The arguments are positional, so a context always names its dialplan:
    /// leaving it to the channel needs [`transfer`](Self::transfer).
    pub fn transfer_to_context(
        extension: &str,
        dialplan: DialplanType,
        context: &str,
    ) -> Result<EslCommand, TransferArgumentError> {
        transfer_command(&[
            (TransferField::Extension, extension),
            (TransferField::Dialplan, dialplan.name()),
            (TransferField::Context, context),
        ])
    }
}

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

/// A `transfer` argument the switch would read as something else.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TransferArgumentError {
    /// `transfer_function` splits its argument on blanks, honouring `'` quotes and
    /// `\` escapes, and this field would not come back as itself.
    #[error("transfer {field} {value:?} does not survive the switch's argument split")]
    Unsplittable {
        /// Which field.
        field: TransferField,
        /// The field as given.
        value: String,
    },
    /// `-bleg` or `-both` in any case, which `transfer_function` reads as the
    /// leg to transfer rather than as an extension.
    #[error("transfer extension {extension:?} is read as a leg selector")]
    LegSelector {
        /// The extension as given.
        extension: String,
    },
}

/// Whether `value` comes back from the switch's blank split as one token equal
/// to itself. Only the line's first token can open with a `^^X` delimiter head,
/// so a later one is checked behind a stand-in token.
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

/// First arguments `transfer_function` reads as the leg to transfer, in any case.
const LEG_SELECTORS: [&str; 2] = ["-bleg", "-both"];

fn transfer_command(fields: &[(TransferField, &str)]) -> Result<EslCommand, TransferArgumentError> {
    if let Some((_, extension)) = fields.first() {
        if LEG_SELECTORS
            .iter()
            .any(|s| extension.eq_ignore_ascii_case(s))
        {
            return Err(TransferArgumentError::LegSelector {
                extension: extension.to_string(),
            });
        }
    }
    for (index, (field, value)) in fields
        .iter()
        .enumerate()
    {
        if !survives_blank_split(value, index == 0) {
            return Err(TransferArgumentError::Unsplittable {
                field: *field,
                value: value.to_string(),
            });
        }
    }
    let args: Vec<&str> = fields
        .iter()
        .map(|(_, value)| *value)
        .collect();
    Ok(execute("transfer", Some(args.join(" "))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_args(cmd: &EslCommand) -> Option<&str> {
        match cmd {
            EslCommand::Execute { args, .. } => args.as_deref(),
            _ => panic!("expected Execute variant"),
        }
    }

    #[test]
    fn transfer_extension_only() {
        let cmd = AppCommand::transfer("1000").unwrap();
        assert_eq!(extract_args(&cmd), Some("1000"));
    }

    #[test]
    fn transfer_with_dialplan() {
        let cmd = AppCommand::transfer_to_dialplan("1000", DialplanType::XML).unwrap();
        assert_eq!(extract_args(&cmd), Some("1000 XML"));
    }

    #[test]
    fn transfer_with_dialplan_and_context() {
        let cmd = AppCommand::transfer_to_context("1000", DialplanType::XML, "myctx").unwrap();
        assert_eq!(extract_args(&cmd), Some("1000 XML myctx"));
    }

    /// transfer_function runs its argument through switch_separate_string on a
    /// blank: spaces split, `'` groups, `\` escapes, and a leading `^^X` picks
    /// another delimiter.
    #[test]
    fn transfer_refuses_a_field_the_blank_split_would_misread() {
        for extension in ["a b", "it's", "a\\\\b", "^^:1000:XML", "", " 1000"] {
            let err = AppCommand::transfer(extension).expect_err(extension);
            assert!(
                matches!(
                    &err,
                    TransferArgumentError::Unsplittable { field: TransferField::Extension, value }
                        if value == extension
                ),
                "{extension:?}: {err}"
            );
        }
        for context in ["my ctx", "", "'ctx'", "c\\tx"] {
            let err = AppCommand::transfer_to_context("1000", DialplanType::XML, context)
                .expect_err(context);
            assert!(
                matches!(
                    &err,
                    TransferArgumentError::Unsplittable { field: TransferField::Context, value }
                        if value == context
                ),
                "{context:?}: {err}"
            );
        }
    }

    #[test]
    fn transfer_refuses_an_extension_read_as_a_leg_selector() {
        for extension in ["-bleg", "-BOTH", "-Both"] {
            let err = AppCommand::transfer(extension).expect_err(extension);
            assert!(
                matches!(&err, TransferArgumentError::LegSelector { extension: e } if e == extension),
                "{extension:?}: {err}"
            );
        }
    }

    /// A `"` only means something after `\`, a `\` only before a character it
    /// escapes, and `^^` only at the very start.
    #[test]
    fn transfer_keeps_what_the_split_leaves_alone() {
        for (extension, context) in [
            ("+15551234567", "public"),
            ("*98", "x^^y"),
            ("a\"b", "default"),
            ("a\\b", "^^:x"),
        ] {
            let cmd = AppCommand::transfer_to_context(extension, DialplanType::XML, context)
                .unwrap_or_else(|e| panic!("{extension:?} {context:?}: {e}"));
            assert_eq!(
                extract_args(&cmd),
                Some(format!("{extension} XML {context}").as_str())
            );
        }
    }

    #[test]
    fn bridge_renders_the_dial_string() {
        use crate::commands::{Endpoint, SofiaGateway};
        let gateway = SofiaGateway::new("primary", "18005551234").unwrap();
        let bridge = BridgeDialString::new(vec![vec![Endpoint::SofiaGateway(gateway)]]).unwrap();
        let cmd = AppCommand::bridge(&bridge, BlockParse::PairSplitCleans);
        assert_eq!(
            extract_args(&cmd),
            Some("sofia/gateway/primary/18005551234")
        );
    }

    #[test]
    fn set_var_refuses_a_control_character_in_a_header_bound_value() {
        assert!(AppCommand::set_var("sip_h_X-Tag", "a\u{1}b").is_err());
        for (name, value) in [("sip_h_X-Tag", "a\tb"), ("probe", "a\u{1}b"), ("probe", "")] {
            assert!(AppCommand::set_var(name, value).is_ok(), "{name}={value:?}");
        }
    }
}
