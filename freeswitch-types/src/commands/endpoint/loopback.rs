use super::{after_prefix, check_field, undeliverable, EndpointFieldFault};
use crate::commands::originate::{OriginateError, DEFAULT_CONTEXT};
use crate::commands::variables::Variables;

/// Internal loopback endpoint: `loopback/{extension}[/{context}[/{dialplan}]]`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "config::LoopbackEndpoint"))]
pub struct LoopbackEndpoint {
    /// Extension number or pattern, or `app=<name>[:<args>]`, which mod_loopback runs as an
    /// application and follows with no context or dialplan.
    extension: String,
    /// Dialplan context. `None` omits the context segment, letting
    /// FreeSWITCH use its default.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    context: Option<String>,
    /// Dialplan for the re-entered leg. `None` omits the segment, letting
    /// FreeSWITCH use its default.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    dialplan: Option<String>,
    /// Per-channel variables prepended as `{key=value}`.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    variables: Option<Variables>,
}

impl LoopbackEndpoint {
    /// Create a new loopback endpoint with no explicit context, refusing what parse refuses.
    pub fn new(extension: impl Into<String>) -> Result<Self, OriginateError> {
        Self {
            extension: extension.into(),
            context: None,
            dialplan: None,
            variables: None,
        }
        .checked()
    }

    /// Set the dialplan context, refusing what parse refuses.
    pub fn with_context(mut self, context: impl Into<String>) -> Result<Self, OriginateError> {
        self.context = Some(context.into());
        self.checked()
    }

    /// Set the dialplan, refusing what parse refuses. It is the third positional segment, so
    /// rendering one without a context emits [`DEFAULT_CONTEXT`] in the gap.
    pub fn with_dialplan(mut self, dialplan: impl Into<String>) -> Result<Self, OriginateError> {
        self.dialplan = Some(dialplan.into());
        self.checked()
    }

    /// Extension number or pattern, or `app=<name>[:<args>]`, which mod_loopback runs as an
    /// application.
    pub fn extension(&self) -> &str {
        &self.extension
    }

    /// Dialplan context, `None` for the switch's default.
    pub fn context(&self) -> Option<&str> {
        self.context
            .as_deref()
    }

    /// Dialplan for the re-entered leg, `None` for the switch's default.
    pub fn dialplan(&self) -> Option<&str> {
        self.dialplan
            .as_deref()
    }
}

impl_dial_string_with_variables!(LoopbackEndpoint, write_module_text, |this| match (
    &this.context,
    &this.dialplan
) {
    (None, None) => format!("loopback/{}", this.extension),
    (Some(ctx), None) => format!("loopback/{}/{}", this.extension, ctx),
    (ctx, Some(dialplan)) => format!(
        "loopback/{}/{}/{}",
        this.extension,
        ctx.as_deref()
            .unwrap_or(DEFAULT_CONTEXT),
        dialplan
    ),
});

const KIND: &str = "loopback";

/// `channel_outgoing_channel` runs an extension opening `app=` in any case as an application.
fn is_application(extension: &str) -> bool {
    extension
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("app="))
}

impl LoopbackEndpoint {
    /// Refuse what `channel_outgoing_channel` reads as something else: a `/` in the extension,
    /// the context or an application's name, an empty context or dialplan, and either after an
    /// application.
    pub(crate) fn check_deliverable(&self) -> Result<(), OriginateError> {
        if is_application(&self.extension) {
            let name = self
                .extension
                .split_once(':')
                .map_or(
                    self.extension
                        .as_str(),
                    |(name, _)| name,
                );
            check_field(KIND, "extension", name, &["/"])?;
            check_field(KIND, "extension", &self.extension, &[])?;
            for (field, value) in [("context", &self.context), ("dialplan", &self.dialplan)] {
                if value.is_some() {
                    return Err(undeliverable(
                        KIND,
                        field,
                        EndpointFieldFault::FollowsAnApplication,
                    ));
                }
            }
            return Ok(());
        }
        check_field(KIND, "extension", &self.extension, &["/"])?;
        for (field, value, separators) in [
            ("context", &self.context, &["/"][..]),
            ("dialplan", &self.dialplan, &[][..]),
        ] {
            let Some(value) = value else {
                continue;
            };
            check_field(KIND, field, value, separators)?;
            if value.is_empty() {
                return Err(undeliverable(
                    KIND,
                    field,
                    EndpointFieldFault::EmptyReadsAsDefault,
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn parse_bare(text: &str) -> Result<Self, OriginateError> {
        let rest = after_prefix(text, "loopback/", KIND)?;
        let ep = if is_application(rest) {
            Self {
                extension: rest.into(),
                context: None,
                dialplan: None,
                variables: None,
            }
        } else {
            let mut segments = rest.splitn(3, '/');
            Self {
                extension: segments
                    .next()
                    .unwrap_or(rest)
                    .into(),
                context: segments
                    .next()
                    .map(str::to_string),
                dialplan: segments
                    .next()
                    .map(str::to_string),
                variables: None,
            }
        };
        ep.checked()
    }
}

impl_endpoint_parse!(parse_for: LoopbackEndpoint);

impl_endpoint_parse!(config:
    LoopbackEndpoint {
        extension: String,
        ;
        context: Option<String>,
        dialplan: Option<String>,
        variables: Option<Variables>,
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::variables::DialStringCarrier;
    use crate::commands::variables::VariablesType;

    #[test]
    fn loopback_display_no_context() {
        let ep = LoopbackEndpoint::new("9199").unwrap();
        assert_eq!(ep.render(DialStringCarrier::EslApi), "loopback/9199");
    }

    #[test]
    fn loopback_display_with_context() {
        let ep = LoopbackEndpoint::new("9199")
            .unwrap()
            .with_context("default")
            .unwrap();
        assert_eq!(
            ep.render(DialStringCarrier::EslApi),
            "loopback/9199/default"
        );
    }

    #[test]
    fn loopback_display_with_variables() {
        let mut vars = Variables::new(VariablesType::Default);
        vars.insert("loopback_initial_codec", "L16@48000h")
            .unwrap();
        let ep = LoopbackEndpoint::new("100")
            .unwrap()
            .with_context("test")
            .unwrap()
            .with_variables(vars);
        assert_eq!(
            ep.render(DialStringCarrier::EslApi),
            "{loopback_initial_codec=L16@48000h}loopback/100/test"
        );
    }

    #[test]
    fn loopback_from_str_with_context() {
        let ep =
            LoopbackEndpoint::parse_for("loopback/9199/test", DialStringCarrier::EslApi).unwrap();
        assert_eq!(ep.extension, "9199");
        assert_eq!(
            ep.context
                .as_deref(),
            Some("test")
        );
    }

    #[test]
    fn loopback_from_str_no_context() {
        let ep = LoopbackEndpoint::parse_for("loopback/9199", DialStringCarrier::EslApi).unwrap();
        assert_eq!(ep.extension, "9199");
        assert!(ep
            .context
            .is_none());
    }

    #[test]
    fn loopback_round_trip_with_context() {
        let ep = LoopbackEndpoint::new("100")
            .unwrap()
            .with_context("myctx")
            .unwrap();
        let s = ep.render(DialStringCarrier::EslApi);
        let parsed = LoopbackEndpoint::parse_for(&s, DialStringCarrier::EslApi).unwrap();
        assert_eq!(parsed, ep);
    }

    /// `mod_loopback` splits the dial string into extension, context and
    /// dialplan. Modelling two of the three folds `default/xml` into the
    /// context, which then renders back as one segment the switch re-splits.
    #[test]
    fn loopback_carries_a_dialplan_segment() {
        let ep =
            LoopbackEndpoint::parse_for("loopback/9199/default/xml", DialStringCarrier::EslApi)
                .unwrap();
        assert_eq!(ep.extension, "9199");
        assert_eq!(
            ep.context
                .as_deref(),
            Some("default")
        );
        assert_eq!(
            ep.dialplan
                .as_deref(),
            Some("xml")
        );
        assert_eq!(
            ep.render(DialStringCarrier::EslApi),
            "loopback/9199/default/xml"
        );
    }

    /// The dialplan is the third positional segment, so it cannot be written
    /// without the second; the builder supplies the switch's own default.
    #[test]
    fn loopback_dialplan_without_a_context_fills_the_gap() {
        let ep = LoopbackEndpoint::new("9199")
            .unwrap()
            .with_dialplan("inline")
            .unwrap();
        assert_eq!(
            ep.render(DialStringCarrier::EslApi),
            "loopback/9199/default/inline"
        );
        assert_eq!(
            LoopbackEndpoint::parse_for(
                &ep.render(DialStringCarrier::EslApi),
                DialStringCarrier::EslApi
            )
            .unwrap()
            .dialplan
            .as_deref(),
            Some("inline")
        );
    }

    #[test]
    fn loopback_round_trip_no_context() {
        let ep = LoopbackEndpoint::new("9199").unwrap();
        let s = ep.render(DialStringCarrier::EslApi);
        let parsed = LoopbackEndpoint::parse_for(&s, DialStringCarrier::EslApi).unwrap();
        assert_eq!(parsed, ep);
    }

    #[test]
    fn loopback_display_parse_display_stable() {
        let inputs = [
            "loopback/9199",
            "loopback/100/default",
            "loopback/ext123/custom_ctx",
            "loopback/9199/default/xml",
            "loopback/100/custom_ctx/inline",
        ];
        for input in inputs {
            let parsed = LoopbackEndpoint::parse_for(input, DialStringCarrier::EslApi).unwrap();
            let displayed = parsed.render(DialStringCarrier::EslApi);
            assert_eq!(displayed, input, "round-trip failed for: {}", input);
            let reparsed =
                LoopbackEndpoint::parse_for(&displayed, DialStringCarrier::EslApi).unwrap();
            assert_eq!(reparsed, parsed);
        }
    }

    #[test]
    fn serde_loopback_endpoint() {
        let ep = LoopbackEndpoint::new("9199")
            .unwrap()
            .with_context("default")
            .unwrap();
        let json = serde_json::to_string(&ep).unwrap();
        let parsed: LoopbackEndpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ep);
    }
}
