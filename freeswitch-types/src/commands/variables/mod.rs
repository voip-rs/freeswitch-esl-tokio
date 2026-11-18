//! Channel variable scope and ordered key-value storage for originate commands.

use indexmap::IndexMap;
use std::fmt;
use std::str::FromStr;

use super::originate::OriginateError;
use crate::switch_passes::brackets::{check_separator, same_header, unbalanced, Block, PairEffect};
use crate::switch_passes::escape::{escape_text, escape_value, EscapedField};
use crate::switch_passes::originate_legs::splits_into_threads;
use crate::switch_passes::{pipeline, PipelineError};
use crate::wire_safety::check_header_value;

mod fault;
mod target;
#[cfg(test)]
mod tests;

pub use fault::{UndeliverableVariable, VariableFault, VariablePart};
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
/// bracket.
fn check_key(key: &str, vars_type: VariablesType) -> Result<(), UndeliverableVariable> {
    let fault = if key.is_empty() {
        Some(VariableFault::Empty)
    } else if key.contains('[') {
        Some(VariableFault::ArrayIndex)
    } else if splits_into_threads(key) {
        Some(VariableFault::EnterpriseSeparator)
    } else if vars_type == VariablesType::Channel && key.contains('\'') {
        Some(VariableFault::QuoteInChannelScope)
    } else {
        unbalanced(key, vars_type.delimiters())
    };
    fault.map_or(Ok(()), |fault| {
        Err(UndeliverableVariable::in_name(key, fault))
    })
}

/// Reject a pair no escaping carries: a key [`check_key`] refuses, or a value carrying `:_:`, a
/// quote in channel scope, a header control, nothing or an unbalanced bracket.
fn check_representable(
    key: &str,
    value: &str,
    vars_type: VariablesType,
) -> Result<(), UndeliverableVariable> {
    check_key(key, vars_type)?;
    check_header_value(key, value)?;
    let fault = if splits_into_threads(value) {
        Some(VariableFault::EnterpriseSeparator)
    } else if vars_type == VariablesType::Channel && value.contains('\'') {
        Some(VariableFault::QuoteInChannelScope)
    } else if value.is_empty() {
        Some(VariableFault::Empty)
    } else {
        unbalanced(value, vars_type.delimiters())
    };
    fault.map_or(Ok(()), |fault| {
        Err(UndeliverableVariable::in_value(key, fault))
    })
}

/// Reject a key naming, but for ASCII case, one of `others`: the block's event replaces a header
/// by `strcasecmp`, so the two install as one variable.
fn check_case_collision<'a>(
    others: impl IntoIterator<Item = &'a String>,
    key: &str,
) -> Result<(), UndeliverableVariable> {
    let collides = others
        .into_iter()
        .any(|name| name != key && same_header(name, key));
    match collides {
        true => Err(UndeliverableVariable::in_name(
            key,
            VariableFault::CaseCollision,
        )),
        false => Ok(()),
    }
}

/// Reject a pair carrying `sep`, which the switch splits the block on.
fn check_separator_free(key: &str, value: &str, sep: char) -> Result<(), UndeliverableVariable> {
    if key.contains(sep) {
        return Err(UndeliverableVariable::in_name(
            key,
            VariableFault::BlockSeparator(sep),
        ));
    }
    match value.contains(sep) {
        true => Err(UndeliverableVariable::in_value(
            key,
            VariableFault::BlockSeparator(sep),
        )),
        false => Ok(()),
    }
}

/// Reject a pair joining a block of `vars_type` that holds `others` and splits on `sep`, if not
/// the comma: everything a block cannot carry.
fn check_pair<'a>(
    key: &str,
    value: &str,
    vars_type: VariablesType,
    others: impl IntoIterator<Item = &'a String>,
    sep: Option<char>,
) -> Result<(), UndeliverableVariable> {
    check_representable(key, value, vars_type)?;
    check_case_collision(others, key)?;
    sep.map_or(Ok(()), |sep| check_separator_free(key, value, sep))
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

    /// Create from an existing set of key-value pairs, refusing the first one
    /// [`insert`](Self::insert) refuses.
    pub fn with_vars(
        vars_type: VariablesType,
        vars: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Result<Self, OriginateError> {
        let mut built = Self::new(vars_type);
        for (key, value) in vars {
            built.insert(key, value)?;
        }
        Ok(built)
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
    /// present contains it; [`insert`](Self::insert) refuses one carrying it afterwards.
    ///
    /// Refused: space, controls, non-ASCII, `\`, `'` and lowercase `n r t s`, which break the
    /// switch's split or its escapes; either of the block's brackets, `=` and `^`; `$` and `{`,
    /// which dialplan expansion reads as a reference across a pair boundary; `|` in a `[]` block.
    pub fn with_separator(mut self, sep: char) -> Result<Self, OriginateError> {
        check_separator(sep, self.vars_type)?;
        for (index, (key, value)) in self
            .inner
            .iter()
            .enumerate()
        {
            check_separator_free(key, value, sep).map_err(|variable| variable.at(index))?;
        }
        self.separator = Some(sep);
        Ok(self)
    }

    /// The pairs the switch itself installed, unchecked: text it produced is reported, not
    /// refused, so a pair the builder would refuse still renders as the switch read it.
    pub(crate) fn installed_by_switch<'a>(
        vars_type: VariablesType,
        pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Self {
        Self {
            vars_type,
            inner: pairs
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            separator: None,
        }
    }

    /// The `^^` separator this block renders with, if one was chosen.
    pub fn separator(&self) -> Option<char> {
        self.separator
    }

    /// Insert a variable, or replace the value of one already named in place.
    ///
    /// Refuses, leaving the set unchanged, what parsing a block refuses and a pair carrying the
    /// separator [`with_separator`](Self::with_separator) chose. The error's index is the
    /// position the pair would take.
    pub fn insert(
        &mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<(), OriginateError> {
        let (key, value) = (key.into(), value.into());
        let index = self
            .inner
            .get_index_of(&key)
            .unwrap_or(
                self.inner
                    .len(),
            );
        check_pair(
            &key,
            &value,
            self.vars_type,
            self.inner
                .keys(),
            self.separator,
        )
        .map_err(|variable| variable.at(index))?;
        self.inner
            .insert(key, value);
        Ok(())
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

    /// Change the variable scope, which picks the brackets, the channel-scope quote rule and the
    /// separators that delimit the block. Refuses, keeping the old scope, when a held pair or
    /// the chosen separator cannot be written in the new one.
    pub fn set_scope(&mut self, scope: VariablesType) -> Result<(), OriginateError> {
        if let Some(sep) = self.separator {
            check_separator(sep, scope)?;
        }
        for (index, (key, value)) in self
            .inner
            .iter()
            .enumerate()
        {
            check_representable(key, value, scope).map_err(|variable| variable.at(index))?;
        }
        self.vars_type = scope;
        Ok(())
    }

    /// Iterate over key-value pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.inner
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
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
        let vars = Self::with_vars(vars_type, inner).map_err(serde::de::Error::custom)?;
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
            check_pair(
                key,
                value,
                vars_type,
                inner.keys(),
                (block.separator != ',').then_some(block.separator),
            )
            .map_err(|variable| variable.at(i))?;
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
