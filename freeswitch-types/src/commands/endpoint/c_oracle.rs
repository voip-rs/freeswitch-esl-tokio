//! What each endpoint module or expression function reads of the crate's endpoint text, against
//! the switch's own C on every built tree.

use freeswitch_c_oracle::{
    against_the_c, oracles, ContactSelect, GroupCall as GroupCallRead, Oracle, Sofia,
    SofiaOutgoing, UserOutgoing, DEFAULT_DOMAIN,
};
use std::borrow::Cow;

use proptest::prelude::*;
use proptest::sample::select;

use super::{DestinationEncoding, Endpoint, GroupCallOrder};
use crate::commands::proptests::{bare_endpoint, c_reads_the_leg, fields_name_a_variable, targets};
use crate::commands::variables::{DialStringCarrier, DialStringTarget};
use crate::switch_passes::against_the_c_at_its_revision;
use crate::test_text::text;

fn bytes(text: &str) -> Vec<u8> {
    text.as_bytes()
        .to_vec()
}

fn fail(message: String) -> TestCaseError {
    TestCaseError::fail(message)
}

const SUPPRESSED: &[(&[u8], &[u8])] = &[(b"sofia_suppress_url_encoding", b"true")];

/// `out` with `name`, the profile or gateway key the text named, written `N` wherever the reader
/// copied it, so two readings differing only in that name compare equal.
fn named(mut out: SofiaOutgoing, name: &str, prefix: &str) -> SofiaOutgoing {
    let name = bytes(name);
    let rename = |field: &mut Vec<u8>| {
        if *field == name {
            *field = bytes("N");
        }
    };
    out.profile_lookups
        .iter_mut()
        .for_each(rename);
    out.gateway_lookups
        .iter_mut()
        .for_each(rename);
    if let Some(gateway) = &mut out.gateway_name {
        rename(gateway);
    }
    let named_variable: &[u8] = if prefix.is_empty() {
        b"sip_profile_name"
    } else {
        b"sip_gateway_name"
    };
    for (variable, value) in &mut out.variables {
        if variable == named_variable {
            rename(value);
        }
    }
    let head = [bytes(prefix), name.clone(), bytes("/")].concat();
    if let Some(rest) = out
        .destination_number
        .strip_prefix(&head[..])
    {
        out.destination_number = [bytes(prefix), bytes("N/"), rest.to_vec()].concat();
    }
    out
}

/// Two readings look up the same registrations, a host that is each reading's profile name taken as
/// the same; a destination can itself hold text equal to the name, so no value is renamed.
fn compare_registrations(
    read: &mut SofiaOutgoing,
    name: &str,
    reference: &mut SofiaOutgoing,
    reference_name: &str,
) -> bool {
    let same = read
        .registration_lookups
        .len()
        == reference
            .registration_lookups
            .len()
        && read
            .registration_lookups
            .iter()
            .zip(&reference.registration_lookups)
            .all(|((user, host), (reference_user, reference_host))| {
                user == reference_user
                    && (host == reference_host
                        || (*host == bytes(name) && *reference_host == bytes(reference_name)))
            });
    read.registration_lookups
        .clear();
    reference
        .registration_lookups
        .clear();
    same
}

/// `read` left `destination`, the text the fields render, as `protect_dest_uri` leaves it unless
/// `headers` suppress the encoding.
fn holds_destination(
    c: Oracle,
    text: &str,
    destination: &str,
    headers: &[(&[u8], &[u8])],
    read: &SofiaOutgoing,
) -> Result<(), TestCaseError> {
    let want = if headers.is_empty() {
        c.protect_dest_uri(destination.as_bytes())
            .destination
    } else {
        bytes(destination)
    };
    prop_assert_eq!(
        &read.destination_number,
        &want,
        "{:?} with {:?}",
        text,
        headers
    );
    Ok(())
}

fn encoding(headers: &[(&[u8], &[u8])]) -> DestinationEncoding {
    if headers.is_empty() {
        DestinationEncoding::Encoded
    } else {
        DestinationEncoding::Suppressed
    }
}

/// The user `read` dialed: the user its registration lookup named, else its request URI's,
/// `None` where it reached neither.
fn dialed_user(read: &SofiaOutgoing) -> Option<Vec<u8>> {
    if let [(user, _)] = &read.registration_lookups[..] {
        return Some(user.clone());
    }
    let dest = read
        .dest
        .as_ref()?;
    let rest = dest
        .strip_prefix(b"sip:")
        .or_else(|| dest.strip_prefix(b"sips:"))
        .unwrap_or(dest);
    let at = rest
        .iter()
        .position(|&b| b == b'@');
    Some(at.map_or_else(Vec::new, |at| rest[..at].to_vec()))
}

/// `user` is the user `read` dialed, decoded where `protect_dest_uri` may have encoded it, an
/// empty one read as none. A `%` in an `Encoded` text is skipped: decoding cannot tell it apart.
fn holds_user(
    text: &str,
    encoding: DestinationEncoding,
    user: Option<Cow<'_, str>>,
    read: &SofiaOutgoing,
) -> Result<(), TestCaseError> {
    let Some(dialed) = dialed_user(read) else {
        return Ok(());
    };
    let dialed = match encoding {
        DestinationEncoding::Encoded if text.contains('%') => return Ok(()),
        DestinationEncoding::Encoded => percent_encoding::percent_decode(&dialed).collect(),
        DestinationEncoding::Suppressed => dialed,
    };
    let dialed = Some(dialed).filter(|user| !user.is_empty());
    let user = user.map(|user| bytes(&user));
    let lossy =
        |user: Option<Vec<u8>>| user.map(|user| String::from_utf8_lossy(&user).into_owned());
    prop_assert_eq!(
        lossy(user),
        lossy(dialed),
        "{:?} {:?}: the user against what the switch dialed",
        text,
        encoding
    );
    Ok(())
}

/// The argument the expansion hands `function` for the expression `text`.
fn expression_argument(c: Oracle, text: &str, function: &str) -> Result<Vec<u8>, TestCaseError> {
    let expansion = c.expand(text.as_bytes());
    match &expansion.api_calls[..] {
        [(called, Some(arg))] if *called == bytes(function) => Ok(arg.clone()),
        calls => Err(fail(format!("{text:?} calls {calls:?}"))),
    }
}

/// The module or function reading `text`, a leg's endpoint text, extracts the fields `endpoint`
/// holds.
fn reads_the_fields(c: Oracle, text: &str, endpoint: &Endpoint) -> Result<(), TestCaseError> {
    let after = |prefix: &str| {
        text.strip_prefix(prefix)
            .ok_or_else(|| fail(format!("{text:?} does not open {prefix}")))
    };
    match endpoint {
        Endpoint::Loopback(ep) => {
            let read = c.loopback_outgoing_channel(after("loopback/")?.as_bytes());
            let application = ep
                .extension
                .get(..4)
                .is_some_and(|head| head.eq_ignore_ascii_case("app="));
            if application {
                let (name, arg) = match ep.extension[4..].split_once(':') {
                    Some((name, arg)) => (name, Some(arg)),
                    None => (&ep.extension[4..], None),
                };
                let mut want = vec![(bytes("loopback_app"), bytes(name))];
                want.extend(arg.map(|arg| (bytes("loopback_app_arg"), bytes(arg))));
                prop_assert_eq!((read.app, read.variables), (true, want), "{:?}", text);
            } else {
                prop_assert_eq!(
                    (
                        read.app,
                        read.destination_number,
                        read.context,
                        read.dialplan
                    ),
                    (
                        false,
                        bytes(&ep.extension),
                        Some(bytes(
                            ep.context
                                .as_deref()
                                .unwrap_or("default")
                        )),
                        Some(bytes(
                            ep.dialplan
                                .as_deref()
                                .unwrap_or("xml")
                        )),
                    ),
                    "{:?}",
                    text
                );
            }
        }
        Endpoint::Sofia(ep) => {
            let destination = after("sofia/")?;
            for headers in [SUPPRESSED, &[][..]] {
                let read = c.sofia_outgoing_channel(
                    destination.as_bytes(),
                    &Sofia {
                        profiles: &[ep
                            .profile
                            .as_bytes()],
                        gateways: &[],
                        headers,
                    },
                );
                let mut reference = c.sofia_outgoing_channel(
                    format!("internal/{}", ep.destination).as_bytes(),
                    &Sofia {
                        profiles: &[b"internal"],
                        gateways: &[],
                        headers,
                    },
                );
                let destination = format!("{}/{}", ep.profile, ep.destination);
                holds_destination(c, text, &destination, headers, &read)?;
                let encoding = encoding(headers);
                holds_user(text, encoding, ep.user(encoding), &read)?;
                let mut read = read;
                prop_assert!(
                    compare_registrations(&mut read, &ep.profile, &mut reference, "internal"),
                    "{:?} with {:?}: registrations differ",
                    text,
                    headers
                );
                prop_assert_eq!(
                    named(read, &ep.profile, ""),
                    named(reference, "internal", ""),
                    "{:?} with {:?}",
                    text,
                    headers
                );
            }
        }
        Endpoint::SofiaGateway(ep) => {
            let destination = after("sofia/")?;
            let key = match &ep.profile {
                Some(profile) => format!("{profile}::{}", ep.gateway),
                None => ep
                    .gateway
                    .clone(),
            };
            for headers in [SUPPRESSED, &[][..]] {
                let read = c.sofia_outgoing_channel(
                    destination.as_bytes(),
                    &Sofia {
                        profiles: &[],
                        gateways: &[key.as_bytes()],
                        headers,
                    },
                );
                let destination = format!("gateway/{key}/{}", ep.destination);
                holds_destination(c, text, &destination, headers, &read)?;
                let encoding = encoding(headers);
                holds_user(text, encoding, ep.user(encoding), &read)?;
                let reference = c.sofia_outgoing_channel(
                    format!("gateway/gw1/{}", ep.destination).as_bytes(),
                    &Sofia {
                        profiles: &[],
                        gateways: &[b"gw1"],
                        headers,
                    },
                );
                prop_assert_eq!(
                    named(read, &key, "gateway/"),
                    named(reference, "gw1", "gateway/"),
                    "{:?} with {:?}",
                    text,
                    headers
                );
            }
        }
        Endpoint::User(ep) => {
            let read = c.user_outgoing_channel(after("user/")?.as_bytes());
            let want = UserOutgoing {
                user: bytes(&ep.name),
                domain: ep
                    .domain
                    .as_deref()
                    .map_or_else(|| DEFAULT_DOMAIN.to_vec(), bytes),
                default_domain: ep
                    .domain
                    .is_none(),
            };
            prop_assert_eq!(read, Some(want), "{:?}", text);
        }
        Endpoint::SofiaContact(ep) => {
            let arg = expression_argument(c, text, "sofia_contact")?;
            let searched = ep
                .profile
                .as_deref()
                .filter(|profile| *profile != "*");
            let profile = searched.unwrap_or(&ep.domain);
            let everywhere = ep
                .profile
                .as_deref()
                == Some("*");
            let read = c.sofia_contact(&arg, &[profile.as_bytes()]);
            let (lookups, selects) = if everywhere {
                (vec![], vec![])
            } else {
                (
                    vec![bytes(profile)],
                    vec![ContactSelect {
                        profile: bytes(profile),
                        user: Some(bytes(&ep.user)),
                        domain: Some(bytes(&ep.domain)),
                        ..ContactSelect::default()
                    }],
                )
            };
            prop_assert_eq!(
                (read.profile_lookups, read.selects, read.assertion),
                (lookups, selects, None),
                "{:?}",
                text
            );
        }
        Endpoint::GroupCall(ep) => {
            let arg = expression_argument(c, text, "group_call")?;
            let call_delim = match ep.order {
                None | Some(GroupCallOrder::All) => ",",
                Some(GroupCallOrder::Enterprise) => ":_:",
                Some(GroupCallOrder::First) => "|",
            };
            let want = GroupCallRead {
                group: bytes(&ep.group),
                domain: Some(bytes(&ep.domain)),
                call_delim: bytes(call_delim),
                default_domain: false,
            };
            prop_assert_eq!(c.group_call(&arg), Some(want), "{:?}", text);
        }
        Endpoint::Error(_)
        | Endpoint::PortAudio(_)
        | Endpoint::PulseAudio(_)
        | Endpoint::Alsa(_) => {}
    }
    Ok(())
}

#[test]
fn url_unsafe_is_the_switch_set_on_every_tree() {
    for (tree, c) in oracles() {
        assert_eq!(
            super::sofia::URL_UNSAFE,
            c.url_unsafe(),
            "SWITCH_URL_UNSAFE on {tree}"
        );
    }
}

fn config_refuses(endpoint: &Endpoint) -> bool {
    serde_json::to_value(endpoint)
        .and_then(serde_json::from_value::<Endpoint>)
        .is_err()
}

/// An endpoint config load accepts, rendered at any target and read through the C passes, reaches
/// its module or function as the fields it holds; an expression through the dialplan carrier.
#[test]
fn endpoint_modules_read_the_fields_the_crate_holds() {
    against_the_c_at_its_revision(
        file!(),
        "endpoint_modules_read_the_fields_the_crate_holds",
        (bare_endpoint(), select(targets())),
        |c, block_parse, (endpoint, target)| {
            if fields_name_a_variable(&endpoint) || config_refuses(&endpoint) {
                return Ok(());
            }
            if matches!(endpoint, Endpoint::SofiaContact(_) | Endpoint::GroupCall(_)) {
                let rendered = endpoint
                    .display_for(
                        DialStringTarget::new(DialStringCarrier::Dialplan)
                            .with_block_parse(block_parse),
                    )
                    .to_string();
                return reads_the_fields(c, &rendered, &endpoint);
            }
            let target = target.with_block_parse(block_parse);
            let dial = endpoint
                .display_for(target)
                .to_string();
            let (_, text) = c_reads_the_leg(c, &dial, target).map_err(fail)?;
            reads_the_fields(c, &text, &endpoint)
        },
    );
}

const PREFIXES: &[&str] = &[
    "loopback/",
    "loopback/app=",
    "sofia/",
    "sofia/internal/",
    "sofia/gateway/",
    "user/",
    "${sofia_contact(",
    "${group_call(",
];

/// Endpoint text as the leg splits leave it: whatever `Endpoint` parses of it holds what the
/// module or function reads.
#[test]
fn a_parsed_endpoint_holds_what_its_module_reads() {
    let module_text = (select(PREFIXES), text(), text(), any::<bool>()).prop_map(
        |(prefix, head, tail, closed)| {
            let close = if closed && prefix.starts_with('$') {
                ")}"
            } else {
                ""
            };
            format!("{prefix}{head}{tail}{close}")
        },
    );
    against_the_c(
        file!(),
        "a_parsed_endpoint_holds_what_its_module_reads",
        module_text,
        |c, text| {
            let Ok(endpoint) = Endpoint::parse_bare(&text) else {
                return Ok(());
            };
            if fields_name_a_variable(&endpoint) {
                return Ok(());
            }
            reads_the_fields(c, &text, &endpoint)
        },
    );
}
