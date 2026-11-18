//! The originate command's target and dialplan: the application or extension it runs, and the
//! dialplan module a transfer names.
//!
//! [`DialplanType`] keeps its representation private so a known name always resolves to its
//! typed value: `Originate`'s dialplan check and equality compare the typed `inline`, and a
//! public variant holding a name would let `"inline"` or `"XML"` past both.

use std::fmt;
use std::str::FromStr;

use crate::switch_passes::originate_function::UNDEF;

/// The dialplan module a transfer looks up: `inline`, `XML`, or any other name.
///
/// `originate_function` hands whatever word sits in the dialplan slot to the transfer, and a
/// name no module registers hangs the channel up. Names compare ignoring ASCII case, as the
/// switch looks a dialplan module up.
#[derive(Debug, Clone)]
pub struct DialplanType(Repr);

#[derive(Debug, Clone)]
enum Repr {
    Inline,
    Xml,
    Named(String),
}

impl DialplanType {
    /// Inline dialplan: applications execute directly without XML lookup.
    pub const INLINE: Self = Self(Repr::Inline);
    /// XML dialplan: route through the XML dialplan engine.
    pub const XML: Self = Self(Repr::Xml);

    /// The dialplan module registered under `name`. `inline` and `xml` in any case resolve to
    /// [`INLINE`](Self::INLINE) and [`XML`](Self::XML).
    ///
    /// Refuses `undef` in any case, which `originate_function` reads as an absent argument.
    pub fn named(name: impl Into<String>) -> Result<Self, ParseDialplanTypeError> {
        let name = name.into();
        if name.eq_ignore_ascii_case(UNDEF) {
            return Err(ParseDialplanTypeError(name));
        }
        Ok(Self::from_name(name))
    }

    /// `name` resolved to a known dialplan where it reads as one. The caller has already
    /// read `undef` as absent.
    pub(crate) fn from_name(name: String) -> Self {
        if name.eq_ignore_ascii_case("inline") {
            Self::INLINE
        } else if name.eq_ignore_ascii_case("xml") {
            Self::XML
        } else {
            Self(Repr::Named(name))
        }
    }

    /// The name as the switch receives it.
    pub fn name(&self) -> &str {
        match &self.0 {
            Repr::Inline => "inline",
            Repr::Xml => "XML",
            Repr::Named(name) => name,
        }
    }

    pub(crate) fn is_inline(&self) -> bool {
        matches!(self.0, Repr::Inline)
    }
}

impl PartialEq for DialplanType {
    fn eq(&self, other: &Self) -> bool {
        self.name()
            .eq_ignore_ascii_case(other.name())
    }
}

impl Eq for DialplanType {}

impl std::hash::Hash for DialplanType {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        for b in self
            .name()
            .bytes()
        {
            state.write_u8(b.to_ascii_lowercase());
        }
    }
}

impl fmt::Display for DialplanType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A dialplan name the switch reads as an absent argument: `undef` in any case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseDialplanTypeError(pub String);

impl fmt::Display for ParseDialplanTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("dialplan named undef, which the switch reads as absent")
    }
}

impl std::error::Error for ParseDialplanTypeError {}

impl FromStr for DialplanType {
    type Err = ParseDialplanTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::named(s)
    }
}

/// Serialized as the name, but the known two in lowercase (`xml`), the spelling configs use.
#[cfg(feature = "serde")]
impl serde::Serialize for DialplanType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.0 {
            Repr::Inline => serializer.serialize_str("inline"),
            Repr::Xml => serializer.serialize_str("xml"),
            Repr::Named(name) => serializer.serialize_str(name),
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for DialplanType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::named(name).map_err(serde::de::Error::custom)
    }
}

/// A single dialplan application with optional arguments.
///
/// Formats differently depending on [`DialplanType`]:
/// - Inline: `name` or `name:args`
/// - XML: `&name(args)`
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Application {
    name: String,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    args: Option<String>,
}

impl Application {
    /// Create an application with optional arguments.
    pub fn new(name: impl Into<String>, args: Option<impl Into<String>>) -> Self {
        Self {
            name: name.into(),
            args: args.map(|a| a.into()),
        }
    }

    /// Create an application with no arguments.
    pub fn simple(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args: None,
        }
    }

    /// Park the channel (hold in place without bridging).
    pub fn park() -> Self {
        Self::simple("park")
    }

    /// Application name (e.g. `park`, `conference`, `socket`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Application arguments, if any.
    pub fn args(&self) -> Option<&str> {
        self.args
            .as_deref()
    }

    /// Format as inline (`name:args`) syntax under the inline dialplan, else as an application
    /// target (`&name(args)`), which `originate` runs whatever the dialplan.
    pub(crate) fn to_string_with_dialplan(&self, dialplan: &DialplanType) -> String {
        if dialplan.is_inline() {
            return match &self.args {
                Some(args) => format!("{}:{}", self.name, args),
                None => self
                    .name
                    .clone(),
            };
        }
        let args = self
            .args
            .as_deref()
            .unwrap_or("");
        format!("&{}({})", self.name, args)
    }
}

/// The target of an originate command: either a dialplan extension or
/// application(s) to execute directly.
///
/// FreeSWITCH syntax: `originate <endpoint> <target> [dialplan] ...`
/// where `<target>` is either a bare extension string (routes through
/// the dialplan engine) or `&app(args)` / `app:args` (executes inline).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum OriginateTarget {
    /// Route through the dialplan engine to this extension.
    Extension(String),
    /// Single application for XML dialplan: `&app(args)`.
    Application(Application),
    /// One or more applications for inline dialplan: `app:args,app:args`.
    InlineApplications(Vec<Application>),
}

impl From<Application> for OriginateTarget {
    fn from(app: Application) -> Self {
        Self::Application(app)
    }
}

impl From<Vec<Application>> for OriginateTarget {
    fn from(apps: Vec<Application>) -> Self {
        Self::InlineApplications(apps)
    }
}
