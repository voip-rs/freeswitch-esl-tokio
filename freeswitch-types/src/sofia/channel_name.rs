//! Parser for sofia channel names (`sofia/<profile>/<destination>`).
//!
//! mod_sofia builds every channel name as `sofia/<profile>/<channame>` and
//! truncates it at the first `;` (`sofia_glue_set_name()` in `sofia_glue.c`),
//! so names never carry URI parameters. For inbound calls the destination is
//! `user@host[:port]` from the parsed From URL (or bare `host[:port]` when the
//! user part is empty); for outbound calls it is the freeform dial-string tail,
//! which may contain a `sip:` scheme, further `/`, or no `@` at all.

/// Borrowed view of a parsed sofia channel name. No allocation.
///
/// Only `sofia/...` names parse; other drivers (`loopback/...`, `error/...`)
/// return `None`. All accessors return verbatim slices of the input: no
/// URL-decoding, no `sip:` stripping, no port splitting — callers decide.
///
/// The name `SofiaEndpoint::channel_name` (feature `esl`) builds for an
/// outbound leg parses back into this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SofiaChannelName<'a> {
    profile: &'a str,
    destination: &'a str,
}

impl<'a> SofiaChannelName<'a> {
    /// Parses a channel name; `Some` only for well-formed
    /// `sofia/<profile>/<destination>` names (non-empty profile).
    pub fn parse(name: &'a str) -> Option<Self> {
        let rest = name.strip_prefix("sofia/")?;
        let (profile, destination) = rest.split_once('/')?;
        if profile.is_empty() {
            return None;
        }
        Some(Self {
            profile,
            destination,
        })
    }

    /// Sofia profile name (second segment).
    pub fn profile(&self) -> &'a str {
        self.profile
    }

    /// Raw third segment, verbatim. May contain further `/` (dial-string
    /// tails), a `sip:` scheme, or a `:port` suffix.
    pub fn destination(&self) -> &'a str {
        self.destination
    }

    /// Part of the destination before the last `@`, or `None` when there is
    /// no `@`. The split is on the *last* `@` because mod_sofia URL-decodes
    /// the user part, which may therefore contain `@`; a host never can.
    pub fn user(&self) -> Option<&'a str> {
        self.destination
            .rsplit_once('@')
            .map(|(user, _)| user)
    }

    /// Part of the destination after the last `@` (may include `:port`), or
    /// `None` when there is no `@`. A destination without `@` is ambiguous
    /// (bare host on inbound, user/number on gateway dials) — use
    /// [`destination`](Self::destination) and decide from context.
    pub fn host(&self) -> Option<&'a str> {
        self.destination
            .rsplit_once('@')
            .map(|(_, host)| host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbound_full() {
        let n = SofiaChannelName::parse("sofia/esinet1-v4-tcp/+15550001@bcf.example.test")
            .expect("sofia name parses");
        assert_eq!(n.profile(), "esinet1-v4-tcp");
        assert_eq!(n.destination(), "+15550001@bcf.example.test");
        assert_eq!(n.user(), Some("+15550001"));
        assert_eq!(n.host(), Some("bcf.example.test"));
    }

    #[test]
    fn simple_user_host() {
        let n = SofiaChannelName::parse("sofia/internal-v6/x@host").expect("sofia name parses");
        assert_eq!(n.profile(), "internal-v6");
        assert_eq!(n.user(), Some("x"));
        assert_eq!(n.host(), Some("host"));
    }

    #[test]
    fn host_with_port() {
        let n = SofiaChannelName::parse("sofia/external/1001@10.0.0.5:5080")
            .expect("sofia name parses");
        assert_eq!(n.user(), Some("1001"));
        assert_eq!(n.host(), Some("10.0.0.5:5080"));
    }

    #[test]
    fn bare_host_inbound_no_at() {
        let n = SofiaChannelName::parse("sofia/internal/10.0.0.5").expect("sofia name parses");
        assert_eq!(n.profile(), "internal");
        assert_eq!(n.destination(), "10.0.0.5");
        assert_eq!(n.user(), None);
        assert_eq!(n.host(), None);
    }

    #[test]
    fn gateway_dial_no_at() {
        let n = SofiaChannelName::parse("sofia/gw1/1002").expect("sofia name parses");
        assert_eq!(n.profile(), "gw1");
        assert_eq!(n.destination(), "1002");
        assert_eq!(n.user(), None);
        assert_eq!(n.host(), None);
    }

    #[test]
    fn dial_tail_keeps_slashes() {
        let n = SofiaChannelName::parse("sofia/internal/sip:a@b/extra").expect("sofia name parses");
        assert_eq!(n.destination(), "sip:a@b/extra");
        assert_eq!(n.user(), Some("sip:a"));
        assert_eq!(n.host(), Some("b/extra"));
    }

    #[test]
    fn multi_at_splits_on_last() {
        let n = SofiaChannelName::parse("sofia/internal/a@b@c.example.test")
            .expect("sofia name parses");
        assert_eq!(n.user(), Some("a@b"));
        assert_eq!(n.host(), Some("c.example.test"));
    }

    #[test]
    fn empty_destination() {
        let n = SofiaChannelName::parse("sofia/internal/").expect("sofia name parses");
        assert_eq!(n.profile(), "internal");
        assert_eq!(n.destination(), "");
        assert_eq!(n.user(), None);
        assert_eq!(n.host(), None);
    }

    #[test]
    fn non_sofia_and_malformed_are_none() {
        for name in [
            "loopback/x",
            "error/USER_BUSY",
            "sofia/internal",
            "sofia//x",
            "sofia/",
            "sofia",
            "",
            "Sofia/internal/x@h",
        ] {
            assert_eq!(SofiaChannelName::parse(name), None, "{name:?}");
        }
    }
}
