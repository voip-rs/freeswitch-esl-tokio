use std::fmt;

use super::{
    after_prefix, check_field, undeliverable, write_module_text, write_variables,
    EndpointFieldFault,
};
use crate::commands::originate::OriginateError;
use crate::commands::variables::{DialStringTarget, Variables};

/// Audio device endpoint for portaudio, pulseaudio, or ALSA modules.
///
/// Wire format: `{module}[/{destination}]` where destination is typically
/// empty or `auto_answer` (recognized by portaudio and pulseaudio). The struct
/// does not record which of the three modules it belongs to, so it renders and
/// parses only through [`Endpoint::PortAudio`](super::Endpoint::PortAudio),
/// [`Endpoint::PulseAudio`](super::Endpoint::PulseAudio) or
/// [`Endpoint::Alsa`](super::Endpoint::Alsa).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "config::AudioEndpoint"))]
pub struct AudioEndpoint {
    /// Destination string (e.g. `auto_answer`). `None` for bare module name.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    destination: Option<String>,
    /// Per-channel variables prepended as `{key=value}`.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    variables: Option<Variables>,
}

impl AudioEndpoint {
    /// Create a new audio endpoint with no destination.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the destination string, refusing what parse refuses.
    pub fn with_destination(
        mut self,
        destination: impl Into<String>,
    ) -> Result<Self, OriginateError> {
        self.destination = Some(destination.into());
        self.checked()
    }

    /// Destination string (e.g. `auto_answer`), `None` for the bare module name.
    pub fn destination(&self) -> Option<&str> {
        self.destination
            .as_deref()
    }

    /// Write with the module `prefix` the [`Endpoint`](super::Endpoint) variant names.
    pub(super) fn write_with_prefix(
        &self,
        f: &mut fmt::Formatter<'_>,
        prefix: &str,
        target: DialStringTarget,
    ) -> fmt::Result {
        write_variables(f, &self.variables, target)?;
        write_module_text(f, &self.module_text(prefix), target)
    }

    /// The text after the variable block, as the `prefix` module receives it.
    pub(crate) fn module_text(&self, prefix: &str) -> String {
        match &self.destination {
            Some(dest) => format!("{prefix}/{dest}"),
            None => prefix.to_owned(),
        }
    }

    /// Refuse `:_:` and an empty destination, which the module reads as none.
    pub(crate) fn check_deliverable(&self) -> Result<(), OriginateError> {
        match self
            .destination
            .as_deref()
        {
            Some("") => Err(undeliverable(
                "audio",
                "destination",
                EndpointFieldFault::EmptyReadsAsDefault,
            )),
            Some(destination) => check_field("audio", "destination", destination, &[]),
            None => Ok(()),
        }
    }

    pub(crate) fn parse_bare(text: &str, prefix: &str) -> Result<Self, OriginateError> {
        let rest = after_prefix(text, prefix, prefix)?;
        Self {
            destination: rest
                .strip_prefix('/')
                .filter(|d| !d.is_empty())
                .map(str::to_string),
            variables: None,
        }
        .checked()
    }
}

impl_dial_string_with_variables!(AudioEndpoint);

impl_endpoint_parse!(config:
    AudioEndpoint {
        ;
        destination: Option<String>,
        variables: Option<Variables>,
    }
);

#[cfg(test)]
mod tests {
    use super::super::Endpoint;
    use super::*;
    use crate::commands::variables::DialStringCarrier;

    fn parse(s: &str) -> Endpoint {
        Endpoint::parse_for(s, DialStringCarrier::EslApi).unwrap()
    }

    #[test]
    fn parse_portaudio() {
        let Endpoint::PortAudio(ep) = parse("portaudio/auto_answer") else {
            panic!("not portaudio");
        };
        assert_eq!(
            ep.destination
                .as_deref(),
            Some("auto_answer")
        );
        assert!(ep
            .variables()
            .is_none());
    }

    #[test]
    fn parse_bare_module() {
        assert_eq!(parse("alsa"), Endpoint::Alsa(AudioEndpoint::new()));
    }

    #[test]
    fn parse_trailing_slash() {
        assert_eq!(
            parse("pulseaudio/"),
            Endpoint::PulseAudio(AudioEndpoint::new())
        );
    }

    #[test]
    fn parse_bare_refuses_another_module() {
        assert!(AudioEndpoint::parse_bare("portaudio/x", "alsa").is_err());
    }

    /// A module name is a whole path segment. Accepting a longer one and
    /// keeping the prefix drops the rest of the dial string without a word.
    #[test]
    fn parse_refuses_a_longer_module_name() {
        assert!(AudioEndpoint::parse_bare("alsafoo/bar", "alsa").is_err());
        assert!(Endpoint::parse_for("alsafoo/bar", DialStringCarrier::EslApi).is_err());
    }
}
