//! [`EslHeaders`] — a flat header store that understands FreeSWITCH's
//! transport encodings.
//!
//! FreeSWITCH's ESL wire format carries headers and channel variables in the
//! same flat key-value namespace, but with two transport quirks that plain
//! RFC-SIP parsers don't account for:
//!
//! - **ARRAY encoding** — repeating SIP headers arrive as
//!   `ARRAY::value1|:value2|:value3` (see [`EslArray`]).
//! - **Bracket wrapping** — some log-sourced headers arrive as `[value]`.
//!
//! Routing those values through the default [`SipHeaderLookup`] methods
//! produces parse errors because the string doesn't match RFC syntax.
//! [`EslHeaders`] wraps an [`IndexMap<String, String>`] and overrides the
//! relevant `SipHeaderLookup` methods to strip both quirks before parsing.
//! The design-rationale doc §"EslHeaders: making the transport boundary
//! visible" explains the layering.

use indexmap::IndexMap;
use sip_header::{HistoryInfo, HistoryInfoError, SipHeaderLookup, UriInfo, UriInfoError};

use crate::headers::{case_alias_key, normalize_header_key};
use crate::lookup::{variable_key, HeaderLookup};
use crate::variables::{EslArray, EslArrayError, MAX_ARRAY_ITEMS};

/// A flat header store that decodes FreeSWITCH ARRAY and bracket encoding
/// when answering typed SIP header queries.
///
/// Construct with [`EslHeaders::new`] or [`EslHeaders::from_map`]. Use it
/// anywhere a [`HeaderLookup`] or [`SipHeaderLookup`] implementor is
/// expected:
///
/// ```
/// use freeswitch_types::{EslHeaders, HeaderLookup};
/// use freeswitch_types::sip_header::SipHeaderLookup;
///
/// let mut h = EslHeaders::new();
/// h.insert("Unique-ID", "abc-123");
/// h.insert("Call-Info", "ARRAY::<sip:a@example.com>;purpose=icon|:<sip:b@example.com>");
///
/// assert_eq!(h.header_str("Unique-ID"), Some("abc-123"));
/// let ci = h.call_info().unwrap().unwrap();
/// assert_eq!(ci.entries().len(), 2);
/// ```
///
/// `HeaderLookup` delegates straight to the map; `SipHeaderLookup` methods
/// that parse RFC-structured values (`call_info`, `history_info`, and any
/// future multi-value parsers) first peel the FreeSWITCH encoding and then
/// hand pre-split entries to `sip-header`. Non-parsing lookups
/// (`sip_header_str`, `sip_header`) return the raw stored value untouched —
/// the caller sees exactly what FreeSWITCH put on the wire.
///
/// Every write normalizes its key with
/// [`normalize_header_key`](crate::normalize_header_key), so a payload
/// spelling one header two ways collapses to one entry, and every read falls
/// back to a lowercase alias, so a query in any casing resolves. Keys
/// containing `_` are exempt from both: their suffix carries SIP wire casing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize), serde(transparent))]
pub struct EslHeaders {
    map: IndexMap<String, String>,
    /// Lowercase alias to canonical key, for the case-insensitive read path.
    /// Derived from `map`, so it is rebuilt rather than serialized.
    #[cfg_attr(feature = "serde", serde(skip))]
    aliases: IndexMap<String, String>,
}

impl EslHeaders {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Wrap an existing map, normalizing every key.
    pub fn from_map(map: IndexMap<String, String>) -> Self {
        map.into_iter()
            .collect()
    }

    /// Access the underlying map, keyed canonically.
    pub fn as_map(&self) -> &IndexMap<String, String> {
        &self.map
    }

    /// Consume and return the underlying map, keyed canonically.
    pub fn into_map(self) -> IndexMap<String, String> {
        self.map
    }

    /// Insert a header under its canonical key, replacing any entry there.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = normalize_header_key(&key.into());
        if let Some(alias) = case_alias_key(&key) {
            self.aliases
                .insert(alias, key.clone());
        }
        self.map
            .insert(key, value.into());
    }

    /// Remove a header, by canonical key or by any other casing of it.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        let canonical = if self
            .map
            .contains_key(key)
        {
            key.to_string()
        } else {
            let alias = case_alias_key(key)?;
            self.aliases
                .get(&alias)?
                .clone()
        };
        if let Some(alias) = case_alias_key(&canonical) {
            self.aliases
                .shift_remove(&alias);
        }
        self.map
            .shift_remove(&canonical)
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.map
            .len()
    }

    /// `true` if there are no entries.
    pub fn is_empty(&self) -> bool {
        self.map
            .is_empty()
    }

    fn get(&self, name: &str) -> Option<&str> {
        if let Some(value) = self
            .map
            .get(name)
        {
            return Some(value.as_str());
        }
        self.aliases
            .get(&case_alias_key(name)?)
            .and_then(|canonical| {
                self.map
                    .get(canonical)
            })
            .map(|s| s.as_str())
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for EslHeaders {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut headers = Self::new();
        headers.extend(iter);
        headers
    }
}

impl<K: Into<String>, V: Into<String>> Extend<(K, V)> for EslHeaders {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (key, value) in iter {
            self.insert(key, value);
        }
    }
}

impl From<IndexMap<String, String>> for EslHeaders {
    fn from(map: IndexMap<String, String>) -> Self {
        Self::from_map(map)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for EslHeaders {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self::from_map(IndexMap::deserialize(deserializer)?))
    }
}

/// Strip a single pair of outer `[...]` brackets from FreeSWITCH log-derived
/// header values. If the value is not bracket-wrapped, returns it unchanged.
fn strip_brackets(s: &str) -> &str {
    if let Some(inner) = s.strip_prefix('[') {
        if let Some(inner) = inner.strip_suffix(']') {
            return inner;
        }
    }
    s
}

/// The one splitter every list-valued lookup here goes through: bracket
/// unwrapping, then `ARRAY::` splitting, falling back to the RFC comma split.
fn split_entries_ref(value: &str) -> Result<Vec<&str>, EslArrayError> {
    let value = strip_brackets(value);
    match value.strip_prefix(EslArray::PREFIX) {
        Some(body) => {
            let items: Vec<&str> = body
                .split(EslArray::SEPARATOR)
                .collect();
            if items.len() > MAX_ARRAY_ITEMS {
                return Err(EslArrayError::TooManyItems {
                    count: items.len(),
                    max: MAX_ARRAY_ITEMS,
                });
            }
            Ok(items)
        }
        None => {
            let value = value.trim();
            if value.is_empty() {
                Ok(Vec::new())
            } else {
                Ok(sip_header::split_comma_entries(value))
            }
        }
    }
}

/// A structurally invalid ESL encoding hands the value back whole, so the RFC
/// parser reports it rather than this layer dropping entries.
fn entries_or_whole(value: &str) -> Vec<&str> {
    split_entries_ref(value).unwrap_or_else(|_| vec![value])
}

impl EslHeaders {
    /// Parse a FreeSWITCH-transported SIP URI-list value into typed [`UriInfo`]
    /// entries, handling both `ARRAY::` encoding and bracket wrapping.
    ///
    /// Use this when you hold a *raw* value — e.g. the `sip_call_info` /
    /// `sip_alert_info` channel variable fetched over ESL — rather than a
    /// populated [`EslHeaders`]. It accepts any of the forms FreeSWITCH emits:
    ///
    /// - **Single RFC entry**: `<sip:a@example.test>;purpose=emergency-CallId`
    /// - **ARRAY encoding**: `ARRAY::<sip:a@example.test>;purpose=icon|:<sip:b@example.test>`
    /// - **Bracket-wrapped**: `[<sip:a@example.test>;purpose=icon]`
    ///
    /// This is the same decoding the [`call_info`](SipHeaderLookup::call_info)
    /// and [`alert_info`](SipHeaderLookup::alert_info) methods apply; iterate
    /// the result via `.entries()`.
    ///
    /// # Errors
    ///
    /// Returns [`UriInfoError`] if the value is malformed or if the `ARRAY::`
    /// structure is invalid. Structural `EslArrayError` cases (e.g.
    /// `TooManyItems`) are surfaced as [`UriInfoError::Malformed`] carrying
    /// the cause.
    ///
    /// # Example
    ///
    /// ```
    /// use freeswitch_types::EslHeaders;
    ///
    /// let value = "ARRAY::<urn:emergency:uid:callid:bcf.test>;purpose=emergency-CallId\
    ///              |:<urn:emergency:uid:incidentid:bcf.test>;purpose=emergency-IncidentId";
    /// let info = EslHeaders::parse_uri_info(value).unwrap();
    /// assert_eq!(info.entries().len(), 2);
    /// ```
    pub fn parse_uri_info(value: &str) -> Result<UriInfo, UriInfoError> {
        let entries =
            Self::split_entries(value).map_err(|e| UriInfoError::Malformed(e.to_string()))?;
        UriInfo::from_entries(
            entries
                .iter()
                .map(String::as_str),
        )
    }

    /// Split one comma-list SIP header value held as an ESL variable into its
    /// entries, applying the same decoding as [`parse_uri_info`](Self::parse_uri_info)
    /// and [`parse_history_info`](Self::parse_history_info): bracket unwrapping,
    /// then `ARRAY::` splitting, falling back to the RFC comma split.
    ///
    /// Only for headers that are lists (`Call-Info`, `Alert-Info`,
    /// `History-Info`, `Diversion`, …). A header whose value legitimately
    /// contains commas without being a list — `Date`, `Subject`, `User-Agent` —
    /// must not go through it.
    ///
    /// # Errors
    ///
    /// A missing `ARRAY::` prefix is the RFC form, not an error. Every other
    /// [`EslArrayError`] is a structural fault in the ESL encoding and is
    /// returned as-is. An empty or whitespace-only value yields no entries,
    /// which the typed parsers turn into their own `Empty` error.
    ///
    /// ```
    /// use freeswitch_types::EslHeaders;
    ///
    /// let entries =
    ///     EslHeaders::split_entries("ARRAY::<sip:a@example.test>|:<sip:b@example.test>").unwrap();
    /// assert_eq!(entries, ["<sip:a@example.test>", "<sip:b@example.test>"]);
    /// ```
    pub fn split_entries(value: &str) -> Result<Vec<String>, EslArrayError> {
        Ok(split_entries_ref(value)?
            .into_iter()
            .map(str::to_string)
            .collect())
    }

    /// Entries of a header `sip-header` marks multi-valued, for
    /// [`sip_header_all_str`](SipHeaderLookup::sip_header_all_str). Any other
    /// header answers with its value whole: only a list may be split.
    #[doc(hidden)]
    pub fn split_multi_value<'a>(value: Option<&'a str>, name: &str) -> Vec<&'a str> {
        let Some(value) = value else {
            return Vec::new();
        };
        let multi = name
            .parse::<sip_header::SipHeader>()
            .is_ok_and(|h| h.is_multi_valued());
        if multi {
            entries_or_whole(value)
        } else {
            vec![value]
        }
    }

    /// Parse a FreeSWITCH-transported `History-Info` value into a typed
    /// [`HistoryInfo`], handling both `ARRAY::` encoding and bracket wrapping.
    ///
    /// The raw-value counterpart to [`history_info`](SipHeaderLookup::history_info).
    ///
    /// # Errors
    ///
    /// Structural `EslArrayError` cases (e.g. `TooManyItems`) are surfaced as
    /// [`HistoryInfoError::Malformed`] carrying the cause.
    pub fn parse_history_info(value: &str) -> Result<HistoryInfo, HistoryInfoError> {
        let entries =
            Self::split_entries(value).map_err(|e| HistoryInfoError::Malformed(e.to_string()))?;
        HistoryInfo::from_entries(
            entries
                .iter()
                .map(String::as_str),
        )
    }
}

/// Internal: emits the one [`SipHeaderLookup`] override that peels
/// FreeSWITCH's ARRAY and bracket encoding, `sip_header_all_str`, which every
/// list-valued default routes through. Invoke inside an
/// `impl SipHeaderLookup for T` block. Not part of the stable API.
#[doc(hidden)]
#[macro_export]
macro_rules! esl_sip_header_overrides {
    () => {
        fn sip_header_all_str<'a>(&'a self, name: &str) -> Vec<&'a str> {
            $crate::variables::EslHeaders::split_multi_value(self.sip_header_str(name), name)
        }
    };
}

impl SipHeaderLookup for EslHeaders {
    fn sip_header_str(&self, name: &str) -> Option<&str> {
        self.get(name)
    }

    crate::esl_sip_header_overrides!();
}

impl HeaderLookup for EslHeaders {
    fn header_str(&self, name: &str) -> Option<&str> {
        self.get(name)
    }

    fn variable_str(&self, name: &str) -> Option<&str> {
        self.get(&variable_key(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::EventHeader;

    #[test]
    fn every_write_path_normalizes_the_key() {
        let mut inserted = EslHeaders::new();
        inserted.insert("unique-id", "abc-123");

        let mut extended = EslHeaders::new();
        extended.extend([("unique-id", "abc-123")]);

        let map: IndexMap<String, String> = [("unique-id".to_string(), "abc-123".to_string())]
            .into_iter()
            .collect();

        for h in [
            inserted,
            extended,
            [("unique-id", "abc-123")]
                .into_iter()
                .collect(),
            EslHeaders::from_map(map.clone()),
            EslHeaders::from(map),
        ] {
            assert_eq!(
                h.as_map()
                    .keys()
                    .collect::<Vec<_>>(),
                vec!["Unique-ID"]
            );
            assert_eq!(h.unique_id(), Some("abc-123"));
        }
    }

    #[test]
    fn lookup_resolves_a_non_canonical_spelling() {
        let mut h = EslHeaders::new();
        h.insert("Unique-ID", "abc-123");
        h.insert("Call-Info", "<sip:a@example.test>;purpose=icon");
        assert_eq!(h.header_str("unique-id"), Some("abc-123"));
        assert_eq!(h.header_str("UNIQUE-ID"), Some("abc-123"));
        assert_eq!(
            h.sip_header_str("call-info"),
            Some("<sip:a@example.test>;purpose=icon")
        );
    }

    #[test]
    fn underscore_keys_keep_their_wire_casing() {
        let mut h = EslHeaders::new();
        h.insert("variable_sip_h_X-My-CUSTOM-Header", "val");
        assert_eq!(
            h.header_str("variable_sip_h_X-My-CUSTOM-Header"),
            Some("val")
        );
        assert_eq!(h.header_str("variable_sip_h_x-my-custom-header"), None);
    }

    #[test]
    fn two_spellings_of_one_header_are_one_entry() {
        let mut h = EslHeaders::new();
        h.insert("Channel-Read-Codec-Bit-Rate", "first");
        h.insert("channel-read-codec-bit-rate", "second");
        assert_eq!(h.len(), 1);
        assert_eq!(h.header_str("Channel-Read-Codec-Bit-Rate"), Some("second"));
    }

    #[test]
    fn remove_accepts_a_non_canonical_spelling() {
        let mut h = EslHeaders::new();
        h.insert("Unique-ID", "abc-123");
        assert_eq!(h.remove("unique-id"), Some("abc-123".to_string()));
        assert!(h.is_empty());
        assert_eq!(h.header_str("Unique-ID"), None);
        assert_eq!(h.remove("unique-id"), None);
    }

    #[test]
    fn remove_by_canonical_key_drops_the_alias_too() {
        let mut h = EslHeaders::new();
        h.insert("unique-id", "abc-123");
        assert_eq!(h.remove("Unique-ID"), Some("abc-123".to_string()));
        assert_eq!(h.header_str("unique-id"), None);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn deserialization_normalizes_like_the_wire_does() {
        let json = r#"{"unique-id":"abc-123","variable_sip_call_id":"call-1"}"#;
        let h: EslHeaders = serde_json::from_str(json).expect("map payload");
        assert_eq!(h.unique_id(), Some("abc-123"));
        assert_eq!(h.variable_str("sip_call_id"), Some("call-1"));
        assert_eq!(
            h.as_map()
                .keys()
                .collect::<Vec<_>>(),
            vec!["Unique-ID", "variable_sip_call_id"]
        );

        let round_tripped: EslHeaders =
            serde_json::from_str(&serde_json::to_string(&h).expect("serialize"))
                .expect("round trip");
        assert_eq!(round_tripped, h);
    }

    #[test]
    fn array_encoding_decodes_for_every_multi_value_header() {
        let mut h = EslHeaders::new();
        h.insert(
            "P-Asserted-Identity",
            "ARRAY::<sip:alice@example.test>|:<tel:+15551234567>",
        );
        h.insert(
            "Error-Info",
            "ARRAY::<sip:busy@example.test>|:<http://example.test/why.html>",
        );
        h.insert(
            "Contact",
            "ARRAY::<sip:a@192.0.2.1>;expires=60|:<sip:b@192.0.2.2>",
        );
        h.insert(
            "Via",
            "ARRAY::SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bK1|:SIP/2.0/TCP 192.0.2.2;branch=z9hG4bK2",
        );
        h.insert(
            "Warning",
            "ARRAY::370 192.0.2.1 \"Insufficient bandwidth\"|:399 192.0.2.2 \"Misc, warning\"",
        );
        h.insert("Accept", "ARRAY::application/sdp|:text/plain;q=0.5");

        assert_eq!(
            h.via()
                .expect("ARRAY value must decode")
                .expect("header is present")
                .len(),
            2
        );
        assert_eq!(
            h.warning()
                .expect("ARRAY value must decode")
                .expect("header is present")
                .len(),
            2
        );
        assert_eq!(
            h.accept()
                .expect("ARRAY value must decode")
                .expect("header is present")
                .len(),
            2
        );
        assert_eq!(
            h.p_asserted_identity()
                .expect("ARRAY value must decode")
                .len(),
            2
        );
        assert_eq!(
            h.error_info()
                .expect("ARRAY value must decode")
                .expect("header is present")
                .entries()
                .len(),
            2
        );
        assert_eq!(
            h.contact()
                .expect("ARRAY value must decode")
                .len(),
            2
        );
    }

    #[test]
    fn a_single_valued_header_keeps_its_commas() {
        let mut h = EslHeaders::new();
        h.insert("Subject", "one, two");
        assert_eq!(h.sip_header_all_str("Subject"), vec!["one, two"]);
    }

    /// RFC 3261 section 7.3.1 forbids comma-joining the auth headers, so the
    /// commas between their parameters never separate rows.
    #[test]
    fn an_auth_header_keeps_its_parameter_commas() {
        let digest = "Digest username=\"alice\", realm=\"example.com\", nonce=\"abc\"";
        let mut h = EslHeaders::new();
        h.insert("Authorization", digest);
        h.insert("Proxy-Authorization", digest);
        h.insert("WWW-Authenticate", digest);
        h.insert("Proxy-Authenticate", digest);

        for values in [
            h.authorization(),
            h.proxy_authorization(),
            h.www_authenticate(),
            h.proxy_authenticate(),
        ] {
            let values = values.expect("one Digest value parses");
            assert_eq!(values.len(), 1);
            assert_eq!(values[0].username(), Some("alice"));
            assert_eq!(values[0].realm(), Some("example.com"));
        }
    }

    #[test]
    fn an_array_of_auth_headers_is_one_row_per_occurrence() {
        let mut h = EslHeaders::new();
        h.insert(
            "Authorization",
            "ARRAY::Digest username=\"a\", realm=\"r1\"|:Digest username=\"b\", realm=\"r2\"",
        );
        let values = h
            .authorization()
            .expect("both Digest values parse");
        assert_eq!(values.len(), 2);
        assert_eq!(values[1].realm(), Some("r2"));
    }

    #[test]
    fn a_list_header_still_splits_on_commas() {
        let mut h = EslHeaders::new();
        h.insert("Call-Info", "<sip:a@example.com>, <sip:b@example.com>");
        assert_eq!(
            h.call_info()
                .expect("both entries parse")
                .expect("header is present")
                .len(),
            2
        );
    }

    #[test]
    fn header_str_passthrough() {
        let mut h = EslHeaders::new();
        h.insert("Unique-ID", "abc-123");
        assert_eq!(h.header_str("Unique-ID"), Some("abc-123"));
    }

    #[test]
    fn variable_str_prepends_variable_prefix() {
        let mut h = EslHeaders::new();
        h.insert("variable_sip_call_id", "call-1");
        assert_eq!(h.variable_str("sip_call_id"), Some("call-1"));
        assert_eq!(h.variable_str("missing"), None);
    }

    /// The stored-header entry point and the raw-value one must agree, so
    /// every form runs through both.
    #[test]
    fn both_entry_points_decode_every_transport_form() {
        let forms = [
            (
                "rfc",
                "<sip:alice@example.test>;purpose=icon",
                vec![Some("icon")],
            ),
            (
                "array",
                "ARRAY::<sip:a@example.test>;purpose=icon|:<sip:b@example.test>;purpose=info",
                vec![Some("icon"), Some("info")],
            ),
            (
                "bracketed",
                "[<sip:alice@example.test>;purpose=icon]",
                vec![Some("icon")],
            ),
            (
                "bracketed array",
                "[ARRAY::<sip:a@example.test>;purpose=icon|:<sip:b@example.test>]",
                vec![Some("icon"), None],
            ),
        ];

        for (label, value, purposes) in forms {
            let mut h = EslHeaders::new();
            h.insert("Call-Info", value);
            let from_header = h
                .call_info()
                .unwrap_or_else(|e| panic!("{label}: {e}"))
                .unwrap_or_else(|| panic!("{label}: header is present"));
            let from_value = EslHeaders::parse_uri_info(value)
                .unwrap_or_else(|e| panic!("{label} as a raw value: {e}"));
            assert_eq!(from_header, from_value, "{label}");
            assert_eq!(
                from_header
                    .entries()
                    .iter()
                    .map(|e| e.purpose())
                    .collect::<Vec<_>>(),
                purposes,
                "{label}"
            );
        }
    }

    #[test]
    fn call_info_absent_is_ok_none() {
        let h = EslHeaders::new();
        assert!(h
            .call_info()
            .unwrap()
            .is_none());
    }

    #[test]
    fn history_info_array_encoding() {
        let mut h = EslHeaders::new();
        h.insert(
            "History-Info",
            "ARRAY::<sip:a@example.com>;index=1|:<sip:b@example.com>;index=1.1",
        );
        let hi = h
            .history_info()
            .unwrap()
            .expect("present");
        assert_eq!(
            hi.entries()
                .len(),
            2
        );
    }

    #[test]
    fn header_lookup_typed_accessors() {
        let mut h = EslHeaders::new();
        h.insert(EventHeader::UniqueId.as_str(), "uuid-1");
        h.insert(EventHeader::ChannelName.as_str(), "sofia/a/b");
        assert_eq!(h.unique_id(), Some("uuid-1"));
        assert_eq!(h.channel_name(), Some("sofia/a/b"));
    }

    #[test]
    fn parse_uri_info_empty_value() {
        assert!(EslHeaders::parse_uri_info("").is_err());
    }

    #[test]
    fn parse_uri_info_malformed_no_panic() {
        // sip-header's UriInfo is lenient: an addr-spec needs no angle brackets.
        let info = EslHeaders::parse_uri_info("sip:bare@example.test").expect("lenient parse");
        assert_eq!(
            info.entries()
                .len(),
            1
        );
    }

    #[test]
    fn parse_uri_info_structural_failure_is_malformed() {
        let value = format!(
            "ARRAY::{}",
            vec!["<sip:a@example.com>"; crate::variables::MAX_ARRAY_ITEMS + 1].join("|:")
        );
        let err = EslHeaders::parse_uri_info(&value).expect_err("over-limit array must fail");
        assert!(matches!(err, UriInfoError::Malformed(_)), "got {err:?}");
    }

    #[test]
    fn parse_history_info_structural_failure_is_malformed() {
        let value = format!(
            "ARRAY::{}",
            vec!["<sip:a@example.com>;index=1"; crate::variables::MAX_ARRAY_ITEMS + 1].join("|:")
        );
        let err = EslHeaders::parse_history_info(&value).expect_err("over-limit array must fail");
        assert!(matches!(err, HistoryInfoError::Malformed(_)), "got {err:?}");
    }

    #[test]
    fn collects_from_key_value_pairs() {
        let h: EslHeaders = [
            ("Unique-ID", "uuid-1"),
            ("Caller-Destination-Number", "911"),
        ]
        .into_iter()
        .collect();
        assert_eq!(h.header_str("Unique-ID"), Some("uuid-1"));
    }

    #[test]
    fn extend_adds_pairs() {
        let mut h = EslHeaders::new();
        h.insert("Unique-ID", "uuid-1");
        h.extend([("Channel-Name", "sofia/a/b")]);
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn split_entries_array_form() {
        let entries =
            EslHeaders::split_entries("ARRAY::<sip:a@example.test>|:<sip:b@example.test>")
                .expect("ARRAY form");
        assert_eq!(entries, ["<sip:a@example.test>", "<sip:b@example.test>"]);
    }

    #[test]
    fn split_entries_rfc_comma_form() {
        let entries = EslHeaders::split_entries(
            "<sip:a@example.test>;text=\"one, two\",<sip:b@example.test>",
        )
        .expect("RFC form");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], "<sip:a@example.test>;text=\"one, two\"");
    }

    #[test]
    fn split_entries_bracket_wrapped() {
        let entries =
            EslHeaders::split_entries("[<sip:a@example.test>;purpose=icon]").expect("bracket form");
        assert_eq!(entries, ["<sip:a@example.test>;purpose=icon"]);
    }

    #[test]
    fn split_entries_blank_value_yields_no_entries() {
        let entries = EslHeaders::split_entries("  ").expect("blank value");
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_history_info_blank_value_is_empty_error() {
        let err = EslHeaders::parse_history_info("  ").expect_err("blank value must fail");
        assert!(matches!(err, HistoryInfoError::Empty), "got {err:?}");
    }

    #[test]
    fn split_entries_too_many_items_is_err() {
        let value = format!(
            "ARRAY::{}",
            vec!["<sip:a@example.test>"; crate::variables::MAX_ARRAY_ITEMS + 1].join("|:")
        );
        let err = EslHeaders::split_entries(&value).expect_err("over-limit array must fail");
        assert!(
            matches!(err, EslArrayError::TooManyItems { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_history_info_array_form() {
        let value = "ARRAY::<sip:a@example.com>;index=1|:<sip:b@example.com>;index=1.1";
        let info = EslHeaders::parse_history_info(value).expect("parse ARRAY form");
        assert_eq!(
            info.entries()
                .len(),
            2
        );
    }
}
