//! FreeSWITCH SIP header passthrough variables (`sip_h_*`, `sip_i_*`, etc.).
//!
//! FreeSWITCH exposes SIP headers as channel variables through six prefixes,
//! each controlling a different direction or SIP method:
//!
//! | Prefix | Wire example | Purpose |
//! |---|---|---|
//! | `sip_i_` | `sip_i_call_info` | Read incoming INVITE headers |
//! | `sip_h_` | `sip_h_Call-Info` | Inject header on outgoing request |
//! | `sip_rh_` | `sip_rh_Call-Info` | Inject header on outgoing response |
//! | `sip_ph_` | `sip_ph_Call-Info` | Inject header on provisional response |
//! | `sip_bye_h_` | `sip_bye_h_Call-Info` | Inject header on BYE |
//! | `sip_nobye_h_` | `sip_nobye_h_Call-Info` | Suppress header on BYE |
//!
//! The `Invite` prefix uses a different wire format from the others:
//! lowercase with hyphens replaced by underscores (`sip_i_call_info`),
//! while all other prefixes preserve the canonical SIP header casing
//! (`sip_h_Call-Info`).

use sip_header::SipHeader;

/// Error returned when a raw header name is empty or carries a wire terminator.
///
/// The rejected name stays on the field; `Display` names only what was wrong
/// with it, since a header name a caller supplied can carry subscriber identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidHeaderName(pub String);

impl std::fmt::Display for InvalidHeaderName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self
            .0
            .is_empty()
        {
            return f.write_str("empty SIP header name");
        }
        write!(
            f,
            "invalid SIP header name ({} bytes): contains CR, LF or NUL",
            self.0
                .len()
        )
    }
}

impl std::error::Error for InvalidHeaderName {}

/// FreeSWITCH SIP header passthrough variable prefix.
///
/// Each prefix controls which SIP message direction the header variable
/// applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SipHeaderPrefix {
    /// `sip_i_` — raw INVITE header (read-only, set by `parse-all-invite-headers`).
    Invite,
    /// `sip_h_` — inject header on outgoing request (INVITE, REFER, etc.).
    Request,
    /// `sip_rh_` — inject header on outgoing response (200 OK, etc.).
    Response,
    /// `sip_ph_` — inject header on provisional response (180, 183).
    Provisional,
    /// `sip_bye_h_` — inject header on outgoing BYE.
    Bye,
    /// `sip_nobye_h_` — suppress a specific header on outgoing BYE.
    NoBye,
}

impl SipHeaderPrefix {
    /// Every prefix, in declaration order.
    pub const ALL: &'static [Self] = &[
        Self::Invite,
        Self::Request,
        Self::Response,
        Self::Provisional,
        Self::Bye,
        Self::NoBye,
    ];

    /// Wire prefix string including trailing separator.
    ///
    /// ```
    /// use freeswitch_types::SipHeaderPrefix;
    ///
    /// const PREFIX: &str = SipHeaderPrefix::Request.as_str();
    /// assert_eq!(PREFIX, "sip_h_");
    /// ```
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Invite => "sip_i_",
            Self::Request => "sip_h_",
            Self::Response => "sip_rh_",
            Self::Provisional => "sip_ph_",
            Self::Bye => "sip_bye_h_",
            Self::NoBye => "sip_nobye_h_",
        }
    }

    /// Whether mod_sofia copies a variable under this prefix into an outgoing
    /// SIP message as a header.
    #[cfg(feature = "esl")]
    pub(crate) const fn emits_header(self) -> bool {
        matches!(
            self,
            Self::Request | Self::Response | Self::Provisional | Self::Bye
        )
    }
}

/// Whether `var` names a variable mod_sofia sends as an outgoing SIP header.
/// The prefix matches case-insensitively, as `sofia_glue_get_extra_headers` does.
#[cfg(feature = "esl")]
pub(crate) fn names_outgoing_header(var: &str) -> bool {
    SipHeaderPrefix::ALL
        .iter()
        .filter(|p| p.emits_header())
        .any(|p| {
            var.get(
                ..p.as_str()
                    .len(),
            )
            .is_some_and(|head| head.eq_ignore_ascii_case(p.as_str()))
        })
}

impl std::fmt::Display for SipHeaderPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when parsing an unrecognized passthrough header variable name.
///
/// The rejected name stays on the field; `Display` names only which rule it
/// broke, since a variable name off the wire can carry subscriber identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseSipPassthroughError(pub String);

impl std::fmt::Display for ParseSipPassthroughError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let len = self
            .0
            .len();
        if PREFIX_PATTERNS
            .iter()
            .any(|p| {
                self.0
                    .eq_ignore_ascii_case(p.as_str())
            })
        {
            return write!(f, "SIP passthrough variable ({len} bytes): no header name");
        }
        write!(
            f,
            "not a SIP passthrough variable ({len} bytes): no recognised sip_*_ prefix"
        )
    }
}

impl std::error::Error for ParseSipPassthroughError {}

/// A FreeSWITCH SIP passthrough header variable name.
///
/// Combines a [`SipHeaderPrefix`] (direction/method) with a SIP header name
/// to produce the channel variable name used on the wire. Use with
/// [`HeaderLookup::variable()`](crate::HeaderLookup::variable) for lookups,
/// or insert into [`Variables`](crate::Variables) for originate commands.
///
/// # Typed constructors (known SIP headers)
///
/// ```
/// use freeswitch_types::variables::{SipPassthroughHeader, SipHeaderPrefix};
/// use sip_header::SipHeader;
///
/// let h = SipPassthroughHeader::request(SipHeader::CallInfo);
/// assert_eq!(h.as_str(), "sip_h_Call-Info");
///
/// let h = SipPassthroughHeader::invite(SipHeader::CallInfo);
/// assert_eq!(h.as_str(), "sip_i_call_info");
/// ```
///
/// # Raw constructors (custom headers)
///
/// ```
/// use freeswitch_types::variables::SipPassthroughHeader;
///
/// let h = SipPassthroughHeader::request_raw("X-Tenant").unwrap();
/// assert_eq!(h.as_str(), "sip_h_X-Tenant");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SipPassthroughHeader {
    prefix: SipHeaderPrefix,
    canonical_name: String,
    wire: String,
}

/// Prefixes longest-first, so `FromStr` never matches a shorter one first.
const PREFIX_PATTERNS: &[SipHeaderPrefix] = &[
    SipHeaderPrefix::NoBye,
    SipHeaderPrefix::Bye,
    SipHeaderPrefix::Provisional,
    SipHeaderPrefix::Response,
    SipHeaderPrefix::Invite,
    SipHeaderPrefix::Request,
];

/// Canonical header name for a wire suffix, reversing the `Invite` prefix's
/// lowercase-and-underscore spelling.
fn canonical_from_suffix(prefix: SipHeaderPrefix, suffix: &str) -> String {
    match prefix {
        SipHeaderPrefix::Invite => {
            let with_hyphens = suffix.replace('_', "-");
            match with_hyphens.parse::<SipHeader>() {
                Ok(h) => h
                    .as_str()
                    .to_string(),
                Err(_) => with_hyphens,
            }
        }
        _ => match suffix.parse::<SipHeader>() {
            Ok(h) => h
                .as_str()
                .to_string(),
            Err(_) => suffix.to_string(),
        },
    }
}

fn validate_header_name(name: &str) -> Result<(), InvalidHeaderName> {
    if name.is_empty() || crate::wire_safety::contains_wire_terminator(name) {
        return Err(InvalidHeaderName(name.to_string()));
    }
    Ok(())
}

fn build_wire(prefix: SipHeaderPrefix, canonical: &str) -> String {
    match prefix {
        SipHeaderPrefix::Invite => {
            let pfx = SipHeaderPrefix::Invite.as_str();
            let mut wire = String::with_capacity(pfx.len() + canonical.len());
            wire.push_str(pfx);
            for ch in canonical.chars() {
                if ch == '-' {
                    wire.push('_');
                } else {
                    wire.push(ch.to_ascii_lowercase());
                }
            }
            wire
        }
        _ => {
            let pfx = prefix.as_str();
            let mut wire = String::with_capacity(pfx.len() + canonical.len());
            wire.push_str(pfx);
            wire.push_str(canonical);
            wire
        }
    }
}

impl SipPassthroughHeader {
    /// Create from a prefix and a known [`SipHeader`].
    pub fn new(prefix: SipHeaderPrefix, header: SipHeader) -> Self {
        let canonical = header
            .as_str()
            .to_string();
        let wire = build_wire(prefix, &canonical);
        Self {
            prefix,
            canonical_name: canonical,
            wire,
        }
    }

    /// Create from a prefix and an arbitrary header name.
    ///
    /// Returns `Err` if the name contains `\n` or `\r` (wire injection risk).
    pub fn new_raw(
        prefix: SipHeaderPrefix,
        name: impl Into<String>,
    ) -> Result<Self, InvalidHeaderName> {
        let canonical = name.into();
        validate_header_name(&canonical)?;
        let wire = build_wire(prefix, &canonical);
        Ok(Self {
            prefix,
            canonical_name: canonical,
            wire,
        })
    }

    /// Incoming INVITE header (`sip_i_*`).
    pub fn invite(header: SipHeader) -> Self {
        Self::new(SipHeaderPrefix::Invite, header)
    }

    /// Incoming INVITE header from raw name.
    pub fn invite_raw(name: impl Into<String>) -> Result<Self, InvalidHeaderName> {
        Self::new_raw(SipHeaderPrefix::Invite, name)
    }

    /// Outgoing request header (`sip_h_*`).
    pub fn request(header: SipHeader) -> Self {
        Self::new(SipHeaderPrefix::Request, header)
    }

    /// Outgoing request header from raw name.
    pub fn request_raw(name: impl Into<String>) -> Result<Self, InvalidHeaderName> {
        Self::new_raw(SipHeaderPrefix::Request, name)
    }

    /// Outgoing response header (`sip_rh_*`).
    pub fn response(header: SipHeader) -> Self {
        Self::new(SipHeaderPrefix::Response, header)
    }

    /// Outgoing response header from raw name.
    pub fn response_raw(name: impl Into<String>) -> Result<Self, InvalidHeaderName> {
        Self::new_raw(SipHeaderPrefix::Response, name)
    }

    /// Provisional response header (`sip_ph_*`).
    pub fn provisional(header: SipHeader) -> Self {
        Self::new(SipHeaderPrefix::Provisional, header)
    }

    /// Provisional response header from raw name.
    pub fn provisional_raw(name: impl Into<String>) -> Result<Self, InvalidHeaderName> {
        Self::new_raw(SipHeaderPrefix::Provisional, name)
    }

    /// BYE request header (`sip_bye_h_*`).
    pub fn bye(header: SipHeader) -> Self {
        Self::new(SipHeaderPrefix::Bye, header)
    }

    /// BYE request header from raw name.
    pub fn bye_raw(name: impl Into<String>) -> Result<Self, InvalidHeaderName> {
        Self::new_raw(SipHeaderPrefix::Bye, name)
    }

    /// Suppress header on BYE (`sip_nobye_h_*`).
    pub fn no_bye(header: SipHeader) -> Self {
        Self::new(SipHeaderPrefix::NoBye, header)
    }

    /// Suppress header on BYE from raw name.
    pub fn no_bye_raw(name: impl Into<String>) -> Result<Self, InvalidHeaderName> {
        Self::new_raw(SipHeaderPrefix::NoBye, name)
    }

    /// The prefix (direction/method) of this variable.
    pub fn prefix(&self) -> SipHeaderPrefix {
        self.prefix
    }

    /// The canonical SIP header name (e.g. `"Call-Info"`, `"X-Tenant"`).
    pub fn canonical_name(&self) -> &str {
        &self.canonical_name
    }

    /// The pre-computed wire variable name (e.g. `"sip_h_Call-Info"`).
    pub fn as_str(&self) -> &str {
        &self.wire
    }

    /// Extract this header's values from a raw SIP message.
    ///
    /// Returns each occurrence as a separate entry (RFC 3261 §7.3.1).
    /// Delegates to [`sip_header::extract_header()`] using the canonical name.
    pub fn extract_from(&self, message: &str) -> Vec<String> {
        sip_header::extract_header(message, &self.canonical_name)
    }

    /// Whether this header may contain ARRAY-encoded values when read from FreeSWITCH.
    ///
    /// Only meaningful for the `Invite` prefix — FreeSWITCH stores multi-valued
    /// incoming SIP headers in `ARRAY::val1|:val2` format. For other prefixes
    /// (which are write-only), this always returns `false`.
    pub fn is_array_header(&self) -> bool {
        if self.prefix != SipHeaderPrefix::Invite {
            return false;
        }
        self.canonical_name
            .parse::<SipHeader>()
            .map(|h| h.is_multi_valued())
            .unwrap_or(false)
    }
}

impl super::VariableName for SipPassthroughHeader {
    fn as_str(&self) -> &str {
        &self.wire
    }
}

impl std::fmt::Display for SipPassthroughHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.wire)
    }
}

impl AsRef<str> for SipPassthroughHeader {
    fn as_ref(&self) -> &str {
        &self.wire
    }
}

impl From<SipPassthroughHeader> for String {
    fn from(h: SipPassthroughHeader) -> Self {
        h.wire
    }
}

impl std::str::FromStr for SipPassthroughHeader {
    type Err = ParseSipPassthroughError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        for &prefix in PREFIX_PATTERNS {
            let pfx = prefix.as_str();
            if !s
                .get(..pfx.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(pfx))
            {
                continue;
            }
            let suffix = &s[pfx.len()..];
            if suffix.is_empty() {
                return Err(ParseSipPassthroughError(s.to_string()));
            }
            let canonical = canonical_from_suffix(prefix, suffix);
            let wire = match prefix {
                SipHeaderPrefix::Invite => build_wire(prefix, &canonical),
                _ => format!("{pfx}{suffix}"),
            };
            return Ok(Self {
                prefix,
                canonical_name: canonical,
                wire,
            });
        }

        Err(ParseSipPassthroughError(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Display / wire format ---

    #[test]
    fn invite_typed_wire_format() {
        assert_eq!(
            SipPassthroughHeader::invite(SipHeader::CallInfo).as_str(),
            "sip_i_call_info"
        );
        assert_eq!(
            SipPassthroughHeader::invite(SipHeader::PAssertedIdentity).as_str(),
            "sip_i_p_asserted_identity"
        );
        assert_eq!(
            SipPassthroughHeader::invite(SipHeader::Via).as_str(),
            "sip_i_via"
        );
        assert_eq!(
            SipPassthroughHeader::invite(SipHeader::CallId).as_str(),
            "sip_i_call_id"
        );
    }

    #[test]
    fn request_typed_wire_format() {
        assert_eq!(
            SipPassthroughHeader::request(SipHeader::CallInfo).as_str(),
            "sip_h_Call-Info"
        );
        assert_eq!(
            SipPassthroughHeader::request(SipHeader::PAssertedIdentity).as_str(),
            "sip_h_P-Asserted-Identity"
        );
    }

    #[test]
    fn response_typed_wire_format() {
        assert_eq!(
            SipPassthroughHeader::response(SipHeader::CallInfo).as_str(),
            "sip_rh_Call-Info"
        );
    }

    #[test]
    fn provisional_typed_wire_format() {
        assert_eq!(
            SipPassthroughHeader::provisional(SipHeader::AlertInfo).as_str(),
            "sip_ph_Alert-Info"
        );
    }

    #[test]
    fn bye_typed_wire_format() {
        assert_eq!(
            SipPassthroughHeader::bye(SipHeader::Reason).as_str(),
            "sip_bye_h_Reason"
        );
    }

    #[test]
    fn no_bye_typed_wire_format() {
        assert_eq!(
            SipPassthroughHeader::no_bye(SipHeader::Reason).as_str(),
            "sip_nobye_h_Reason"
        );
    }

    #[test]
    fn raw_custom_header() {
        assert_eq!(
            SipPassthroughHeader::request_raw("X-Tenant")
                .unwrap()
                .as_str(),
            "sip_h_X-Tenant"
        );
        assert_eq!(
            SipPassthroughHeader::invite_raw("X-Custom")
                .unwrap()
                .as_str(),
            "sip_i_x_custom"
        );
    }

    #[test]
    fn raw_rejects_newlines() {
        assert!(SipPassthroughHeader::request_raw("X-Bad\nHeader").is_err());
        assert!(SipPassthroughHeader::request_raw("X-Bad\rHeader").is_err());
        assert!(SipPassthroughHeader::request_raw("X-Bad\0Header").is_err());
        assert!(SipPassthroughHeader::request_raw("").is_err());
    }

    /// A rejected header name is caller-supplied and routinely carries
    /// subscriber identity, so the value stays on the field.
    #[test]
    fn invalid_name_error_names_the_shape_not_the_value() {
        let err = SipPassthroughHeader::request_raw("X-Tenant\nInjected").unwrap_err();
        let msg = err.to_string();
        assert!(!msg.contains("X-Tenant"), "error quoted its input: {msg}");
        assert!(
            msg.contains("17 bytes"),
            "error does not give the byte length: {msg}"
        );
        assert_eq!(err.0, "X-Tenant\nInjected");
    }

    #[test]
    fn empty_name_error_says_which_rule_it_broke() {
        let msg = SipPassthroughHeader::request_raw("")
            .unwrap_err()
            .to_string();
        assert!(msg.contains("empty"), "{msg}");
    }

    #[test]
    fn parse_error_names_the_shape_not_the_value() {
        let msg = "X-Tenant: secret"
            .parse::<SipPassthroughHeader>()
            .unwrap_err()
            .to_string();
        assert!(!msg.contains("secret"), "error quoted its input: {msg}");
        assert!(
            msg.contains("16 bytes"),
            "error does not give the byte length: {msg}"
        );
    }

    #[test]
    fn parse_error_separates_a_missing_header_name_from_a_missing_prefix() {
        let msg = "sip_h_"
            .parse::<SipPassthroughHeader>()
            .unwrap_err()
            .to_string();
        assert!(msg.contains("no header name"), "{msg}");
    }

    // --- Display trait ---

    #[test]
    fn display_matches_as_str() {
        let h = SipPassthroughHeader::request(SipHeader::CallInfo);
        assert_eq!(h.to_string(), h.as_str());
    }

    // --- FromStr round-trips ---

    #[test]
    fn from_str_request() {
        let parsed: SipPassthroughHeader = "sip_h_Call-Info"
            .parse()
            .unwrap();
        assert_eq!(parsed.prefix(), SipHeaderPrefix::Request);
        assert_eq!(parsed.canonical_name(), "Call-Info");
        assert_eq!(parsed.as_str(), "sip_h_Call-Info");
    }

    #[test]
    fn from_str_invite() {
        let parsed: SipPassthroughHeader = "sip_i_call_info"
            .parse()
            .unwrap();
        assert_eq!(parsed.prefix(), SipHeaderPrefix::Invite);
        assert_eq!(parsed.canonical_name(), "Call-Info");
        assert_eq!(parsed.as_str(), "sip_i_call_info");
    }

    #[test]
    fn from_str_invite_p_asserted_identity() {
        let parsed: SipPassthroughHeader = "sip_i_p_asserted_identity"
            .parse()
            .unwrap();
        assert_eq!(parsed.prefix(), SipHeaderPrefix::Invite);
        assert_eq!(parsed.canonical_name(), "P-Asserted-Identity");
    }

    #[test]
    fn from_str_response() {
        let parsed: SipPassthroughHeader = "sip_rh_Call-Info"
            .parse()
            .unwrap();
        assert_eq!(parsed.prefix(), SipHeaderPrefix::Response);
        assert_eq!(parsed.canonical_name(), "Call-Info");
    }

    #[test]
    fn from_str_bye() {
        let parsed: SipPassthroughHeader = "sip_bye_h_Reason"
            .parse()
            .unwrap();
        assert_eq!(parsed.prefix(), SipHeaderPrefix::Bye);
        assert_eq!(parsed.canonical_name(), "Reason");
    }

    #[test]
    fn from_str_no_bye() {
        let parsed: SipPassthroughHeader = "sip_nobye_h_Reason"
            .parse()
            .unwrap();
        assert_eq!(parsed.prefix(), SipHeaderPrefix::NoBye);
        assert_eq!(parsed.canonical_name(), "Reason");
    }

    #[test]
    fn from_str_unknown_custom_header() {
        let parsed: SipPassthroughHeader = "sip_h_X-Tenant"
            .parse()
            .unwrap();
        assert_eq!(parsed.prefix(), SipHeaderPrefix::Request);
        assert_eq!(parsed.canonical_name(), "X-Tenant");
    }

    #[test]
    fn from_str_invite_unknown_custom() {
        let parsed: SipPassthroughHeader = "sip_i_x_custom"
            .parse()
            .unwrap();
        assert_eq!(parsed.prefix(), SipHeaderPrefix::Invite);
        assert_eq!(parsed.canonical_name(), "x-custom");
    }

    #[test]
    fn from_str_case_insensitive_invite() {
        let parsed: SipPassthroughHeader = "SIP_I_CALL_INFO"
            .parse()
            .unwrap();
        assert_eq!(parsed.prefix(), SipHeaderPrefix::Invite);
        assert_eq!(parsed.canonical_name(), "Call-Info");
    }

    #[test]
    fn from_str_case_insensitive_prefix_keeps_the_suffix_as_written() {
        let parsed: SipPassthroughHeader = "SIP_H_X-Tenant"
            .parse()
            .unwrap();
        assert_eq!(parsed.prefix(), SipHeaderPrefix::Request);
        assert_eq!(parsed.canonical_name(), "X-Tenant");
        assert_eq!(parsed.as_str(), "sip_h_X-Tenant");
    }

    #[test]
    fn from_str_rejects_no_prefix() {
        assert!("call_info"
            .parse::<SipPassthroughHeader>()
            .is_err());
        assert!("sip_call_info"
            .parse::<SipPassthroughHeader>()
            .is_err());
    }

    #[test]
    fn from_str_rejects_empty_suffix() {
        assert!("sip_h_"
            .parse::<SipPassthroughHeader>()
            .is_err());
        assert!("sip_i_"
            .parse::<SipPassthroughHeader>()
            .is_err());
    }

    #[test]
    fn from_str_round_trip_all_prefixes() {
        let headers = [
            SipPassthroughHeader::invite(SipHeader::CallInfo),
            SipPassthroughHeader::request(SipHeader::CallInfo),
            SipPassthroughHeader::response(SipHeader::CallInfo),
            SipPassthroughHeader::provisional(SipHeader::AlertInfo),
            SipPassthroughHeader::bye(SipHeader::Reason),
            SipPassthroughHeader::no_bye(SipHeader::Reason),
        ];
        for h in &headers {
            let parsed: SipPassthroughHeader = h
                .as_str()
                .parse()
                .unwrap();
            assert_eq!(&parsed, h, "round-trip failed for {}", h.as_str());
        }
    }

    // --- Accessors ---

    #[test]
    fn prefix_accessor() {
        assert_eq!(
            SipPassthroughHeader::invite(SipHeader::Via).prefix(),
            SipHeaderPrefix::Invite
        );
        assert_eq!(
            SipPassthroughHeader::request(SipHeader::Via).prefix(),
            SipHeaderPrefix::Request
        );
    }

    #[test]
    fn canonical_name_accessor() {
        assert_eq!(
            SipPassthroughHeader::invite(SipHeader::CallInfo).canonical_name(),
            "Call-Info"
        );
        assert_eq!(
            SipPassthroughHeader::request_raw("X-Tenant")
                .unwrap()
                .canonical_name(),
            "X-Tenant"
        );
    }

    // --- is_array_header ---

    #[test]
    fn is_array_header_invite_multi_valued() {
        assert!(SipPassthroughHeader::invite(SipHeader::Via).is_array_header());
        assert!(SipPassthroughHeader::invite(SipHeader::CallInfo).is_array_header());
        assert!(SipPassthroughHeader::invite(SipHeader::PAssertedIdentity).is_array_header());
        assert!(SipPassthroughHeader::invite(SipHeader::RecordRoute).is_array_header());
    }

    #[test]
    fn is_array_header_invite_single_valued() {
        assert!(!SipPassthroughHeader::invite(SipHeader::From).is_array_header());
        assert!(!SipPassthroughHeader::invite(SipHeader::CallId).is_array_header());
        assert!(!SipPassthroughHeader::invite(SipHeader::ContentType).is_array_header());
    }

    #[test]
    fn is_array_header_non_invite_always_false() {
        assert!(!SipPassthroughHeader::request(SipHeader::Via).is_array_header());
        assert!(!SipPassthroughHeader::response(SipHeader::CallInfo).is_array_header());
    }

    #[test]
    fn is_array_header_raw_unknown() {
        assert!(!SipPassthroughHeader::invite_raw("X-Custom")
            .unwrap()
            .is_array_header());
    }

    // --- extract_from ---

    #[test]
    fn extract_from_sip_message() {
        let msg = "INVITE sip:bob@example.com SIP/2.0\r\n\
                   Call-Info: <sip:example.com>;answer-after=0\r\n\
                   \r\n";
        let h = SipPassthroughHeader::invite(SipHeader::CallInfo);
        assert_eq!(
            h.extract_from(msg),
            vec!["<sip:example.com>;answer-after=0"]
        );
    }

    #[test]
    fn extract_from_missing() {
        let msg = "INVITE sip:bob@example.com SIP/2.0\r\n\
                   From: Alice <sip:alice@example.com>\r\n\
                   \r\n";
        let h = SipPassthroughHeader::invite(SipHeader::CallInfo);
        assert!(h
            .extract_from(msg)
            .is_empty());
    }

    // --- PREFIX_PATTERNS completeness ---

    #[test]
    fn prefix_patterns_covers_all_variants_exactly_once() {
        for &variant in SipHeaderPrefix::ALL {
            let count = PREFIX_PATTERNS
                .iter()
                .filter(|&&p| p == variant)
                .count();
            assert_eq!(
                count, 1,
                "{:?} appears {count} times in PREFIX_PATTERNS (expected exactly 1)",
                variant
            );
        }
        assert_eq!(
            PREFIX_PATTERNS.len(),
            SipHeaderPrefix::ALL.len(),
            "PREFIX_PATTERNS length differs from SipHeaderPrefix::ALL"
        );
    }

    // --- VariableName trait ---

    #[test]
    fn variable_name_trait() {
        use crate::variables::VariableName;
        let h = SipPassthroughHeader::request(SipHeader::CallInfo);
        let name: &str = VariableName::as_str(&h);
        assert_eq!(name, "sip_h_Call-Info");
    }
}
