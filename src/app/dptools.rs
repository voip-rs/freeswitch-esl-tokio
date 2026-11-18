//! FreeSWITCH dptools application commands (`answer`, `hangup`, `playback`, etc.).

use std::fmt;

use crate::channel::HangupCause;
use crate::command::{EslCommand, ExecuteOptions};
use crate::commands::originate::DialplanType;
use crate::commands::UndeliverableVariable;

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

    /// `destination`: dial string for the B-leg. Accepts `&str`,
    /// [`BridgeDialString`](crate::commands::BridgeDialString), [`Endpoint`](crate::Endpoint), etc.
    pub fn bridge(destination: impl fmt::Display) -> EslCommand {
        execute("bridge", Some(destination.to_string()))
    }

    /// Set a channel variable (`set` application).
    pub fn set_var(name: &str, value: &str) -> Result<EslCommand, UndeliverableVariable> {
        Ok(execute("set", Some(format!("{}={}", name, value))))
    }

    /// Park the channel (suspends dialplan execution until another command picks it up).
    pub fn park() -> EslCommand {
        execute("park", None)
    }

    /// Transfer the channel to another dialplan extension.
    ///
    /// FreeSWITCH transfer args are positional: `extension [dialplan [context]]`.
    /// When `context` is set but `dialplan` is `None`, the default `XML` is
    /// emitted to fill the positional gap.
    pub fn transfer(
        extension: &str,
        dialplan: Option<DialplanType>,
        context: Option<&str>,
    ) -> EslCommand {
        let mut args = extension.to_string();
        let has_ctx = context.is_some();
        if let Some(dp) = dialplan {
            args.push(' ');
            args.push_str(&dp.to_string());
        } else if has_ctx {
            args.push_str(" XML");
        }
        if let Some(ctx) = context {
            args.push(' ');
            args.push_str(ctx);
        }

        execute("transfer", Some(args))
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
        let cmd = AppCommand::transfer("1000", None, None);
        assert_eq!(extract_args(&cmd), Some("1000"));
    }

    #[test]
    fn transfer_with_dialplan() {
        let cmd = AppCommand::transfer("1000", Some(DialplanType::XML), None);
        assert_eq!(extract_args(&cmd), Some("1000 XML"));
    }

    #[test]
    fn transfer_with_dialplan_and_context() {
        let cmd = AppCommand::transfer("1000", Some(DialplanType::XML), Some("myctx"));
        assert_eq!(extract_args(&cmd), Some("1000 XML myctx"));
    }

    #[test]
    fn transfer_context_without_dialplan_fills_gap() {
        let cmd = AppCommand::transfer("1000", None, Some("myctx"));
        assert_eq!(extract_args(&cmd), Some("1000 XML myctx"));
    }
}
