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
//! [`EslHeaders`] wraps an [`IndexMap<String, String>`] and is a
//! [`SipHeaderRows`] store whose rows have both quirks peeled, so sip-header's
//! typed accessors read FreeSWITCH values as RFC header rows. The
//! design-rationale doc §"EslHeaders: making the transport boundary visible"
//! explains the layering.

use indexmap::IndexMap;
use sip_header_catalog::{RowError, SipHeader, SipHeaderRows};

use crate::headers::{case_alias_key, normalize_header_key};
use crate::lookup::{variable_key, HeaderLookup};
use crate::variables::{EslArray, MAX_ARRAY_ITEMS};

/// A flat header store that decodes FreeSWITCH ARRAY and bracket encoding
/// when answering SIP header row queries.
///
/// Construct with [`EslHeaders::new`] or [`EslHeaders::from_map`]. Use it
/// anywhere a [`HeaderLookup`] or [`SipHeaderRows`] implementor is expected;
/// sip-header's accessors read it through their blanket impl:
///
/// ```
/// use freeswitch_types::{EslHeaders, HeaderLookup};
/// use sip_header::prelude::*;
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
/// `HeaderLookup` delegates straight to the map and returns the value as
/// FreeSWITCH sent it. The rows of a header the catalog says may repeat are
/// its `ARRAY::` items with `[...]` wrapping removed; any other header is one
/// row, whole. Commas are left to the accessor, which knows whether a header
/// is a list. An `ARRAY::` past [`MAX_ARRAY_ITEMS`] is a [`RowError`].
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

/// Rows of one stored value under `name`: a header the catalog says may repeat
/// is bracket-unwrapped, then split at `ARRAY::` items; any other header,
/// unregistered names included, is one row, whole.
fn value_rows<'a>(name: &str, value: Option<&'a str>) -> Result<Vec<&'a str>, RowError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if !SipHeader::parse_name(name).is_ok_and(|h| h.may_repeat()) {
        return Ok(vec![value]);
    }
    let value = strip_brackets(value);
    let Some(body) = value.strip_prefix(EslArray::PREFIX) else {
        return Ok(vec![value]);
    };
    let items: Vec<&str> = body
        .split(EslArray::SEPARATOR)
        .take(MAX_ARRAY_ITEMS + 1)
        .collect();
    if items.len() > MAX_ARRAY_ITEMS {
        return Err(RowError::too_many_entries(
            body.split(EslArray::SEPARATOR)
                .count(),
            MAX_ARRAY_ITEMS,
        ));
    }
    Ok(items)
}

impl SipHeaderRows for EslHeaders {
    fn sip_header_rows_str<'a>(&'a self, name: &str) -> Result<Vec<&'a str>, RowError> {
        value_rows(name, self.get(name))
    }
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
    use sip_header::prelude::*;
    use sip_header_catalog::RowErrorKind;

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
            Ok(Some("<sip:a@example.test>;purpose=icon"))
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
                .expect("header is present")
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
                .expect("header is present")
                .len(),
            2
        );
    }

    #[test]
    fn a_single_valued_header_keeps_its_commas() {
        let mut h = EslHeaders::new();
        h.insert("Subject", "one, two");
        assert_eq!(h.sip_header_rows_str("Subject"), Ok(vec!["one, two"]));
    }

    #[test]
    fn an_unregistered_header_is_one_row_even_in_array_form() {
        let mut h = EslHeaders::new();
        h.insert("X-Custom", "[ARRAY::a|:b]");
        assert_eq!(h.sip_header_rows_str("X-Custom"), Ok(vec!["[ARRAY::a|:b]"]));
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
            let values = values
                .expect("one Digest value parses")
                .expect("header is present");
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
            .expect("both Digest values parse")
            .expect("header is present");
        assert_eq!(values.len(), 2);
        assert_eq!(values[1].realm(), Some("r2"));
    }

    fn over_cap_array(item: &str) -> String {
        format!(
            "ARRAY::{}",
            vec![item; crate::variables::MAX_ARRAY_ITEMS + 1].join("|:")
        )
    }

    #[test]
    fn an_over_cap_array_fails_every_accessor() {
        let mut h = EslHeaders::new();
        h.insert("Call-Info", over_cap_array("<sip:a@example.com>"));
        h.insert(
            "History-Info",
            over_cap_array("<sip:a@example.com>;index=1"),
        );
        h.insert("P-Asserted-Identity", over_cap_array("<sip:a@example.com>"));
        assert!(h
            .call_info()
            .is_err());
        assert!(h
            .history_info()
            .is_err());
        assert!(h
            .p_asserted_identity()
            .is_err());
    }

    #[test]
    fn an_over_cap_array_is_a_row_error_naming_the_cap() {
        let mut h = EslHeaders::new();
        h.insert("Call-Info", over_cap_array("<sip:a@example.com>"));
        let err = h
            .sip_header_rows(SipHeader::CallInfo)
            .expect_err("over-cap ARRAY must fail");
        assert_eq!(err.kind(), RowErrorKind::TooManyEntries);
        assert_eq!(err.count(), Some(MAX_ARRAY_ITEMS + 1));
        assert_eq!(err.limit(), Some(MAX_ARRAY_ITEMS));
        assert!(matches!(
            h.call_info(),
            Err(sip_header::ParseError::Row(e)) if e == err
        ));
        assert_eq!(h.sip_header(SipHeader::CallInfo), Err(err));
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
}
