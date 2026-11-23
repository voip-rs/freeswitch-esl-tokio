//! The sofia endpoints and what mod_sofia reads of them. A leg's name follows `sofia_glue_set_name`
//! over the destination `sofia_outgoing_channel` keeps, under the profile as written, never resolved.

use std::borrow::Cow;
use std::ops::Range;

use super::{after_prefix, check_expression_field, check_field, undeliverable, EndpointFieldFault};
use crate::commands::originate::OriginateError;
use crate::commands::variables::Variables;
use crate::switch_passes::originate_legs::splits_into_threads;

/// SIP endpoint via a named profile: `sofia/{profile}/{destination}`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "config::SofiaEndpoint"))]
pub struct SofiaEndpoint {
    /// SIP profile name (e.g. `internal`, `external`).
    profile: String,
    /// SIP URI or destination number.
    destination: String,
    /// Per-channel variables prepended as `{key=value}`.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    variables: Option<Variables>,
}

/// SIP endpoint via a configured gateway:
/// `sofia/gateway/[{profile}::]{gateway}/{destination}`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "config::SofiaGateway"))]
pub struct SofiaGateway {
    /// Gateway name as configured in the SIP profile.
    gateway: String,
    /// Destination number or SIP user part.
    destination: String,
    /// SIP profile name to qualify the gateway lookup.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    profile: Option<String>,
    /// Per-channel variables prepended as `{key=value}`.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    variables: Option<Variables>,
}

/// Runtime expression resolving registered SIP contacts:
/// `${sofia_contact([profile/]user@domain)}`.
///
/// The library produces the expression string; a dialplan application or the
/// `expand` API evaluates it, and `originate` over the API never does. Parse one
/// with [`Endpoint::parse_for`](super::Endpoint::parse_for) at
/// [`DialStringCarrier::Dialplan`](crate::commands::variables::DialStringCarrier::Dialplan).
///
/// The special profile `"*"` makes FreeSWITCH iterate over every loaded
/// SIP profile to find the user's registration, instead of searching a
/// single named profile (see `sofia_contact_function` in
/// `mod_sofia.c` where `strcmp(profile_name, "*")` skips the single-profile
/// lookup and falls through to the all-profiles hash iteration).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "config::SofiaContact"))]
pub struct SofiaContact {
    /// User part of the contact lookup.
    user: String,
    /// Domain for the contact lookup.
    domain: String,
    /// SIP profile name, or `"*"` for all profiles.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    profile: Option<String>,
    /// Per-channel variables prepended as `{key=value}`.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    variables: Option<Variables>,
}

/// `SWITCH_URL_UNSAFE`: a byte of it ahead of the user's `@` makes `protect_dest_uri` rewrite.
pub(super) const URL_UNSAFE: &[u8] = b"\r\n #%&+:;<=>?@[\\]^`{|}\"";

/// The digits an escape the switch's URL code leaves alone is written in: uppercase only.
const ESCAPE_HEX: &[u8] = b"0123456789ABCDEF";

/// Whether `bytes` opens with a `%XX` escape the switch's URL code leaves alone.
fn opens_an_escape(bytes: &[u8]) -> bool {
    matches!(bytes, [b'%', high, low, ..] if ESCAPE_HEX.contains(high) && ESCAPE_HEX.contains(low))
}

/// `switch_needs_url_encode`: a `SWITCH_URL_UNSAFE` byte outside a `%XX` escape.
fn needs_url_encode(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        if opens_an_escape(&bytes[at..]) {
            at += 3;
            continue;
        }
        if URL_UNSAFE.contains(&byte) {
            return true;
        }
        at += 1;
    }
    false
}

/// `switch_url_encode_opt`, writing a control, non-ASCII or `SWITCH_URL_UNSAFE` byte as `%XX`, and
/// a `%XX` escape's `%` too when `double_encode`. The fork (since f47ca7c) always double-encodes.
fn url_encode(text: &str, double_encode: bool) -> String {
    let bytes = text.as_bytes();
    let mut encoded = String::with_capacity(bytes.len() * 3);
    for (at, &byte) in bytes
        .iter()
        .enumerate()
    {
        let kept = !double_encode && opens_an_escape(&bytes[at..]);
        if !kept && (!(b' '..=b'~').contains(&byte) || URL_UNSAFE.contains(&byte)) {
            encoded.push('%');
            encoded.push(char::from(ESCAPE_HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(ESCAPE_HEX[usize::from(byte & 0x0f)]));
        } else {
            encoded.push(char::from(byte));
        }
    }
    encoded
}

/// Whether `protect_dest_uri` rewrites a sofia destination before mod_sofia reads it, which
/// moves where the dialed user ends.
///
/// `Suppressed` when `sofia_suppress_url_encoding` is true for the leg: set in the dial
/// string's `<>`, `{}` or `[]` block, or exported by the originating channel. A variable
/// only `set` on the originating channel, or a global, never reaches the leg. Otherwise
/// `Encoded`, the switch's default. The dial string cannot settle it alone, so the caller
/// names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DestinationEncoding {
    /// `sofia_suppress_url_encoding` unset or false.
    Encoded,
    /// `sofia_suppress_url_encoding` true.
    Suppressed,
}

impl SofiaEndpoint {
    /// Create a new SIP profile endpoint, refusing what parse refuses.
    pub fn new(
        profile: impl Into<String>,
        destination: impl Into<String>,
    ) -> Result<Self, OriginateError> {
        Self {
            profile: profile.into(),
            destination: destination.into(),
            variables: None,
        }
        .checked()
    }

    /// SIP profile name (e.g. `internal`, `external`).
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// SIP URI or destination number.
    pub fn destination(&self) -> &str {
        &self.destination
    }

    /// The user this leg dials, unencoded, cut from `destination` where mod_sofia cuts it.
    ///
    /// See [`DestinationEncoding`] for which to pass: `1000^2000@pbx.example.com` dials
    /// `1000^2000` when `Encoded`, and `1000` with a `2000` To override when `Suppressed`.
    /// Without `@` it is the registered user the profile looks up. `None` for a scheme with no
    /// `@`, or an empty user. Owned only where the switch rebuilds text around a `/`.
    /// `sip_destination_prefix`, `sip_invite_tel_params` and `sip_invite_req_uri` are not
    /// applied.
    pub fn user(&self, encoding: DestinationEncoding) -> Option<Cow<'_, str>> {
        dialed_user(&self.destination, encoding, DialPath::Profile)
    }

    /// The name the switch gives this leg, `sofia/<profile>/<destination as mod_sofia keeps it>`,
    /// as its `New Channel` log line and its `Channel-Name` header show it.
    ///
    /// The profile is as written: a leg dialed through a profile alias or domain carries the
    /// profile's own name, which only the switch config knows. `None` where the dial string cannot
    /// settle the name: the encoder runs on a user holding a valid uppercase `%XX`, which upstream
    /// keeps and some builds encode again, and the name keeps the difference; the name fills
    /// mod_sofia's 256-byte buffer before any `;`; or the leg would look up an empty registered
    /// user. A later rename (`origination_channel_name`, `push_channel_name`) is not modelled; the
    /// switch logs it as `Rename Channel`.
    /// [`SofiaChannelName`](crate::sofia::SofiaChannelName) parses the result.
    pub fn channel_name(&self, encoding: DestinationEncoding) -> Option<String> {
        let upstream = self.channel_name_encoding(encoding, false);
        let fork = self.channel_name_encoding(encoding, true);
        (upstream == fork)
            .then_some(upstream)
            .flatten()
    }

    /// [`channel_name`](Self::channel_name) on a tree whose encoder runs with `double_encode`.
    pub(super) fn channel_name_encoding(
        &self,
        encoding: DestinationEncoding,
        double_encode: bool,
    ) -> Option<String> {
        // `encoded` is the `mod` flag `protect_dest_uri` returns, which turns off the `%` cut.
        let (text, encoded) = match encoding {
            DestinationEncoding::Encoded => match protect_dest_uri(&self.destination) {
                (text, Some(user)) if needs_url_encode(&text[user.clone()]) => (
                    Cow::Owned(format!(
                        "{}{}{}",
                        &text[..user.start],
                        url_encode(&text[user.clone()], double_encode),
                        &text[user.end..]
                    )),
                    true,
                ),
                (text, _) => (text, false),
            },
            DestinationEncoding::Suppressed => (Cow::Borrowed(self.destination()), false),
        };
        fn head(text: &str, cut: char) -> &str {
            text.split_once(cut)
                .map_or(text, |(head, _)| head)
        }
        let mut dest = head(&text, '^');
        if scheme_len(dest) == 0 && !dest.contains('@') {
            if !encoded {
                dest = head(dest, '%');
            }
            if dest.is_empty() {
                return None;
            }
        }
        set_name(&self.profile, dest)
    }
}

/// `char name[256]` in `sofia_glue_set_name`.
const CHANNEL_NAME_BUFFER: usize = 256;

/// `sofia_glue_set_name`: `sofia/<profile>/<dest>` cut at its first `;`, `None` where the buffer
/// fills before one.
fn set_name(profile: &str, dest: &str) -> Option<String> {
    let mut name = format!("sofia/{profile}/{dest}");
    let kept = name
        .len()
        .min(CHANNEL_NAME_BUFFER - 1);
    match name.as_bytes()[..kept]
        .iter()
        .position(|&byte| byte == b';')
    {
        Some(semicolon) => name.truncate(semicolon),
        None if name.len() > kept => return None,
        None => {}
    }
    Some(name)
}

impl SofiaGateway {
    /// Create a new SIP gateway endpoint, refusing what parse refuses.
    pub fn new(
        gateway: impl Into<String>,
        destination: impl Into<String>,
    ) -> Result<Self, OriginateError> {
        Self {
            gateway: gateway.into(),
            destination: destination.into(),
            profile: None,
            variables: None,
        }
        .checked()
    }

    /// Create a gateway endpoint under `profile`, for a gateway name holding `::`, which the
    /// lookup key reads as written only after a profile.
    pub fn on_profile(
        profile: impl Into<String>,
        gateway: impl Into<String>,
        destination: impl Into<String>,
    ) -> Result<Self, OriginateError> {
        Self {
            gateway: gateway.into(),
            destination: destination.into(),
            profile: Some(profile.into()),
            variables: None,
        }
        .checked()
    }

    /// Set the SIP profile qualifier, refusing what parse refuses.
    pub fn with_profile(mut self, profile: impl Into<String>) -> Result<Self, OriginateError> {
        self.profile = Some(profile.into());
        self.checked()
    }

    /// Gateway name as configured in the SIP profile.
    pub fn gateway(&self) -> &str {
        &self.gateway
    }

    /// Destination number or SIP user part.
    pub fn destination(&self) -> &str {
        &self.destination
    }

    /// SIP profile name qualifying the gateway lookup.
    pub fn profile(&self) -> Option<&str> {
        self.profile
            .as_deref()
    }

    /// The user this leg dials, unencoded, cut from `destination` where mod_sofia cuts it.
    ///
    /// See [`DestinationEncoding`] for which to pass: `1000^2000@pbx.example.com` dials
    /// `1000^2000` when `Encoded`, and `1000` with a `2000` To override when `Suppressed`.
    /// Without `@` it is the whole destination, the switch appending the gateway's proxy host.
    /// `;` parameters are dropped unless `Encoded` folds them into the user, and `Suppressed`
    /// keeps a `sip:` scheme as text. `None` for an empty user. Owned only where the switch
    /// rebuilds text around a `/`. The gateway's `destination-prefix`, `sip_invite_tel_params`
    /// and `sip_invite_req_uri` are not applied.
    pub fn user(&self, encoding: DestinationEncoding) -> Option<Cow<'_, str>> {
        dialed_user(&self.destination, encoding, DialPath::Gateway)
    }
}

#[derive(Clone, Copy)]
enum DialPath {
    Profile,
    Gateway,
}

/// The user `sofia_outgoing_channel` dials from `destination`, read behind a deliverable profile
/// or gateway.
fn dialed_user(
    destination: &str,
    encoding: DestinationEncoding,
    path: DialPath,
) -> Option<Cow<'_, str>> {
    let (text, encoded) = match encoding {
        DestinationEncoding::Encoded => protect_dest_uri(destination),
        DestinationEncoding::Suppressed => (Cow::Borrowed(destination), None),
    };
    let find = |byte: u8, from: usize, end: usize| {
        (from..end).find(|&at| {
            text.as_bytes()[at] == byte
                && !encoded
                    .as_ref()
                    .is_some_and(|user| user.contains(&at))
        })
    };
    let end = find(b'^', 0, text.len()).unwrap_or(text.len());
    let user = match path {
        DialPath::Profile => {
            let scheme = match &encoded {
                Some(user) if user.start == 0 => 0,
                _ => scheme_len(&text[..end]),
            };
            match (scheme, find(b'@', 0, end)) {
                (0, Some(at)) => 0..at,
                (0, None) => 0..find(b'%', 0, end).unwrap_or(end),
                (len, at) => len..at?,
            }
        }
        DialPath::Gateway => {
            let end = find(b';', 0, end).unwrap_or(end);
            0..find(b'@', 0, end).unwrap_or(end)
        }
    };
    if user.is_empty() {
        return None;
    }
    Some(match text {
        Cow::Borrowed(text) => Cow::Borrowed(&text[user]),
        Cow::Owned(text) => Cow::Owned(text[user].to_owned()),
    })
}

/// Length of a `sip:` or `sips:` scheme opening `text`, in any case, else 0.
fn scheme_len(text: &str) -> usize {
    ["sip:", "sips:"]
        .into_iter()
        .find(|scheme| {
            text.get(..scheme.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(scheme))
        })
        .map_or(0, str::len)
}

/// `destination` as `protect_dest_uri` leaves it behind a deliverable profile or gateway, with the
/// span of the user it encodes, where no later cut reads a separator.
fn protect_dest_uri(destination: &str) -> (Cow<'_, str>, Option<Range<usize>>) {
    let unchanged = (Cow::Borrowed(destination), None);
    if !destination.contains('@') {
        return unchanged;
    }
    let segment_start = destination
        .rfind('/')
        .map_or(0, |slash| slash + 1);
    let (prefix, segment) = destination.split_at(segment_start);
    let ahead = segment
        .split_once('@')
        .map_or(segment, |(ahead, _)| ahead);
    if !ahead
        .bytes()
        .any(|byte| URL_UNSAFE.contains(&byte))
    {
        return unchanged;
    }
    let scheme = scheme_len(segment);
    let rest = &segment[scheme..];
    let Some(at) = rest.find('@') else {
        return (
            Cow::Borrowed(&destination[..segment_start.saturating_sub(1)]),
            None,
        );
    };
    let user = prefix.len()..prefix.len() + at;
    let text = match (scheme, prefix.is_empty()) {
        (0, _) => Cow::Borrowed(destination),
        (_, true) => Cow::Borrowed(rest),
        (_, false) => Cow::Owned(format!("{prefix}{rest}")),
    };
    (text, Some(user))
}

impl SofiaContact {
    /// Create a new sofia_contact runtime expression, refusing what parse refuses.
    pub fn new(user: impl Into<String>, domain: impl Into<String>) -> Result<Self, OriginateError> {
        Self {
            user: user.into(),
            domain: domain.into(),
            profile: None,
            variables: None,
        }
        .checked()
    }

    /// Create a sofia_contact expression under `profile`, for a user holding `/`, which the
    /// function reads as part of the user only after a profile.
    pub fn on_profile(
        profile: impl Into<String>,
        user: impl Into<String>,
        domain: impl Into<String>,
    ) -> Result<Self, OriginateError> {
        Self {
            user: user.into(),
            domain: domain.into(),
            profile: Some(profile.into()),
            variables: None,
        }
        .checked()
    }

    /// Set the SIP profile for the contact lookup, refusing what parse refuses.
    ///
    /// Pass `"*"` to search all loaded SIP profiles for the user's
    /// registration instead of a single named profile.
    pub fn with_profile(mut self, profile: impl Into<String>) -> Result<Self, OriginateError> {
        self.profile = Some(profile.into());
        self.checked()
    }

    /// User part of the contact lookup.
    pub fn user(&self) -> &str {
        &self.user
    }

    /// Domain for the contact lookup.
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// SIP profile name, or `"*"` for all profiles.
    pub fn profile(&self) -> Option<&str> {
        self.profile
            .as_deref()
    }
}

impl_dial_string_with_variables!(SofiaEndpoint, write_module_text, |this| format!(
    "sofia/{}/{}",
    this.profile, this.destination
));

impl_dial_string_with_variables!(
    SofiaGateway,
    write_module_text,
    |this| match &this.profile {
        Some(p) => format!("sofia/gateway/{p}::{}/{}", this.gateway, this.destination),
        None => format!("sofia/gateway/{}/{}", this.gateway, this.destination),
    }
);

impl_dial_string_with_variables!(SofiaContact, write_expression, |this| match &this.profile {
    Some(p) => format!("${{sofia_contact({p}/{}@{})}}", this.user, this.domain),
    None => format!("${{sofia_contact({}@{})}}", this.user, this.domain),
});

impl SofiaEndpoint {
    /// Refuse a profile `sofia_outgoing_channel` cuts at `/` or `^` or reads as the gateway path,
    /// and one carrying `@`, where `protect_dest_uri` reads the text as a URI and may cut it.
    pub(crate) fn check_deliverable(&self) -> Result<(), OriginateError> {
        check_field("sofia", "profile", &self.profile, &["/", "^", "@"])?;
        if self
            .profile
            .eq_ignore_ascii_case("gateway")
        {
            return Err(undeliverable(
                "sofia",
                "profile",
                EndpointFieldFault::ReadsAsGateway,
            ));
        }
        check_field("sofia", "destination", &self.destination, &[])
    }

    pub(crate) fn parse_bare(text: &str) -> Result<Self, OriginateError> {
        let rest = after_prefix(text, "sofia/", "sofia")?;
        let (profile, destination) = rest
            .split_once('/')
            .ok_or_else(|| {
                OriginateError::ParseError("sofia endpoint needs profile/destination".into())
            })?;
        Self {
            profile: profile.into(),
            destination: destination.into(),
            variables: None,
        }
        .checked()
    }
}

impl SofiaGateway {
    /// Refuse a gateway or profile `sofia_outgoing_channel` cuts at `/` or `^`, and a `::` the
    /// `profile::gateway` lookup key would read at another place.
    pub(crate) fn check_deliverable(&self) -> Result<(), OriginateError> {
        const KIND: &str = "sofia gateway";
        let gateway_splits: &[&str] = match self.profile {
            Some(_) => &["/", "^", "@"],
            None => &["/", "^", "@", "::"],
        };
        check_field(KIND, "gateway", &self.gateway, gateway_splits)?;
        if let Some(profile) = &self.profile {
            check_field(KIND, "profile", profile, &["/", "^", "@", "::"])?;
            if profile.ends_with(':') {
                return Err(undeliverable(
                    KIND,
                    "profile",
                    EndpointFieldFault::ModuleSeparator("::"),
                ));
            }
            if splits_into_threads(&format!("{profile}::{}", self.gateway)) {
                let field = if profile.ends_with(":_") {
                    "profile"
                } else {
                    "gateway"
                };
                return Err(undeliverable(
                    KIND,
                    field,
                    EndpointFieldFault::EnterpriseSeparator,
                ));
            }
        }
        check_field(KIND, "destination", &self.destination, &[])
    }

    pub(crate) fn parse_bare(text: &str) -> Result<Self, OriginateError> {
        let rest = after_prefix(text, "sofia/gateway/", "sofia gateway")?;
        let (gateway_part, destination) = rest
            .split_once('/')
            .ok_or_else(|| {
                OriginateError::ParseError("sofia gateway needs gateway/destination".into())
            })?;
        let (profile, gateway) = match gateway_part.split_once("::") {
            Some((profile, gateway)) => (Some(profile.to_owned()), gateway),
            None => (None, gateway_part),
        };
        Self {
            gateway: gateway.into(),
            destination: destination.into(),
            profile,
            variables: None,
        }
        .checked()
    }
}

impl SofiaContact {
    /// Refuse what `sofia_contact_function` splits at its first `~`, `/` and `@` and a `/` after
    /// the domain, an empty domain or profile it replaces, and what a pass ahead of it reads.
    pub(crate) fn check_deliverable(&self) -> Result<(), OriginateError> {
        const KIND: &str = "sofia_contact";
        let user_splits: &[&str] = match self.profile {
            Some(_) => &["~", "@"],
            None => &["~", "@", "/"],
        };
        check_expression_field(KIND, "user", &self.user, user_splits)?;
        for (field, value) in [
            ("domain", Some(&self.domain)),
            (
                "profile",
                self.profile
                    .as_ref(),
            ),
        ] {
            let Some(value) = value else {
                continue;
            };
            check_expression_field(KIND, field, value, &["~", "/"])?;
            if value.is_empty() {
                return Err(undeliverable(
                    KIND,
                    field,
                    EndpointFieldFault::EmptyReadsAsDefault,
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn parse_bare(text: &str) -> Result<Self, OriginateError> {
        let inner = text
            .strip_prefix("${sofia_contact(")
            .and_then(|r| r.strip_suffix(")}"))
            .ok_or_else(|| OriginateError::ParseError("not a sofia_contact expression".into()))?;
        let (profile, user_at_domain) = if let Some((p, rest)) = inner.split_once('/') {
            (Some(p.to_string()), rest)
        } else {
            (None, inner)
        };
        let (user, domain) = user_at_domain
            .split_once('@')
            .ok_or_else(|| OriginateError::ParseError("sofia_contact needs user@domain".into()))?;
        Self {
            user: user.into(),
            domain: domain.into(),
            profile,
            variables: None,
        }
        .checked()
    }
}

impl_endpoint_parse!(parse_for: SofiaEndpoint, SofiaGateway);

impl_endpoint_parse!(config:
    SofiaEndpoint {
        profile: String,
        destination: String,
        ;
        variables: Option<Variables>,
    }
    SofiaContact {
        user: String,
        domain: String,
        ;
        profile: Option<String>,
        variables: Option<Variables>,
    }
    SofiaGateway {
        gateway: String,
        destination: String,
        ;
        profile: Option<String>,
        variables: Option<Variables>,
    }
);

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::commands::variables::DialStringCarrier;
    use crate::commands::variables::VariablesType;
    use crate::sofia::SofiaChannelName;

    // --- SofiaEndpoint ---

    #[test]
    fn sofia_endpoint_display() {
        let ep = SofiaEndpoint {
            profile: "internal".into(),
            destination: "1000@example.com".into(),
            variables: None,
        };
        assert_eq!(
            ep.render(DialStringCarrier::EslApi.measured()),
            "sofia/internal/1000@example.com"
        );
    }

    #[test]
    fn sofia_endpoint_display_with_variables() {
        let mut vars = Variables::new(VariablesType::Default);
        vars.insert("originate_timeout", "30")
            .unwrap();
        let ep = SofiaEndpoint {
            profile: "internal".into(),
            destination: "1000@example.com".into(),
            variables: Some(vars),
        };
        assert_eq!(
            ep.render(DialStringCarrier::EslApi.measured()),
            "{originate_timeout=30}sofia/internal/1000@example.com"
        );
    }

    #[test]
    fn sofia_endpoint_from_str() {
        let ep = SofiaEndpoint::parse_for(
            "sofia/internal/1000@example.com",
            DialStringCarrier::EslApi.measured(),
        )
        .unwrap();
        assert_eq!(ep.profile, "internal");
        assert_eq!(ep.destination, "1000@example.com");
        assert!(ep
            .variables
            .is_none());
    }

    #[test]
    fn sofia_endpoint_from_str_with_variables() {
        let ep = SofiaEndpoint::parse_for(
            "{originate_timeout=30}sofia/internal/1000@example.com",
            DialStringCarrier::EslApi.measured(),
        )
        .unwrap();
        assert_eq!(ep.profile, "internal");
        assert_eq!(ep.destination, "1000@example.com");
        assert_eq!(
            ep.variables
                .unwrap()
                .get("originate_timeout"),
            Some("30")
        );
    }

    #[test]
    fn sofia_endpoint_round_trip() {
        let ep = SofiaEndpoint {
            profile: "external".into(),
            destination: "sip:user@host:5060".into(),
            variables: None,
        };
        let s = ep.render(DialStringCarrier::EslApi.measured());
        let parsed = SofiaEndpoint::parse_for(&s, DialStringCarrier::EslApi.measured()).unwrap();
        assert_eq!(parsed, ep);
    }

    #[test]
    fn serde_sofia_endpoint() {
        let ep = SofiaEndpoint {
            profile: "internal".into(),
            destination: "1000@example.com".into(),
            variables: None,
        };
        let json = serde_json::to_string(&ep).unwrap();
        let parsed: SofiaEndpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ep);
    }

    #[test]
    fn serde_sofia_endpoint_with_variables() {
        let mut vars = Variables::new(VariablesType::Default);
        vars.insert("originate_timeout", "30")
            .unwrap();
        let ep = SofiaEndpoint {
            profile: "internal".into(),
            destination: "1000@example.com".into(),
            variables: Some(vars),
        };
        let json = serde_json::to_string(&ep).unwrap();
        let parsed: SofiaEndpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ep);
    }

    // --- SofiaGateway ---

    #[test]
    fn sofia_gateway_display() {
        let ep = SofiaGateway {
            gateway: "my_provider".into(),
            destination: "18005551234".into(),
            profile: None,
            variables: None,
        };
        assert_eq!(
            ep.render(DialStringCarrier::EslApi.measured()),
            "sofia/gateway/my_provider/18005551234"
        );
    }

    #[test]
    fn sofia_gateway_display_with_profile() {
        let ep = SofiaGateway {
            gateway: "my_provider".into(),
            destination: "18005551234".into(),
            profile: Some("external".into()),
            variables: None,
        };
        assert_eq!(
            ep.render(DialStringCarrier::EslApi.measured()),
            "sofia/gateway/external::my_provider/18005551234"
        );
    }

    #[test]
    fn sofia_gateway_display_with_variables() {
        let mut vars = Variables::new(VariablesType::Default);
        vars.insert("sip_h_X-Custom", "value")
            .unwrap();
        let ep = SofiaGateway {
            gateway: "gw1".into(),
            destination: "1234".into(),
            profile: None,
            variables: Some(vars),
        };
        assert_eq!(
            ep.render(DialStringCarrier::EslApi.measured()),
            "{sip_h_X-Custom=value}sofia/gateway/gw1/1234"
        );
    }

    #[test]
    fn sofia_gateway_from_str() {
        let ep = SofiaGateway::parse_for(
            "sofia/gateway/my_provider/18005551234",
            DialStringCarrier::EslApi.measured(),
        )
        .unwrap();
        assert_eq!(ep.gateway, "my_provider");
        assert_eq!(ep.destination, "18005551234");
        assert!(ep
            .profile
            .is_none());
    }

    #[test]
    fn sofia_gateway_from_str_with_profile() {
        let ep = SofiaGateway::parse_for(
            "sofia/gateway/external::my_provider/18005551234",
            DialStringCarrier::EslApi.measured(),
        )
        .unwrap();
        assert_eq!(ep.gateway, "my_provider");
        assert_eq!(ep.destination, "18005551234");
        assert_eq!(
            ep.profile
                .as_deref(),
            Some("external")
        );
    }

    #[test]
    fn sofia_gateway_round_trip() {
        let ep = SofiaGateway {
            gateway: "carrier".into(),
            destination: "15551234567".into(),
            profile: Some("external".into()),
            variables: None,
        };
        let s = ep.render(DialStringCarrier::EslApi.measured());
        let parsed = SofiaGateway::parse_for(&s, DialStringCarrier::EslApi.measured()).unwrap();
        assert_eq!(parsed, ep);
    }

    #[test]
    fn serde_sofia_gateway() {
        let ep = SofiaGateway {
            gateway: "my_provider".into(),
            destination: "18005551234".into(),
            profile: None,
            variables: None,
        };
        let json = serde_json::to_string(&ep).unwrap();
        let parsed: SofiaGateway = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ep);
    }

    #[test]
    fn serde_sofia_gateway_with_profile() {
        let ep = SofiaGateway {
            gateway: "my_provider".into(),
            destination: "18005551234".into(),
            profile: Some("external".into()),
            variables: None,
        };
        let json = serde_json::to_string(&ep).unwrap();
        let parsed: SofiaGateway = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ep);
    }

    // --- SofiaContact ---

    #[test]
    fn sofia_contact_display() {
        let ep = SofiaContact {
            user: "1000".into(),
            domain: "example.com".into(),
            profile: None,
            variables: None,
        };
        assert_eq!(
            ep.render(DialStringCarrier::EslApi.measured()),
            "${sofia_contact(1000@example.com)}"
        );
    }

    #[test]
    fn sofia_contact_display_with_profile() {
        let ep = SofiaContact {
            user: "1000".into(),
            domain: "example.com".into(),
            profile: Some("internal".into()),
            variables: None,
        };
        assert_eq!(
            ep.render(DialStringCarrier::EslApi.measured()),
            "${sofia_contact(internal/1000@example.com)}"
        );
    }

    #[test]
    fn sofia_contact_display_all_profiles() {
        let ep = SofiaContact {
            user: "1000".into(),
            domain: "example.com".into(),
            profile: Some("*".into()),
            variables: None,
        };
        assert_eq!(
            ep.render(DialStringCarrier::EslApi.measured()),
            "${sofia_contact(*/1000@example.com)}"
        );
    }

    #[test]
    fn sofia_contact_display_with_variables() {
        let mut vars = Variables::new(VariablesType::Default);
        vars.insert("presence_id", "1000@example.com")
            .unwrap();
        let ep = SofiaContact {
            user: "1000".into(),
            domain: "example.com".into(),
            profile: None,
            variables: Some(vars),
        };
        assert_eq!(
            ep.render(DialStringCarrier::EslApi.measured()),
            "{presence_id=1000@example.com}${sofia_contact(1000@example.com)}"
        );
    }

    #[test]
    fn sofia_contact_from_str() {
        let ep = SofiaContact::parse_bare("${sofia_contact(1000@example.com)}").unwrap();
        assert_eq!(ep.user, "1000");
        assert_eq!(ep.domain, "example.com");
        assert!(ep
            .profile
            .is_none());
    }

    #[test]
    fn sofia_contact_from_str_with_profile() {
        let ep = SofiaContact::parse_bare("${sofia_contact(internal/1000@example.com)}").unwrap();
        assert_eq!(ep.user, "1000");
        assert_eq!(ep.domain, "example.com");
        assert_eq!(
            ep.profile
                .as_deref(),
            Some("internal")
        );
    }

    #[test]
    fn sofia_contact_round_trip() {
        let ep = SofiaContact {
            user: "bob".into(),
            domain: "example.com".into(),
            profile: Some("*".into()),
            variables: None,
        };
        let s = ep.render(DialStringCarrier::EslApi.measured());
        let parsed = SofiaContact::parse_bare(&s).unwrap();
        assert_eq!(parsed, ep);
    }

    #[test]
    fn sofia_contact_profile_with_at_sign() {
        let ep = SofiaContact::new("1000", "example.com")
            .unwrap()
            .with_profile("user@realm")
            .unwrap();
        let s = ep.render(DialStringCarrier::EslApi.measured());
        assert_eq!(s, "${sofia_contact(user@realm/1000@example.com)}");
        // Round-trip: the first @ is in profile, the parser splits on / first
        // then finds @ in the user_at_domain part
        let parsed = SofiaContact::parse_bare(&s).unwrap();
        assert_eq!(
            parsed
                .profile
                .as_deref(),
            Some("user@realm")
        );
        assert_eq!(parsed.user, "1000");
        assert_eq!(parsed.domain, "example.com");
    }

    #[test]
    fn serde_sofia_contact() {
        let ep = SofiaContact {
            user: "1000".into(),
            domain: "example.com".into(),
            profile: Some("*".into()),
            variables: None,
        };
        let json = serde_json::to_string(&ep).unwrap();
        let parsed: SofiaContact = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ep);
    }

    // --- user() ---

    type UserCase = (&'static str, Option<&'static str>, Option<&'static str>);

    fn assert_users(
        cases: &[UserCase],
        user: impl Fn(&str, DestinationEncoding) -> Option<String>,
    ) {
        for &(destination, encoded, suppressed) in cases {
            assert_eq!(
                user(destination, DestinationEncoding::Encoded).as_deref(),
                encoded,
                "{destination:?} encoded"
            );
            assert_eq!(
                user(destination, DestinationEncoding::Suppressed).as_deref(),
                suppressed,
                "{destination:?} suppressed"
            );
        }
    }

    #[test]
    fn sofia_endpoint_user() {
        assert_users(
            &[
                (
                    "5550001234@pbx.example.com",
                    Some("5550001234"),
                    Some("5550001234"),
                ),
                ("sip:1000@pbx.example.com:5060", Some("1000"), Some("1000")),
                ("SIPS:1000@pbx.example.com", Some("1000"), Some("1000")),
                ("1000^2000@pbx.example.com", Some("1000^2000"), Some("1000")),
                ("1000@pbx.example.com^2000", Some("1000"), Some("1000")),
                ("1000%pbx.example.com", Some("1000"), Some("1000")),
                ("1000", Some("1000"), Some("1000")),
                ("sip:pbx.example.com", None, None),
                ("a@b@pbx.example.com", Some("a"), Some("a")),
                ("x@pbx.example.com/z w", Some("x"), Some("x")),
                ("a/sip:b@pbx.example.com", Some("a/b"), Some("a/sip:b")),
                ("@pbx.example.com", None, None),
                (
                    "+15550001234;npdi=yes@pbx.example.com",
                    Some("+15550001234;npdi=yes"),
                    Some("+15550001234;npdi=yes"),
                ),
            ],
            |destination, encoding| {
                SofiaEndpoint::new("internal", destination)
                    .unwrap()
                    .user(encoding)
                    .map(Cow::into_owned)
            },
        );
    }

    /// Destinations on profile `internal`, with the name each encoding gives the leg.
    pub(in crate::commands::endpoint) const NAMED: &[UserCase] = &[
        (
            "sip:+1555@pbx.example.com;transport=tcp",
            Some("sofia/internal/%2B1555@pbx.example.com"),
            Some("sofia/internal/sip:+1555@pbx.example.com"),
        ),
        (
            "sip:a b@pbx.example.com",
            Some("sofia/internal/a%20b@pbx.example.com"),
            Some("sofia/internal/sip:a b@pbx.example.com"),
        ),
        (
            "1000^2000@pbx.example.com",
            Some("sofia/internal/1000%5E2000@pbx.example.com"),
            Some("sofia/internal/1000"),
        ),
        (
            "+1555;npdi=yes@pbx.example.com",
            Some("sofia/internal/%2B1555%3Bnpdi%3Dyes@pbx.example.com"),
            Some("sofia/internal/+1555"),
        ),
        (
            "user%pbx.example.com",
            Some("sofia/internal/user"),
            Some("sofia/internal/user"),
        ),
        (
            "a%20b@pbx.example.com",
            Some("sofia/internal/a%20b@pbx.example.com"),
            Some("sofia/internal/a%20b@pbx.example.com"),
        ),
        (
            "a b%20c@pbx.example.com",
            None,
            Some("sofia/internal/a b%20c@pbx.example.com"),
        ),
        (
            "x@pbx.example.com/z w",
            Some("sofia/internal/x@pbx.example.com"),
            Some("sofia/internal/x@pbx.example.com/z w"),
        ),
        (
            "1000",
            Some("sofia/internal/1000"),
            Some("sofia/internal/1000"),
        ),
        ("^2000", None, None),
    ];

    #[test]
    fn sofia_endpoint_channel_name() {
        let long = format!("{}@pbx.example.com", "1".repeat(300));
        let cut = format!("{}@pbx.example.com;{}", "1".repeat(200), "x".repeat(100));
        let fits = format!("sofia/internal/{}@pbx.example.com", "1".repeat(200));
        let cases: &[UserCase] = &[
            (long.leak(), None, None),
            (
                cut.leak(),
                Some(
                    fits.clone()
                        .leak(),
                ),
                Some(fits.leak()),
            ),
        ];
        let name = |destination: &str, encoding| {
            SofiaEndpoint::new("internal", destination)
                .unwrap()
                .channel_name(encoding)
        };
        assert_users(NAMED, name);
        assert_users(cases, name);
    }

    #[test]
    fn a_channel_name_parses_back_to_its_profile_and_destination() {
        for &(destination, encoded, suppressed) in NAMED {
            for (name, encoding) in [
                (encoded, DestinationEncoding::Encoded),
                (suppressed, DestinationEncoding::Suppressed),
            ] {
                let Some(name) = name else {
                    continue;
                };
                let built = SofiaEndpoint::new("internal", destination)
                    .unwrap()
                    .channel_name(encoding)
                    .unwrap();
                let parsed = SofiaChannelName::parse(&built).unwrap();
                assert_eq!(parsed.profile(), "internal", "{destination:?}");
                assert_eq!(
                    parsed.destination(),
                    &name["sofia/internal/".len()..],
                    "{destination:?} {encoding:?}"
                );
            }
        }
    }

    #[test]
    fn sofia_gateway_user() {
        assert_users(
            &[
                ("5550009999", Some("5550009999"), Some("5550009999")),
                (
                    "+15550001234@agent.example.com",
                    Some("+15550001234"),
                    Some("+15550001234"),
                ),
                ("1000;transport=tcp", Some("1000"), Some("1000")),
                ("1000;x=a@b.example.com", Some("1000;x=a"), Some("1000")),
                ("sip:1000@example.com", Some("1000"), Some("sip:1000")),
                ("1000^2000@example.com", Some("1000^2000"), Some("1000")),
                ("", None, None),
            ],
            |destination, encoding| {
                SofiaGateway::new("montreal", destination)
                    .unwrap()
                    .with_profile("voipms")
                    .unwrap()
                    .user(encoding)
                    .map(Cow::into_owned)
            },
        );
    }
}
