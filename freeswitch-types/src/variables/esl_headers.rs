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
//! typed accessors read FreeSWITCH values as RFC header rows.
//!
//! Every carrier stores headers here, and nothing reads them by exact key. The
//! switch looks headers up case-insensitively, emits one header in several
//! casings (sometimes twice in one event), and a consumer queries in whatever
//! casing its config holds. A known [`EventHeader`](crate::EventHeader) is
//! stored in its canonical spelling. Any other key keeps the spelling it first
//! arrived in, because nothing else records the wire spelling and a consumer
//! forwarding keys needs it; a later casing lands on that entry. A key
//! containing `_` is never folded: a `variable_*` name is a user variable in
//! any case the dialplan chose, and a `sip_h_`-style suffix is the SIP header
//! name the switch emits verbatim. Deserialization goes through `insert`, since
//! a payload is an entry point like the wire. Equality and hashing fold the way
//! lookup does.
//!
//! The rows of a header the catalog marks repeatable are its `ARRAY::` items
//! with `[...]` wrapping removed; any other header is one row, whole. Commas
//! are left to the accessor, which alone knows whether a header is a list, so
//! an authentication header's parameters are never cut into rows. An `ARRAY::`
//! past [`MAX_ARRAY_ITEMS`] is a row error rather than the whole value, so
//! every accessor fails on it instead of reading garbage. A raw value goes
//! through the same decoder ([`EslHeaders::decode_value`]).

use indexmap::IndexMap;
use sip_header_catalog::{RowError, SipHeader, SipHeaderField, SipHeaderRows};

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
/// use sip_header::SipHeaderLookup;
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
/// Every write runs its key through
/// [`normalize_header_key`](crate::normalize_header_key), so a known header
/// takes its canonical spelling and any other keeps the one it arrived in.
/// A later write in another casing lands on the entry already stored, under
/// its first spelling, and every read falls back to a lowercase alias, so a
/// query in any casing resolves. Keys containing `_` are exempt from both:
/// their suffix carries SIP wire casing. Equality follows the same rule, so
/// two stores that first saw one header in different casings are equal.
#[derive(Debug, Clone, Default)]
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

    /// Access the underlying map, keyed by stored spelling.
    pub fn as_map(&self) -> &IndexMap<String, String> {
        &self.map
    }

    /// Consume and return the underlying map, keyed by stored spelling.
    pub fn into_map(self) -> IndexMap<String, String> {
        self.map
    }

    /// Insert a header, replacing the value of any entry it matches in another
    /// casing and keeping that entry's spelling.
    ///
    /// Collecting from a map that holds one header in two casings keeps
    /// whichever the map yields first, with the value of the last.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = normalize_header_key(&key.into());
        let value = value.into();
        let Some(alias) = case_alias_key(&key) else {
            self.map
                .insert(key, value);
            return;
        };
        let stored = self
            .aliases
            .entry(alias)
            .or_insert(key);
        self.map
            .insert(stored.clone(), value);
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

    /// Every entry in insertion order, under its stored spelling.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.map
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    /// Entries whose key starts with `prefix`, under the rule lookups use: a
    /// key without `_` matches in any ASCII casing, a key with `_` exactly.
    ///
    /// Yields the stored key, whose casing may differ from `prefix`, so
    /// `key.strip_prefix(prefix)` can fail on a matched entry; slice off
    /// `prefix.len()` bytes instead.
    ///
    /// ```
    /// use freeswitch_types::EslHeaders;
    ///
    /// let h: EslHeaders = [("C911P-Action", "dispatch"), ("Unique-ID", "abc")]
    ///     .into_iter()
    ///     .collect();
    /// let custom: Vec<_> = h.prefixed("c911p-").collect();
    /// assert_eq!(custom.len(), 1);
    /// assert_eq!(custom[0].1, "dispatch");
    /// ```
    pub fn prefixed<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = (&'a str, &'a str)> {
        self.map
            .iter()
            .filter(move |(key, _)| key_has_prefix(key, prefix))
            .map(|(key, value)| (key.as_str(), value.as_str()))
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

/// The stored key decides the rule, since a prefix such as `X-` can begin a
/// key of either kind; `get` keeps a non-char boundary from panicking.
fn key_has_prefix(key: &str, prefix: &str) -> bool {
    if case_alias_key(key).is_none() {
        return key.starts_with(prefix);
    }
    key.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

impl PartialEq for EslHeaders {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .map
                .iter()
                .all(|(key, value)| other.get(key) == Some(value.as_str()))
    }
}

impl Eq for EslHeaders {}

/// Hashes what `PartialEq` compares: entries regardless of order, and a dash key by its
/// lowercase alias, so equal stores hash alike.
impl std::hash::Hash for EslHeaders {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let mut entries: Vec<(String, &str)> = self
            .iter()
            .map(|(k, v)| (case_alias_key(k).unwrap_or_else(|| k.to_string()), v))
            .collect();
        entries.sort_unstable();
        entries.hash(state);
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
        return Err(RowError::too_many_rows(
            body.split(EslArray::SEPARATOR)
                .count(),
            MAX_ARRAY_ITEMS,
        ));
    }
    Ok(items)
}

impl EslHeaders {
    /// A raw value of `header`, such as the `sip_call_info` channel variable
    /// fetched over ESL, decoded into the rows a stored header gives.
    ///
    /// Type the field with sip-header's accessors, as any store here:
    ///
    /// ```
    /// use freeswitch_types::{EslHeaders, SipHeader};
    /// use sip_header::SipHeaderLookup;
    ///
    /// let raw = "ARRAY::<urn:emergency:uid:callid:bcf.test>;purpose=emergency-CallId\
    ///            |:<urn:emergency:uid:incidentid:bcf.test>;purpose=emergency-IncidentId";
    /// let field = EslHeaders::decode_value(SipHeader::CallInfo, raw)?;
    /// assert_eq!(field.call_info()?.unwrap().len(), 2);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// An `ARRAY::` past [`MAX_ARRAY_ITEMS`] is [`RowError::too_many_rows`].
    pub fn decode_value(header: SipHeader, raw: &str) -> Result<SipHeaderField<'_>, RowError> {
        Self::decode_value_raw(header.as_str(), raw)
    }

    /// [`decode_value`](Self::decode_value) for a header by name, one the
    /// catalog does not register included; such a value is one row, whole.
    ///
    /// # Errors
    ///
    /// An `ARRAY::` past [`MAX_ARRAY_ITEMS`] is [`RowError::too_many_rows`].
    pub fn decode_value_raw<'a>(
        name: &'a str,
        raw: &'a str,
    ) -> Result<SipHeaderField<'a>, RowError> {
        Ok(SipHeaderField::new(name, value_rows(name, Some(raw))?))
    }
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
    use sip_header::SipHeaderLookup;
    use sip_header_catalog::{RowErrorKind, SipHeaderRowsExt};

    /// Equal stores hash alike, whichever spelling and order they first saw.
    #[test]
    fn stores_equal_under_folding_hash_alike() {
        use std::hash::{BuildHasher, RandomState};

        let a: EslHeaders = [("C911P-Action", "x"), ("Unique-ID", "u")]
            .into_iter()
            .collect();
        let b: EslHeaders = [("unique-id", "u"), ("c911p-action", "x")]
            .into_iter()
            .collect();
        assert_eq!(a, b);
        let state = RandomState::new();
        assert_eq!(state.hash_one(&a), state.hash_one(&b));
    }

    #[test]
    fn iter_yields_stored_spellings_in_insertion_order() {
        let h: EslHeaders = [
            ("C911P-Action", "x"),
            ("unique-id", "u"),
            ("c911p-action", "y"),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            h.iter()
                .collect::<Vec<_>>(),
            [("C911P-Action", "y"), ("Unique-ID", "u")]
        );
    }

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
    fn with_prefix_matches_dash_keys_in_any_casing() {
        let h: EslHeaders = [
            ("C911P-Action", "dispatch"),
            ("C911P-Incident-Id", "inc-1"),
            ("Unique-ID", "abc"),
        ]
        .into_iter()
        .collect();
        for prefix in ["C911P-", "c911p-", "C911p-"] {
            let values: Vec<&str> = h
                .prefixed(prefix)
                .map(|(_, v)| v)
                .collect();
            assert_eq!(values, ["dispatch", "inc-1"], "{prefix}");
        }
    }

    #[test]
    fn with_prefix_compares_underscore_keys_exactly() {
        let h: EslHeaders = [("X-Foo_Bar", "under"), ("X-Plain", "dash")]
            .into_iter()
            .collect();
        let keys: Vec<&str> = h
            .prefixed("x-")
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, ["X-Plain"]);
        assert_eq!(
            h.prefixed("X-")
                .count(),
            2
        );
    }

    #[test]
    fn with_prefix_survives_a_multibyte_key() {
        let h: EslHeaders = [("É-Header", "x")]
            .into_iter()
            .collect();
        assert_eq!(
            h.prefixed("a")
                .count(),
            0
        );
        assert_eq!(
            h.prefixed("é-")
                .count(),
            0
        );
    }

    #[test]
    fn with_prefix_reaches_an_event_header_by_its_family() {
        let h: EslHeaders = [
            ("caller-caller-id-name", "Alice"),
            ("Other-Leg-Unique-ID", "x"),
        ]
        .into_iter()
        .collect();
        let keys: Vec<&str> = h
            .prefixed("CALLER-")
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, ["Caller-Caller-ID-Name"]);
    }

    /// A gRPC hop hands consumers a plain map; the store rebuilt from it must
    /// answer an unknown header and a configured name in any casing.
    #[test]
    fn a_store_from_a_wire_cased_map_answers_every_casing() {
        let map: std::collections::HashMap<String, String> = [
            ("C911P-Action", "dispatch"),
            ("caller-direction", "inbound"),
            ("variable_sip_h_X-Incident-Id", "inc-1"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let h: EslHeaders = map
            .into_iter()
            .collect();

        for name in ["C911P-Action", "C911p-Action", "c911p-action"] {
            assert_eq!(h.header_str(name), Some("dispatch"), "{name}");
        }
        for name in ["caller-direction", "Caller-Direction", "CALLER-DIRECTION"] {
            assert_eq!(h.header_str(name), Some("inbound"), "{name}");
        }
        assert_eq!(h.variable_str("sip_h_X-Incident-Id"), Some("inc-1"));
        assert_eq!(h.variable_str("sip_h_x-incident-id"), None);
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
    fn an_unknown_dash_key_keeps_its_first_spelling() {
        let mut h = EslHeaders::new();
        h.insert("C911P-Action", "first");
        h.insert("c911p-action", "second");
        assert_eq!(
            h.as_map()
                .iter()
                .collect::<Vec<_>>(),
            [(&"C911P-Action".to_string(), &"second".to_string())]
        );

        h.insert("unique-id", "abc");
        assert!(h
            .as_map()
            .contains_key("Unique-ID"));
    }

    #[test]
    fn one_header_under_two_first_spellings_is_one_store() {
        let a: EslHeaders = [("C911P-Action", "x")]
            .into_iter()
            .collect();
        let b: EslHeaders = [("c911p-action", "x")]
            .into_iter()
            .collect();
        assert_eq!(a, b);

        let c: EslHeaders = [("c911p-action", "y")]
            .into_iter()
            .collect();
        assert_ne!(a, c);
        let d: EslHeaders = [("variable_X-a", "x")]
            .into_iter()
            .collect();
        let e: EslHeaders = [("variable_x-a", "x")]
            .into_iter()
            .collect();
        assert_ne!(d, e);
    }

    /// Spools written before first-seen spelling hold Title-Cased keys.
    #[cfg(feature = "serde")]
    #[test]
    fn a_title_cased_spool_record_answers_the_wire_spelling() {
        let h: EslHeaders = serde_json::from_str(r#"{"C911p-Action":"dispatch"}"#).expect("record");
        assert_eq!(h.header_str("C911P-Action"), Some("dispatch"));
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
            assert_eq!(
                values
                    .get(0)
                    .and_then(|v| v.username()),
                Some("alice")
            );
            assert_eq!(
                values
                    .get(0)
                    .and_then(|v| v.realm()),
                Some("example.com")
            );
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
        assert_eq!(
            values
                .get(1)
                .and_then(|v| v.realm()),
            Some("r2")
        );
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
        assert_eq!(err.kind(), RowErrorKind::TooManyRows);
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
    fn a_raw_call_info_array_types_through_its_field() {
        let raw = "ARRAY::<urn:emergency:uid:callid:abc>;purpose=emergency-CallId\
                   |:<urn:emergency:uid:incidentid:def>;purpose=emergency-IncidentId";
        let field = EslHeaders::decode_value(SipHeader::CallInfo, raw)
            .expect("two items are under the cap");
        assert_eq!(field.name(), "Call-Info");
        assert_eq!(
            field
                .rows()
                .len(),
            2
        );
        let ci = field
            .call_info()
            .expect("both entries parse")
            .expect("header is present");
        assert_eq!(ci.len(), 2);
        assert_eq!(ci.entries()[1].purpose(), Some("emergency-IncidentId"));
    }

    /// The stored-header path and the raw-value one must agree, so every
    /// transport form runs through both.
    #[test]
    fn a_raw_value_decodes_like_a_stored_header() {
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
            let field = EslHeaders::decode_value(SipHeader::CallInfo, value)
                .unwrap_or_else(|e| panic!("{label} as a raw value: {e}"));
            let from_value = field
                .call_info()
                .unwrap_or_else(|e| panic!("{label} as a raw value: {e}"))
                .unwrap_or_else(|| panic!("{label}: field holds the header"));
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
    fn a_raw_over_cap_array_is_a_row_error() {
        let raw = over_cap_array("<sip:a@example.com>");
        for err in [
            EslHeaders::decode_value(SipHeader::CallInfo, &raw),
            EslHeaders::decode_value_raw("Call-Info", &raw),
        ] {
            let err = err.expect_err("over-cap ARRAY must fail");
            assert_eq!(err.kind(), RowErrorKind::TooManyRows);
        }
    }

    #[test]
    fn a_raw_value_of_an_unregistered_header_is_one_row() {
        let field =
            EslHeaders::decode_value_raw("X-Custom", "ARRAY::a|:b").expect("no framing to fail");
        assert_eq!(field.name(), "X-Custom");
        assert_eq!(
            field
                .rows()
                .collect::<Vec<_>>(),
            ["ARRAY::a|:b"]
        );
    }

    /// A registered name spelled by hand, in any casing, decodes as the typed header does,
    /// keeping the name as sent.
    #[test]
    fn a_registered_name_decodes_raw_as_typed() {
        let raw = "ARRAY::<sip:a@example.test>;purpose=icon|:<sip:b@example.test>";
        let typed = EslHeaders::decode_value(SipHeader::CallInfo, raw).unwrap();
        assert_eq!(typed.name(), SipHeader::CallInfo.as_str());
        for name in ["Call-Info", "call-info", "CALL-INFO"] {
            let field = EslHeaders::decode_value_raw(name, raw).unwrap();
            assert_eq!(field.name(), name);
            assert!(field
                .rows()
                .eq(typed.rows()));
        }
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
