//! Where a dial string is headed: the command carrying it, the block-parser revision reading it,
//! and the split cutting that command's arguments.
//!
//! Line numbers in this module index FreeSWITCH `v1.11.1`
//! (`c2c59645f6911a76589e5008c4d73349ded44b65`).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use crate::commands::originate::OriginateError;
use crate::switch_passes::api_argument::{
    escape_argument, sole_argument, usable_argv_separator, write_escaped, ArgumentPass, ArgvCut,
};
use crate::switch_passes::separate::Token;
use crate::switch_passes::{trace, untrace, Traced};
use crate::version::FreeswitchVersion;

/// Which command carries a dial string, and therefore how deeply its variable
/// values must be escaped.
///
/// FreeSWITCH escape-processes a bracket block a different number of times
/// depending on the command it arrived on, and the correct escaping of a value
/// containing a single quote differs between them with no form that satisfies
/// all. Naming the carrier is the only way a renderer can be right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum DialStringCarrier {
    /// `api originate` / `bgapi originate`, whose argument list is split before
    /// the block itself is parsed. The default for [`Display`](fmt::Display).
    EslApi,
    /// A dialplan application such as `bridge`, including via `sendmsg execute`,
    /// which receives its argument whole.
    Dialplan,
    /// Handed straight to `switch_ivr_originate`, as a script binding's originate does
    /// (`freeswitch.Session(dest)` and `session:originate` through `CoreSession`): no argument
    /// split, no expansion.
    Direct,
}

impl DialStringCarrier {
    /// Dialplan variable expansion deletes an escaped quote outright, where every
    /// other pass unescapes it, and drops the first `$` of a `$$`.
    pub(crate) fn expands(self) -> bool {
        matches!(self, Self::Dialplan)
    }
}

/// Which revision of the switch's bracket-block parser a dial string is rendered
/// for.
///
/// How many escape-consuming passes a block meets is the switch's to change, and
/// every escaped quote and backslash depends on it. It covers `{}`, `<>` and `[]`
/// escaping only: the inline action list and the quote pre-scan ahead of a `[]`
/// block are other parsers. Which revisions a FreeSWITCH version is known to run
/// is documented in `docs/dial-string-format.md`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum BlockParse {
    /// Both of the block's own splits, on the separator and on `=`, run the
    /// escape-consuming cleanup.
    #[default]
    PairSplitCleans,
}

impl BlockParse {
    const ALL: &'static [Self] = &[Self::PairSplitCleans];

    fn as_str(self) -> &'static str {
        match self {
            Self::PairSplitCleans => "pair_split_cleans",
        }
    }

    pub(crate) fn cleanup_passes(self) -> u32 {
        match self {
            Self::PairSplitCleans => 2,
        }
    }

    /// The revision a FreeSWITCH release is vouched to run, or why it cannot be.
    ///
    /// A release is vouched for only inside a range whose block parse
    /// (`switch_event.c:1737`, `switch_utils.c:2804`) was checked at every tag and
    /// whose line passed the live escaping suite. Anything else, a development
    /// build included, is refused; name a revision explicitly for it instead.
    pub fn for_version(version: &FreeswitchVersion) -> Result<Self, UnvouchedVersion> {
        let version = *version;
        let (first, last) = VOUCHED_PAIR_SPLIT_CLEANS;
        if version.is_dev() {
            Err(UnvouchedVersion::Dev { version })
        } else if version < first {
            Err(UnvouchedVersion::OlderThanVouched { version })
        } else if version > last {
            Err(UnvouchedVersion::NewerThanVouched { version })
        } else {
            Ok(Self::PairSplitCleans)
        }
    }
}

const VOUCHED_PAIR_SPLIT_CLEANS: (FreeswitchVersion, FreeswitchVersion) = (
    FreeswitchVersion::new(1, 10, 0),
    FreeswitchVersion::new(1, 11, 3),
);

/// A FreeSWITCH version [`BlockParse::for_version`] cannot vouch for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnvouchedVersion {
    /// A development build, whose version stays the same across commits.
    Dev {
        /// The version refused.
        version: FreeswitchVersion,
    },
    /// A release newer than the vouched range.
    NewerThanVouched {
        /// The version refused.
        version: FreeswitchVersion,
    },
    /// A release older than the vouched range.
    OlderThanVouched {
        /// The version refused.
        version: FreeswitchVersion,
    },
}

impl UnvouchedVersion {
    /// The version refused.
    pub fn version(&self) -> FreeswitchVersion {
        match *self {
            Self::Dev { version }
            | Self::NewerThanVouched { version }
            | Self::OlderThanVouched { version } => version,
        }
    }
}

impl fmt::Display for UnvouchedVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (first, last) = VOUCHED_PAIR_SPLIT_CLEANS;
        let why = match self {
            Self::Dev { .. } => "is a development build",
            Self::NewerThanVouched { .. } => "is newer than any vouched release",
            Self::OlderThanVouched { .. } => "is older than any vouched release",
        };
        write!(
            f,
            "FreeSWITCH {} {why}; releases {first} to {last} are vouched for, name a BlockParse explicitly",
            self.version()
        )
    }
}

impl std::error::Error for UnvouchedVersion {}

impl fmt::Display for BlockParse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for BlockParse {
    type Err = ParseBlockParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|revision| {
                revision
                    .as_str()
                    .eq_ignore_ascii_case(s)
            })
            .ok_or_else(|| ParseBlockParseError(s.to_string()))
    }
}

/// A [`BlockParse`] name this crate does not know. The rejected name is the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseBlockParseError(pub String);

impl fmt::Display for ParseBlockParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unknown block-parse revision, expected one of:")?;
        for revision in BlockParse::ALL {
            write!(f, " {revision}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ParseBlockParseError {}

/// Where a dial string is headed: the command carrying it, the block-parser
/// revision of the switch reading it, and the split cutting that command's arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialStringTarget {
    carrier: DialStringCarrier,
    block_parse: BlockParse,
    argument: ArgumentPass,
}

/// Why a [`DialStringTarget`] takes no argument separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InvalidArgvSeparator {
    /// The carrier hands the dial string over whole, so no argument split reads it.
    WrongCarrier(DialStringCarrier),
    /// A separator [`DialStringTarget::with_argv_separator`] refuses.
    Unusable(char),
}

impl fmt::Display for InvalidArgvSeparator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongCarrier(carrier) => {
                write!(f, "the {carrier:?} carrier takes no argument separator")
            }
            Self::Unusable(sep) => write!(f, "{sep:?} cannot separate originate's arguments"),
        }
    }
}

impl std::error::Error for InvalidArgvSeparator {}

impl DialStringTarget {
    /// Target `carrier` at the default [`BlockParse`].
    pub fn new(carrier: DialStringCarrier) -> Self {
        Self {
            carrier,
            block_parse: BlockParse::default(),
            argument: ArgumentPass::Blank,
        }
    }

    /// Split `originate`'s arguments on `sep`, as a line opening with `^^<sep>` asks the switch.
    ///
    /// The dial string is one argument of that split: rendered, it is escaped once for the
    /// split's cleanup; parsed, that cleanup runs first and a second argument is refused.
    ///
    /// Refused: separators the switch splits or unescapes wrongly (space, `\`, `'`, lowercase
    /// `n r t s`, controls, non-ASCII) and, as policy, `^ " , | [ ] { } < > = :`, which read as
    /// dial-string grammar or quoting, and every other letter or digit, which reads as part of a word.
    /// A `'` separator pairs with the next one as quotes.
    pub fn with_argv_separator(mut self, sep: char) -> Result<Self, InvalidArgvSeparator> {
        match self.carrier {
            DialStringCarrier::EslApi => {}
            DialStringCarrier::Dialplan | DialStringCarrier::Direct => {
                return Err(InvalidArgvSeparator::WrongCarrier(self.carrier))
            }
        }
        if !usable_argv_separator(sep) {
            return Err(InvalidArgvSeparator::Unusable(sep));
        }
        self.argument = ArgumentPass::Char(sep);
        Ok(self)
    }

    /// The separator cutting `originate`'s arguments in place of the blank split.
    pub fn argv_separator(&self) -> Option<char> {
        match self.argument {
            ArgumentPass::Char(sep) => Some(sep),
            ArgumentPass::Blank | ArgumentPass::Consumed => None,
        }
    }

    /// `text` escaped as one argument of `originate`'s split, on blanks or on the
    /// [`argv_separator`](Self::argv_separator), or `None` at a carrier that splits none.
    ///
    /// An empty text is written `''`, which the split keeps as an argument. On blanks a text
    /// opening `^^` is written after `''`, since a line opening `^^` names its own separator.
    pub fn escape_argument<'a>(&self, text: &'a str) -> Option<Cow<'a, str>> {
        self.split_delimiter()
            .and_then(|sep| escape_argument(text, sep))
    }

    /// `render` at this target, escaped once at its edge for an argv separator's split.
    pub(crate) fn write_argument(
        self,
        f: &mut fmt::Formatter<'_>,
        render: impl Fn(&mut fmt::Formatter<'_>, Self) -> fmt::Result,
    ) -> fmt::Result {
        match self.argv_separator() {
            Some(sep) => write_escaped(
                f,
                sep,
                RenderedAt {
                    target: self.inner(),
                    render: &render,
                },
            ),
            None => render(f, self),
        }
    }

    /// A separator [`with_argv_separator`](Self::with_argv_separator) refuses as policy, for
    /// reading captures the switch accepts under it.
    #[cfg(test)]
    pub(crate) fn with_unchecked_argv_separator(mut self, sep: char) -> Self {
        self.argument = ArgumentPass::Char(sep);
        self
    }

    /// The delimiter of `originate`'s argument split still ahead of this target, if any.
    fn split_delimiter(self) -> Option<char> {
        match (self.carrier, self.argument) {
            (DialStringCarrier::EslApi, ArgumentPass::Blank) => Some(' '),
            (_, ArgumentPass::Char(sep)) => Some(sep),
            (DialStringCarrier::Dialplan | DialStringCarrier::Direct, ArgumentPass::Blank)
            | (_, ArgumentPass::Consumed) => None,
        }
    }

    /// The one argument `originate`'s split leaves of `text`, or `None` where no split reads it.
    pub(crate) fn split_argument(self, text: &[Traced]) -> Option<Result<Option<Token>, ArgvCut>> {
        self.split_delimiter()
            .map(|delim| sole_argument(text, delim))
    }

    /// This target inside an argument its split already read.
    pub(crate) fn inner(mut self) -> Self {
        if self
            .split_delimiter()
            .is_some()
        {
            self.argument = ArgumentPass::Consumed;
        }
        self
    }

    /// What this target's argument split leaves of `s`, and the target reading that.
    pub(crate) fn read_argument(self, s: &str) -> Result<(Cow<'_, str>, Self), OriginateError> {
        let Some(token) = self.split_argument(&trace(s)) else {
            return Ok((Cow::Borrowed(s), self));
        };
        let token = token.map_err(|ArgvCut| {
            OriginateError::ParseError("originate's argument split cuts the dial string".into())
        })?;
        let argument = token.map_or_else(String::new, |token| untrace(&token.text));
        Ok((Cow::Owned(argument), self.inner()))
    }

    /// Target a switch running `block_parse`.
    pub fn with_block_parse(mut self, block_parse: BlockParse) -> Self {
        self.block_parse = block_parse;
        self
    }

    /// The command carrying the dial string.
    pub fn carrier(&self) -> DialStringCarrier {
        self.carrier
    }

    /// The block-parser revision rendered for.
    pub fn block_parse(&self) -> BlockParse {
        self.block_parse
    }

    /// The carrier's own pass still ahead of this target, the argument split or the dialplan
    /// expansion: one, or none once consumed and at the direct carrier.
    pub(crate) fn argument_passes(self) -> u32 {
        match (self.carrier, self.argument) {
            (DialStringCarrier::Direct, _) | (_, ArgumentPass::Consumed) => 0,
            (_, ArgumentPass::Blank | ArgumentPass::Char(_)) => 1,
        }
    }
}

struct RenderedAt<'r, R> {
    target: DialStringTarget,
    render: &'r R,
}

impl<R> fmt::Display for RenderedAt<'_, R>
where
    R: Fn(&mut fmt::Formatter<'_>, DialStringTarget) -> fmt::Result,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (self.render)(f, self.target)
    }
}

impl From<DialStringCarrier> for DialStringTarget {
    fn from(carrier: DialStringCarrier) -> Self {
        Self::new(carrier)
    }
}
