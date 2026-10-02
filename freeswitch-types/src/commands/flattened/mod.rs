//! Dial strings the switch produced, read the way the switch reads them.
//!
//! A directory group's expansion or any other list the switch wrote is read by
//! the switch's own passes for a named [`DialStringTarget`], so each leg's
//! variables are the ones its channel receives. Every leg keeps its input text,
//! and [`FlattenedDialString::display_raw`] forwards the kept legs unchanged.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use crate::channel::HangupCause;
use crate::commands::endpoint::Endpoint;
use crate::commands::originate::OriginateError;
use crate::commands::variables::{DialStringTarget, Variables, VariablesType};
use crate::switch_passes::brackets::{Block, Pair, PairEffect};
use crate::switch_passes::escape::{escape_text, EscapedField};
use crate::switch_passes::expansion::names_a_variable;
use crate::switch_passes::originate_legs::{
    resolve, DialList, Leg, PastLimit, Thread, ENTERPRISE_DELIM, MAX_PEERS,
};
use crate::switch_passes::{pipeline, PipelineError};
use crate::variables::VariableName;

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "serde"))]
mod c_oracle;

/// A dial string read by the passes the switch applies for one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlattenedDialString {
    head: String,
    blocks: Vec<Block>,
    threads: Vec<FlattenedThread>,
    tail: String,
    warnings: Vec<ListWarning>,
}

/// One `:_:` thread, or the whole list when there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlattenedThread {
    separator: String,
    head: String,
    blocks: Vec<Block>,
    groups: Vec<FlattenedGroup>,
    trailer: String,
}

/// Legs rung together; groups of a thread are tried in turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlattenedGroup {
    separator: String,
    legs: Vec<FlattenedLeg>,
}

/// One leg as its channel receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlattenedLeg {
    separator: String,
    raw: String,
    leg: Leg,
    inherited: Arc<[Block]>,
    nested_vars: bool,
    target: LegTarget,
    warnings: Vec<LegWarning>,
}

/// What a leg dials.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LegTarget {
    /// `error/`, which ends the leg with a cause and places no call.
    Error(ErrorLeg),
    /// An endpoint this crate models; it carries no variable block.
    Endpoint(Endpoint),
    /// An endpoint this crate does not model, or an empty leg.
    Unparsed(UnparsedLeg),
}

/// An `error/` leg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorLeg {
    as_written: String,
    reading: CauseReading,
}

/// Endpoint text no typed endpoint accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnparsedLeg {
    endpoint: String,
    error: OriginateError,
}

/// How `switch_channel_str2cause` reads the text after `error/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CauseReading {
    /// A cause name, in any case.
    Name(HangupCause),
    /// A leading digit run, read with `atoi`.
    Number(u32),
    /// Neither; the switch ends the leg with its default cause.
    Unrecognized,
}

/// A pair or block of the leg's own that does not reach the channel as written.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LegWarning {
    /// The pair has no `=`, so nothing is installed.
    PairIgnored {
        /// Index among the leg's blocks.
        block: usize,
        /// Variable name.
        key: String,
    },
    /// The value is empty, which deletes every earlier value of the key.
    PairCleared {
        /// Index among the leg's blocks.
        block: usize,
        /// Variable name.
        key: String,
    },
    /// The value holds `${`, which the switch refuses unless `origination_nested_vars=true`
    /// appears in the leg's thread or a `<>` block ahead of an enterprise split sets it true.
    NestedVarsRefused {
        /// Index among the leg's blocks.
        block: usize,
        /// Variable name.
        key: String,
    },
    /// The block's `^^` separator is not ASCII. The switch splits on its first
    /// byte, which no typed name can carry, so the block contributes no pairs.
    BlockSeparatorUnreadable {
        /// Index among the leg's blocks.
        block: usize,
    },
    /// Parsing the block writes into the leg's text after it, as a block of `^^` alone does. The
    /// endpoint is read from the rewritten text, so a render of the leg reads differently.
    BlockRewritesFollowingText {
        /// Index among the leg's blocks.
        block: usize,
    },
    /// A pair opens a `^^` head naming a non-ASCII separator, which the switch splits on its
    /// first byte; what it installs, if anything, no string carries.
    PairUnreadable {
        /// Index among the leg's blocks.
        block: usize,
    },
}

/// Something about the whole list the typed view cannot show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ListWarning {
    /// A quote kept a leg or group separator from splitting.
    QuoteSpansLegs,
    /// The dialplan carrier substitutes a `${}` reference with a value only the
    /// switch knows; the reference is kept as written.
    CarrierExpands,
    /// A list or thread block's `^^` separator is not ASCII. The switch splits on
    /// its first byte, which no typed name can carry, so the block contributes no pairs.
    BlockSeparatorUnreadable {
        /// Index among the list's blocks and then each thread's, in reading order.
        block: usize,
    },
    /// Parsing a list or thread block writes into the text after it, as a block of `^^` alone
    /// does. What follows is read from the rewritten text, so a render of the list reads differently.
    BlockRewritesFollowingText {
        /// Index among the list's blocks and then each thread's, in reading order.
        block: usize,
    },
    /// A split stopped at the switch's `MAX_PEERS` with text left: its last thread, group or leg
    /// keeps the rest, separators included, and nothing past it is dialled.
    LegsPastLimit {
        /// Index among [`FlattenedDialString::threads`] of the thread whose groups or legs were
        /// split, `None` for the `:_:` split.
        thread: Option<usize>,
        /// Index among [`FlattenedThread::groups`] of the group whose legs were split, `None`
        /// for a `:_:` or `|` split.
        group: Option<usize>,
        /// Threads, groups or legs past the limit the switch never splits off. Empty tokens do
        /// not count, so this is zero when only empty ones or a trailing separator follow.
        excess: usize,
    },
}

/// What stops the switch from reading the dial string at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FlattenedDialStringError {
    /// Nothing to dial.
    Empty,
    /// The API carrier's argument split cuts the dial string into more than one
    /// argument, leaves a quote open, or finds a quote holding a
    /// [`DialStringTarget::argv_separator`].
    ArgvSplit,
    /// A block never closes, which aborts the whole originate.
    UnclosedBlock {
        /// Index of the leg in reading order.
        leg: usize,
    },
    /// The text ends in `\n`, as a reply body does before its suffix is stripped.
    TrailingNewline,
    /// A split the switch runs on a non-ASCII `^^` separator's first byte reaches text no string
    /// carries: a group or leg opening such a head, or a block with one ending in a backslash.
    SplitSeparatorUnreadable,
}

/// Renders a [`FlattenedDialString`]. Returned by
/// [`FlattenedDialString::display_raw`] and [`FlattenedDialString::display_for`].
#[derive(Debug, Clone, Copy)]
pub struct FlattenedDialStringDisplay<'a> {
    list: &'a FlattenedDialString,
    render: Render,
}

#[derive(Debug, Clone, Copy)]
enum Render {
    Raw,
    For(DialStringTarget),
}

impl FlattenedDialString {
    /// Read `input` through every pass `target` applies.
    pub fn parse_for(
        input: &str,
        target: impl Into<DialStringTarget>,
    ) -> Result<Self, FlattenedDialStringError> {
        if input.ends_with('\n') {
            return Err(FlattenedDialStringError::TrailingNewline);
        }
        let list = pipeline::read(input, target.into())
            .map_err(FlattenedDialStringError::from_pipeline)?;
        let DialList {
            blocks,
            threads,
            quote_spans_legs,
            carrier_expands,
            past_limit,
        } = list;
        let past_limit: Vec<ListWarning> = past_limit
            .into_iter()
            .map(|cut| ListWarning::past_limit(&threads, cut))
            .collect();
        let unreadable = blocks
            .iter()
            .chain(
                threads
                    .iter()
                    .flat_map(|thread| &thread.blocks),
            )
            .enumerate()
            .filter_map(|(block, parsed)| {
                if parsed.separator_unreadable() {
                    Some(ListWarning::BlockSeparatorUnreadable { block })
                } else {
                    parsed
                        .rewrites_following_text
                        .then_some(ListWarning::BlockRewritesFollowingText { block })
                }
            });
        let warnings = [
            (quote_spans_legs, ListWarning::QuoteSpansLegs),
            (carrier_expands, ListWarning::CarrierExpands),
        ]
        .into_iter()
        .filter_map(|(raised, warning)| raised.then_some(warning))
        .chain(unreadable)
        .chain(past_limit)
        .collect();
        let mut kept: Vec<(Range<usize>, FlattenedThread)> = Vec::new();
        for thread in threads {
            let raw = thread
                .raw
                .clone();
            let separator_start = kept
                .last()
                .map_or(raw.start, |(before, _)| before.end);
            if let Some(read) = FlattenedThread::read(input, thread, &blocks, separator_start) {
                kept.push((raw, read));
            }
        }
        let (Some((first, _)), Some((last, _))) = (kept.first(), kept.last()) else {
            return Err(FlattenedDialStringError::Empty);
        };
        Ok(Self {
            head: slice(input, 0..first.start),
            tail: slice(input, last.end..input.len()),
            blocks,
            threads: kept
                .into_iter()
                .map(|(_, thread)| thread)
                .collect(),
            warnings,
        })
    }

    /// Threads in reading order.
    pub fn threads(&self) -> impl Iterator<Item = &FlattenedThread> {
        self.threads
            .iter()
    }

    /// Every leg of every thread, in reading order.
    pub fn legs(&self) -> impl Iterator<Item = &FlattenedLeg> {
        self.threads
            .iter()
            .flat_map(FlattenedThread::legs)
    }

    /// Warnings about the whole list.
    pub fn warnings(&self) -> &[ListWarning] {
        &self.warnings
    }

    /// Keep the legs `keep` accepts. A group or thread left with no leg goes too.
    pub fn retain(&mut self, mut keep: impl FnMut(&FlattenedLeg) -> bool) {
        for thread in &mut self.threads {
            for group in &mut thread.groups {
                group
                    .legs
                    .retain(&mut keep);
            }
            thread
                .groups
                .retain(|group| {
                    !group
                        .legs
                        .is_empty()
                });
        }
        self.threads
            .retain(|thread| {
                !thread
                    .groups
                    .is_empty()
            });
    }

    /// Whether no leg is left.
    pub fn is_empty(&self) -> bool {
        self.threads
            .is_empty()
    }

    fn write_raw(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return Ok(());
        }
        f.write_str(&self.head)?;
        for (t, thread) in self
            .threads
            .iter()
            .enumerate()
        {
            if t > 0 {
                f.write_str(&thread.separator)?;
            }
            f.write_str(&thread.head)?;
            for (g, group) in thread
                .groups
                .iter()
                .enumerate()
            {
                if g > 0 {
                    f.write_str(&group.separator)?;
                }
                for (l, leg) in group
                    .legs
                    .iter()
                    .enumerate()
                {
                    if l > 0 {
                        f.write_str(&leg.separator)?;
                    }
                    f.write_str(&leg.raw)?;
                }
            }
            f.write_str(&thread.trailer)?;
        }
        f.write_str(&self.tail)
    }

    fn write_for(&self, f: &mut fmt::Formatter<'_>, target: DialStringTarget) -> fmt::Result {
        if self.is_empty() {
            return Ok(());
        }
        write_blocks(f, &self.blocks, target)?;
        for (t, thread) in self
            .threads
            .iter()
            .enumerate()
        {
            if t > 0 {
                f.write_str(ENTERPRISE_DELIM)?;
            }
            write_blocks(f, &thread.blocks, target)?;
            for (g, group) in thread
                .groups
                .iter()
                .enumerate()
            {
                if g > 0 {
                    f.write_str("|")?;
                }
                for (l, leg) in group
                    .legs
                    .iter()
                    .enumerate()
                {
                    if l > 0 {
                        f.write_str(",")?;
                    }
                    write_blocks(
                        f,
                        &leg.leg
                            .blocks,
                        target,
                    )?;
                    f.write_str(&escape_text(
                        &leg.leg
                            .endpoint,
                        target,
                        EscapedField::Endpoint,
                    ))?;
                }
            }
        }
        Ok(())
    }

    /// The input text of the kept legs, joined by the separators that stood
    /// before each of them in the input; byte-identical when nothing was removed.
    pub fn display_raw(&self) -> FlattenedDialStringDisplay<'_> {
        FlattenedDialStringDisplay {
            list: self,
            render: Render::Raw,
        }
    }

    /// The list rendered by this crate for `target`, carrying only the pairs
    /// that set a value.
    pub fn display_for(
        &self,
        target: impl Into<DialStringTarget>,
    ) -> FlattenedDialStringDisplay<'_> {
        FlattenedDialStringDisplay {
            list: self,
            render: Render::For(target.into()),
        }
    }
}

impl fmt::Display for FlattenedDialStringDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.render {
            Render::Raw => self
                .list
                .write_raw(f),
            Render::For(target) => target.write_argument(f, |f, target| {
                self.list
                    .write_for(f, target)
            }),
        }
    }
}

/// Every block through [`Variables`], with only the pairs that set a value.
fn write_blocks(
    f: &mut fmt::Formatter<'_>,
    blocks: &[Block],
    target: DialStringTarget,
) -> fmt::Result {
    for block in blocks {
        let mut vars = Variables::new(VariablesType::of_block(block));
        for pair in &block.pairs {
            if let PairEffect::Set(value) = &pair.effect {
                vars.insert(
                    pair.key
                        .as_str(),
                    value.as_str(),
                );
            }
        }
        if vars.is_empty() {
            continue;
        }
        // A `^^` separator the builder refuses renders comma-separated, which
        // reads back to the same values.
        let vars = match block.separator {
            ',' => vars,
            sep => vars
                .clone()
                .with_separator(sep)
                .unwrap_or(vars),
        };
        write!(f, "{}", vars.display_for(target))?;
    }
    Ok(())
}

/// `range` is a pipeline trace: char-boundary offsets into `input`, in reading order.
fn slice(input: &str, range: Range<usize>) -> String {
    debug_assert!(input
        .get(range.clone())
        .is_some());
    input
        .get(range)
        .unwrap_or_default()
        .to_owned()
}

impl FlattenedThread {
    fn read(
        input: &str,
        thread: Thread,
        list_blocks: &[Block],
        separator_start: usize,
    ) -> Option<Self> {
        let Thread {
            raw,
            blocks,
            groups: pipeline_groups,
            nested_vars,
        } = thread;
        let inherited: Arc<[Block]> = list_blocks
            .iter()
            .chain(&blocks)
            .cloned()
            .collect();
        let mut first_start = None;
        let mut last_end = None;
        let mut groups = Vec::new();
        for group in pipeline_groups {
            let mut separator = None;
            let mut legs = Vec::with_capacity(group.len());
            let mut leg_end = None;
            for leg in group {
                let start = leg
                    .raw
                    .start;
                separator.get_or_insert_with(|| slice(input, last_end.unwrap_or(start)..start));
                first_start.get_or_insert(start);
                let leg_separator = slice(input, leg_end.unwrap_or(start)..start);
                leg_end = Some(
                    leg.raw
                        .end,
                );
                legs.push(FlattenedLeg::read(
                    input,
                    leg,
                    leg_separator,
                    &inherited,
                    nested_vars,
                ));
            }
            let Some(separator) = separator else {
                continue;
            };
            last_end = leg_end;
            groups.push(FlattenedGroup { separator, legs });
        }
        let (start, end) = first_start.zip(last_end)?;
        Some(Self {
            separator: slice(input, separator_start..raw.start),
            head: slice(input, raw.start..start),
            blocks,
            groups,
            trailer: slice(input, end..raw.end),
        })
    }

    /// Groups in the order they are tried.
    pub fn groups(&self) -> impl Iterator<Item = &FlattenedGroup> {
        self.groups
            .iter()
    }

    /// Every leg of every group.
    pub fn legs(&self) -> impl Iterator<Item = &FlattenedLeg> {
        self.groups
            .iter()
            .flat_map(FlattenedGroup::legs)
    }
}

impl FlattenedGroup {
    /// Legs rung together.
    pub fn legs(&self) -> impl Iterator<Item = &FlattenedLeg> {
        self.legs
            .iter()
    }
}

impl FlattenedLeg {
    fn read(
        input: &str,
        leg: Leg,
        separator: String,
        inherited: &Arc<[Block]>,
        nested_vars: bool,
    ) -> Self {
        let warnings = leg
            .blocks
            .iter()
            .enumerate()
            .flat_map(|(block, parsed)| {
                parsed
                    .separator_unreadable()
                    .then_some(LegWarning::BlockSeparatorUnreadable { block })
                    .into_iter()
                    .chain(
                        parsed
                            .rewrites_following_text
                            .then_some(LegWarning::BlockRewritesFollowingText { block }),
                    )
                    .chain(
                        parsed
                            .pairs
                            .iter()
                            .filter_map(move |pair| LegWarning::of(block, pair, nested_vars)),
                    )
            })
            .collect();
        Self {
            separator,
            raw: slice(
                input,
                leg.raw
                    .clone(),
            ),
            target: LegTarget::read(&leg.endpoint),
            leg,
            inherited: Arc::clone(inherited),
            nested_vars,
            warnings,
        }
    }

    /// The leg's own input text.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// The value the leg's channel receives, after the list and thread blocks
    /// are installed in the order `local_var_clobber` decides.
    ///
    /// A value naming a variable is refused at install, as the switch refuses it, unless
    /// `origination_nested_vars=true` appears in the leg's thread or a `<>` block ahead of an
    /// enterprise split sets it true. On a channel with `CF_NO_PRESENCE`, originate deletes
    /// `presence_id`.
    pub fn variable(&self, name: impl VariableName) -> Option<&str> {
        self.variable_str(name.as_str())
    }

    /// Look up a leg variable by its bare wire name, for a key with no
    /// [`VariableName`] enum of its own (e.g. a `sip_h_` passthrough).
    ///
    /// ```
    /// use freeswitch_types::commands::{DialStringCarrier, FlattenedDialString};
    ///
    /// let list = FlattenedDialString::parse_for(
    ///     "[sip_h_X-Seat=42]loopback/9199/test",
    ///     DialStringCarrier::EslApi,
    /// )
    /// .unwrap();
    /// let leg = list.legs().next().unwrap();
    /// assert_eq!(leg.variable_str("sip_h_X-Seat"), Some("42"));
    /// ```
    pub fn variable_str(&self, name: &str) -> Option<&str> {
        resolve(
            self.inherited
                .iter(),
            &self.leg,
            name,
            self.nested_vars,
        )
    }

    /// What the leg dials.
    pub fn target(&self) -> &LegTarget {
        &self.target
    }

    /// Pairs and blocks of the leg's own that do not reach the channel as written.
    pub fn warnings(&self) -> &[LegWarning] {
        &self.warnings
    }
}

impl ErrorLeg {
    /// The text after `error/`.
    pub fn as_written(&self) -> &str {
        &self.as_written
    }

    /// How the switch reads that text.
    pub fn reading(&self) -> CauseReading {
        self.reading
    }

    /// The cause named, or the numbered cause when the number fits `u16` and
    /// names one.
    ///
    /// The switch ends a zero with `DESTINATION_OUT_OF_ORDER` and an
    /// unrecognized cause with `NORMAL_CLEARING`; this crate fabricates neither.
    pub fn cause(&self) -> Option<HangupCause> {
        match self.reading {
            CauseReading::Name(cause) => Some(cause),
            CauseReading::Number(number) => u16::try_from(number)
                .ok()
                .and_then(HangupCause::from_number),
            CauseReading::Unrecognized => None,
        }
    }
}

impl LegTarget {
    fn read(endpoint: &str) -> Self {
        if let Some(cause) = endpoint.strip_prefix("error/") {
            return Self::Error(ErrorLeg {
                as_written: cause.to_owned(),
                reading: str2cause(cause),
            });
        }
        match Endpoint::parse_bare(endpoint) {
            Ok(parsed) => Self::Endpoint(parsed),
            Err(error) => Self::Unparsed(UnparsedLeg {
                endpoint: endpoint.to_owned(),
                error,
            }),
        }
    }
}

/// `switch_channel_str2cause` on the text after `error/`.
fn str2cause(text: &str) -> CauseReading {
    if text.starts_with(|c: char| c.is_ascii_digit()) {
        // glibc's atoi: strtol clamps to a 64-bit long, which the switch truncates to its cause.
        let long = text
            .bytes()
            .take_while(u8::is_ascii_digit)
            .fold(0i128, |n, digit| {
                (n * 10 + i128::from(digit - b'0')).min(i128::from(i64::MAX))
            });
        return CauseReading::Number(long as i64 as u32);
    }
    text.to_ascii_uppercase()
        .parse::<HangupCause>()
        .map_or(CauseReading::Unrecognized, CauseReading::Name)
}

impl LegWarning {
    fn of(block: usize, pair: &Pair, nested_vars: bool) -> Option<Self> {
        let key = || {
            pair.key
                .clone()
        };
        match &pair.effect {
            PairEffect::Ignored | PairEffect::Valueless => {
                Some(Self::PairIgnored { block, key: key() })
            }
            PairEffect::Unreadable => Some(Self::PairUnreadable { block }),
            PairEffect::Cleared => Some(Self::PairCleared { block, key: key() }),
            PairEffect::Set(value) if !nested_vars && names_a_variable(value) => {
                Some(Self::NestedVarsRefused { block, key: key() })
            }
            PairEffect::Set(_) => None,
        }
    }
}

impl ListWarning {
    /// `cut`, its indices counting only the threads and groups [`FlattenedThread::read`] keeps.
    fn past_limit(threads: &[Thread], cut: PastLimit) -> Self {
        let kept_groups = |thread: &Thread, before: usize| {
            thread
                .groups
                .iter()
                .take(before)
                .filter(|group| !group.is_empty())
                .count()
        };
        Self::LegsPastLimit {
            thread: cut
                .thread
                .map(|t| {
                    threads
                        .iter()
                        .take(t)
                        .filter(|thread| kept_groups(thread, usize::MAX) > 0)
                        .count()
                }),
            group: cut
                .thread
                .and_then(|t| threads.get(t))
                .zip(cut.group)
                .map(|(thread, g)| kept_groups(thread, g)),
            excess: cut.excess,
        }
    }
}

impl FlattenedDialStringError {
    fn from_pipeline(error: PipelineError) -> Self {
        match error {
            PipelineError::Empty => Self::Empty,
            PipelineError::ArgvSplit => Self::ArgvSplit,
            PipelineError::UnclosedBlock { leg } => Self::UnclosedBlock { leg },
            PipelineError::SplitSeparatorUnreadable => Self::SplitSeparatorUnreadable,
        }
    }
}

impl UnparsedLeg {
    /// The endpoint text after the leg's blocks.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Why no typed endpoint accepts that text.
    pub fn error(&self) -> &OriginateError {
        &self.error
    }
}

impl fmt::Display for UnparsedLeg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = if self
            .endpoint
            .is_empty()
        {
            "empty leg"
        } else {
            "unrecognized endpoint"
        };
        write!(
            f,
            "{kind} ({} bytes)",
            self.endpoint
                .len()
        )
    }
}

impl fmt::Display for LegWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PairIgnored { block, key } => {
                write!(
                    f,
                    "pair {key} in block {block} has no value and is not installed"
                )
            }
            Self::PairCleared { block, key } => {
                write!(
                    f,
                    "pair {key} in block {block} is empty and deletes earlier values"
                )
            }
            Self::NestedVarsRefused { block, key } => write!(
                f,
                "pair {key} in block {block} names a variable while origination_nested_vars is off"
            ),
            Self::BlockSeparatorUnreadable { block } => write!(
                f,
                "block {block} has a non-ASCII separator and contributes no pairs"
            ),
            Self::BlockRewritesFollowingText { block } => {
                write!(f, "parsing block {block} rewrites the leg's text after it")
            }
            Self::PairUnreadable { block } => write!(
                f,
                "a pair in block {block} opens a non-ASCII ^^ separator the switch splits by byte"
            ),
        }
    }
}

impl fmt::Display for ListWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QuoteSpansLegs => f.write_str("a quote holds a leg or group separator"),
            Self::CarrierExpands => {
                f.write_str("the dialplan carrier expands a variable reference kept as written")
            }
            Self::BlockSeparatorUnreadable { block } => write!(
                f,
                "list block {block} has a non-ASCII separator and contributes no pairs"
            ),
            Self::BlockRewritesFollowingText { block } => {
                write!(f, "parsing list block {block} rewrites the text after it")
            }
            Self::LegsPastLimit {
                thread,
                group,
                excess,
            } => {
                match (thread, group) {
                    (Some(thread), Some(group)) => {
                        write!(f, "the leg split of group {group} in thread {thread}")?
                    }
                    (Some(thread), None) => write!(f, "the group split of thread {thread}")?,
                    (None, _) => f.write_str("the thread split")?,
                }
                write!(
                    f,
                    " stops at the switch's limit of {MAX_PEERS}; \
                     {excess} more are never split off"
                )
            }
        }
    }
}

impl fmt::Display for FlattenedDialStringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("dial string has no leg to dial"),
            Self::ArgvSplit => f.write_str("originate's argument split cuts the dial string"),
            Self::UnclosedBlock { leg } => write!(f, "a block on leg {leg} never closes"),
            Self::TrailingNewline => f.write_str("dial string ends in a newline"),
            Self::SplitSeparatorUnreadable => f.write_str(
                "a split on a non-ASCII ^^ separator's first byte reaches past its text",
            ),
        }
    }
}

impl std::error::Error for FlattenedDialStringError {}
