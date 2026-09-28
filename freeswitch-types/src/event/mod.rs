//! ESL event types and structures

mod event_type;
mod format;
mod subscription;

pub use event_type::{EslEventType, ParseEventTypeError};
pub use format::{EventFormat, ParseEventFormatError};
pub use subscription::{
    order_event_tokens, swallowed_event_types, EventSubscription, EventSubscriptionError,
};

use crate::headers::EventHeader;
use crate::lookup::{variable_key, HeaderLookup};
use crate::lossy_values::LossyValues;
use crate::variables::{EslArray, EslArrayError, EslHeaders};
use indexmap::IndexMap;
use percent_encoding::{percent_encode, NON_ALPHANUMERIC};

wire_enum! {
    /// Event priority levels matching FreeSWITCH `esl_priority_t`
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum EslEventPriority {
        /// Default priority.
        Normal => "NORMAL",
        /// Lower than normal.
        Low => "LOW",
        /// Higher than normal.
        High => "HIGH",
    }
    error ParsePriorityError("priority");
    tests: esl_event_priority_wire_tests;
}

/// ESL Event structure containing headers and optional body
// qual:allow(srp, god_struct) reason: "public data carrier; accessors read disjoint fields"
#[derive(Debug, Clone, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct EslEvent {
    headers: EslHeaders,
    body: Option<String>,
    /// Exact wire bytes of a body that was not valid UTF-8; `body` then
    /// holds the U+FFFD-substituted string. `None` in the normal case.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    raw_body: Option<Vec<u8>>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "LossyValues::is_empty")
    )]
    lossy_values: LossyValues,
}

impl EslEvent {
    /// Create a new empty event
    pub fn new() -> Self {
        Self {
            headers: EslHeaders::new(),
            body: None,
            raw_body: None,
            lossy_values: LossyValues::default(),
        }
    }

    /// Create event with the `Event-Name` header set to the given type's
    /// wire name. The event type is derived lazily from this header on
    /// every [`event_type()`](Self::event_type) call — there is no
    /// separate `event_type` field.
    pub fn with_type(event_type: EslEventType) -> Self {
        let mut event = Self::new();
        event.set_header(EventHeader::EventName.as_str(), event_type.as_str());
        event
    }

    /// Parsed event type, derived from the `Event-Name` header.
    ///
    /// Returns `None` if the header is missing or carries a value that
    /// is not a recognized [`EslEventType`] variant. Single source of
    /// truth: the header. Mutating `Event-Name` via `set_header` will
    /// be reflected on the next call.
    pub fn event_type(&self) -> Option<EslEventType> {
        self.header(EventHeader::EventName)
            .and_then(EslEventType::parse_event_type)
    }

    /// Look up a header by its [`EventHeader`] enum variant.
    ///
    /// For headers not covered by `EventHeader`, use [`header_str()`](Self::header_str).
    pub fn header(&self, name: EventHeader) -> Option<&str> {
        HeaderLookup::header(self, name)
    }

    /// Look up a header by name, in any casing the switch might have spelled
    /// it.
    ///
    /// Use [`header()`](Self::header) with an [`EventHeader`] variant for known
    /// headers. This method is for headers not (yet) covered by the enum,
    /// such as custom `X-` headers or FreeSWITCH headers added after this
    /// library was published.
    pub fn header_str(&self, name: &str) -> Option<&str> {
        self.headers
            .header_str(name)
    }

    /// Look up a channel variable by its bare name.
    ///
    /// Equivalent to [`variable()`](Self::variable) but matches the
    /// [`HeaderLookup`] trait signature.
    pub fn variable_str(&self, name: &str) -> Option<&str> {
        self.header_str(&variable_key(name))
    }

    /// All headers as a map, keyed canonically.
    pub fn headers(&self) -> &IndexMap<String, String> {
        self.headers
            .as_map()
    }

    /// Set or overwrite a header, normalizing the key.
    pub fn set_header(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.headers
            .insert(name, value);
    }

    /// Remove a header, returning its value if it existed.
    ///
    /// Accepts both canonical and original (non-normalized) key names.
    pub fn remove_header(&mut self, name: impl AsRef<str>) -> Option<String> {
        self.headers
            .remove(name.as_ref())
    }

    /// Event body (the content after the blank line in plain-text events).
    pub fn body(&self) -> Option<&str> {
        self.body
            .as_deref()
    }

    /// Set the event body.
    pub fn set_body(&mut self, body: impl Into<String>) {
        self.body = Some(body.into());
    }

    /// Exact wire bytes of the event body when it was not valid UTF-8.
    ///
    /// `Some` is the lossy signal: [`body()`](Self::body) then holds the
    /// U+FFFD-substituted string and these are the original payload bytes
    /// (e.g. a Latin-1 SMS body), so the app can re-decode or audit them.
    /// For plain and log events these are the inner body bytes. For JSON/XML
    /// events, wire bytes cannot be mapped back to the decoded body, so this
    /// carries the whole event envelope body (the serialized JSON/XML
    /// document as sent on the wire) — the signal is still observable.
    /// `None` in the normal case.
    pub fn raw_body(&self) -> Option<&[u8]> {
        self.raw_body
            .as_deref()
    }

    /// Set the raw body bytes.
    ///
    /// Used internally by the ESL parser; consumers don't call this directly.
    #[doc(hidden)]
    pub fn set_raw_body(&mut self, bytes: Vec<u8>) {
        self.raw_body = Some(bytes);
    }

    /// Headers whose percent-decoded value contained invalid UTF-8 and was
    /// decoded lossily (U+FFFD substituted).
    ///
    /// Each entry carries the on-wire `raw_value()` (the percent-encoded source
    /// text) so the app can re-decode it (e.g. as Latin-1) or audit it instead
    /// of being stuck with the U+FFFD-substituted string in `headers`. Empty in
    /// the normal case.
    pub fn lossy_values(&self) -> &LossyValues {
        &self.lossy_values
    }

    /// Set the lossy values.
    ///
    /// Used internally by the ESL parser; consumers don't call this directly.
    #[doc(hidden)]
    pub fn set_lossy_values(&mut self, v: LossyValues) {
        self.lossy_values = v;
    }

    /// Sets the `priority` header carried on the event.
    ///
    /// FreeSWITCH stores this as metadata but does **not** use it for dispatch
    /// ordering -- all events are delivered FIFO regardless of priority.
    pub fn set_priority(&mut self, priority: EslEventPriority) {
        self.set_header(EventHeader::Priority.as_str(), priority.to_string());
    }

    /// Append a value to a multi-value header (PUSH semantics).
    ///
    /// If the header doesn't exist, sets it as a plain value.
    /// If it exists as a plain value, converts to `ARRAY::old|:new`.
    /// If it already has an `ARRAY::` prefix, appends the new value.
    ///
    /// Returns [`EslArrayError::TooManyItems`] if the existing header already
    /// contains [`MAX_ARRAY_ITEMS`](crate::MAX_ARRAY_ITEMS) items.
    ///
    /// ```
    /// # use freeswitch_types::EslEvent;
    /// let mut event = EslEvent::new();
    /// event.push_header("X-Test", "first").unwrap();
    /// event.push_header("X-Test", "second").unwrap();
    /// assert_eq!(event.header_str("X-Test"), Some("ARRAY::first|:second"));
    /// ```
    pub fn push_header(&mut self, name: &str, value: &str) -> Result<(), EslArrayError> {
        self.stack_header(name, value, EslArray::push)
    }

    /// Prepend a value to a multi-value header (UNSHIFT semantics).
    ///
    /// Same conversion rules as [`push_header()`](Self::push_header), but
    /// inserts at the front.
    ///
    /// ```
    /// # use freeswitch_types::EslEvent;
    /// let mut event = EslEvent::new();
    /// event.set_header("X-Test", "ARRAY::b|:c");
    /// event.unshift_header("X-Test", "a").unwrap();
    /// assert_eq!(event.header_str("X-Test"), Some("ARRAY::a|:b|:c"));
    /// ```
    pub fn unshift_header(&mut self, name: &str, value: &str) -> Result<(), EslArrayError> {
        self.stack_header(name, value, EslArray::unshift)
    }

    fn stack_header(
        &mut self,
        name: &str,
        value: &str,
        op: fn(&mut EslArray, String),
    ) -> Result<(), EslArrayError> {
        match self
            .header_str(name)
            .map(str::to_string)
        {
            None => {
                self.set_header(name, value);
            }
            Some(existing) => {
                let arr = match EslArray::parse(&existing) {
                    Ok(arr) => arr,
                    Err(EslArrayError::MissingPrefix) => EslArray::new(vec![existing]),
                    Err(e) => return Err(e),
                };
                if arr.len() >= crate::variables::MAX_ARRAY_ITEMS {
                    return Err(EslArrayError::TooManyItems {
                        count: arr.len(),
                        max: crate::variables::MAX_ARRAY_ITEMS,
                    });
                }
                let mut arr = arr;
                op(&mut arr, value.into());
                self.set_header(name, arr.to_string());
            }
        }
        Ok(())
    }

    /// Check whether this event matches the given type.
    pub fn is_event_type(&self, event_type: EslEventType) -> bool {
        self.event_type() == Some(event_type)
    }

    /// Serialize to ESL plain text wire format with percent-encoded header values.
    ///
    /// This is the inverse of `EslParser::parse_plain_event()`. The output can
    /// be fed back through the parser to reconstruct an equivalent `EslEvent`
    /// (round-trip).
    ///
    /// Headers are emitted in insertion order (which matches wire order when the
    /// event was parsed from the network). `Content-Length` from stored headers
    /// is skipped and recomputed from the body if present.
    pub fn to_plain_format(&self) -> String {
        let mut result = String::new();

        for (key, value) in self.headers() {
            if key == "Content-Length" {
                continue;
            }
            result.extend([key.as_str(), ": "]);
            result.extend(percent_encode(value.as_bytes(), NON_ALPHANUMERIC));
            result.push('\n');
        }

        if let Some(body) = &self.body {
            result.extend([
                "Content-Length: ",
                &body
                    .len()
                    .to_string(),
                "\n\n",
            ]);
            result.push_str(body);
        } else {
            result.push('\n');
        }

        result
    }
}

impl Default for EslEvent {
    fn default() -> Self {
        Self::new()
    }
}

impl HeaderLookup for EslEvent {
    fn header_str(&self, name: &str) -> Option<&str> {
        EslEvent::header_str(self, name)
    }

    fn variable_str(&self, name: &str) -> Option<&str> {
        EslEvent::variable_str(self, name)
    }
}

impl sip_header::SipHeaderLookup for EslEvent {
    fn sip_header_str(&self, name: &str) -> Option<&str> {
        EslEvent::header_str(self, name)
    }

    crate::esl_sip_header_overrides!();
}

impl PartialEq for EslEvent {
    fn eq(&self, other: &Self) -> bool {
        self.headers == other.headers && self.body == other.body
    }
}

impl std::hash::Hash for EslEvent {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        for (k, v) in self.headers() {
            k.hash(state);
            v.hash(state);
        }
        self.body
            .hash(state);
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for EslEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Raw {
            headers: EslHeaders,
            body: Option<String>,
            #[serde(default)]
            raw_body: Option<Vec<u8>>,
            #[serde(default)]
            lossy_values: LossyValues,
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(EslEvent {
            headers: raw.headers,
            body: raw.body,
            raw_body: raw.raw_body,
            lossy_values: raw.lossy_values,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_preserve_insertion_order() {
        let mut event = EslEvent::new();
        event.set_header("Zebra", "last");
        event.set_header("Alpha", "first");
        event.set_header("Middle", "mid");
        let keys: Vec<&str> = event
            .headers()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(keys, vec!["Zebra", "Alpha", "Middle"]);
    }

    #[test]
    fn test_remove_header() {
        let mut event = EslEvent::new();
        event.set_header("Foo", "bar");
        event.set_header("Baz", "qux");

        let removed = event.remove_header("Foo");
        assert_eq!(removed, Some("bar".to_string()));
        assert!(event
            .header_str("Foo")
            .is_none());
        assert_eq!(event.header_str("Baz"), Some("qux"));

        let removed_again = event.remove_header("Foo");
        assert_eq!(removed_again, None);
    }

    #[test]
    fn test_to_plain_format_basic() {
        let mut event = EslEvent::with_type(EslEventType::Heartbeat);
        event.set_header("Event-Name", "HEARTBEAT");
        event.set_header("Core-UUID", "abc-123");

        let plain = event.to_plain_format();

        assert!(plain.starts_with("Event-Name: "));
        assert!(plain.contains("Core-UUID: "));
        assert!(plain.ends_with("\n\n"));
    }

    #[test]
    fn test_to_plain_format_percent_encoding() {
        let mut event = EslEvent::with_type(EslEventType::Heartbeat);
        event.set_header("Event-Name", "HEARTBEAT");
        event.set_header("Up-Time", "0 years, 0 days");

        let plain = event.to_plain_format();

        assert!(!plain.contains("0 years, 0 days"));
        assert!(plain.contains("Up-Time: "));
        assert!(plain.contains("%20"));
    }

    #[test]
    fn test_to_plain_format_with_body() {
        let mut event = EslEvent::with_type(EslEventType::BackgroundJob);
        event.set_header("Event-Name", "BACKGROUND_JOB");
        event.set_header("Job-UUID", "def-456");
        event.set_body("+OK result\n".to_string());

        let plain = event.to_plain_format();

        assert!(plain.contains("Content-Length: 11\n"));
        assert!(plain.ends_with("\n\n+OK result\n"));
    }

    #[test]
    fn test_to_plain_format_preserves_insertion_order() {
        let mut event = EslEvent::with_type(EslEventType::Heartbeat);
        event.set_header("Event-Name", "HEARTBEAT");
        event.set_header("Core-UUID", "abc-123");
        event.set_header("FreeSWITCH-Hostname", "fs01");
        event.set_header("Up-Time", "0 years, 1 day");

        let plain = event.to_plain_format();
        let lines: Vec<&str> = plain
            .lines()
            .collect();
        assert!(lines[0].starts_with("Event-Name: "));
        assert!(lines[1].starts_with("Core-UUID: "));
        assert!(lines[2].starts_with("FreeSWITCH-Hostname: "));
        assert!(lines[3].starts_with("Up-Time: "));
    }

    #[test]
    fn test_set_priority_normal() {
        let mut event = EslEvent::new();
        event.set_priority(EslEventPriority::Normal);
        assert_eq!(
            event
                .priority()
                .unwrap(),
            Some(EslEventPriority::Normal)
        );
        assert_eq!(event.header(EventHeader::Priority), Some("NORMAL"));
    }

    #[test]
    fn test_set_priority_high() {
        let mut event = EslEvent::new();
        event.set_priority(EslEventPriority::High);
        assert_eq!(
            event
                .priority()
                .unwrap(),
            Some(EslEventPriority::High)
        );
        assert_eq!(event.header(EventHeader::Priority), Some("HIGH"));
    }

    #[test]
    fn test_push_header_new() {
        let mut event = EslEvent::new();
        event
            .push_header("X-Test", "first")
            .unwrap();
        assert_eq!(event.header_str("X-Test"), Some("first"));
    }

    #[test]
    fn test_push_header_existing_plain() {
        let mut event = EslEvent::new();
        event.set_header("X-Test", "first");
        event
            .push_header("X-Test", "second")
            .unwrap();
        assert_eq!(event.header_str("X-Test"), Some("ARRAY::first|:second"));
    }

    #[test]
    fn test_push_header_existing_array() {
        let mut event = EslEvent::new();
        event.set_header("X-Test", "ARRAY::a|:b");
        event
            .push_header("X-Test", "c")
            .unwrap();
        assert_eq!(event.header_str("X-Test"), Some("ARRAY::a|:b|:c"));
    }

    #[test]
    fn test_push_header_at_capacity() {
        use crate::variables::MAX_ARRAY_ITEMS;
        let mut event = EslEvent::new();
        let items: Vec<&str> = (0..MAX_ARRAY_ITEMS)
            .map(|_| "x")
            .collect();
        event.set_header("X-Test", format!("ARRAY::{}", items.join("|:")).as_str());
        assert!(matches!(
            event.push_header("X-Test", "overflow"),
            Err(EslArrayError::TooManyItems { .. })
        ));
    }

    #[test]
    fn test_unshift_header_new() {
        let mut event = EslEvent::new();
        event
            .unshift_header("X-Test", "only")
            .unwrap();
        assert_eq!(event.header_str("X-Test"), Some("only"));
    }

    #[test]
    fn test_unshift_header_existing_array() {
        let mut event = EslEvent::new();
        event.set_header("X-Test", "ARRAY::b|:c");
        event
            .unshift_header("X-Test", "a")
            .unwrap();
        assert_eq!(event.header_str("X-Test"), Some("ARRAY::a|:b|:c"));
    }

    #[test]
    fn test_sendevent_with_priority_wire_format() {
        let mut event = EslEvent::with_type(EslEventType::Custom);
        event.set_header("Event-Name", "CUSTOM");
        event.set_header("Event-Subclass", "test::priority");
        event.set_priority(EslEventPriority::High);

        let plain = event.to_plain_format();
        assert!(plain.contains("priority: HIGH\n"));
    }

    #[test]
    fn test_convenience_accessors() {
        let mut event = EslEvent::new();
        event.set_header("Channel-Name", "sofia/internal/1000@example.com");
        event.set_header("Caller-Caller-ID-Number", "1000");
        event.set_header("Caller-Caller-ID-Name", "Alice");
        event.set_header("Hangup-Cause", "NORMAL_CLEARING");
        event.set_header("Event-Subclass", "sofia::register");
        event.set_header("variable_sip_from_display", "Bob");

        assert_eq!(
            event.channel_name(),
            Some("sofia/internal/1000@example.com")
        );
        assert_eq!(event.caller_id_number(), Some("1000"));
        assert_eq!(event.caller_id_name(), Some("Alice"));
        assert_eq!(
            event
                .hangup_cause()
                .unwrap(),
            Some(crate::channel::HangupCause::NormalClearing)
        );
        assert_eq!(event.event_subclass(), Some("sofia::register"));
        assert_eq!(event.variable_str("sip_from_display"), Some("Bob"));
        assert_eq!(event.variable_str("nonexistent"), None);
    }

    // --- EslEvent accessor tests (via HeaderLookup trait) ---

    #[test]
    fn test_event_channel_state_accessor() {
        use crate::channel::ChannelState;
        let mut event = EslEvent::new();
        event.set_header("Channel-State", "CS_EXECUTE");
        assert_eq!(
            event
                .channel_state()
                .unwrap(),
            Some(ChannelState::CsExecute)
        );
    }

    #[test]
    fn test_event_channel_state_number_accessor() {
        use crate::channel::ChannelState;
        let mut event = EslEvent::new();
        event.set_header("Channel-State-Number", "4");
        assert_eq!(
            event
                .channel_state_number()
                .unwrap(),
            Some(ChannelState::CsExecute)
        );
    }

    #[test]
    fn test_event_call_state_accessor() {
        use crate::channel::CallState;
        let mut event = EslEvent::new();
        event.set_header("Channel-Call-State", "ACTIVE");
        assert_eq!(
            event
                .call_state()
                .unwrap(),
            Some(CallState::Active)
        );
    }

    #[test]
    fn test_event_answer_state_accessor() {
        use crate::channel::AnswerState;
        let mut event = EslEvent::new();
        event.set_header("Answer-State", "answered");
        assert_eq!(
            event
                .answer_state()
                .unwrap(),
            Some(AnswerState::Answered)
        );
    }

    #[test]
    fn test_event_call_direction_accessor() {
        use crate::channel::CallDirection;
        let mut event = EslEvent::new();
        event.set_header("Call-Direction", "inbound");
        assert_eq!(
            event
                .call_direction()
                .unwrap(),
            Some(CallDirection::Inbound)
        );
    }

    #[test]
    fn test_event_typed_accessors_missing_headers() {
        let event = EslEvent::new();
        assert_eq!(
            event
                .channel_state()
                .unwrap(),
            None
        );
        assert_eq!(
            event
                .channel_state_number()
                .unwrap(),
            None
        );
        assert_eq!(
            event
                .call_state()
                .unwrap(),
            None
        );
        assert_eq!(
            event
                .answer_state()
                .unwrap(),
            None
        );
        assert_eq!(
            event
                .call_direction()
                .unwrap(),
            None
        );
    }

    // --- Repeating SIP header tests ---

    #[test]
    fn test_sip_p_asserted_identity_comma_separated() {
        let mut event = EslEvent::new();
        // RFC 3325: P-Asserted-Identity can carry two identities (one sip:, one tel:)
        // FreeSWITCH stores the comma-separated value as a single channel variable
        event.set_header(
            "variable_sip_P-Asserted-Identity",
            "<sip:alice@atlanta.example.com>, <tel:+15551234567>",
        );

        assert_eq!(
            event.variable_str("sip_P-Asserted-Identity"),
            Some("<sip:alice@atlanta.example.com>, <tel:+15551234567>")
        );
    }

    #[test]
    fn test_sip_p_asserted_identity_array_format() {
        let mut event = EslEvent::new();
        // When FreeSWITCH stores repeated SIP headers via ARRAY format
        event
            .push_header(
                "variable_sip_P-Asserted-Identity",
                "<sip:alice@atlanta.example.com>",
            )
            .unwrap();
        event
            .push_header("variable_sip_P-Asserted-Identity", "<tel:+15551234567>")
            .unwrap();

        let raw = event
            .header_str("variable_sip_P-Asserted-Identity")
            .unwrap();
        assert_eq!(
            raw,
            "ARRAY::<sip:alice@atlanta.example.com>|:<tel:+15551234567>"
        );

        let arr = crate::variables::EslArray::parse(raw).unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr.items()[0], "<sip:alice@atlanta.example.com>");
        assert_eq!(arr.items()[1], "<tel:+15551234567>");
    }

    #[test]
    fn test_sip_header_with_colons_in_uri() {
        let mut event = EslEvent::new();
        // SIP URIs contain colons (sip:, sips:) which must not confuse ARRAY parsing
        event
            .push_header(
                "variable_sip_h_Diversion",
                "<sip:+15551234567@gw.example.com;reason=unconditional>",
            )
            .unwrap();
        event
            .push_header(
                "variable_sip_h_Diversion",
                "<sips:+15559876543@secure.example.com;reason=no-answer;counter=3>",
            )
            .unwrap();

        let raw = event
            .header_str("variable_sip_h_Diversion")
            .unwrap();
        let arr = crate::variables::EslArray::parse(raw).unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(
            arr.items()[0],
            "<sip:+15551234567@gw.example.com;reason=unconditional>"
        );
        assert_eq!(
            arr.items()[1],
            "<sips:+15559876543@secure.example.com;reason=no-answer;counter=3>"
        );
    }

    #[test]
    fn test_sip_p_asserted_identity_plain_format_round_trip() {
        let mut event = EslEvent::with_type(EslEventType::ChannelCreate);
        event.set_header("Event-Name", "CHANNEL_CREATE");
        event.set_header(
            "variable_sip_P-Asserted-Identity",
            "<sip:alice@atlanta.example.com>, <tel:+15551234567>",
        );

        let plain = event.to_plain_format();
        // The comma-separated value should be percent-encoded on the wire
        assert!(plain.contains("variable_sip_P-Asserted-Identity:"));
        // Angle brackets and comma should be encoded
        assert!(!plain.contains("<sip:alice"));
    }

    // --- Header key normalization on EslEvent ---

    #[test]
    fn set_header_normalizes_known_enum_variant() {
        let mut event = EslEvent::new();
        event.set_header("unique-id", "abc-123");
        assert_eq!(event.header(EventHeader::UniqueId), Some("abc-123"));
    }

    #[test]
    fn set_header_keeps_both_profile_name_spellings() {
        let mut event = EslEvent::new();
        event.set_header("profile-name", "internal");
        event.set_header("profile_name", "external");
        assert_eq!(event.header(EventHeader::ProfileName), Some("internal"));
        assert_eq!(
            event.header(EventHeader::ProfileNameSnake),
            Some("external")
        );
        assert_eq!(event.profile_name(), Some("internal"));
    }

    #[test]
    fn set_header_normalizes_codec_header() {
        let mut event = EslEvent::new();
        event.set_header("channel-read-codec-bit-rate", "128000");
        assert_eq!(
            event.header(EventHeader::ChannelReadCodecBitRate),
            Some("128000")
        );
    }

    #[test]
    fn header_str_finds_by_original_key() {
        let mut event = EslEvent::new();
        event.set_header("unique-id", "abc-123");
        assert_eq!(event.header_str("unique-id"), Some("abc-123"));
        assert_eq!(event.header_str("Unique-ID"), Some("abc-123"));
    }

    #[test]
    fn header_str_finds_unknown_dash_header_by_original() {
        let mut event = EslEvent::new();
        event.set_header("x-custom-header", "val");
        assert_eq!(event.header_str("X-Custom-Header"), Some("val"));
        assert_eq!(event.header_str("x-custom-header"), Some("val"));
    }

    #[test]
    fn set_header_underscore_passthrough_preserves_sip_h() {
        let mut event = EslEvent::new();
        event.set_header("variable_sip_h_X-My-CUSTOM-Header", "val");
        assert_eq!(
            event.header_str("variable_sip_h_X-My-CUSTOM-Header"),
            Some("val")
        );
    }

    #[test]
    fn set_header_different_casing_overwrites() {
        let mut event = EslEvent::new();
        event.set_header("Unique-ID", "first");
        event.set_header("unique-id", "second");
        // Both normalize to "Unique-ID", second overwrites first
        assert_eq!(event.header(EventHeader::UniqueId), Some("second"));
    }

    #[test]
    fn remove_header_by_original_key() {
        let mut event = EslEvent::new();
        event.set_header("unique-id", "abc-123");
        let removed = event.remove_header("unique-id");
        assert_eq!(removed, Some("abc-123".to_string()));
        assert_eq!(event.header(EventHeader::UniqueId), None);
    }

    #[test]
    fn remove_header_by_canonical_key() {
        let mut event = EslEvent::new();
        event.set_header("unique-id", "abc-123");
        let removed = event.remove_header("Unique-ID");
        assert_eq!(removed, Some("abc-123".to_string()));
        assert_eq!(event.header_str("unique-id"), None);
    }

    #[test]
    fn serde_round_trip_preserves_canonical_lookups() {
        let mut event = EslEvent::new();
        event.set_header("unique-id", "abc-123");
        event.set_header("channel-read-codec-bit-rate", "128000");
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: EslEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.header(EventHeader::UniqueId), Some("abc-123"));
        assert_eq!(
            deserialized.header(EventHeader::ChannelReadCodecBitRate),
            Some("128000")
        );
    }

    #[test]
    fn serde_deserialize_normalizes_external_json() {
        let json = r#"{"event_type":null,"headers":{"unique-id":"abc-123","channel-read-codec-bit-rate":"128000"},"body":null}"#;
        let event: EslEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.header(EventHeader::UniqueId), Some("abc-123"));
        assert_eq!(
            event.header(EventHeader::ChannelReadCodecBitRate),
            Some("128000")
        );
        assert_eq!(event.header_str("unique-id"), Some("abc-123"));
    }

    #[test]
    fn both_spellings_resolve_after_serde_roundtrip() {
        // A CODEC event carries `Channel-Write-Codec-Name` alongside
        // `channel-write-codec-bit-rate`, and JSON-format events arrive that
        // way too, so deserialization is a wire entry point like any other.
        let external_json = r#"{
            "event_type": null,
            "headers": {
                "Channel-Write-Codec-Name": "opus",
                "channel-write-codec-bit-rate": "64000",
                "Custom-X-Header": "preserved"
            },
            "body": null
        }"#;
        let parsed: EslEvent = serde_json::from_str(external_json).unwrap();

        assert_eq!(
            parsed.header(EventHeader::ChannelWriteCodecName),
            Some("opus")
        );
        assert_eq!(
            parsed.header(EventHeader::ChannelWriteCodecBitRate),
            Some("64000")
        );

        assert_eq!(
            parsed.header_str("channel-write-codec-bit-rate"),
            Some("64000")
        );
        assert_eq!(
            parsed.header_str("Channel-Write-Codec-Bit-Rate"),
            Some("64000")
        );
        assert_eq!(parsed.header_str("Custom-X-Header"), Some("preserved"));

        let json = serde_json::to_string(&parsed).unwrap();
        let re_parsed: EslEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(
            re_parsed.header(EventHeader::ChannelWriteCodecBitRate),
            Some("64000")
        );
    }

    #[test]
    fn test_event_typed_accessors_invalid_values() {
        let mut event = EslEvent::new();
        event.set_header("Channel-State", "BOGUS");
        event.set_header("Channel-State-Number", "999");
        event.set_header("Channel-Call-State", "BOGUS");
        event.set_header("Answer-State", "bogus");
        event.set_header("Call-Direction", "bogus");
        assert!(event
            .channel_state()
            .is_err());
        assert!(event
            .channel_state_number()
            .is_err());
        assert!(event
            .call_state()
            .is_err());
        assert!(event
            .answer_state()
            .is_err());
        assert!(event
            .call_direction()
            .is_err());
    }

    // --- SipHeaderLookup over FreeSWITCH's ARRAY encoding ---

    #[test]
    fn esl_event_authorization_keeps_its_parameter_commas() {
        use sip_header::SipHeaderLookup;

        let mut event = EslEvent::new();
        event.set_header(
            "Authorization".to_string(),
            "Digest username=\"alice\", realm=\"example.com\", nonce=\"abc\"".to_string(),
        );
        let values = event
            .authorization()
            .expect("one Digest value parses");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].username(), Some("alice"));
    }

    #[test]
    fn esl_event_call_info_array_encoding() {
        use sip_header::SipHeaderLookup;

        let mut event = EslEvent::new();
        event.set_header(
            "Call-Info".to_string(),
            "ARRAY::<urn:emergency:uid:callid:abc>;purpose=emergency-CallId\
             |:<urn:emergency:uid:incidentid:def>;purpose=emergency-IncidentId"
                .to_string(),
        );
        let ci = event
            .call_info()
            .expect("should parse")
            .expect("should be present");
        assert_eq!(
            ci.entries()
                .len(),
            2,
            "ARRAY:: entries should expand"
        );
    }

    #[test]
    fn esl_event_call_info_plain_value_unchanged() {
        use sip_header::SipHeaderLookup;

        let mut event = EslEvent::new();
        event.set_header(
            "Call-Info".to_string(),
            "<sip:pbx.example.com>;purpose=icon".to_string(),
        );
        let ci = event
            .call_info()
            .expect("plain value should parse")
            .expect("should be present");
        assert_eq!(
            ci.entries()
                .len(),
            1
        );
    }

    #[test]
    fn esl_event_history_info_array_encoding() {
        use sip_header::SipHeaderLookup;

        let mut event = EslEvent::new();
        event.set_header(
            "History-Info".to_string(),
            "ARRAY::<sip:user@pbx.example.com>;index=1\
             |:<sip:forward@pbx.example.com?Reason=unconditional>;index=1.1"
                .to_string(),
        );
        let hi = event
            .history_info()
            .expect("should parse")
            .expect("should be present");
        assert_eq!(
            hi.entries()
                .len(),
            2,
            "ARRAY:: entries should expand"
        );
    }
}
