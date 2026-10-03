//! FreeSWITCH endpoint types for originate and bridge dial strings.
//!
//! Each endpoint type corresponds to a real FreeSWITCH endpoint module
//! or runtime expression. Concrete structs implement the [`DialString`]
//! trait independently; the [`Endpoint`] enum wraps them for
//! serialization and polymorphic storage.

/// Emit the [`DialString`] impl and the `with_variables` builder for an endpoint
/// struct holding `variables: Option<Variables>`. Given the module text and the
/// function writing it, also emit the target-aware `write_for` — variable-block
/// prologue included — and the [`Display`](std::fmt::Display) that renders it for
/// the default target.
macro_rules! impl_dial_string_with_variables {
    ($ty:ty) => {
        impl $crate::commands::endpoint::DialString for $ty {
            fn variables(&self) -> Option<&$crate::commands::variables::Variables> {
                self.variables
                    .as_ref()
            }
            fn variables_mut(&mut self) -> Option<&mut $crate::commands::variables::Variables> {
                self.variables
                    .as_mut()
            }
            fn set_variables(&mut self, vars: Option<$crate::commands::variables::Variables>) {
                self.variables = vars;
            }
        }

        impl $ty {
            /// Set per-channel variables.
            pub fn with_variables(
                mut self,
                variables: $crate::commands::variables::Variables,
            ) -> Self {
                self.variables = Some(variables);
                self
            }
        }
    };
    ($ty:ty, $write:ident, |$this:ident| $text:expr) => {
        impl_dial_string_with_variables!($ty);

        impl $ty {
            /// The text after the variable block, as the endpoint module receives it.
            pub(crate) fn module_text(&self) -> String {
                let $this = self;
                $text
            }

            pub(super) fn write_for(
                &self,
                f: &mut ::std::fmt::Formatter<'_>,
                target: $crate::commands::variables::DialStringTarget,
            ) -> ::std::fmt::Result {
                $crate::commands::endpoint::write_variables(f, &self.variables, target)?;
                $crate::commands::endpoint::$write(f, &self.module_text(), target)
            }
        }

        impl ::std::fmt::Display for $ty {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                self.write_for(
                    f,
                    $crate::commands::variables::DialStringCarrier::EslApi.into(),
                )
            }
        }
    };
}

/// Dispatch a [`DialString`] method to whichever concrete endpoint the
/// [`Endpoint`] variant wraps.
macro_rules! forward_to_variant {
    ($self:ident, $method:ident $(, $arg:expr)?) => {
        match $self {
            Self::Sofia(ep) => ep.$method($($arg)?),
            Self::SofiaGateway(ep) => ep.$method($($arg)?),
            Self::Loopback(ep) => ep.$method($($arg)?),
            Self::User(ep) => ep.$method($($arg)?),
            Self::SofiaContact(ep) => ep.$method($($arg)?),
            Self::GroupCall(ep) => ep.$method($($arg)?),
            Self::Error(ep) => ep.$method($($arg)?),
            Self::PortAudio(ep) | Self::PulseAudio(ep) | Self::Alsa(ep) => ep.$method($($arg)?),
        }
    };
}

/// Emit `FromStr` at the ESL API carrier for each endpoint parsed from a dial string, or the serde
/// `config` module a file's endpoints load through, each load refusing what `check_deliverable` refuses.
macro_rules! impl_endpoint_parse {
    (from_str: $($ty:ident),* $(,)?) => {
        $(
            impl ::std::str::FromStr for $ty {
                type Err = $crate::commands::originate::OriginateError;

                fn from_str(s: &str) -> Result<Self, Self::Err> {
                    let (variables, ep) = $crate::commands::endpoint::parse_leg(
                        s,
                        $crate::commands::variables::DialStringCarrier::EslApi.into(),
                        Self::parse_bare,
                    )?;
                    Ok(Self { variables, ..ep })
                }
            }
        )*
    };
    (expression_from_str: $($ty:ident => $kind:literal),* $(,)?) => {
        $(
            impl ::std::str::FromStr for $ty {
                type Err = $crate::commands::originate::OriginateError;

                /// Refused once parsed: `FromStr` reads the API carrier, which never expands it.
                fn from_str(s: &str) -> Result<Self, Self::Err> {
                    $crate::commands::endpoint::parse_leg(
                        s,
                        $crate::commands::variables::DialStringCarrier::EslApi.into(),
                        Self::parse_bare,
                    )?;
                    Err($crate::commands::originate::OriginateError::UnexpandedExpression {
                        endpoint: $kind,
                    })
                }
            }
        )*
    };
    (config: $($ty:ident { $($required:ident: $rty:ty,)* ; $($optional:ident: $oty:ty,)* })*) => {
        #[cfg(feature = "serde")]
        mod config {
            use super::*;

            $(
                #[derive(serde::Deserialize)]
                pub(super) struct $ty {
                    $(pub(super) $required: $rty,)*
                    $(
                        #[serde(default)]
                        pub(super) $optional: $oty,
                    )*
                }
            )*
        }

        $(
            #[cfg(feature = "serde")]
            impl TryFrom<config::$ty> for $ty {
                type Error = $crate::commands::originate::OriginateError;

                fn try_from(config: config::$ty) -> Result<Self, Self::Error> {
                    let ep = Self {
                        $($required: config.$required,)*
                        $($optional: config.$optional,)*
                    };
                    ep.check_deliverable()?;
                    Ok(ep)
                }
            }
        )*
    };
}

mod audio;
mod error;
mod group_call;
mod loopback;
mod sofia;
mod user;

pub use audio::AudioEndpoint;
pub use error::ErrorEndpoint;
pub use group_call::{GroupCall, GroupCallOrder, ParseGroupCallOrderError};
pub use loopback::LoopbackEndpoint;
pub use sofia::{SofiaContact, SofiaEndpoint, SofiaGateway};
pub use user::UserEndpoint;

use std::fmt;
use std::str::FromStr;

use super::originate::OriginateError;
use super::variables::{
    installed_variables, read_leg, DialStringCarrier, DialStringTarget, Variables,
};
use crate::switch_passes::brackets::unbalanced;
use crate::switch_passes::escape::{escape_text, EscapedField};
use crate::switch_passes::originate_legs::splits_into_threads;

type PrefixParser = fn(&str) -> Result<Endpoint, OriginateError>;

/// Why an endpoint field cannot reach the switch as written, whatever the escaping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EndpointFieldFault {
    /// Carries `:_:`, on which `switch_ivr_originate` splits the dial string into threads.
    EnterpriseSeparator,
    /// Carries a separator the endpoint module splits its text on.
    ModuleSeparator(&'static str),
    /// A sofia profile reading `gateway` in any case, which mod_sofia takes for the gateway path.
    ReadsAsGateway,
    /// Set after an `app=` loopback extension, which mod_loopback reads as that application's.
    FollowsAnApplication,
    /// Empty, which the module or function reads as its default.
    EmptyReadsAsDefault,
    /// Carries a separator a `sofia_contact` or `group_call` function splits its argument on.
    FunctionSeparator(&'static str),
    /// Carries what a pass reads ahead of a `sofia_contact` or `group_call` expansion: a space,
    /// `,`, `|`, a quote, a backslash, or an unbalanced brace or parenthesis.
    ReadBeforeTheFunction,
    /// Empty where the module needs text to dial, so it stops before placing a call.
    EmptyDialsNothing,
}

impl fmt::Display for EndpointFieldFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EnterpriseSeparator => {
                f.write_str("carries :_:, on which the switch splits the dial string into threads")
            }
            Self::ModuleSeparator(sep) => {
                write!(
                    f,
                    "carries {sep}, on which the endpoint module splits its text"
                )
            }
            Self::ReadsAsGateway => f.write_str("reads as gateway, which mod_sofia dials as one"),
            Self::FollowsAnApplication => f.write_str(
                "follows an app= extension, which mod_loopback reads as the application's argument",
            ),
            Self::EmptyReadsAsDefault => {
                f.write_str("is empty, which the switch reads as its default")
            }
            Self::FunctionSeparator(sep) => write!(
                f,
                "carries {sep}, on which the expression's function splits its argument"
            ),
            Self::ReadBeforeTheFunction => f.write_str(
                "carries a space, comma, pipe, quote, backslash or unbalanced bracket, \
                 which a pass reads before the expression's function",
            ),
            Self::EmptyDialsNothing => {
                f.write_str("is empty, so the endpoint module dials nothing")
            }
        }
    }
}

/// Refuse `text` as `field` of the `endpoint` expression when it carries `:_:`, one of the
/// function's `separators`, or what [`EndpointFieldFault::ReadBeforeTheFunction`] names.
pub(super) fn check_expression_field(
    endpoint: &'static str,
    field: &'static str,
    text: &str,
    separators: &[&'static str],
) -> Result<(), OriginateError> {
    let fault = if splits_into_threads(text) {
        Some(EndpointFieldFault::EnterpriseSeparator)
    } else if let Some(sep) = separators
        .iter()
        .find(|sep| text.contains(**sep))
    {
        Some(EndpointFieldFault::FunctionSeparator(sep))
    } else if text.contains([' ', ',', '|', '\'', '\\'])
        || unbalanced(text, ('{', '}')).is_some()
        || unbalanced(text, ('(', ')')).is_some()
    {
        Some(EndpointFieldFault::ReadBeforeTheFunction)
    } else {
        None
    };
    fault.map_or(Ok(()), |fault| Err(undeliverable(endpoint, field, fault)))
}

pub(super) fn undeliverable(
    endpoint: &'static str,
    field: &'static str,
    fault: EndpointFieldFault,
) -> OriginateError {
    OriginateError::UndeliverableEndpointField {
        endpoint,
        field,
        fault,
    }
}

/// Refuse `text` as `field` of `endpoint` when it carries `:_:` or one of `separators`.
pub(super) fn check_field(
    endpoint: &'static str,
    field: &'static str,
    text: &str,
    separators: &[&'static str],
) -> Result<(), OriginateError> {
    let fault = if splits_into_threads(text) {
        Some(EndpointFieldFault::EnterpriseSeparator)
    } else {
        separators
            .iter()
            .find(|sep| text.contains(**sep))
            .map(|sep| EndpointFieldFault::ModuleSeparator(sep))
    };
    fault.map_or(Ok(()), |fault| Err(undeliverable(endpoint, field, fault)))
}

/// Common interface for anything that formats as a FreeSWITCH dial string.
///
/// Implemented on each concrete endpoint struct and on the [`Endpoint`] enum.
/// Downstream crates can implement this on custom endpoint types.
pub trait DialString: fmt::Display {
    /// Per-endpoint variables, if any.
    fn variables(&self) -> Option<&Variables>;
    /// Mutable access to per-endpoint variables.
    fn variables_mut(&mut self) -> Option<&mut Variables>;
    /// Replace per-endpoint variables.
    fn set_variables(&mut self, vars: Option<Variables>);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn write_variables(
    f: &mut fmt::Formatter<'_>,
    vars: &Option<Variables>,
    target: DialStringTarget,
) -> fmt::Result {
    if let Some(vars) = vars {
        if !vars.is_empty() {
            write!(f, "{}", vars.display_for(target))?;
        }
    }
    Ok(())
}

/// Module text, escaped for the carrier's pass and the leg splits ahead of the module.
fn write_module_text(
    f: &mut fmt::Formatter<'_>,
    text: &str,
    target: DialStringTarget,
) -> fmt::Result {
    f.write_str(&escape_text(text, target, EscapedField::Endpoint))
}

/// A runtime expression, written for the switch to expand.
fn write_expression(f: &mut fmt::Formatter<'_>, text: &str, _: DialStringTarget) -> fmt::Result {
    f.write_str(text)
}

/// The module text after a fixed endpoint prefix. A prefix that does not itself end a path
/// segment (`alsa`, not `sofia/`) must be followed by `/` or by nothing, or `alsafoo/bar` strips
/// to a bare `alsa`.
pub(super) fn after_prefix<'a>(
    text: &'a str,
    prefix: &str,
    kind: &str,
) -> Result<&'a str, OriginateError> {
    text.strip_prefix(prefix)
        .filter(|rest| prefix.ends_with('/') || rest.is_empty() || rest.starts_with('/'))
        .ok_or_else(|| OriginateError::ParseError(format!("not a {kind} endpoint")))
}

/// Parse a dial string written for `target`: the one leg the switch reads of it, its block's
/// variables, and its module text through `bare`.
pub(super) fn parse_leg<T>(
    s: &str,
    target: DialStringTarget,
    bare: impl FnOnce(&str) -> Result<T, OriginateError>,
) -> Result<(Option<Variables>, T), OriginateError> {
    let (argument, target) = target.read_argument(s)?;
    let (block, text) = read_leg(&argument, target)?;
    Ok((installed_variables(block.as_ref())?, bare(&text)?))
}

// ---------------------------------------------------------------------------
// Endpoint enum
// ---------------------------------------------------------------------------

/// Polymorphic endpoint wrapping all concrete types.
///
/// Use this in [`Originate`](super::originate::Originate) and
/// [`BridgeDialString`](super::bridge::BridgeDialString) where any endpoint type must be accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum Endpoint {
    /// `sofia/{profile}/{destination}`
    Sofia(SofiaEndpoint),
    /// `sofia/gateway/[{profile}::]{gateway}/{destination}`
    SofiaGateway(SofiaGateway),
    /// `loopback/{extension}[/{context}]`
    Loopback(LoopbackEndpoint),
    /// `user/{name}[@{domain}]`
    User(UserEndpoint),
    /// `${sofia_contact([profile/]user@domain)}`
    SofiaContact(SofiaContact),
    /// `${group_call(group@domain[+order])}`
    GroupCall(GroupCall),
    /// `error/{cause}`
    Error(ErrorEndpoint),
    /// `portaudio[/{destination}]`
    #[cfg_attr(feature = "serde", serde(rename = "portaudio"))]
    PortAudio(AudioEndpoint),
    /// `pulseaudio[/{destination}]`
    #[cfg_attr(feature = "serde", serde(rename = "pulseaudio"))]
    PulseAudio(AudioEndpoint),
    /// `alsa[/{destination}]`
    Alsa(AudioEndpoint),
}

// ---------------------------------------------------------------------------
// From impls
// ---------------------------------------------------------------------------

impl From<SofiaEndpoint> for Endpoint {
    fn from(ep: SofiaEndpoint) -> Self {
        Self::Sofia(ep)
    }
}

impl From<SofiaGateway> for Endpoint {
    fn from(ep: SofiaGateway) -> Self {
        Self::SofiaGateway(ep)
    }
}

impl From<LoopbackEndpoint> for Endpoint {
    fn from(ep: LoopbackEndpoint) -> Self {
        Self::Loopback(ep)
    }
}

impl From<UserEndpoint> for Endpoint {
    fn from(ep: UserEndpoint) -> Self {
        Self::User(ep)
    }
}

impl From<SofiaContact> for Endpoint {
    fn from(ep: SofiaContact) -> Self {
        Self::SofiaContact(ep)
    }
}

impl From<GroupCall> for Endpoint {
    fn from(ep: GroupCall) -> Self {
        Self::GroupCall(ep)
    }
}

impl From<ErrorEndpoint> for Endpoint {
    fn from(ep: ErrorEndpoint) -> Self {
        Self::Error(ep)
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

impl Endpoint {
    pub(crate) fn write_for(
        &self,
        f: &mut fmt::Formatter<'_>,
        target: DialStringTarget,
    ) -> fmt::Result {
        match self {
            Self::Sofia(ep) => ep.write_for(f, target),
            Self::SofiaGateway(ep) => ep.write_for(f, target),
            Self::Loopback(ep) => ep.write_for(f, target),
            Self::User(ep) => ep.write_for(f, target),
            Self::SofiaContact(ep) => ep.write_for(f, target),
            Self::GroupCall(ep) => ep.write_for(f, target),
            Self::Error(ep) => fmt::Display::fmt(ep, f),
            Self::PortAudio(ep) => ep.write_with_prefix(f, "portaudio", target),
            Self::PulseAudio(ep) => ep.write_with_prefix(f, "pulseaudio", target),
            Self::Alsa(ep) => ep.write_with_prefix(f, "alsa", target),
        }
    }

    /// Render for a named carrier or [`DialStringTarget`] rather than the
    /// [`DialStringCarrier::EslApi`] default of [`Display`](fmt::Display).
    pub fn display_for(&self, target: impl Into<DialStringTarget>) -> EndpointDisplay<'_> {
        EndpointDisplay {
            endpoint: self,
            target: target.into(),
        }
    }

    /// Parse a dial string written for a named carrier or [`DialStringTarget`],
    /// mirroring [`display_for`](Self::display_for). [`FromStr`] uses the
    /// [`DialStringCarrier::EslApi`] default.
    ///
    /// A `sofia_contact` or `group_call` expression parses at the dialplan carrier only.
    pub fn parse_for(s: &str, target: impl Into<DialStringTarget>) -> Result<Self, OriginateError> {
        let target = target.into();
        let (variables, mut endpoint) = parse_leg(s, target, Self::parse_bare)?;
        endpoint.check_expanded(target.carrier())?;
        if variables.is_some() {
            endpoint.set_variables(variables);
            if endpoint
                .variables()
                .is_none()
            {
                return Err(OriginateError::VariablesNotSupported(endpoint.kind()));
            }
        }
        Ok(endpoint)
    }

    /// Refuse an expression at a carrier that never expands it.
    pub(crate) fn check_expanded(&self, carrier: DialStringCarrier) -> Result<(), OriginateError> {
        match self {
            Self::SofiaContact(_) | Self::GroupCall(_) if !carrier.expands() => {
                Err(OriginateError::UnexpandedExpression {
                    endpoint: self.kind(),
                })
            }
            _ => Ok(()),
        }
    }

    /// The module name this variant renders, for diagnostics.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Sofia(_) => "sofia",
            Self::SofiaGateway(_) => "sofia gateway",
            Self::Loopback(_) => "loopback",
            Self::User(_) => "user",
            Self::SofiaContact(_) => "sofia_contact",
            Self::GroupCall(_) => "group_call",
            Self::Error(_) => "error",
            Self::PortAudio(_) => "portaudio",
            Self::PulseAudio(_) => "pulseaudio",
            Self::Alsa(_) => "alsa",
        }
    }

    /// Module prefix to parser, first match wins.
    const PREFIX_PARSERS: &[(&str, PrefixParser)] = &[
        ("${sofia_contact(", |u| {
            Ok(Self::SofiaContact(SofiaContact::parse_bare(u)?))
        }),
        ("${group_call(", |u| {
            Ok(Self::GroupCall(GroupCall::parse_bare(u)?))
        }),
        ("error/", |u| Ok(Self::Error(u.parse()?))),
        ("loopback/", |u| {
            Ok(Self::Loopback(LoopbackEndpoint::parse_bare(u)?))
        }),
        // Must precede "sofia/", which also matches a gateway string.
        ("sofia/gateway/", |u| {
            Ok(Self::SofiaGateway(SofiaGateway::parse_bare(u)?))
        }),
        ("sofia/", |u| Ok(Self::Sofia(SofiaEndpoint::parse_bare(u)?))),
        ("user/", |u| Ok(Self::User(UserEndpoint::parse_bare(u)?))),
        ("portaudio", |u| {
            Ok(Self::PortAudio(AudioEndpoint::parse_bare(u, "portaudio")?))
        }),
        ("pulseaudio", |u| {
            Ok(Self::PulseAudio(AudioEndpoint::parse_bare(
                u,
                "pulseaudio",
            )?))
        }),
        ("alsa", |u| {
            Ok(Self::Alsa(AudioEndpoint::parse_bare(u, "alsa")?))
        }),
    ];

    /// The text after the variable block, as the endpoint module receives it.
    #[cfg(test)]
    pub(crate) fn module_text(&self) -> String {
        match self {
            Self::Sofia(ep) => ep.module_text(),
            Self::SofiaGateway(ep) => ep.module_text(),
            Self::Loopback(ep) => ep.module_text(),
            Self::User(ep) => ep.module_text(),
            Self::SofiaContact(ep) => ep.module_text(),
            Self::GroupCall(ep) => ep.module_text(),
            Self::Error(ep) => ep.to_string(),
            Self::PortAudio(ep) => ep.module_text("portaudio"),
            Self::PulseAudio(ep) => ep.module_text("pulseaudio"),
            Self::Alsa(ep) => ep.module_text("alsa"),
        }
    }

    /// Dispatch on the module prefix of module text as the switch's leg splits leave it,
    /// refusing a field the module reads as something else.
    pub(crate) fn parse_bare(uri: &str) -> Result<Self, OriginateError> {
        let (_, parse) = Self::PREFIX_PARSERS
            .iter()
            .find(|(prefix, _)| uri.starts_with(prefix))
            .ok_or_else(|| OriginateError::UnknownEndpointType(uri.to_string()))?;
        parse(uri)
    }
}

/// Renders an [`Endpoint`] for one target. Returned by
/// [`Endpoint::display_for`].
#[derive(Debug, Clone, Copy)]
pub struct EndpointDisplay<'a> {
    endpoint: &'a Endpoint,
    target: DialStringTarget,
}

impl fmt::Display for EndpointDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.target
            .write_argument(f, |f, target| {
                self.endpoint
                    .write_for(f, target)
            })
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_for(f, DialStringCarrier::EslApi.into())
    }
}

// ---------------------------------------------------------------------------
// FromStr
// ---------------------------------------------------------------------------

impl FromStr for Endpoint {
    type Err = OriginateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_for(s, DialStringCarrier::EslApi)
    }
}

// ---------------------------------------------------------------------------
// DialString impls
// ---------------------------------------------------------------------------

impl DialString for ErrorEndpoint {
    fn variables(&self) -> Option<&Variables> {
        None
    }
    fn variables_mut(&mut self) -> Option<&mut Variables> {
        None
    }
    fn set_variables(&mut self, _vars: Option<Variables>) {}
}

impl DialString for Endpoint {
    fn variables(&self) -> Option<&Variables> {
        forward_to_variant!(self, variables)
    }
    fn variables_mut(&mut self) -> Option<&mut Variables> {
        forward_to_variant!(self, variables_mut)
    }
    fn set_variables(&mut self, vars: Option<Variables>) {
        forward_to_variant!(self, set_variables, vars)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "serde"))]
mod c_oracle;
