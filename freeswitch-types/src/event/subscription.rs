use super::event_type::EslEventType;
use super::format::EventFormat;
use crate::headers::EventHeader;
use crate::sofia::SofiaEventSubclass;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WireTokenFault {
    Empty,
    Newline,
    Space,
}

impl WireTokenFault {
    fn classify(s: &str) -> Option<Self> {
        if s.is_empty() {
            Some(Self::Empty)
        } else if crate::wire_safety::contains_wire_terminator(s) {
            Some(Self::Newline)
        } else if s.contains(' ') {
            Some(Self::Space)
        } else {
            None
        }
    }
}

/// Error returned when an [`EventSubscription`] builder method receives invalid input.
///
/// Custom subclasses and filter values are validated against ESL wire-safety
/// constraints: no newlines, carriage returns, or (for subclasses) spaces.
/// `Display` names the character class that made the value unusable and the
/// value's size; the value itself is subscriber data and stays on the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSubscriptionError(pub String);

impl fmt::Display for EventSubscriptionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let class = match WireTokenFault::classify(&self.0) {
            Some(WireTokenFault::Empty) => return f.write_str("event subscription token is empty"),
            Some(WireTokenFault::Newline) => "a newline",
            Some(WireTokenFault::Space) => "a space",
            None => "an unusable character",
        };
        write!(
            f,
            "event subscription token contains {class} ({} bytes)",
            self.0
                .len()
        )
    }
}

impl std::error::Error for EventSubscriptionError {}

/// Declarative description of an ESL event subscription.
///
/// Captures the event format, event types, custom subclasses, and filters
/// as a single unit. Useful for config-driven subscriptions and reconnection
/// patterns where the caller needs to rebuild subscriptions from a saved
/// description.
///
/// # Wire safety
///
/// Builder methods validate inputs against ESL wire injection risks.
/// Custom subclasses reject `\n`, `\r`, spaces, and empty strings.
/// Filter headers and values reject `\n` and `\r`.
///
/// # Example
///
/// ```rust
/// use freeswitch_types::{EventSubscription, EventFormat, EslEventType, EventHeader};
///
/// let sub = EventSubscription::new(EventFormat::Plain)
///     .events(EslEventType::CHANNEL_EVENTS)
///     .event(EslEventType::Heartbeat)
///     .custom_subclass("sofia::register").unwrap()
///     .filter(EventHeader::CallDirection, "inbound").unwrap();
///
/// assert!(!sub.is_empty());
/// assert!(!sub.is_all());
/// ```
// qual:allow(srp, god_struct) reason: "public builder; accessors read disjoint fields"
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSubscription {
    format: EventFormat,
    events: Vec<EslEventType>,
    raw_events: Vec<String>,
    custom_subclasses: Vec<String>,
    filters: Vec<(String, String)>,
}

/// What a field accepts beyond the newline ban every wire value carries.
#[derive(Debug, Clone, Copy)]
struct WireTokenRules {
    reject_empty: bool,
    reject_space: bool,
}

impl WireTokenRules {
    /// One wire token: the switch splits on spaces and drops an empty one.
    const TOKEN: Self = Self {
        reject_empty: true,
        reject_space: true,
    };

    /// Free text within a header line, where only the terminators are unusable.
    const TEXT: Self = Self {
        reject_empty: false,
        reject_space: false,
    };
}

fn validate_wire_token(s: &str, rules: WireTokenRules) -> Result<(), EventSubscriptionError> {
    let rejected = match WireTokenFault::classify(s) {
        Some(WireTokenFault::Empty) => rules.reject_empty,
        Some(WireTokenFault::Newline) => true,
        Some(WireTokenFault::Space) => rules.reject_space,
        None => false,
    };
    if rejected {
        return Err(EventSubscriptionError(s.to_string()));
    }
    Ok(())
}

fn validate_raw_event(s: &str) -> Result<(), EventSubscriptionError> {
    validate_wire_token(s, WireTokenRules::TOKEN)
}

fn validate_custom_subclass(s: &str) -> Result<(), EventSubscriptionError> {
    validate_wire_token(s, WireTokenRules::TOKEN)
}

fn validate_filter_field(s: &str) -> Result<(), EventSubscriptionError> {
    validate_wire_token(s, WireTokenRules::TEXT)
}

const CUSTOM_TOKEN: &str = "CUSTOM";

fn is_custom_token(name: &str) -> bool {
    name.eq_ignore_ascii_case(CUSTOM_TOKEN)
}

/// Order event names and custom subclasses for the ESL `event` / `nixevent`
/// grammar.
///
/// `CUSTOM` is terminal: FreeSWITCH reads every token after it as a subclass
/// name rather than as an event type. A `CUSTOM` appearing anywhere in `names`
/// is moved to the tail, deduplicated, and emitted in the canonical spelling
/// (event names match case-insensitively on the wire); one is introduced when
/// `subclasses` is non-empty and `names` did not carry it. The relative order
/// of every other name is preserved.
///
/// Pass an empty `subclasses` for a `nixevent` list.
pub fn order_event_tokens<'a>(
    names: impl IntoIterator<Item = &'a str>,
    subclasses: impl IntoIterator<Item = &'a str>,
) -> Vec<&'a str> {
    let mut tokens: Vec<&str> = Vec::new();
    let mut custom = false;

    for name in names {
        if is_custom_token(name) {
            custom = true;
        } else {
            tokens.push(name);
        }
    }

    let mut subclasses = subclasses
        .into_iter()
        .peekable();
    if custom
        || subclasses
            .peek()
            .is_some()
    {
        tokens.push(CUSTOM_TOKEN);
        tokens.extend(subclasses);
    }

    tokens
}

/// Event-type names in a raw `event` / `nixevent` token list that FreeSWITCH
/// will register as `CUSTOM` subclass names instead of subscribing.
///
/// Empty when the list is well ordered or carries no `CUSTOM`. A subclass
/// genuinely named like an event type is indistinguishable from the mistake in
/// a flat token list, so this reports rather than rejects — it exists for the
/// raw string commands, which cannot tell the two apart.
/// [`EventSubscription`] keeps them in separate fields and cannot produce the
/// mistake at all.
///
/// ```rust
/// use freeswitch_types::event::swallowed_event_types;
///
/// assert!(swallowed_event_types("HEARTBEAT CUSTOM sofia::register").is_empty());
/// assert_eq!(
///     swallowed_event_types("CUSTOM sofia::register CHANNEL_CREATE"),
///     ["CHANNEL_CREATE"]
/// );
/// ```
pub fn swallowed_event_types(events: &str) -> Vec<&str> {
    events
        .split_whitespace()
        .skip_while(|t| !is_custom_token(t))
        .skip(1)
        .filter(|t| {
            t.parse::<EslEventType>()
                .is_ok()
        })
        .collect()
}

impl EventSubscription {
    /// Create an empty subscription with the given format.
    pub fn new(format: EventFormat) -> Self {
        Self {
            format,
            events: Vec::new(),
            raw_events: Vec::new(),
            custom_subclasses: Vec::new(),
            filters: Vec::new(),
        }
    }

    /// Create a subscription for all events.
    pub fn all(format: EventFormat) -> Self {
        Self {
            format,
            events: vec![EslEventType::All],
            raw_events: Vec::new(),
            custom_subclasses: Vec::new(),
            filters: Vec::new(),
        }
    }

    /// Add a single event type.
    pub fn event(mut self, event: EslEventType) -> Self {
        self.events
            .push(event);
        self
    }

    /// Add multiple event types (e.g. from group constants like `EslEventType::CHANNEL_EVENTS`).
    pub fn events<T: IntoIterator<Item = impl std::borrow::Borrow<EslEventType>>>(
        mut self,
        events: T,
    ) -> Self {
        self.events
            .extend(
                events
                    .into_iter()
                    .map(|e| *e.borrow()),
            );
        self
    }

    /// Add a single event by wire name.
    ///
    /// Escape hatch for events the [`EslEventType`] enum hasn't yet been
    /// updated to cover. The argument is validated for newline injection,
    /// spaces, and emptiness.
    ///
    /// Raw events appear on the wire alongside typed events when
    /// [`to_event_string()`](Self::to_event_string) is called.
    pub fn event_raw(mut self, event: impl Into<String>) -> Result<Self, EventSubscriptionError> {
        let s = event.into();
        validate_raw_event(&s)?;
        self.raw_events
            .push(s);
        Ok(self)
    }

    /// Add multiple events by wire name.
    ///
    /// Returns `Err` on the first invalid entry.
    pub fn events_raw<I, S>(mut self, events: I) -> Result<Self, EventSubscriptionError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for e in events {
            let s = e.into();
            validate_raw_event(&s)?;
            self.raw_events
                .push(s);
        }
        Ok(self)
    }

    /// Add a custom subclass (e.g. `"sofia::register"`).
    ///
    /// Returns `Err` if the subclass contains spaces, newlines, or is empty.
    pub fn custom_subclass(
        mut self,
        subclass: impl Into<String>,
    ) -> Result<Self, EventSubscriptionError> {
        let s = subclass.into();
        validate_custom_subclass(&s)?;
        self.custom_subclasses
            .push(s);
        Ok(self)
    }

    /// Add multiple custom subclasses.
    ///
    /// Returns `Err` on the first invalid subclass.
    pub fn custom_subclasses(
        mut self,
        subclasses: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, EventSubscriptionError> {
        for s in subclasses {
            let s = s.into();
            validate_custom_subclass(&s)?;
            self.custom_subclasses
                .push(s);
        }
        Ok(self)
    }

    /// Subscribe to a single Sofia event subclass.
    ///
    /// Convenience wrapper around [`custom_subclass()`](Self::custom_subclass) that
    /// accepts a typed [`SofiaEventSubclass`] instead of a raw string.
    pub fn sofia_event(mut self, subclass: SofiaEventSubclass) -> Self {
        self.custom_subclasses
            .push(
                subclass
                    .as_str()
                    .to_string(),
            );
        self
    }

    /// Subscribe to multiple Sofia event subclasses.
    pub fn sofia_events(
        mut self,
        subclasses: impl IntoIterator<Item = impl std::borrow::Borrow<SofiaEventSubclass>>,
    ) -> Self {
        self.custom_subclasses
            .extend(
                subclasses
                    .into_iter()
                    .map(|s| {
                        s.borrow()
                            .as_str()
                            .to_string()
                    }),
            );
        self
    }

    /// Add a filter with a typed header.
    ///
    /// The header enum is always valid; only the value is validated.
    pub fn filter(
        self,
        header: EventHeader,
        value: impl Into<String>,
    ) -> Result<Self, EventSubscriptionError> {
        let v = value.into();
        validate_filter_field(&v)?;
        let mut s = self;
        s.filters
            .push((
                header
                    .as_str()
                    .to_string(),
                v,
            ));
        Ok(s)
    }

    /// Add a filter with raw header and value strings.
    ///
    /// Both header and value are validated against newline injection.
    pub fn filter_raw(
        self,
        header: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, EventSubscriptionError> {
        let h = header.into();
        let v = value.into();
        validate_filter_field(&h)?;
        validate_filter_field(&v)?;
        let mut s = self;
        s.filters
            .push((h, v));
        Ok(s)
    }

    /// Change the event format.
    pub fn with_format(mut self, format: EventFormat) -> Self {
        self.format = format;
        self
    }

    /// The event format.
    pub fn format(&self) -> EventFormat {
        self.format
    }

    /// Mutable reference to the event format.
    pub fn format_mut(&mut self) -> &mut EventFormat {
        &mut self.format
    }

    /// The subscribed event types.
    pub fn event_types(&self) -> &[EslEventType] {
        &self.events
    }

    /// Mutable access to the event types list.
    pub fn event_types_mut(&mut self) -> &mut Vec<EslEventType> {
        &mut self.events
    }

    /// Events subscribed by raw wire name (see [`event_raw`](Self::event_raw)).
    pub fn event_types_raw(&self) -> &[String] {
        &self.raw_events
    }

    /// Mutable access to the raw event list.
    ///
    /// Direct push to this list bypasses [`event_raw`](Self::event_raw)'s
    /// validation. Callers are responsible for ensuring entries contain no
    /// newlines, spaces, or empty strings.
    pub fn event_types_raw_mut(&mut self) -> &mut Vec<String> {
        &mut self.raw_events
    }

    /// The subscribed custom subclasses.
    pub fn custom_subclass_list(&self) -> &[String] {
        &self.custom_subclasses
    }

    /// Mutable access to the custom subclasses list.
    pub fn custom_subclasses_mut(&mut self) -> &mut Vec<String> {
        &mut self.custom_subclasses
    }

    /// The event filters as (header, value) pairs.
    pub fn filters(&self) -> &[(String, String)] {
        &self.filters
    }

    /// Mutable access to the filters list.
    pub fn filters_mut(&mut self) -> &mut Vec<(String, String)> {
        &mut self.filters
    }

    /// Whether the subscription includes all events.
    pub fn is_all(&self) -> bool {
        self.events
            .contains(&EslEventType::All)
    }

    /// Whether the subscription has no events, no raw events, and no
    /// custom subclasses.
    pub fn is_empty(&self) -> bool {
        self.events
            .is_empty()
            && self
                .raw_events
                .is_empty()
            && self
                .custom_subclasses
                .is_empty()
    }

    /// Build the event string for the ESL `event` command.
    ///
    /// Returns `None` if no events, raw events, or custom subclasses are
    /// configured. Returns `Some("ALL")` if `EslEventType::All` is present.
    ///
    /// Otherwise: typed event names then raw event names in the order they were
    /// added, followed by `CUSTOM` and the custom subclasses. The order the
    /// caller added event types in is not load-bearing — see
    /// [`order_event_tokens`].
    pub fn to_event_string(&self) -> Option<String> {
        if self
            .events
            .contains(&EslEventType::All)
        {
            return Some("ALL".to_string());
        }

        let parts = order_event_tokens(
            self.events
                .iter()
                .map(|e| e.as_str())
                .chain(
                    self.raw_events
                        .iter()
                        .map(|s| s.as_str()),
                ),
            self.custom_subclasses
                .iter()
                .map(|s| s.as_str()),
        );

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
    }
}

#[cfg(feature = "serde")]
mod event_subscription_serde {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct EventSubscriptionRaw {
        format: EventFormat,
        #[serde(default)]
        events: Vec<EslEventType>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        raw_events: Vec<String>,
        #[serde(default)]
        custom_subclasses: Vec<String>,
        #[serde(default)]
        filters: Vec<(String, String)>,
    }

    impl TryFrom<EventSubscriptionRaw> for EventSubscription {
        type Error = EventSubscriptionError;

        fn try_from(raw: EventSubscriptionRaw) -> Result<Self, Self::Error> {
            for re in &raw.raw_events {
                validate_raw_event(re)?;
            }
            for sc in &raw.custom_subclasses {
                validate_custom_subclass(sc)?;
            }
            for (h, v) in &raw.filters {
                validate_filter_field(h)?;
                validate_filter_field(v)?;
            }
            Ok(EventSubscription {
                format: raw.format,
                events: raw.events,
                raw_events: raw.raw_events,
                custom_subclasses: raw.custom_subclasses,
                filters: raw.filters,
            })
        }
    }

    impl Serialize for EventSubscription {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let raw = EventSubscriptionRaw {
                format: self.format,
                events: self
                    .events
                    .clone(),
                raw_events: self
                    .raw_events
                    .clone(),
                custom_subclasses: self
                    .custom_subclasses
                    .clone(),
                filters: self
                    .filters
                    .clone(),
            };
            raw.serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for EventSubscription {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let raw = EventSubscriptionRaw::deserialize(deserializer)?;
            EventSubscription::try_from(raw).map_err(serde::de::Error::custom)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_empty() {
        let sub = EventSubscription::new(EventFormat::Plain);
        assert!(sub.is_empty());
        assert!(!sub.is_all());
        assert_eq!(sub.format(), EventFormat::Plain);
        assert!(sub
            .event_types()
            .is_empty());
        assert!(sub
            .custom_subclass_list()
            .is_empty());
        assert!(sub
            .filters()
            .is_empty());
    }

    #[test]
    fn all_creates_all() {
        let sub = EventSubscription::all(EventFormat::Json);
        assert!(sub.is_all());
        assert!(!sub.is_empty());
        assert_eq!(sub.to_event_string(), Some("ALL".to_string()));
    }

    #[test]
    fn event_string_typed_only() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event(EslEventType::ChannelCreate)
            .event(EslEventType::ChannelAnswer);
        assert_eq!(
            sub.to_event_string(),
            Some("CHANNEL_CREATE CHANNEL_ANSWER".to_string())
        );
    }

    #[test]
    fn event_string_custom_only() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .custom_subclass("sofia::register")
            .unwrap()
            .custom_subclass("sofia::unregister")
            .unwrap();
        assert_eq!(
            sub.to_event_string(),
            Some("CUSTOM sofia::register sofia::unregister".to_string())
        );
    }

    #[test]
    fn event_string_mixed() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event(EslEventType::Heartbeat)
            .custom_subclass("sofia::register")
            .unwrap();
        assert_eq!(
            sub.to_event_string(),
            Some("HEARTBEAT CUSTOM sofia::register".to_string())
        );
    }

    #[test]
    fn event_string_custom_not_duplicated() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event(EslEventType::Custom)
            .custom_subclass("sofia::register")
            .unwrap();
        // Should not have "CUSTOM" twice
        assert_eq!(
            sub.to_event_string(),
            Some("CUSTOM sofia::register".to_string())
        );
    }

    #[test]
    fn order_event_tokens_moves_custom_to_tail() {
        assert_eq!(
            order_event_tokens(["CUSTOM", "CHANNEL_CREATE", "HEARTBEAT"], []),
            ["CHANNEL_CREATE", "HEARTBEAT", "CUSTOM"]
        );
    }

    #[test]
    fn order_event_tokens_without_custom_is_identity() {
        assert_eq!(
            order_event_tokens(["CHANNEL_CREATE", "HEARTBEAT"], []),
            ["CHANNEL_CREATE", "HEARTBEAT"]
        );
    }

    #[test]
    fn order_event_tokens_introduces_custom_for_subclasses() {
        assert_eq!(
            order_event_tokens(["HEARTBEAT"], ["sofia::register"]),
            ["HEARTBEAT", "CUSTOM", "sofia::register"]
        );
    }

    #[test]
    fn order_event_tokens_keeps_bare_custom_without_subclasses() {
        assert_eq!(order_event_tokens(["CUSTOM"], []), ["CUSTOM"]);
    }

    #[test]
    fn order_event_tokens_empty_is_empty() {
        assert!(order_event_tokens([], []).is_empty());
    }

    #[test]
    fn swallowed_event_types_reports_a_type_after_custom() {
        assert_eq!(
            swallowed_event_types("HEARTBEAT CUSTOM sofia::register CHANNEL_CREATE"),
            ["CHANNEL_CREATE"]
        );
    }

    #[test]
    fn swallowed_event_types_empty_when_well_ordered() {
        assert!(swallowed_event_types("HEARTBEAT CUSTOM sofia::register").is_empty());
    }

    #[test]
    fn swallowed_event_types_empty_without_custom() {
        assert!(swallowed_event_types("HEARTBEAT CHANNEL_CREATE").is_empty());
    }

    #[test]
    fn swallowed_event_types_matches_custom_case_insensitively() {
        assert_eq!(swallowed_event_types("custom HEARTBEAT"), ["HEARTBEAT"]);
    }

    #[test]
    fn custom_first_serialises_last() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event(EslEventType::Custom)
            .event(EslEventType::ChannelCreate)
            .event(EslEventType::ChannelAnswer);
        assert_eq!(
            sub.to_event_string(),
            Some("CHANNEL_CREATE CHANNEL_ANSWER CUSTOM".to_string())
        );
    }

    #[test]
    fn custom_in_middle_serialises_last() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event(EslEventType::Heartbeat)
            .event(EslEventType::Custom)
            .event(EslEventType::ChannelCreate);
        assert_eq!(
            sub.to_event_string(),
            Some("HEARTBEAT CHANNEL_CREATE CUSTOM".to_string())
        );
    }

    #[test]
    fn custom_and_subclasses_follow_every_event_type() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event(EslEventType::Heartbeat)
            .event(EslEventType::BackgroundJob)
            .event(EslEventType::Custom)
            .event(EslEventType::Unpublish)
            .event(EslEventType::ChannelCreate)
            .custom_subclass("sofia::gateway_state")
            .unwrap();
        assert_eq!(
            sub.to_event_string(),
            Some(
                "HEARTBEAT BACKGROUND_JOB UNPUBLISH CHANNEL_CREATE CUSTOM sofia::gateway_state"
                    .to_string()
            )
        );
    }

    #[test]
    fn raw_events_order_before_custom() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event(EslEventType::Custom)
            .event_raw("FUTURE_EVENT")
            .unwrap()
            .custom_subclass("sofia::register")
            .unwrap();
        assert_eq!(
            sub.to_event_string(),
            Some("FUTURE_EVENT CUSTOM sofia::register".to_string())
        );
    }

    /// A raw `CUSTOM` is the terminal marker, not a plain name. FreeSWITCH
    /// matches event names case-insensitively, so canonicalising is lossless.
    #[test]
    fn raw_custom_token_is_the_terminal_marker() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event(EslEventType::Custom)
            .event_raw("custom")
            .unwrap()
            .event_raw("CHANNEL_CREATE")
            .unwrap();
        assert_eq!(
            sub.to_event_string(),
            Some("CHANNEL_CREATE CUSTOM".to_string())
        );
    }

    #[test]
    fn order_preserved_without_custom() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event(EslEventType::Heartbeat)
            .event(EslEventType::ChannelAnswer)
            .event(EslEventType::ChannelCreate)
            .event(EslEventType::BackgroundJob);
        assert_eq!(
            sub.to_event_string(),
            Some("HEARTBEAT CHANNEL_ANSWER CHANNEL_CREATE BACKGROUND_JOB".to_string())
        );
    }

    #[test]
    fn all_wins_from_any_position() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event(EslEventType::Custom)
            .event(EslEventType::ChannelCreate)
            .event(EslEventType::All)
            .custom_subclass("sofia::register")
            .unwrap();
        assert_eq!(sub.to_event_string(), Some("ALL".to_string()));
    }

    /// The property FreeSWITCH's grammar requires, asserted against
    /// `event_types()` rather than against the emitted tokens: a subclass may
    /// legitimately be named like an event type, and the flat string cannot
    /// tell the two apart.
    #[test]
    fn no_requested_event_type_lands_after_custom() {
        let subs = [
            EventSubscription::new(EventFormat::Plain)
                .event(EslEventType::Custom)
                .event(EslEventType::ChannelCreate),
            EventSubscription::new(EventFormat::Plain)
                .event(EslEventType::Heartbeat)
                .event(EslEventType::Custom)
                .event(EslEventType::Unpublish)
                .custom_subclass("sofia::register")
                .unwrap(),
            EventSubscription::new(EventFormat::Plain)
                .events(EslEventType::CHANNEL_EVENTS)
                .event(EslEventType::Custom)
                .sofia_events(SofiaEventSubclass::GATEWAY_EVENTS),
        ];

        for sub in subs {
            let out = sub
                .to_event_string()
                .unwrap();
            let tokens: Vec<&str> = out
                .split(' ')
                .collect();
            let custom_at = tokens
                .iter()
                .position(|t| *t == "CUSTOM")
                .expect("CUSTOM present");
            for want in sub.event_types() {
                if *want == EslEventType::Custom {
                    continue;
                }
                let at = tokens
                    .iter()
                    .position(|t| *t == want.as_str())
                    .unwrap_or_else(|| panic!("{} missing from {out:?}", want.as_str()));
                assert!(
                    at < custom_at,
                    "{} lands after CUSTOM in {out:?}",
                    want.as_str()
                );
            }
        }
    }

    #[test]
    fn event_string_empty_is_none() {
        let sub = EventSubscription::new(EventFormat::Plain);
        assert_eq!(sub.to_event_string(), None);
    }

    #[test]
    fn filters_preserve_order() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .filter(EventHeader::CallDirection, "inbound")
            .unwrap()
            .filter_raw("X-Custom", "value1")
            .unwrap()
            .filter(EventHeader::ChannelState, "CS_EXECUTE")
            .unwrap();
        assert_eq!(
            sub.filters(),
            &[
                ("Call-Direction".to_string(), "inbound".to_string()),
                ("X-Custom".to_string(), "value1".to_string()),
                ("Channel-State".to_string(), "CS_EXECUTE".to_string()),
            ]
        );
    }

    #[test]
    fn builder_chain() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .events(EslEventType::CHANNEL_EVENTS)
            .event(EslEventType::Heartbeat)
            .custom_subclass("sofia::register")
            .unwrap()
            .filter(EventHeader::CallDirection, "inbound")
            .unwrap()
            .with_format(EventFormat::Json);

        assert_eq!(sub.format(), EventFormat::Json);
        assert!(!sub.is_empty());
        assert!(!sub.is_all());
        assert!(sub
            .event_types()
            .contains(&EslEventType::ChannelCreate));
        assert!(sub
            .event_types()
            .contains(&EslEventType::Heartbeat));
        assert_eq!(sub.custom_subclass_list(), &["sofia::register"]);
        assert_eq!(
            sub.filters()
                .len(),
            1
        );
    }

    #[test]
    fn serde_round_trip_subscription() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event(EslEventType::ChannelCreate)
            .event(EslEventType::Heartbeat)
            .custom_subclass("sofia::register")
            .unwrap()
            .filter(EventHeader::CallDirection, "inbound")
            .unwrap();

        let json = serde_json::to_string(&sub).unwrap();
        let deserialized: EventSubscription = serde_json::from_str(&json).unwrap();
        assert_eq!(sub, deserialized);
    }

    #[test]
    fn serde_rejects_invalid_subclass() {
        let json =
            r#"{"format":"Plain","events":[],"custom_subclasses":["bad subclass"],"filters":[]}"#;
        let result: Result<EventSubscription, _> = serde_json::from_str(json);
        assert!(result.is_err());
        let err = result
            .unwrap_err()
            .to_string();
        assert!(err.contains("space"), "error should mention space: {err}");
    }

    #[test]
    fn serde_rejects_newline_in_filter() {
        let json = r#"{"format":"Plain","events":[],"custom_subclasses":[],"filters":[["Header","val\n"]]}"#;
        let result: Result<EventSubscription, _> = serde_json::from_str(json);
        assert!(result.is_err());
        let err = result
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("newline"),
            "error should mention newline: {err}"
        );
    }

    #[test]
    fn error_display_names_the_character_class_only() {
        let err = EventSubscription::new(EventFormat::Plain)
            .custom_subclass("bad subclass")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "event subscription token contains a space (12 bytes)"
        );
        assert_eq!(err.0, "bad subclass");

        let err = EventSubscription::new(EventFormat::Plain)
            .custom_subclass("")
            .unwrap_err();
        assert_eq!(err.to_string(), "event subscription token is empty");

        let err = EventSubscription::new(EventFormat::Plain)
            .filter_raw("Header", "val\n")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "event subscription token contains a newline (4 bytes)"
        );
        assert_eq!(err.0, "val\n");
    }

    #[test]
    fn custom_subclass_rejects_space() {
        let result = EventSubscription::new(EventFormat::Plain).custom_subclass("bad subclass");
        assert!(result.is_err());
    }

    #[test]
    fn custom_subclass_rejects_newline() {
        let result = EventSubscription::new(EventFormat::Plain).custom_subclass("bad\nsubclass");
        assert!(result.is_err());
    }

    #[test]
    fn custom_subclass_rejects_empty() {
        let result = EventSubscription::new(EventFormat::Plain).custom_subclass("");
        assert!(result.is_err());
    }

    #[test]
    fn filter_raw_rejects_newline_in_header() {
        let result = EventSubscription::new(EventFormat::Plain).filter_raw("Bad\nHeader", "value");
        assert!(result.is_err());
    }

    #[test]
    fn filter_raw_rejects_newline_in_value() {
        let result = EventSubscription::new(EventFormat::Plain).filter_raw("Header", "bad\nvalue");
        assert!(result.is_err());
    }

    #[test]
    fn filter_raw_rejects_nul() {
        assert!(EventSubscription::new(EventFormat::Plain)
            .filter_raw("Header", "bad\0value")
            .is_err());
        assert!(EventSubscription::new(EventFormat::Plain)
            .filter_raw("Bad\0Header", "value")
            .is_err());
    }

    #[test]
    fn filter_typed_rejects_newline_in_value() {
        let result = EventSubscription::new(EventFormat::Plain)
            .filter(EventHeader::CallDirection, "bad\nvalue");
        assert!(result.is_err());
    }

    #[test]
    fn sofia_event_single() {
        let sub =
            EventSubscription::new(EventFormat::Plain).sofia_event(SofiaEventSubclass::Register);
        assert_eq!(
            sub.to_event_string(),
            Some("CUSTOM sofia::register".to_string())
        );
    }

    #[test]
    fn sofia_events_group() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .sofia_events(SofiaEventSubclass::GATEWAY_EVENTS);
        let event_str = sub
            .to_event_string()
            .unwrap();
        assert!(event_str.starts_with("CUSTOM"));
        assert!(event_str.contains("sofia::gateway_state"));
        assert!(event_str.contains("sofia::gateway_add"));
        assert!(event_str.contains("sofia::gateway_delete"));
        assert!(event_str.contains("sofia::gateway_invalid_digest_req"));
    }

    #[test]
    fn event_raw_wire_string() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event(EslEventType::Heartbeat)
            .event_raw("NEW_EVENT_NOT_IN_ENUM")
            .unwrap();
        assert_eq!(
            sub.to_event_string(),
            Some("HEARTBEAT NEW_EVENT_NOT_IN_ENUM".to_string())
        );
    }

    #[test]
    fn events_raw_wire_string() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .events_raw(["FUTURE_A", "FUTURE_B"])
            .unwrap();
        assert_eq!(sub.to_event_string(), Some("FUTURE_A FUTURE_B".to_string()));
    }

    #[test]
    fn event_raw_with_custom_subclass() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event_raw("NEW_EVENT")
            .unwrap()
            .custom_subclass("sofia::register")
            .unwrap();
        assert_eq!(
            sub.to_event_string(),
            Some("NEW_EVENT CUSTOM sofia::register".to_string())
        );
    }

    #[test]
    fn event_raw_rejects_newline() {
        assert!(EventSubscription::new(EventFormat::Plain)
            .event_raw("bad\nevent")
            .is_err());
    }

    #[test]
    fn event_raw_rejects_space() {
        assert!(EventSubscription::new(EventFormat::Plain)
            .event_raw("bad event")
            .is_err());
    }

    #[test]
    fn event_raw_rejects_empty() {
        assert!(EventSubscription::new(EventFormat::Plain)
            .event_raw("")
            .is_err());
    }

    #[test]
    fn events_raw_errors_on_first_invalid() {
        let result =
            EventSubscription::new(EventFormat::Plain).events_raw(["GOOD", "bad event", "OTHER"]);
        assert!(result.is_err());
    }

    #[test]
    fn event_types_raw_mut_mutable() {
        let mut sub = EventSubscription::new(EventFormat::Plain);
        sub.event_types_raw_mut()
            .push("DIRECT_PUSH".to_string());
        assert_eq!(sub.event_types_raw(), &["DIRECT_PUSH".to_string()]);
    }

    #[test]
    fn is_empty_sees_raw_events() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event_raw("ONLY_RAW")
            .unwrap();
        assert!(!sub.is_empty());
    }

    #[test]
    fn serde_round_trip_with_raw_events() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event(EslEventType::ChannelCreate)
            .event_raw("FUTURE_EVENT")
            .unwrap()
            .custom_subclass("sofia::register")
            .unwrap();

        let json = serde_json::to_string(&sub).unwrap();
        let deserialized: EventSubscription = serde_json::from_str(&json).unwrap();
        assert_eq!(sub, deserialized);
    }

    #[test]
    fn serde_rejects_invalid_raw_event() {
        let json = r#"{"format":"Plain","events":[],"raw_events":["bad event"],"custom_subclasses":[],"filters":[]}"#;
        let result: Result<EventSubscription, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn serde_missing_raw_events_field_defaults_to_empty() {
        // Back-compat: configs written before raw_events was added must still
        // deserialize.
        let json =
            r#"{"format":"Plain","events":["Heartbeat"],"custom_subclasses":[],"filters":[]}"#;
        let sub: EventSubscription = serde_json::from_str(json).unwrap();
        assert!(sub
            .event_types_raw()
            .is_empty());
    }

    #[test]
    fn sofia_event_mixed_with_typed_events() {
        let sub = EventSubscription::new(EventFormat::Plain)
            .event(EslEventType::Heartbeat)
            .sofia_event(SofiaEventSubclass::GatewayState);
        assert_eq!(
            sub.to_event_string(),
            Some("HEARTBEAT CUSTOM sofia::gateway_state".to_string())
        );
    }
}
