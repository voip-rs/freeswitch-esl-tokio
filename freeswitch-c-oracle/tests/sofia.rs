//! mod_sofia's destination readers on every built tree: `protect_dest_uri`,
//! `sofia_outgoing_channel` and `sofia_contact_function`.

use freeswitch_c_oracle::{
    on_every_tree, oracles, trees_agree, ContactSelect, Oracle, ProtectedDestination, Sofia,
    SofiaOutgoing, DEFAULT_DOMAIN,
};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::select;

const PIECES: &[&[u8]] = &[
    b"internal",
    b"gateway",
    b"gw1",
    b"/",
    b"@",
    b"^",
    b"%",
    b"%20",
    b";",
    b"~",
    b"*",
    b":",
    b"sip:",
    b"sips:",
    b"SIP:",
    b" ",
    b"#",
    b"example.com",
    b"host",
    b"1000",
    b"transport=tcp",
    b"transport=udp",
    b"fs_path=",
    b"sip%3A1000%40192.0.2.9",
    b"transport=ws",
    b"'",
    b"\\",
    b"\xc3\xa9",
];

const SOFIA: Sofia<'static> = Sofia {
    profiles: &[b"internal"],
    gateways: &[b"gw1"],
    headers: &[],
    registered: false,
};

fn text() -> impl Strategy<Value = Vec<u8>> {
    vec(select(PIECES), 0..8).prop_map(|pieces| pieces.concat())
}

fn bytes(text: &str) -> Vec<u8> {
    text.as_bytes()
        .to_vec()
}

fn some(text: &str) -> Option<Vec<u8>> {
    Some(bytes(text))
}

fn lossy(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

#[test]
fn protect_dest_uri_encodes_the_user_after_the_last_slash() {
    let cases: &[(&[u8], &str, bool)] = &[
        (
            b"internal/1000@example.com",
            "internal/1000@example.com",
            false,
        ),
        (
            b"internal/us er@example.com",
            "internal/us%20er@example.com",
            true,
        ),
        (
            b"internal/sip:a#b@example.com",
            "internal/a%23b@example.com",
            true,
        ),
        (
            b"internal/sip:1000@example.com",
            "internal/1000@example.com",
            false,
        ),
        (
            b"internal/b%20c@example.com",
            "internal/b%20c@example.com",
            false,
        ),
        (b"1000@example.com/pa th", "1000@example.com", false),
        (b"internal/x y", "internal/x y", false),
        (
            b"internal/1000@exa mple.com",
            "internal/1000@exa mple.com",
            false,
        ),
    ];
    for (tree, c) in oracles() {
        for &(destination, left, encoded) in cases {
            let protected = c.protect_dest_uri(destination);
            assert_eq!(
                (lossy(&protected.destination), protected.encoded),
                (left.into(), encoded),
                "{:?} on tree {tree}",
                lossy(destination)
            );
        }
    }
}

/// A valid escape in the user part: kept by the pin's encoder, encoded again by the fork's.
#[test]
fn protect_dest_uri_encodes_an_escape_as_the_tree_encoder_does() {
    for (tree, c) in oracles() {
        let expected = match tree {
            "pin" | "master" => "internal/a%20%20b@example.com",
            "fork" => "internal/a%2520%20b@example.com",
            other => panic!("tree {other} has no rule; read its switch_url_encode_opt"),
        };
        let protected = c.protect_dest_uri(b"internal/a%20 b@example.com");
        assert_eq!(
            (lossy(&protected.destination), protected.encoded),
            (expected.into(), true),
            "tree {tree}"
        );
    }
}

/// `protect_dest_uri` as its C reads, encoding through the tree's own `switch_url_encode`.
fn protected(c: Oracle, destination: &[u8]) -> ProtectedDestination {
    let unchanged = ProtectedDestination {
        destination: destination.to_vec(),
        encoded: false,
    };
    let (Some(_), Some(slash)) = (
        destination
            .iter()
            .position(|&byte| byte == b'@'),
        destination
            .iter()
            .rposition(|&byte| byte == b'/'),
    ) else {
        return unchanged;
    };
    let (outer, user_and_host) = (&destination[..slash], &destination[slash + 1..]);
    let go = user_and_host
        .iter()
        .take_while(|&&byte| byte != b'@')
        .any(|byte| {
            c.url_unsafe()
                .contains(byte)
        });
    if !go {
        return unchanged;
    }
    let scheme = [&b"sips:"[..], b"sip:"]
        .into_iter()
        .find(|scheme| {
            user_and_host
                .get(..scheme.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(scheme))
        })
        .map_or(0, <[u8]>::len);
    let rest = &user_and_host[scheme..];
    let Some(at) = rest
        .iter()
        .position(|&byte| byte == b'@')
    else {
        return ProtectedDestination {
            destination: outer.to_vec(),
            encoded: false,
        };
    };
    let (user, host) = (&rest[..at], &rest[at + 1..]);
    let encoded = c.needs_url_encode(user);
    let user = if encoded {
        c.url_encode(user, user.len() * 3 + 2)
    } else {
        user.to_vec()
    };
    ProtectedDestination {
        destination: [outer, b"/", &user, b"@", host].concat(),
        encoded,
    }
}

#[test]
fn protect_dest_uri_reads_as_its_tree_encoder_composes() {
    on_every_tree(
        file!(),
        "protect_dest_uri_reads_as_its_tree_encoder_composes",
        text(),
        |_, c, destination| {
            prop_assert_eq!(
                c.protect_dest_uri(&destination),
                protected(c, &destination),
                "{:?}",
                lossy(&destination)
            );
            Ok(())
        },
    );
}

fn cause(c: Oracle, name: &[u8]) -> Option<i32> {
    Some(c.str2cause(name))
}

fn profile_route(c: Oracle, destination: &[u8], sofia: &Sofia<'_>) -> SofiaOutgoing {
    c.sofia_outgoing_channel(destination, sofia)
}

#[test]
fn a_profile_destination_composes_the_request_uri() {
    for (tree, c) in oracles() {
        let outgoing = profile_route(c, b"internal/1000@example.com", &SOFIA);
        assert_eq!(
            outgoing,
            SofiaOutgoing {
                cause: None,
                destination_number: bytes("internal/1000@example.com"),
                header_lookups: vec![
                    bytes("sofia_suppress_url_encoding"),
                    bytes("sip_invite_to_uri"),
                    bytes("sip_destination_prefix"),
                    bytes("sip_gethostbyname"),
                ],
                profile_lookups: vec![bytes("internal")],
                variables: vec![
                    (bytes("sip_local_network_addr"), bytes("192.0.2.1")),
                    (bytes("sip_profile_name"), bytes("internal")),
                ],
                dest: some("sip:1000@example.com"),
                e_dest: some("1000@example.com"),
                dest_to: some("sip:1000@example.com"),
                remote_ip: some("example.com"),
                channel_name: some("sofia/internal/1000@example.com"),
                ..SofiaOutgoing::default()
            },
            "tree {tree}"
        );
    }
}

#[test]
fn a_caret_names_the_to_user_on_the_request_host() {
    for (tree, c) in oracles() {
        let outgoing = profile_route(c, b"internal/1000@example.com^2000", &SOFIA);
        assert_eq!(outgoing.cause, None, "tree {tree}");
        assert_eq!(outgoing.dest, some("sip:1000@example.com"), "tree {tree}");
        assert_eq!(
            outgoing.dest_to,
            some("sip:2000@example.com"),
            "tree {tree}"
        );
        let invite_to = Sofia {
            headers: &[(b"sip_invite_to_uri", b"sip:alice@example.org")],
            ..SOFIA
        };
        let outgoing = profile_route(c, b"internal/1000@example.com^2000", &invite_to);
        assert_eq!(
            outgoing.dest_to,
            some("sip:sip:alice@example.org"),
            "tree {tree}"
        );
    }
}

#[test]
fn a_gateway_destination_composes_on_its_proxy() {
    for (tree, c) in oracles() {
        let outgoing = profile_route(c, b"gateway/gw1/1000", &SOFIA);
        assert_eq!(
            outgoing,
            SofiaOutgoing {
                cause: None,
                destination_number: bytes("gateway/gw1/1000"),
                header_lookups: vec![
                    bytes("sofia_suppress_url_encoding"),
                    bytes("sip_invite_to_uri"),
                ],
                gateway_lookups: vec![bytes("gw1")],
                variables: vec![
                    (bytes("sip_gateway_name"), bytes("gw1")),
                    (bytes("sip_local_network_addr"), bytes("192.0.2.1")),
                    (bytes("sip_profile_name"), bytes("gateway")),
                ],
                transport: 1,
                gateway_name: some("gw1"),
                gateway_from_str: some("<sip:gw@gateway.example.com>"),
                dest: some("sip:1000@gateway.example.com"),
                dest_to: some("sip:1000@gateway.example.com"),
                invite_contact: some("<sip:gw@192.0.2.1:5060>"),
                remote_ip: some("gateway.example.com"),
                channel_name: some("sofia/gateway-profile/1000"),
                ..SofiaOutgoing::default()
            },
            "tree {tree}"
        );
        let params = profile_route(c, b"gateway/gw1/1000;fs_path=x", &SOFIA);
        assert_eq!(
            (params.cause, params.dest, params.invite_contact),
            (
                None,
                some("sip:1000@gateway.example.com;fs_path=x"),
                some("<sip:gw@192.0.2.1:5060>;fs_path=x")
            ),
            "tree {tree}"
        );
    }
}

#[test]
fn refused_destinations_name_their_cause() {
    for (tree, c) in oracles() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"", b"DESTINATION_OUT_OF_ORDER"),
            (b"internal", b"INVALID_URL"),
            (b"unknown/1000@example.com", b"INVALID_PROFILE"),
            (b"gateway/gw2/1000", b"INVALID_GATEWAY"),
            (b"gateway/gw1", b"INVALID_URL"),
            (
                b"gateway/gw1/1000;transport=tcp",
                b"DESTINATION_OUT_OF_ORDER",
            ),
            (
                b"gateway/gw1/1000;transport=bogus",
                b"DESTINATION_OUT_OF_ORDER",
            ),
            (b"internal/1000", b"USER_NOT_REGISTERED"),
            (b"internal/1000%example.com", b"USER_NOT_REGISTERED"),
        ];
        for &(destination, name) in cases {
            assert_eq!(
                profile_route(c, destination, &SOFIA).cause,
                cause(c, name),
                "{:?} on tree {tree}",
                lossy(destination)
            );
        }
        let bare = profile_route(c, b"internal/1000", &SOFIA);
        assert_eq!(
            bare.registration_lookups,
            [(bytes("1000"), bytes("internal"))],
            "tree {tree}"
        );
        let percent = profile_route(c, b"internal/1000%example.com", &SOFIA);
        assert_eq!(
            (percent.registration_lookups, percent.e_dest),
            (
                vec![(bytes("1000"), bytes("example.com"))],
                some("1000@example.com")
            ),
            "tree {tree}"
        );
    }
}

#[test]
fn a_scheme_or_a_dotless_host_takes_its_own_path() {
    for (tree, c) in oracles() {
        let stripped = profile_route(c, b"internal/sip:1000@example.com", &SOFIA);
        assert_eq!(
            (stripped.destination_number, stripped.dest),
            (
                bytes("internal/1000@example.com"),
                some("sip:1000@example.com")
            ),
            "tree {tree}"
        );
        let kept = Sofia {
            headers: &[(b"sofia_suppress_url_encoding", b"true")],
            ..SOFIA
        };
        let sips = profile_route(c, b"internal/sips:1000@example.com", &kept);
        assert_eq!(
            (sips.dest, sips.e_dest),
            (some("sips:1000@example.com"), some("1000@example.com")),
            "tree {tree}"
        );
        let encoded = profile_route(c, b"internal/us er@example.com", &SOFIA);
        assert_eq!(encoded.dest, some("sip:us%20er@example.com"), "tree {tree}");
        let dotless = profile_route(c, b"internal/1000@host", &SOFIA);
        assert_eq!(
            (dotless.cause, dotless.host_lookups, dotless.dest),
            (None, vec![bytes("host")], some("sip:1000@host")),
            "tree {tree}"
        );
    }
}

/// The attach names the channel `sofia/<profile name>/<dest>`, cut at its first `;` and its
/// 256-byte buffer; a call that fails before the attach names none.
#[test]
fn the_attach_names_the_channel_after_the_profile_and_destination() {
    let registered = Sofia {
        registered: true,
        ..SOFIA
    };
    let suppressed = Sofia {
        headers: &[(b"sofia_suppress_url_encoding", b"true")],
        ..registered
    };
    let cases: &[(&[u8], &Sofia<'_>, Option<&str>)] = &[
        (
            b"internal/sip:+1555@host;transport=tcp",
            &suppressed,
            Some("sofia/internal/sip:+1555@host"),
        ),
        (
            b"internal/sip:+1555@host;transport=tcp",
            &registered,
            Some("sofia/internal/%2B1555@host"),
        ),
        (
            b"internal/sip:a b@host",
            &registered,
            Some("sofia/internal/a%20b@host"),
        ),
        (
            b"internal/1000^2000@host",
            &registered,
            Some("sofia/internal/1000%5E2000@host"),
        ),
        (
            b"internal/1000^2000@host",
            &suppressed,
            Some("sofia/internal/1000"),
        ),
        (
            b"internal/user%host",
            &registered,
            Some("sofia/internal/user"),
        ),
        (b"internal/x@y/z w", &registered, Some("sofia/internal/x@y")),
        (b"internal/1000", &registered, Some("sofia/internal/1000")),
        (b"internal/1000", &SOFIA, None),
        (b"unknown/1000@host", &registered, None),
    ];
    let long = format!("internal/{}@example.com", "1".repeat(300));
    for (tree, c) in oracles() {
        for &(destination, sofia, name) in cases {
            assert_eq!(
                c.sofia_outgoing_channel(destination, sofia)
                    .channel_name,
                name.map(bytes),
                "{:?} on tree {tree}",
                lossy(destination)
            );
        }
        let cut = c
            .sofia_outgoing_channel(long.as_bytes(), &registered)
            .channel_name;
        assert_eq!(
            cut,
            Some(
                long.replace("internal/", "sofia/internal/")
                    .as_bytes()[..255]
                    .to_vec()
            ),
            "tree {tree}"
        );
    }
}

/// With URL encoding suppressed, the encoder that differs between trees never runs.
#[test]
fn sofia_outgoing_reads_alike_on_every_tree_unencoded() {
    let unencoded = Sofia {
        headers: &[(b"sofia_suppress_url_encoding", b"true")],
        ..SOFIA
    };
    trees_agree(
        file!(),
        "sofia_outgoing_reads_alike_on_every_tree_unencoded",
        text(),
        |c, destination| c.sofia_outgoing_channel(destination, &unencoded),
    );
}

/// Encoding on, each tree reads the destination `protect_dest_uri` leaves it as the pin reads it.
#[test]
fn sofia_outgoing_reads_the_protected_destination_alike() {
    let reference = oracles().next();
    on_every_tree(
        file!(),
        "sofia_outgoing_reads_the_protected_destination_alike",
        text(),
        |tree, c, destination| {
            let Some((first, pin)) = reference else {
                return Ok(());
            };
            let left = c.protect_dest_uri(&destination);
            let unencoded = Sofia {
                headers: &[(b"sofia_suppress_url_encoding", b"true")],
                ..SOFIA
            };
            let encoded = c.sofia_outgoing_channel(&destination, &SOFIA);
            let read = pin.sofia_outgoing_channel(&left.destination, &unencoded);
            prop_assert_eq!(
                encoded,
                read,
                "tree {} against {} on {:?}",
                tree,
                first,
                lossy(&destination)
            );
            Ok(())
        },
    );
}

fn select_on(profile: &str, user: &str, domain: &str) -> ContactSelect {
    ContactSelect {
        profile: bytes(profile),
        user: some(user),
        domain: some(domain),
        ..ContactSelect::default()
    }
}

#[test]
fn sofia_contact_cuts_at_tilde_slash_and_at() {
    let default_domain = String::from_utf8_lossy(DEFAULT_DOMAIN).into_owned();
    for (tree, c) in oracles() {
        let not_registered = vec![bytes("error/user_not_registered")];

        let named = c.sofia_contact(b"internal/1000@example.com", SOFIA.profiles);
        assert_eq!(named.output, not_registered, "tree {tree}");
        assert_eq!(named.profile_lookups, [bytes("internal")], "tree {tree}");
        assert_eq!(
            named.selects,
            [select_on("internal", "1000", "example.com")],
            "tree {tree}"
        );

        let concat = c.sofia_contact(b"internal/1000@example.com/;fs_path=x", SOFIA.profiles);
        assert_eq!(
            concat.selects,
            [ContactSelect {
                concat: some(";fs_path=x"),
                ..select_on("internal", "1000", "example.com")
            }],
            "tree {tree}"
        );

        let defaulted = c.sofia_contact(b"internal/1000", SOFIA.profiles);
        assert_eq!(defaulted.default_domain_lookups, 1, "tree {tree}");
        assert_eq!(
            defaulted.selects,
            [select_on("internal", "1000", &default_domain)],
            "tree {tree}"
        );

        let agent = c.sofia_contact(b"internal~Polycom/1000@example.com", SOFIA.profiles);
        assert_eq!(
            agent.selects,
            [ContactSelect {
                match_user_agent: some("Polycom/1000@example.com"),
                ..select_on("internal", "internal", &default_domain)
            }],
            "tree {tree}"
        );

        let by_domain = c.sofia_contact(b"1000@example.com", SOFIA.profiles);
        assert_eq!(
            (
                by_domain.profile_lookups,
                by_domain.selects,
                by_domain.output
            ),
            (vec![bytes("example.com")], vec![], not_registered.clone()),
            "tree {tree}"
        );

        let every = c.sofia_contact(b"*/1000@example.com", SOFIA.profiles);
        assert_eq!(
            (every.profile_lookups, every.selects),
            (vec![], vec![]),
            "tree {tree}"
        );

        let domain_profile = c.sofia_contact(b"1000@internal", SOFIA.profiles);
        assert_eq!(
            domain_profile.selects,
            [select_on("internal", "1000", "internal")],
            "tree {tree}"
        );
    }
}

#[test]
fn sofia_contact_reads_alike_on_every_tree() {
    trees_agree(
        file!(),
        "sofia_contact_reads_alike_on_every_tree",
        text(),
        |c, arg| c.sofia_contact(arg, SOFIA.profiles),
    );
}
