//! Builders for `uuid_*` API commands that target a specific channel by UUID.

use std::fmt;

use crate::channel::HangupCause;
use crate::commands::originate::DialplanType;
use crate::commands::transfer::{check_fields, TransferArgumentError, TransferField, TransferLine};
use crate::commands::variables::check_header_value;
use crate::commands::variables::UndeliverableVariable;
use crate::switch_passes::api_argument::quote_for_uuid_setvar;

/// Answer a channel: `uuid_answer <uuid>`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UuidAnswer {
    /// Channel UUID.
    pub uuid: String,
}

impl UuidAnswer {
    /// Create a new uuid_answer command.
    pub fn new(uuid: impl Into<String>) -> Self {
        Self { uuid: uuid.into() }
    }
}

impl fmt::Display for UuidAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "uuid_answer {}", self.uuid)
    }
}

/// Bridge two channels: `uuid_bridge <uuid> <other_uuid>`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UuidBridge {
    /// Channel UUID.
    pub uuid: String,
    /// UUID of the channel to bridge to.
    pub other: String,
}

impl UuidBridge {
    /// Create a new uuid_bridge command.
    pub fn new(uuid: impl Into<String>, other: impl Into<String>) -> Self {
        Self {
            uuid: uuid.into(),
            other: other.into(),
        }
    }
}

impl fmt::Display for UuidBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "uuid_bridge {} {}", self.uuid, self.other)
    }
}

/// Deflect (redirect) a channel to a new SIP URI: `uuid_deflect <uuid> <uri>`.
///
/// Sends a SIP REFER to the endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UuidDeflect {
    /// Channel UUID.
    pub uuid: String,
    /// SIP URI to redirect to (e.g. `sip:user@host`).
    pub uri: String,
}

impl UuidDeflect {
    /// Create a new uuid_deflect command.
    pub fn new(uuid: impl Into<String>, uri: impl Into<String>) -> Self {
        Self {
            uuid: uuid.into(),
            uri: uri.into(),
        }
    }
}

impl fmt::Display for UuidDeflect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "uuid_deflect {} {}", self.uuid, self.uri)
    }
}

/// What `uuid_hold` does to the channel, read from its optional first word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UuidHoldAction {
    /// No keyword: place on hold.
    Hold,
    /// `off`: take off hold.
    Unhold,
    /// `toggle`: hold if active, unhold if held.
    Toggle,
}

/// Place a channel on hold, take it off, or toggle:
/// `uuid_hold [off|toggle] <uuid>`.
///
/// The optional `<display>` argument mod_commands takes after the UUID for
/// hold and toggle is not modelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UuidHold {
    uuid: String,
    action: UuidHoldAction,
}

impl UuidHold {
    /// Place a channel on hold.
    pub fn hold(uuid: impl Into<String>) -> Self {
        Self::new(uuid, UuidHoldAction::Hold)
    }

    /// Take a channel off hold.
    pub fn unhold(uuid: impl Into<String>) -> Self {
        Self::new(uuid, UuidHoldAction::Unhold)
    }

    /// Toggle a channel's hold state.
    pub fn toggle(uuid: impl Into<String>) -> Self {
        Self::new(uuid, UuidHoldAction::Toggle)
    }

    /// Build for an action chosen at run time.
    pub fn new(uuid: impl Into<String>, action: UuidHoldAction) -> Self {
        Self {
            uuid: uuid.into(),
            action,
        }
    }

    /// Channel UUID.
    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    /// What the command does to the channel.
    pub fn action(&self) -> UuidHoldAction {
        self.action
    }
}

impl fmt::Display for UuidHold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.action {
            UuidHoldAction::Hold => write!(f, "uuid_hold {}", self.uuid),
            UuidHoldAction::Unhold => write!(f, "uuid_hold off {}", self.uuid),
            UuidHoldAction::Toggle => write!(f, "uuid_hold toggle {}", self.uuid),
        }
    }
}

/// Kill a channel: `uuid_kill <uuid> [cause]`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UuidKill {
    /// Channel UUID.
    pub uuid: String,
    /// Hangup cause. If `None`, uses FreeSWITCH default.
    pub cause: Option<HangupCause>,
}

impl UuidKill {
    /// Kill a channel with the default hangup cause.
    pub fn new(uuid: impl Into<String>) -> Self {
        Self {
            uuid: uuid.into(),
            cause: None,
        }
    }

    /// Kill a channel with a specific hangup cause.
    pub fn with_cause(uuid: impl Into<String>, cause: HangupCause) -> Self {
        Self {
            uuid: uuid.into(),
            cause: Some(cause),
        }
    }
}

impl fmt::Display for UuidKill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "uuid_kill {}", self.uuid)?;
        if let Some(ref cause) = self.cause {
            write!(f, " {}", cause)?;
        }
        Ok(())
    }
}

/// Get a channel variable: `uuid_getvar <uuid> <key>`.
///
/// Note: FreeSWITCH returns the bare value (no `+OK` prefix), so
/// [`EslResponse::reply_status()`] will be [`ReplyStatus::Other`].
///
/// [`EslResponse::reply_status()`]: https://docs.rs/freeswitch-esl-tokio/latest/freeswitch_esl_tokio/struct.EslResponse.html#method.reply_status
/// [`ReplyStatus::Other`]: https://docs.rs/freeswitch-esl-tokio/latest/freeswitch_esl_tokio/enum.ReplyStatus.html#variant.Other
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UuidGetVar {
    /// Channel UUID.
    pub uuid: String,
    /// Variable name to read.
    pub key: String,
}

impl UuidGetVar {
    /// Create a new uuid_getvar command.
    pub fn new(uuid: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            uuid: uuid.into(),
            key: key.into(),
        }
    }
}

impl fmt::Display for UuidGetVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "uuid_getvar {} {}", self.uuid, self.key)
    }
}

/// Set a channel variable: `uuid_setvar <uuid> <key> <value>`.
///
/// `Display` single-quotes and escapes the value for the API's blank split, which would
/// otherwise strip its quotes and read its backslashes as escapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UuidSetVar {
    uuid: String,
    key: String,
    value: String,
}

impl UuidSetVar {
    /// Create a new uuid_setvar command, refusing a value with a control character other than
    /// tab under a name mod_sofia sends as an outgoing SIP header.
    pub fn new(
        uuid: impl Into<String>,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, UndeliverableVariable> {
        let (key, value) = (key.into(), value.into());
        check_header_value(&key, &value)?;
        Ok(Self {
            uuid: uuid.into(),
            key,
            value,
        })
    }

    /// Channel UUID.
    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    /// Variable name to set.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Variable value as the channel will hold it, before the quoting `Display` adds.
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for UuidSetVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = quote_for_uuid_setvar(&self.value);
        write!(f, "uuid_setvar {} {} {}", self.uuid, self.key, value)
    }
}

/// Transfer a channel to a new destination: `uuid_transfer <uuid> <dest> [dialplan]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UuidTransfer {
    uuid: String,
    destination: String,
    dialplan: Option<DialplanType>,
}

impl UuidTransfer {
    /// Transfer a channel to a new destination, refusing one the switch would read as
    /// something else; see [`TransferArgumentError`].
    pub fn new(
        uuid: impl Into<String>,
        destination: impl Into<String>,
    ) -> Result<Self, TransferArgumentError> {
        let destination = destination.into();
        check_fields(
            &[(TransferField::Extension, &destination)],
            TransferLine::UuidTransfer,
        )?;
        Ok(Self {
            uuid: uuid.into(),
            destination,
            dialplan: None,
        })
    }

    /// Set the dialplan to use for the transfer, refusing a name the blank split would cut.
    pub fn with_dialplan(mut self, dialplan: DialplanType) -> Result<Self, TransferArgumentError> {
        check_fields(
            &[
                (TransferField::Extension, &self.destination),
                (TransferField::Dialplan, dialplan.name()),
            ],
            TransferLine::UuidTransfer,
        )?;
        self.dialplan = Some(dialplan);
        Ok(self)
    }

    /// Channel UUID.
    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    /// Destination extension or dial string.
    pub fn destination(&self) -> &str {
        &self.destination
    }

    /// Dialplan to use; `None` keeps the channel's current one.
    pub fn dialplan(&self) -> Option<&DialplanType> {
        self.dialplan
            .as_ref()
    }
}

impl fmt::Display for UuidTransfer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "uuid_transfer {} {}", self.uuid, self.destination)?;
        if let Some(ref dp) = self.dialplan {
            write!(f, " {}", dp)?;
        }
        Ok(())
    }
}

/// Send DTMF digits to a channel: `uuid_send_dtmf <uuid> <digits>`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UuidSendDtmf {
    /// Channel UUID.
    pub uuid: String,
    /// DTMF digit string (e.g. `"1234#"`).
    pub dtmf: String,
}

impl UuidSendDtmf {
    /// Create a new uuid_send_dtmf command.
    pub fn new(uuid: impl Into<String>, dtmf: impl Into<String>) -> Self {
        Self {
            uuid: uuid.into(),
            dtmf: dtmf.into(),
        }
    }
}

impl fmt::Display for UuidSendDtmf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "uuid_send_dtmf {} {}", self.uuid, self.dtmf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{originate_split, originate_unquote};

    const UUID: &str = "abc12345-6789-0abc-def0-123456789abc";
    const OTHER: &str = "def12345-6789-0abc-def0-123456789abc";

    /// Every type here is `#[non_exhaustive]`, so its constructor is the only
    /// path an external caller has and the only one worth pinning.
    #[test]
    fn uuid_answer() {
        let cmd = UuidAnswer::new(UUID);
        assert_eq!(cmd.to_string(), format!("uuid_answer {}", UUID));
    }

    #[test]
    fn uuid_bridge() {
        let cmd = UuidBridge::new(UUID, OTHER);
        assert_eq!(cmd.to_string(), format!("uuid_bridge {} {}", UUID, OTHER));
    }

    #[test]
    fn uuid_deflect() {
        let cmd = UuidDeflect::new(UUID, "sip:user@host");
        assert_eq!(
            cmd.to_string(),
            format!("uuid_deflect {} sip:user@host", UUID)
        );
    }

    #[test]
    fn uuid_hold_on() {
        let cmd = UuidHold::hold(UUID);
        assert_eq!(cmd.to_string(), format!("uuid_hold {}", UUID));
    }

    #[test]
    fn uuid_hold_off() {
        let cmd = UuidHold::unhold(UUID);
        assert_eq!(cmd.to_string(), format!("uuid_hold off {}", UUID));
    }

    #[test]
    fn uuid_hold_toggle() {
        let cmd = UuidHold::toggle(UUID);
        assert_eq!(cmd.to_string(), format!("uuid_hold toggle {}", UUID));
        assert_eq!(cmd.action(), UuidHoldAction::Toggle);
        assert_eq!(cmd.uuid(), UUID);
    }

    #[test]
    fn uuid_kill_no_cause() {
        let cmd = UuidKill::new(UUID);
        assert_eq!(cmd.to_string(), format!("uuid_kill {}", UUID));
    }

    #[test]
    fn uuid_kill_with_cause() {
        let cmd = UuidKill::with_cause(UUID, HangupCause::NormalClearing);
        assert_eq!(
            cmd.to_string(),
            format!("uuid_kill {} NORMAL_CLEARING", UUID)
        );
    }

    #[test]
    fn uuid_getvar() {
        let cmd = UuidGetVar::new(UUID, "sip_call_id");
        assert_eq!(cmd.to_string(), format!("uuid_getvar {} sip_call_id", UUID));
    }

    #[test]
    fn uuid_setvar() {
        let cmd = UuidSetVar::new(UUID, "hangup_after_bridge", "true").unwrap();
        assert_eq!(
            cmd.to_string(),
            format!("uuid_setvar {} hangup_after_bridge 'true'", UUID)
        );
    }

    /// The value goes through the switch's blank split, so `Display` quotes it and
    /// `value()` keeps what the channel will hold.
    #[test]
    fn uuid_setvar_quotes_the_value_for_the_blank_split() {
        let value = "a b 'c' \\d";
        let cmd = UuidSetVar::new(UUID, "probe", value).unwrap();
        assert_eq!(cmd.value(), value);
        assert_eq!(
            cmd.to_string(),
            format!("uuid_setvar {UUID} probe {}", quote_for_uuid_setvar(value))
        );
        let tokens = originate_split(&cmd.to_string(), ' ').unwrap();
        assert_eq!(tokens.len(), 4);
        assert_eq!(originate_unquote(&tokens[3]), value);
    }

    #[test]
    fn uuid_transfer_no_dialplan() {
        let cmd = UuidTransfer::new(UUID, "1000").unwrap();
        assert_eq!(cmd.to_string(), format!("uuid_transfer {} 1000", UUID));
    }

    #[test]
    fn uuid_transfer_with_dialplan() {
        let cmd = UuidTransfer::new(UUID, "1000")
            .unwrap()
            .with_dialplan(DialplanType::XML)
            .unwrap();
        assert_eq!(cmd.to_string(), format!("uuid_transfer {} 1000 XML", UUID));
    }

    /// What `transfer_function` reads off a `uuid_transfer` line: the destination and
    /// dialplan back as built, and no leg selector, which it takes from any leading `-`.
    #[test]
    fn uuid_transfer_reads_back_as_built() {
        let (mut misread, mut refused) = (Vec::new(), Vec::new());
        for destination in [
            "1000", "a b", "it's", "a\\\\b", "", "-bleg", "-1000", "-BOTH",
        ] {
            for dialplan in [None, Some("XML"), Some("my plan")] {
                let built = UuidTransfer::new(UUID, destination).and_then(|cmd| match dialplan {
                    Some(name) => cmd.with_dialplan(DialplanType::named(name).unwrap()),
                    None => Ok(cmd),
                });
                let Ok(cmd) = built else {
                    refused.push((destination, dialplan));
                    continue;
                };
                let line = cmd.to_string();
                let read: Option<Vec<String>> = originate_split(&line, ' ')
                    .ok()
                    .map(|tokens| {
                        tokens
                            .iter()
                            .map(|t| originate_unquote(t))
                            .collect()
                    });
                let mut expected =
                    vec!["uuid_transfer".to_string(), UUID.into(), destination.into()];
                expected.extend(dialplan.map(String::from));
                if read.as_ref() != Some(&expected) || destination.starts_with('-') {
                    misread.push(format!("{line:?} -> {read:?}"));
                }
            }
        }
        assert!(misread.is_empty(), "{misread:#?}");
        assert!(
            !refused.contains(&("1000", None)) && !refused.contains(&("1000", Some("XML"))),
            "{refused:?}"
        );
    }

    #[test]
    fn uuid_send_dtmf() {
        let cmd = UuidSendDtmf::new(UUID, "1234#");
        assert_eq!(cmd.to_string(), format!("uuid_send_dtmf {} 1234#", UUID));
    }

    /// sofia copies a header-bound variable into the outgoing message verbatim, and its parser
    /// ends a field only at CR/LF, so any other control character ships.
    #[test]
    fn uuid_setvar_refuses_a_control_character_in_a_header_bound_value() {
        use crate::commands::variables::{VariableFault, VariablePart};
        for key in ["sip_h_X-Tag", "SIP_RH_X-Tag", "sip_bye_h_X-Tag"] {
            let err = UuidSetVar::new(UUID, key, "a\u{1}b").expect_err(key);
            assert_eq!(
                (err.part, err.fault),
                (VariablePart::Value, VariableFault::HeaderControl),
                "{key}"
            );
        }
    }

    /// Only the header rule is established for `uuid_setvar`: an empty value, a tab or a
    /// control character outside a header name pass.
    #[test]
    fn uuid_setvar_passes_what_no_header_rule_refuses() {
        for (key, value) in [
            ("sip_h_X-Tag", "a\tb"),
            ("probe", "a\u{1}b"),
            ("sip_i_x_tag", "a\u{1}b"),
            ("probe", ""),
        ] {
            assert!(UuidSetVar::new(UUID, key, value).is_ok(), "{key}={value:?}");
        }
    }
}
