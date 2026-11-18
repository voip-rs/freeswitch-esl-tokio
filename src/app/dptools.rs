//! FreeSWITCH dptools application commands (`answer`, `hangup`, `playback`, etc.).

use crate::channel::HangupCause;
use crate::command::{EslCommand, ExecuteOptions};
use crate::commands::originate::DialplanType;
use crate::commands::{BlockParse, BridgeDialString, UndeliverableVariable};
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
    pub fn transfer(extension: &str) -> EslCommand {
        execute("transfer", Some(extension.to_string()))
    }

    /// Transfer the channel to `extension` under `dialplan`, keeping its context.
    pub fn transfer_to_dialplan(extension: &str, dialplan: DialplanType) -> EslCommand {
        execute("transfer", Some(format!("{extension} {dialplan}")))
    }

    /// Transfer the channel to `extension` in `context` under `dialplan`.
    ///
    /// The arguments are positional, so a context always names its dialplan:
    /// leaving it to the channel needs [`transfer`](Self::transfer).
    pub fn transfer_to_context(
        extension: &str,
        dialplan: DialplanType,
        context: &str,
    ) -> EslCommand {
        execute(
            "transfer",
            Some(format!("{extension} {dialplan} {context}")),
        )
    }
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
        let cmd = AppCommand::transfer("1000");
        assert_eq!(extract_args(&cmd), Some("1000"));
    }

    #[test]
    fn transfer_with_dialplan() {
        let cmd = AppCommand::transfer_to_dialplan("1000", DialplanType::XML);
        assert_eq!(extract_args(&cmd), Some("1000 XML"));
    }

    #[test]
    fn transfer_with_dialplan_and_context() {
        let cmd = AppCommand::transfer_to_context("1000", DialplanType::XML, "myctx");
        assert_eq!(extract_args(&cmd), Some("1000 XML myctx"));
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
