//! Channel variable scope and ordered key-value storage for originate commands.

use indexmap::IndexMap;
use std::fmt;
use std::str::FromStr;

use super::originate::OriginateError;
use crate::switch_passes::brackets::{check_separator, same_header, unbalanced, Block, PairEffect};
use crate::switch_passes::escape::{escape_text, escape_value, EscapedField};
use crate::switch_passes::originate_legs::{splits_into_threads, ENTERPRISE_DELIM};
use crate::switch_passes::{pipeline, PipelineError};
use crate::variables::names_outgoing_header;
use crate::wire_safety::contains_header_control;

mod target;
#[cfg(test)]
mod tests;

pub use target::{
    BlockParse, DialStringCarrier, DialStringTarget, InvalidArgvSeparator, ParseBlockParseError,
    UnvouchedVersion,
};

/// Scope for channel variables in an originate command.
///
/// - `Enterprise` (`<>`) -- applies across all threads (`:_:` separated)
/// - `Default` (`{}`) -- applies to all channels in this originate
/// - `Channel` (`[]`) -- applies only to one specific channel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
#[non_exhaustive]
pub enum VariablesType {
    /// `<>` scope -- applies across all `:_:` separated threads.
    Enterprise,
    /// `{}` scope -- applies to all channels in this originate.
    Default,
    /// `[]` scope -- applies to one specific channel.
    Channel,
}

impl VariablesType {
    pub(crate) fn delimiters(self) -> (char, char) {
        match self {
            Self::Enterprise => ('<', '>'),
            Self::Default => ('{', '}'),
            Self::Channel => ('[', ']'),
        }
    }

    /// The scope of the block the switch parsed.
    pub(crate) fn of_block(block: &Block) -> Self {
        match block.open {
            '<' => Self::Enterprise,
            '{' => Self::Default,
            _ => Self::Channel,
        }
    }
}

/// Ordered set of channel variables with FreeSWITCH escaping.
///
/// A comma is escaped with `\,`, a backslash and a single quote with as many
/// backslashes as the [`DialStringTarget`]'s passes consume, a space at either
/// edge of a value as a `\s` escaped for the same passes, and a value with other
/// spaces is wrapped in single quotes. This form round-trips through [`FromStr`];
/// what the switch itself decodes depends on which command carries the block
/// and which parser revision reads it, documented in `docs/dial-string-format.md`.
///
/// A key meets the same passes and is escaped the same way, with `\=` for the `=` split and an
/// empty `''` ahead of one opening `^^`.
///
/// A value naming a variable (`${…}`) is left to the switch, which expands it or
/// drops it at install unless `origination_nested_vars` is true.
///
/// # Serde format
///
/// [`Default`](VariablesType::Default) scope with the comma separator
/// serializes as a flat JSON map: `{"key": "value", ...}`. Anything else
/// serializes as `{"scope": "enterprise", "vars": {"key": "value"}}`, carrying
/// a `"separator"` field only when [`with_separator`](Variables::with_separator)
/// chose one. Deserialization accepts both formats; a flat map implies
/// `Default` scope and the comma. A `separator` that cannot delimit the block,
/// or that a key or value already contains, is refused at load.
// qual:allow(srp, god_struct) reason: "public builder; accessors read disjoint fields"
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variables {
    vars_type: VariablesType,
    inner: IndexMap<String, String>,
    /// Set by [`with_separator`](Variables::with_separator). Parsing a `^^`
    /// block does not populate it: reading one back and writing it out
    /// canonicalises to a comma, and the other form is asked for explicitly.
    separator: Option<char>,
}

/// Reject a key no escaping carries: empty, `:_:`, `[`, a quote in channel scope or an unbalanced
/// bracket. The key is the offending text, so no error quotes it.
fn check_key(key: &str, vars_type: VariablesType) -> Result<(), OriginateError> {
    let fault = if key.is_empty() {
        Some("is empty, which the switch installs under no name".to_owned())
    } else if key.contains('[') {
        Some(
            "carries '[', which the switch reads as an array index, installing the value \
             under the text before it"
                .to_owned(),
        )
    } else if splits_into_threads(key) {
        Some(format!(
            "carries the enterprise separator {ENTERPRISE_DELIM}, on which the switch splits \
             the dial string into threads whatever quoting or escaping surrounds it"
        ))
    } else if vars_type == VariablesType::Channel && key.contains('\'') {
        Some(
            "carries a single quote in channel scope, which the switch pairs with the next \
             quote in the dial string before it parses the block"
                .to_owned(),
        )
    } else {
        unbalanced(key, vars_type.delimiters())
    };
    fault.map_or(Ok(()), |fault| {
        Err(OriginateError::ParseError(format!(
            "a variable name {fault}"
        )))
    })
}

/// Reject a pair no escaping carries: a key [`check_key`] refuses, or a value carrying `:_:`, a
/// quote in channel scope, nothing or an unbalanced bracket. Each error names why the switch loses it.
fn check_representable(
    key: &str,
    value: &str,
    vars_type: VariablesType,
) -> Result<(), OriginateError> {
    check_key(key, vars_type)?;
    if splits_into_threads(value) {
        return Err(OriginateError::ParseError(format!(
            "variable {key} carries the enterprise separator {ENTERPRISE_DELIM}, on which \
             the switch splits the dial string into threads whatever quoting or escaping \
             surrounds it"
        )));
    }
    if vars_type == VariablesType::Channel && value.contains('\'') {
        return Err(OriginateError::ParseError(format!(
            "variable {key} carries a single quote in channel scope: the switch \
             pairs it with the next quote in the dial string before it parses \
             the block, whatever escaping precedes either. Use default scope, \
             or keep the quote out of the value"
        )));
    }
    if names_outgoing_header(key) && contains_header_control(value) {
        return Err(OriginateError::ParseError(format!(
            "variable {key} carries a control character: the switch copies it verbatim \
             into an outgoing SIP header, whose field value cannot hold one"
        )));
    }
    if value.is_empty() {
        return Err(OriginateError::ParseError(format!(
            "variable {key} has an empty value: the switch discards such a pair \
             without logging it, so it cannot be told from an absent variable on \
             the wire. Give it a value or remove it -- and if its presence was \
             itself the signal, that signal needs a home outside the dial string"
        )));
    }

    unbalanced(value, vars_type.delimiters()).map_or(Ok(()), |fault| {
        Err(OriginateError::ParseError(format!(
            "variable {key} {fault}"
        )))
    })
}

/// Reject a key naming, but for ASCII case, one of `earlier`: the block's event replaces a header
/// by `strcasecmp`, so the two install as one variable. Position `at` names it, never its text.
fn check_case_collision<'a>(
    earlier: impl IntoIterator<Item = &'a String>,
    key: &str,
    at: usize,
) -> Result<(), OriginateError> {
    let collides = earlier
        .into_iter()
        .any(|name| name != key && same_header(name, key));
    match collides {
        true => Err(OriginateError::ParseError(format!(
            "variable {at} differs only in case from an earlier name, which the switch \
             installs as the same variable"
        ))),
        false => Ok(()),
    }
}

impl Variables {
    /// Create an empty variable set with the given scope.
    pub fn new(vars_type: VariablesType) -> Self {
        Self {
            vars_type,
            inner: IndexMap::new(),
            separator: None,
        }
    }

    /// Create from an existing set of key-value pairs.
    pub fn with_vars(
        vars_type: VariablesType,
        vars: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        Self {
            vars_type,
            inner: vars
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            separator: None,
        }
    }

    /// Separate the pairs with `sep` instead of a comma, emitting the block in
    /// FreeSWITCH's `^^<sep>` form.
    ///
    /// A value expanded from `${...}` in a dialplan is substituted before the
    /// block is parsed, so no escaping can be inserted into it; choosing a
    /// separator none of the values contain is the only way such a value can
    /// carry a comma. The separator is given rather than derived, so a block
    /// renders the same way whatever its values happen to be that call.
    ///
    /// Fails if `sep` cannot delimit this block, or if a key or value already
    /// present contains it. A pair inserted afterwards is not checked: one carrying
    /// `sep` splits into a pair nobody wrote, silently, until
    /// [`insert`](Self::insert) becomes fallible (`docs/next-major.md`).
    ///
    /// Refused: space, controls, non-ASCII, `\`, `'` and lowercase `n r t s`, which break the
    /// switch's split or its escapes; either of the block's brackets, `=` and `^`; `$` and `{`,
    /// which dialplan expansion reads as a reference across a pair boundary; `|` in a `[]` block.
    pub fn with_separator(mut self, sep: char) -> Result<Self, OriginateError> {
        check_separator(sep, self.vars_type)?;
        if self
            .inner
            .keys()
            .any(|k| k.contains(sep))
        {
            return Err(OriginateError::ParseError(format!(
                "a variable name contains the chosen '{sep}' separator"
            )));
        }
        if let Some((key, _)) = self
            .inner
            .iter()
            .find(|(_, v)| v.contains(sep))
        {
            return Err(OriginateError::ParseError(format!(
                "variable {key} contains the chosen '{sep}' separator"
            )));
        }
        self.separator = Some(sep);
        Ok(self)
    }

    /// The `^^` separator this block renders with, if one was chosen.
    pub fn separator(&self) -> Option<char> {
        self.separator
    }

    /// Insert or overwrite a variable.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.inner
            .insert(key.into(), value.into());
    }

    /// Remove a variable by name, returning its value if it existed.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.inner
            .shift_remove(key)
    }

    /// Look up a variable by name.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.inner
            .get(key)
            .map(|s| s.as_str())
    }

    /// Whether the set contains no variables.
    pub fn is_empty(&self) -> bool {
        self.inner
            .is_empty()
    }

    /// Number of variables.
    pub fn len(&self) -> usize {
        self.inner
            .len()
    }

    /// Variable scope (Enterprise, Default, or Channel).
    pub fn scope(&self) -> VariablesType {
        self.vars_type
    }

    /// Change the variable scope.
    pub fn set_scope(&mut self, scope: VariablesType) {
        self.vars_type = scope;
    }

    /// Iterate over key-value pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.inner
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Mutable iterator over key-value pairs in insertion order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut String)> {
        self.inner
            .iter_mut()
            .map(|(k, v)| (k.as_str(), v))
    }

    /// Mutable iterator over values in insertion order.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut String> {
        self.inner
            .values_mut()
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Variables {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.vars_type == VariablesType::Default
            && self
                .separator
                .is_none()
        {
            self.inner
                .serialize(serializer)
        } else {
            use serde::ser::SerializeStruct;
            let fields = 2 + usize::from(
                self.separator
                    .is_some(),
            );
            let mut s = serializer.serialize_struct("Variables", fields)?;
            s.serialize_field("scope", &self.vars_type)?;
            s.serialize_field("vars", &self.inner)?;
            if let Some(sep) = self.separator {
                s.serialize_field("separator", &sep)?;
            }
            s.end()
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Variables {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum VariablesRepr {
            Scoped {
                scope: VariablesType,
                vars: IndexMap<String, String>,
                #[serde(default)]
                separator: Option<char>,
            },
            Flat(IndexMap<String, String>),
        }

        let (vars_type, inner, separator) = match VariablesRepr::deserialize(deserializer)? {
            VariablesRepr::Scoped {
                scope,
                vars,
                separator,
            } => (scope, vars, separator),
            VariablesRepr::Flat(map) => (VariablesType::Default, map, None),
        };
        // A config naming a value the wire cannot carry fails at load rather
        // than on the call it was loaded for.
        for (at, (key, value)) in inner
            .iter()
            .enumerate()
        {
            check_representable(key, value, vars_type)
                .and_then(|()| {
                    check_case_collision(
                        inner
                            .keys()
                            .take(at),
                        key,
                        at,
                    )
                })
                .map_err(serde::de::Error::custom)?;
        }
        let vars = Self {
            vars_type,
            inner,
            separator: None,
        };
        match separator {
            Some(sep) => vars
                .with_separator(sep)
                .map_err(serde::de::Error::custom),
            None => Ok(vars),
        }
    }
}

/// Renders a [`Variables`] for one target. Returned by
/// [`Variables::display_for`].
#[derive(Debug, Clone, Copy)]
pub struct VariablesDisplay<'a> {
    vars: &'a Variables,
    target: DialStringTarget,
}

impl fmt::Display for VariablesDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.target
            .write_argument(f, |f, target| {
                self.vars
                    .write_for(f, target)
            })
    }
}

impl Variables {
    /// Render for a named carrier or [`DialStringTarget`], rather than the
    /// [`DialStringCarrier::EslApi`] default that [`Display`](fmt::Display) uses.
    pub fn display_for(&self, target: impl Into<DialStringTarget>) -> VariablesDisplay<'_> {
        VariablesDisplay {
            vars: self,
            target: target.into(),
        }
    }

    pub(super) fn write_for(
        &self,
        f: &mut fmt::Formatter<'_>,
        target: DialStringTarget,
    ) -> fmt::Result {
        let (open, close) = self
            .vars_type
            .delimiters();
        f.write_fmt(format_args!("{}", open))?;
        if let Some(sep) = self.separator {
            write!(f, "^^{sep}")?;
        }
        // A chosen separator carries the values that a comma would have needed
        // escaping for, so only a block splitting on commas escapes them.
        let commas_separate = self
            .separator
            .is_none_or(|sep| sep == ',');
        let sep = self
            .separator
            .unwrap_or(',');
        for (i, (key, value)) in self
            .inner
            .iter()
            .enumerate()
        {
            if i > 0 {
                write!(f, "{sep}")?;
            }
            let key = escape_text(
                key,
                target,
                EscapedField::Key {
                    scope: self.vars_type,
                    commas_separate,
                },
            );
            let value = escape_value(value, target, commas_separate, self.vars_type);
            write!(f, "{}={}", key, value)?;
        }
        f.write_fmt(format_args!("{}", close))
    }
}

impl fmt::Display for Variables {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_for(f, DialStringCarrier::EslApi.into())
    }
}

impl FromStr for Variables {
    type Err = OriginateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_for(s, DialStringCarrier::EslApi)
    }
}

/// The endpoint text a lone block is read ahead of, so the switch's passes see a whole leg.
const LONE_BLOCK_ENDPOINT: &str = "null/x";

impl Variables {
    /// Parse a block written for a named carrier or [`DialStringTarget`], as the switch installs
    /// it there: the pairs are read through a port of every pass that target applies, so a block
    /// [`display_for`](Self::display_for) wrote at the same target reads back as built.
    /// [`FromStr`] uses the [`DialStringCarrier::EslApi`] default of [`Display`](fmt::Display).
    pub fn parse_for(s: &str, target: impl Into<DialStringTarget>) -> Result<Self, OriginateError> {
        let (argument, target) = target
            .into()
            .read_argument(s)?;
        let s = argument.trim_matches(' ');
        if s.len() < 2 {
            return Err(OriginateError::ParseError(
                "variable block too short".into(),
            ));
        }
        match (s.as_bytes()[0], s.as_bytes()[s.len() - 1]) {
            (b'{', b'}') | (b'<', b'>') | (b'[', b']') => {}
            (open, close) => {
                return Err(OriginateError::ParseError(format!(
                    "unknown variable delimiters: {:?}..{:?}",
                    open as char, close as char
                )));
            }
        }
        match read_leg(&format!("{s}{LONE_BLOCK_ENDPOINT}"), target)? {
            (Some(block), endpoint) if endpoint == LONE_BLOCK_ENDPOINT => Self::from_block(&block),
            _ => Err(OriginateError::ParseError(
                "the switch ends the block before its last bracket".into(),
            )),
        }
    }

    /// The variables `block` installs, refusing what no render of this crate delivers.
    pub(crate) fn from_block(block: &Block) -> Result<Self, OriginateError> {
        let vars_type = VariablesType::of_block(block);
        if block.separator != ',' {
            check_separator(block.separator, vars_type)?;
        }
        if block.rewrites_following_text {
            return Err(OriginateError::ParseError(
                "the block's parse rewrites the text after it".into(),
            ));
        }
        let mut inner = IndexMap::new();
        for (i, pair) in block
            .pairs
            .iter()
            .enumerate()
        {
            let value = match &pair.effect {
                PairEffect::Set(value) => value.as_str(),
                PairEffect::Cleared | PairEffect::Valueless => "",
                PairEffect::Ignored => {
                    return Err(OriginateError::ParseError(format!(
                        "missing = in variable {i}"
                    )))
                }
                PairEffect::Unreadable => {
                    return Err(OriginateError::ParseError(format!(
                        "variable {i} opens a non-ASCII ^^ separator the switch splits by byte"
                    )))
                }
            };
            let key = pair
                .key
                .as_str();
            check_representable(key, value, vars_type)?;
            check_case_collision(inner.keys(), key, i)?;
            if block.separator != ','
                && (key.contains(block.separator) || value.contains(block.separator))
            {
                return Err(OriginateError::ParseError(format!(
                    "variable {i} contains the block's ^^ separator"
                )));
            }
            inner.insert(key.to_owned(), value.to_owned());
        }
        Ok(Self {
            vars_type,
            inner,
            separator: None,
        })
    }
}

/// The one leg the switch reads of `text` at `target`: its one block, if any, and the endpoint
/// text after it.
pub(crate) fn read_leg(
    text: &str,
    target: DialStringTarget,
) -> Result<(Option<Block>, String), OriginateError> {
    let list = pipeline::read(text, target).map_err(read_error)?;
    let one_leg = || OriginateError::ParseError("the switch reads more than one leg".into());
    let [thread] = &list.threads[..] else {
        return Err(one_leg());
    };
    let [group] = &thread.groups[..] else {
        return Err(one_leg());
    };
    let [leg] = &group[..] else {
        return Err(one_leg());
    };
    let mut blocks = list
        .blocks
        .iter()
        .chain(&thread.blocks)
        .chain(&leg.blocks);
    match (blocks.next(), blocks.next()) {
        (Some(_), Some(_)) => Err(OriginateError::ParseError(
            "an endpoint carries one variable block".into(),
        )),
        (block, _) => Ok((
            block.cloned(),
            leg.endpoint
                .clone(),
        )),
    }
}

/// The variables `block` installs, or `None` for no block or one installing nothing.
pub(crate) fn installed_variables(
    block: Option<&Block>,
) -> Result<Option<Variables>, OriginateError> {
    Ok(block
        .map(Variables::from_block)
        .transpose()?
        .filter(|vars| !vars.is_empty()))
}

/// Why the switch reads no dial string from the text.
pub(crate) fn read_error(error: PipelineError) -> OriginateError {
    OriginateError::ParseError(
        match error {
            PipelineError::Empty => "no endpoint to dial",
            PipelineError::ArgvSplit => "originate's argument split cuts the dial string",
            PipelineError::UnclosedBlock { .. } => "a variable block never closes",
            PipelineError::SplitSeparatorUnreadable => {
                "a split on a non-ASCII ^^ separator's first byte reaches past its text"
            }
        }
        .into(),
    )
}
