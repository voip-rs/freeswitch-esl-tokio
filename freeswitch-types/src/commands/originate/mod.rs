//! Originate command builder with endpoint configuration, variable scoping,
//! and automatic quoting for socket application arguments.

use std::fmt::{self, Write as _};
use std::time::Duration;

use super::variables::{BlockParse, DialStringCarrier, DialStringTarget};
use crate::switch_passes::api_argument::{
    originate_quote, split_line, write_escaped, STRIPPED_WHITESPACE,
};
use crate::switch_passes::inline_hunt::{
    check_inline_delimiter, render_inline, split_inline_prefix, DEFAULT_INLINE_DELIMITER,
};
use crate::switch_passes::originate_function::{
    check_target_readable, parse_originate_target, Slots, UNDEF,
};

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

pub use super::endpoint::Endpoint;

/// Originate command builder: `originate <endpoint> <target> [dialplan] [context] [cid_name] [cid_num] [timeout]`.
///
/// Constructed via [`Originate::extension`], [`Originate::application`], or
/// [`Originate::inline`]; each field is read by its own name and changed only through its
/// `with_*` or `without_*` builder.
///
/// ```
/// # use std::time::Duration;
/// # use freeswitch_types::commands::*;
/// let cmd = Originate::application(
///     Endpoint::Loopback(LoopbackEndpoint::new("9196")?.with_context("default")?),
///     Application::simple("park"),
/// )
/// .with_cid_name("Alice")
/// .with_cid_num("5551234")
/// .with_timeout(Duration::from_secs(30));
/// # Ok::<(), OriginateError>(())
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
    /// `endpoint` dialled into `target`, every optional slot unset, before any check.
    fn unchecked(endpoint: Endpoint, target: OriginateTarget) -> Self {
        Self {
            endpoint,
            target,
            dialplan: None,
            context: None,
            cid_name: None,
            cid_num: None,
            timeout: None,
            inline_delimiter: None,
            argv_separator: None,
        }
    }

    /// Route through the dialplan engine to an extension.
    ///
    /// `undef` in any case aborts the switch, which asserts the target it reads as absent is
    /// set; parse and config load refuse it, this does not.
    pub fn extension(endpoint: Endpoint, extension: impl Into<String>) -> Self {
        Self::unchecked(endpoint, OriginateTarget::Extension(extension.into()))
    }

    /// Execute a single XML-format application on the answered channel.
    ///
    /// `originate` ends the arguments at the first `)`, so an argument carrying one, such as a
    /// `tone_stream://%(500,0,800)` spec, goes through [`inline`](Self::inline).
    pub fn application(endpoint: Endpoint, app: Application) -> Self {
        Self::unchecked(endpoint, OriginateTarget::Application(app))
    }

    /// Execute inline applications on the answered channel.
    ///
    /// FreeSWITCH's inline dialplan splits the action list on `,`, so an
    /// argument carrying one — a `tone_stream://%(500,0,800)` spec, a
    /// `{a=1,b=2}` block on a bridge target — would be read as an action
    /// boundary and silently become applications nobody wrote. Rendering
    /// escapes every occurrence, which the switch undoes when it splits.
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
        Ok(Self::unchecked(
            endpoint,
            OriginateTarget::InlineApplications(apps),
        ))
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
            // The comma needs no prefix, so asking for it is asking for the default.
            inline_delimiter: (delimiter != ',').then_some(delimiter),
            ..Self::unchecked(endpoint, OriginateTarget::InlineApplications(apps))
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
    /// .with_cid_name("Front Desk")
    /// .with_argv_separator('~')?;
    ///
    /// assert_eq!(
    ///     cmd.render(block_parse),
    ///     "originate ^^~loopback/9199/default~&socket(127.0.0.1:8040 async full)~undef~undef~Front Desk"
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_argv_separator(mut self, sep: char) -> Result<Self, OriginateError> {
        // Only the separator is checked here, which no revision changes.
        Self::argv_target(BlockParse::MEASURED, sep)?;
        self.argv_separator = Some(sep);
        Ok(self)
    }

    /// The separator `originate`'s arguments are split on, or `None` for spaces.
    pub fn argv_separator(&self) -> Option<char> {
        self.argv_separator
    }

    /// Set the dialplan, refusing `inline` on an extension and anything but `inline` on inline
    /// applications, which `originate` would transfer as an extension.
    pub fn with_dialplan(mut self, dialplan: DialplanType) -> Result<Self, OriginateError> {
        self.dialplan = Some(dialplan);
        self.checked()
    }

    /// Leave the dialplan to the switch: `inline` for inline applications, else its default.
    pub fn without_dialplan(mut self) -> Self {
        self.dialplan = None;
        self
    }

    /// Set the dialplan context. `undef` in any case reads as absent on the switch.
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// Leave the context to the switch.
    pub fn without_context(mut self) -> Self {
        self.context = None;
        self
    }

    /// Set the caller ID name. `undef` in any case reads as absent on the switch.
    pub fn with_cid_name(mut self, cid_name: impl Into<String>) -> Self {
        self.cid_name = Some(cid_name.into());
        self
    }

    /// Leave the caller ID name to the switch.
    pub fn without_cid_name(mut self) -> Self {
        self.cid_name = None;
        self
    }

    /// Set the caller ID number. `undef` in any case reads as absent on the switch.
    pub fn with_cid_num(mut self, cid_num: impl Into<String>) -> Self {
        self.cid_num = Some(cid_num.into());
        self
    }

    /// Leave the caller ID number to the switch.
    pub fn without_cid_num(mut self) -> Self {
        self.cid_num = None;
        self
    }

    /// Set the originate timeout. Sub-second precision is truncated to whole
    /// seconds on the wire and in serde round-trips.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Leave the timeout to the switch.
    pub fn without_timeout(mut self) -> Self {
        self.timeout = None;
        self
    }

    /// The dial endpoint.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// The originate target (extension, application, or inline apps).
    pub fn target(&self) -> &OriginateTarget {
        &self.target
    }

    /// The dialplan, if explicitly set.
    pub fn dialplan(&self) -> Option<&DialplanType> {
        self.dialplan
            .as_ref()
    }

    /// The dialplan context, if set.
    pub fn context(&self) -> Option<&str> {
        self.context
            .as_deref()
    }

    /// The caller ID name, if set.
    pub fn cid_name(&self) -> Option<&str> {
        self.cid_name
            .as_deref()
    }

    /// The caller ID number, if set.
    pub fn cid_num(&self) -> Option<&str> {
        self.cid_num
            .as_deref()
    }

    /// The timeout, if set.
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// Refuse what `originate_function` reads as another command than this one. Parse,
    /// config load and the builders run it over the whole value.
    fn check_deliverable(&self) -> Result<(), OriginateError> {
        self.endpoint
            .check_expanded(DialStringCarrier::EslApi)?;
        let inline = self
            .dialplan
            .as_ref()
            .is_some_and(DialplanType::is_inline);
        match &self.target {
            OriginateTarget::Extension(_) if inline => {
                return Err(OriginateError::ExtensionWithInlineDialplan)
            }
            OriginateTarget::InlineApplications(_)
                if self
                    .dialplan
                    .is_some()
                    && !inline =>
            {
                return Err(OriginateError::InlineApplicationsWithDialplan)
            }
            OriginateTarget::InlineApplications(apps) if apps.is_empty() => {
                return Err(OriginateError::EmptyInlineApplications)
            }
            _ => {}
        }
        for (field, value) in [
            ("context", &self.context),
            ("cid_name", &self.cid_name),
            ("cid_num", &self.cid_num),
        ] {
            if value
                .as_deref()
                .is_some_and(|v| v.eq_ignore_ascii_case(UNDEF))
            {
                return Err(OriginateError::UndefPositional(field));
            }
        }
        if self
            .target_text()
            .eq_ignore_ascii_case(UNDEF)
        {
            return Err(OriginateError::UndefPositional("target"));
        }
        check_target_readable(&self.target)
    }

    /// This value, or what [`check_deliverable`](Self::check_deliverable) refuses in it.
    fn checked(self) -> Result<Self, OriginateError> {
        self.check_deliverable()?;
        Ok(self)
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

/// An [`Originate`] written for one parser revision.
struct Rendered<'a> {
    originate: &'a Originate,
    block_parse: BlockParse,
}

impl fmt::Display for Rendered<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.originate
            .write_with(f, self.block_parse)
    }
}

impl Originate {
    /// The `originate` line for a switch running `block_parse`.
    ///
    /// The carrier is the API command itself, and the argument separator is the one
    /// [`with_argv_separator`](Self::with_argv_separator) set, so the revision is all
    /// a caller names.
    pub fn render(&self, block_parse: BlockParse) -> String {
        Rendered {
            originate: self,
            block_parse,
        }
        .to_string()
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
        DialStringTarget::new(DialStringCarrier::EslApi, block_parse)
    }

    fn argv_target(block_parse: BlockParse, sep: char) -> Result<DialStringTarget, OriginateError> {
        Self::dial_target(block_parse)
            .with_argv_separator(sep)
            .map_err(OriginateError::InvalidArgvSeparator)
    }

    /// Parse an `originate` line written for a switch running `block_parse`, mirroring
    /// [`render`](Self::render); a `^^<sep>` opening the line names its argument separator.
    pub fn parse_for(s: &str, block_parse: BlockParse) -> Result<Self, OriginateError> {
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

        // Keep the delimiter that was on the wire, so a parsed command renders back byte for byte.
        let inline_delimiter = match target {
            OriginateTarget::InlineApplications(_) => split_inline_prefix(&target_str).0,
            _ => None,
        };
        Self {
            endpoint,
            target,
            dialplan,
            context,
            cid_name,
            cid_num,
            timeout,
            inline_delimiter,
            argv_separator: sep,
        }
        .checked()
    }
}

#[cfg(test)]
mod tests;
