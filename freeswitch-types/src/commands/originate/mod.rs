//! Originate command builder with endpoint configuration, variable scoping,
//! and automatic quoting for socket application arguments.

use std::fmt::{self, Write as _};
use std::str::FromStr;
use std::time::Duration;

use super::variables::{BlockParse, DialStringCarrier, DialStringTarget};
use crate::switch_passes::api_argument::{
    originate_quote, split_line, write_escaped, STRIPPED_WHITESPACE,
};
use crate::switch_passes::inline_hunt::{
    check_inline_delimiter, render_inline, split_inline_prefix, DEFAULT_INLINE_DELIMITER,
};
use crate::switch_passes::originate_function::{parse_originate_target, Slots, UNDEF};

mod application;
mod error;
#[cfg(feature = "serde")]
mod serde_support;

pub use application::{Application, DialplanType, OriginateTarget, ParseDialplanTypeError};
pub use error::OriginateError;

pub use super::variables::{Variables, VariablesType};

/// The context FreeSWITCH itself falls back to, emitted when a later
/// positional argument forces the slot to be present.
pub(super) const DEFAULT_CONTEXT: &str = "default";

/// Reject a dialplan `target` cannot run under: `originate_function` hands the target to the
/// inline hunt only under `inline`, so an extension there and an action list elsewhere misfire.
fn check_dialplan_fits(
    target: &OriginateTarget,
    dialplan: Option<&DialplanType>,
) -> Result<(), OriginateError> {
    let inline = dialplan.is_some_and(DialplanType::is_inline);
    match target {
        OriginateTarget::Extension(_) if inline => Err(OriginateError::ExtensionWithInlineDialplan),
        OriginateTarget::InlineApplications(_) if dialplan.is_some() && !inline => {
            Err(OriginateError::InlineApplicationsWithDialplan)
        }
        _ => Ok(()),
    }
}

pub use super::endpoint::Endpoint;

/// Originate command builder: `originate <endpoint> <target> [dialplan] [context] [cid_name] [cid_num] [timeout]`.
///
/// Constructed via [`Originate::extension`], [`Originate::application`], or
/// [`Originate::inline`]. Invalid states (Extension + Inline dialplan, inline
/// apps under another dialplan, empty inline apps) are rejected at construction
/// time rather than at `Display`.
///
/// Optional fields are set via consuming-self chaining methods:
///
/// ```
/// # use std::time::Duration;
/// # use freeswitch_types::commands::*;
/// let cmd = Originate::application(
///     Endpoint::Loopback(LoopbackEndpoint::new("9196").unwrap().with_context("default").unwrap()),
///     Application::simple("park"),
/// )
/// .cid_name("Alice")
/// .cid_num("5551234")
/// .timeout(Duration::from_secs(30));
/// ```
///
/// [`with_argv_separator`](Self::with_argv_separator) splits the arguments on a separator
/// instead of spaces. An `Originate` dials one [`Endpoint`], so a multi-leg list read into a
/// [`FlattenedDialString`](super::FlattenedDialString) goes out on the caller's own line
/// through its [`display_raw`](super::FlattenedDialString::display_raw).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Originate {
    endpoint: Endpoint,
    target: OriginateTarget,
    dialplan: Option<DialplanType>,
    context: Option<String>,
    cid_name: Option<String>,
    cid_num: Option<String>,
    timeout: Option<Duration>,
    /// `None` renders the conventional comma-separated list; `Some` prefixes
    /// the target with `m:<delim>:` so the hunt splits on that instead.
    inline_delimiter: Option<char>,
    /// `Some` opens the line with `^^<sep>` and splits every argument on it.
    argv_separator: Option<char>,
}

impl Originate {
    /// Route through the dialplan engine to an extension.
    ///
    /// `undef` in any case aborts the switch, which asserts the target it reads as absent is
    /// set; parse and config load refuse it, this does not.
    pub fn extension(endpoint: Endpoint, extension: impl Into<String>) -> Self {
        Self {
            endpoint,
            target: OriginateTarget::Extension(extension.into()),
            dialplan: None,
            context: None,
            cid_name: None,
            cid_num: None,
            timeout: None,
            inline_delimiter: None,
            argv_separator: None,
        }
    }

    /// Execute a single XML-format application on the answered channel.
    ///
    /// `originate` ends the arguments at the first `)`, so an argument carrying one, such as a
    /// `tone_stream://%(500,0,800)` spec, goes through [`inline`](Self::inline).
    pub fn application(endpoint: Endpoint, app: Application) -> Self {
        Self {
            endpoint,
            target: OriginateTarget::Application(app),
            dialplan: None,
            context: None,
            cid_name: None,
            cid_num: None,
            timeout: None,
            inline_delimiter: None,
            argv_separator: None,
        }
    }

    /// Execute inline applications on the answered channel.
    ///
    /// FreeSWITCH's inline dialplan splits the action list on `,`, so an
    /// argument carrying one — a `tone_stream://%(500,0,800)` spec, a
    /// `{a=1,b=2}` block on a bridge target — would be read as an action
    /// boundary and silently become applications nobody wrote. Rendering
    /// escapes every occurrence, which the switch undoes when it splits.
    ///
    /// Escaping happens at render time, over the arguments as they stand then,
    /// so rewriting one afterwards — through
    /// [`args_mut`](Application::args_mut), through
    /// [`target_mut`](Self::target_mut), or by substituting into a template —
    /// cannot leave the command inconsistent.
    ///
    /// Each action is escaped for the hunt's split and again for the line's, so quotes,
    /// backslashes and edge spaces in an argument arrive as written.
    ///
    /// Returns `Err` if the iterator yields no applications.
    pub fn inline(
        endpoint: Endpoint,
        apps: impl IntoIterator<Item = Application>,
    ) -> Result<Self, OriginateError> {
        let apps: Vec<Application> = apps
            .into_iter()
            .collect();
        if apps.is_empty() {
            return Err(OriginateError::EmptyInlineApplications);
        }
        Ok(Self {
            endpoint,
            target: OriginateTarget::InlineApplications(apps),
            dialplan: None,
            context: None,
            cid_name: None,
            cid_num: None,
            timeout: None,
            inline_delimiter: None,
            argv_separator: None,
        })
    }

    /// Execute inline applications, naming the separator yourself.
    ///
    /// Prefer [`Originate::inline`], which uses the conventional comma. Reach
    /// for this when the exact wire form matters — matching a command a switch
    /// already logged, or keeping the form stable across a config that gets
    /// diffed. Occurrences inside an argument are escaped either way, so the
    /// choice never has to depend on the data.
    ///
    /// Returns `Err` if the iterator yields no applications, or if `delimiter`
    /// cannot separate a list: `:`, where the hunt splits an application from its
    /// data, and what [`DialStringTarget::with_argv_separator`] refuses for the
    /// switch's reasons, since the hunt's split is the same one: space, controls,
    /// non-ASCII, `\`, `'` and lowercase `n r t s`. Parse refuses the same.
    pub fn inline_with_delimiter(
        endpoint: Endpoint,
        apps: impl IntoIterator<Item = Application>,
        delimiter: char,
    ) -> Result<Self, OriginateError> {
        let apps: Vec<Application> = apps
            .into_iter()
            .collect();
        if apps.is_empty() {
            return Err(OriginateError::EmptyInlineApplications);
        }
        check_inline_delimiter(delimiter)?;
        Ok(Self {
            endpoint,
            target: OriginateTarget::InlineApplications(apps),
            dialplan: None,
            context: None,
            cid_name: None,
            cid_num: None,
            timeout: None,
            // The comma needs no prefix, so asking for it is asking for the default.
            inline_delimiter: (delimiter != ',').then_some(delimiter),
            argv_separator: None,
        })
    }

    /// The separator this command renders its inline action list with, or
    /// `None` for the conventional comma.
    pub fn inline_delimiter(&self) -> Option<char> {
        self.inline_delimiter
    }

    /// Open the line with `^^<sep>`, so `originate` splits its arguments on `sep` rather
    /// than on spaces.
    ///
    /// Each argument is escaped once for that split, so a caller id, an application argument
    /// or a variable value carries spaces and quotes without quoting. An absent slot a later
    /// one forces reads `undef`; an empty value stays an empty argument, and the switch reads
    /// an empty context as the leg's own rather than `default`.
    ///
    /// Returns `Err` for a separator [`DialStringTarget::with_argv_separator`] refuses.
    ///
    /// ```
    /// # use freeswitch_types::commands::*;
    /// # use freeswitch_types::FreeswitchVersion;
    /// let block_parse = BlockParse::for_version(&"1.10.12".parse::<FreeswitchVersion>()?)?;
    /// let cmd = Originate::application(
    ///     Endpoint::Loopback(LoopbackEndpoint::new("9199")?.with_context("default")?),
    ///     Application::new("socket", Some("127.0.0.1:8040 async full")),
    /// )
    /// .cid_name("Front Desk")
    /// .with_argv_separator('~')?;
    ///
    /// assert_eq!(
    ///     cmd.display_with(block_parse).to_string(),
    ///     "originate ^^~loopback/9199/default~&socket(127.0.0.1:8040 async full)~undef~undef~Front Desk"
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_argv_separator(mut self, sep: char) -> Result<Self, OriginateError> {
        Self::argv_target(BlockParse::default(), sep)?;
        self.argv_separator = Some(sep);
        Ok(self)
    }

    /// The separator `originate`'s arguments are split on, or `None` for spaces.
    pub fn argv_separator(&self) -> Option<char> {
        self.argv_separator
    }

    /// Set the dialplan.
    ///
    /// Returns `Err` if setting `inline` on an `Extension` target, or anything but `inline` on
    /// inline applications, which `originate` would transfer as an extension.
    pub fn dialplan(mut self, dp: DialplanType) -> Result<Self, OriginateError> {
        check_dialplan_fits(&self.target, Some(&dp))?;
        self.dialplan = Some(dp);
        Ok(self)
    }

    /// Set the dialplan context. `undef` in any case reads as absent on the switch.
    pub fn context(mut self, ctx: impl Into<String>) -> Self {
        self.context = Some(ctx.into());
        self
    }

    /// Set the caller ID name. `undef` in any case reads as absent on the switch.
    pub fn cid_name(mut self, name: impl Into<String>) -> Self {
        self.cid_name = Some(name.into());
        self
    }

    /// Set the caller ID number. `undef` in any case reads as absent on the switch.
    pub fn cid_num(mut self, num: impl Into<String>) -> Self {
        self.cid_num = Some(num.into());
        self
    }

    /// Set the originate timeout. Sub-second precision is truncated to whole
    /// seconds on the wire and in serde round-trips.
    pub fn timeout(mut self, duration: Duration) -> Self {
        self.timeout = Some(duration);
        self
    }

    /// The dial endpoint.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Mutable reference to the dial endpoint.
    pub fn endpoint_mut(&mut self) -> &mut Endpoint {
        &mut self.endpoint
    }

    /// The originate target (extension, application, or inline apps).
    pub fn target(&self) -> &OriginateTarget {
        &self.target
    }

    /// Mutable reference to the originate target.
    pub fn target_mut(&mut self) -> &mut OriginateTarget {
        &mut self.target
    }

    /// The dialplan, if explicitly set.
    pub fn dialplan_type(&self) -> Option<&DialplanType> {
        self.dialplan
            .as_ref()
    }

    /// The dialplan context, if set.
    pub fn context_str(&self) -> Option<&str> {
        self.context
            .as_deref()
    }

    /// The caller ID name, if set.
    pub fn caller_id_name(&self) -> Option<&str> {
        self.cid_name
            .as_deref()
    }

    /// The caller ID number, if set.
    pub fn caller_id_number(&self) -> Option<&str> {
        self.cid_num
            .as_deref()
    }

    /// The timeout as a `Duration`, if set.
    pub fn timeout_duration(&self) -> Option<Duration> {
        self.timeout
    }

    /// The timeout in whole seconds, if set.
    pub fn timeout_seconds(&self) -> Option<u64> {
        self.timeout
            .map(|d| d.as_secs())
    }

    /// Override the dialplan type after construction.
    pub fn set_dialplan(&mut self, dp: Option<DialplanType>) {
        self.dialplan = dp;
    }

    /// Override the dialplan context after construction. `undef` in any case reads as absent
    /// on the switch.
    pub fn set_context(&mut self, ctx: Option<impl Into<String>>) {
        self.context = ctx.map(|c| c.into());
    }

    /// Override the caller ID name after construction. `undef` in any case reads as absent
    /// on the switch.
    pub fn set_cid_name(&mut self, name: Option<impl Into<String>>) {
        self.cid_name = name.map(|n| n.into());
    }

    /// Override the caller ID number after construction. `undef` in any case reads as absent
    /// on the switch.
    pub fn set_cid_num(&mut self, num: Option<impl Into<String>>) {
        self.cid_num = num.map(|n| n.into());
    }

    /// Override the timeout after construction.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }

    /// Dialplan, context, cid_name, cid_num and timeout through the last one set. FreeSWITCH
    /// reads them by position, so an absent slot before that is `None` rather than skipped.
    fn positional_tail(&self) -> Vec<Option<String>> {
        let dialplan = match (&self.target, &self.dialplan) {
            (OriginateTarget::InlineApplications(_), None) => Some(&DialplanType::INLINE),
            (_, dialplan) => dialplan.as_ref(),
        };
        let slots = [
            dialplan.map(DialplanType::to_string),
            self.context
                .clone(),
            self.cid_name
                .clone(),
            self.cid_num
                .clone(),
            self.timeout
                .map(|t| {
                    t.as_secs()
                        .to_string()
                }),
        ];
        let present = slots
            .iter()
            .rposition(Option::is_some)
            .map_or(0, |last| last + 1);
        slots
            .into_iter()
            .take(present)
            .collect()
    }

    /// The target and positionals after the endpoint, an absent slot a later one forces written
    /// `undef`. On blanks each goes through [`originate_quote`]; on a separator each is escaped
    /// once.
    fn write_arguments(&self, f: &mut fmt::Formatter<'_>, target: &str) -> fmt::Result {
        let tail = self.positional_tail();
        let arguments = std::iter::once(Some(target)).chain(
            tail.iter()
                .map(Option::as_deref),
        );
        for (at, argument) in arguments.enumerate() {
            match (self.argv_separator, argument.unwrap_or(UNDEF)) {
                (None, text) => write!(f, " {}", originate_quote(text))?,
                // A trailing separator adds no argument; quotes keep a final empty one.
                (Some(sep), "") if at == tail.len() => write!(f, "{sep}''")?,
                (Some(sep), text) => {
                    f.write_char(sep)?;
                    write_escaped(&mut *f, sep, text)?;
                }
            }
        }
        Ok(())
    }
}

/// Renders an [`Originate`] for one parser revision. Returned by
/// [`Originate::display_with`].
#[derive(Debug, Clone, Copy)]
pub struct OriginateDisplay<'a> {
    originate: &'a Originate,
    block_parse: BlockParse,
}

impl fmt::Display for OriginateDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.originate
            .write_with(f, self.block_parse)
    }
}

impl fmt::Display for Originate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_with(f, BlockParse::default())
    }
}

impl FromStr for Originate {
    type Err = OriginateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_with(s, BlockParse::default())
    }
}

impl Originate {
    /// Render for a switch running `block_parse`, rather than the default
    /// revision [`Display`](fmt::Display) uses.
    pub fn display_with(&self, block_parse: BlockParse) -> OriginateDisplay<'_> {
        OriginateDisplay {
            originate: self,
            block_parse,
        }
    }

    /// The target argument before the split's quoting or escaping.
    fn target_text(&self) -> String {
        match &self.target {
            OriginateTarget::Extension(ext) => ext.clone(),
            OriginateTarget::Application(app) => app.to_string_with_dialplan(&DialplanType::XML),
            OriginateTarget::InlineApplications(apps) => match self.inline_delimiter {
                Some(delimiter) => format!("m:{delimiter}:{}", render_inline(apps, delimiter)),
                None => render_inline(apps, DEFAULT_INLINE_DELIMITER),
            },
        }
    }

    fn write_with(&self, f: &mut fmt::Formatter<'_>, block_parse: BlockParse) -> fmt::Result {
        let dial_target = match self.argv_separator {
            // The field holds only separators `with_argv_separator` accepted.
            Some(sep) => {
                f.write_str("originate ^^")?;
                f.write_char(sep)?;
                Self::argv_target(block_parse, sep).map_err(|_| fmt::Error)?
            }
            None => {
                f.write_str("originate ")?;
                Self::dial_target(block_parse)
            }
        };
        write!(
            f,
            "{}",
            self.endpoint
                .display_for(dial_target)
        )?;
        self.write_arguments(f, &self.target_text())
    }

    fn dial_target(block_parse: BlockParse) -> DialStringTarget {
        DialStringTarget::new(DialStringCarrier::EslApi).with_block_parse(block_parse)
    }

    fn argv_target(block_parse: BlockParse, sep: char) -> Result<DialStringTarget, OriginateError> {
        Self::dial_target(block_parse)
            .with_argv_separator(sep)
            .map_err(OriginateError::InvalidArgvSeparator)
    }

    /// Parse an originate written for a switch running `block_parse`, mirroring
    /// [`display_with`](Self::display_with).
    pub fn parse_with(s: &str, block_parse: BlockParse) -> Result<Self, OriginateError> {
        let s = s
            .strip_prefix("originate")
            .unwrap_or(s)
            .trim_matches(STRIPPED_WHITESPACE);
        let line = split_line(s, ' ')?;
        // `^^ ` names the blank split itself.
        let sep = (line.delimiter != ' ').then_some(line.delimiter);
        let dial_target = match sep {
            Some(sep) => Self::argv_target(block_parse, sep)?,
            None => Self::dial_target(block_parse),
        };
        let mut arguments = line
            .arguments
            .into_iter();

        let endpoint_argument = arguments
            .next()
            .ok_or_else(|| OriginateError::ParseError("empty originate".into()))?;
        let endpoint = Endpoint::parse_for(&s[endpoint_argument.raw], dial_target)?;

        let Slots {
            target: target_str,
            dialplan,
            context,
            cid_name,
            cid_num,
            timeout,
        } = Slots::read(arguments.map(|argument| argument.text))?;

        let target = parse_originate_target(&target_str, dialplan.as_ref())?;

        let timeout = match timeout {
            None => None,
            Some(value) => match value.parse::<u64>() {
                Ok(secs) => Some(Duration::from_secs(secs)),
                Err(source) => return Err(OriginateError::InvalidTimeout { value, source }),
            },
        };

        // Validate via constructors then set parsed fields directly (same module)
        let mut orig = match target {
            OriginateTarget::Extension(ref ext) => Self::extension(endpoint, ext.clone()),
            OriginateTarget::Application(ref app) => Self::application(endpoint, app.clone()),
            OriginateTarget::InlineApplications(ref apps) => Self::inline(endpoint, apps.clone())?,
        };
        // Keep what was on the wire rather than what the constructor would have
        // chosen, so a parsed command renders back byte for byte.
        if matches!(orig.target, OriginateTarget::InlineApplications(_)) {
            orig.inline_delimiter = split_inline_prefix(&target_str).0;
        }
        orig.dialplan = dialplan;
        orig.context = context;
        orig.cid_name = cid_name;
        orig.cid_num = cid_num;
        orig.timeout = timeout;
        orig.argv_separator = sep;
        Ok(orig)
    }
}

#[cfg(test)]
mod tests;
