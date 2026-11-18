//! Pins freeswitch-types's command-builder output byte for byte, through the public API
//! only, so an internal file-moving refactor can be checked against unchanged behaviour.
//! Compares against `tests/golden/render_golden.txt`; set `GOLDEN_UPDATE=1` to rewrite it.

use std::time::Duration;

use freeswitch_types::commands::endpoint::{
    AudioEndpoint, ErrorEndpoint, GroupCall, LoopbackEndpoint, SofiaContact, SofiaEndpoint,
    SofiaGateway, UserEndpoint,
};
use freeswitch_types::commands::{
    originate_quote, originate_split, originate_unquote, quote_for_uuid_setvar, CauseReading,
    FlattenedDialString, FlattenedLeg, LegTarget,
};
use freeswitch_types::{
    Application, BlockParse, BridgeDialString, ChannelVariable, DialString, DialStringCarrier,
    DialStringTarget, DialplanType, Endpoint, GroupCallOrder, HangupCause, Originate,
    OriginateError, Variables, VariablesType,
};

/// One line per record: `id<TAB>debug-escaped value`, so any character in a rendered
/// string stays on one line and diffs cleanly.
fn rec(records: &mut Vec<String>, id: impl std::fmt::Display, value: impl std::fmt::Display) {
    records.push(format!("{id}\t{:?}", value.to_string()));
}

fn rec_data(records: &mut Vec<String>, id: impl std::fmt::Display, value: impl std::fmt::Debug) {
    records.push(format!("{id}\t{value:?}"));
}

fn sorted_pairs(vars: &Variables) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    pairs.sort();
    pairs
}

/// Records a parse only where it does not give back the block that was rendered.
fn vars_parse(
    records: &mut Vec<String>,
    id: String,
    built: &Variables,
    parsed: Result<Variables, impl std::fmt::Debug>,
) {
    match parsed {
        Ok(v) => {
            let data = (v.scope(), v.separator(), sorted_pairs(&v));
            if data != (built.scope(), built.separator(), sorted_pairs(built)) {
                rec_data(records, id, data);
            }
        }
        Err(_) => rec(records, id, "Err"),
    }
}

/// Corpus of characters each pass in the wire-format doc treats specially, plus a plain
/// baseline and an empty value.
const VALUES: &[(&str, &str)] = &[
    ("plain", "abc123"),
    ("space-in", "a b c"),
    ("space-lead", " abc"),
    ("space-trail", "abc "),
    ("quote", "it's"),
    ("dquote", "he said \"hi\""),
    ("backslash", "a\\b"),
    ("backslash2", "a\\\\b"),
    ("comma", "a,b"),
    ("pipe", "a|b"),
    ("equals", "a=b"),
    ("brackets", "a{b}c[d]e<f>g"),
    ("dollar", "a$b"),
    ("dolldoll", "a$$b"),
    ("varref", "${foo}"),
    ("caret", "a^^b"),
    ("newline", "a\nb"),
    ("tab", "a\tb"),
    ("nonascii", "caf\u{e9}\u{260e}"),
    ("empty", ""),
];

fn hostile(name: &str) -> &'static str {
    VALUES
        .iter()
        .find(|(n, _)| *n == name)
        .expect("known corpus name")
        .1
}

/// The four render targets asked for: blank-split and dialplan carriers, plus the API
/// carrier with each of the two argv separators the task names.
fn targets() -> Vec<(&'static str, DialStringTarget)> {
    let api = DialStringTarget::new(DialStringCarrier::EslApi);
    vec![
        ("api", api),
        (
            "dialplan",
            DialStringTarget::new(DialStringCarrier::Dialplan),
        ),
        (
            "api-tilde",
            api.with_argv_separator('~')
                .expect("~ is a usable argv separator"),
        ),
        (
            "api-bang",
            api.with_argv_separator('!')
                .expect("! is a usable argv separator"),
        ),
    ]
}

fn variables_corpus(records: &mut Vec<String>) {
    let scopes = [
        VariablesType::Enterprise,
        VariablesType::Default,
        VariablesType::Channel,
    ];
    let seps: [Option<char>; 2] = [None, Some(':')];
    let targets = targets();
    // "empty" is excluded from the combined map: FreeSWITCH discards an empty-value pair
    // outright, which would abort every parse on that one pair and mask the round-trip of
    // every other entry sharing the block. It gets its own single-pair case below instead.
    let entries: Vec<(String, String)> = VALUES
        .iter()
        .filter(|(name, _)| *name != "empty")
        .enumerate()
        .map(|(i, (_, v))| (format!("v{i}"), (*v).to_string()))
        .collect();
    for scope in scopes {
        // Channel scope refuses a quote, so the block keeps every other entry, under its name.
        let entries = entries
            .iter()
            .filter(|(_, v)| scope != VariablesType::Channel || !v.contains('\''))
            .cloned();
        for sep in seps {
            let sep_label = sep.map_or("comma".to_string(), |c| c.to_string());
            let Ok(mut vars) = Variables::with_vars(scope, entries.clone()) else {
                rec(
                    records,
                    format!("vars/{scope:?}/sep-{sep_label}/build-err"),
                    "Err",
                );
                continue;
            };
            if let Some(c) = sep {
                vars = match vars.with_separator(c) {
                    Ok(v) => v,
                    Err(_) => {
                        rec(
                            records,
                            format!("vars/{scope:?}/sep-{sep_label}/build-err"),
                            "Err",
                        );
                        continue;
                    }
                };
            }
            for &(tname, target) in &targets {
                let rendered = vars.render(target);
                rec(
                    records,
                    format!("vars/{scope:?}/sep-{sep_label}/{tname}/render"),
                    &rendered,
                );
                vars_parse(
                    records,
                    format!("vars/{scope:?}/sep-{sep_label}/{tname}/parse"),
                    &vars,
                    Variables::parse_for(&rendered, target),
                );
            }
        }
    }
    // One pair per value per scope, so a value that aborts a whole block (an empty value,
    // a lone quote in channel scope) still shows its own render/parse rather than hiding
    // every other value sharing the block.
    for scope in scopes {
        for &(name, v) in VALUES {
            let Ok(vars) = Variables::with_vars(scope, [("k", v)]) else {
                rec(
                    records,
                    format!("vars/{scope:?}/single-{name}/build-err"),
                    "Err",
                );
                continue;
            };
            for &(tname, target) in &targets {
                let rendered = vars.render(target);
                rec(
                    records,
                    format!("vars/{scope:?}/single-{name}/{tname}/render"),
                    &rendered,
                );
                vars_parse(
                    records,
                    format!("vars/{scope:?}/single-{name}/{tname}/parse"),
                    &vars,
                    Variables::parse_for(&rendered, target),
                );
            }
        }
    }
    for scope in scopes {
        let vars = Variables::new(scope);
        for &(tname, target) in &targets {
            rec(
                records,
                format!("vars/{scope:?}/empty/{tname}/render"),
                vars.render(target),
            );
        }
    }
}

fn endpoint_case(
    records: &mut Vec<String>,
    id: &str,
    ep: &Endpoint,
    targets: &[(&str, DialStringTarget)],
) {
    for &(tname, target) in targets {
        let rendered = ep.render(target);
        rec(records, format!("{id}/{tname}/render"), &rendered);
        match Endpoint::parse_for(&rendered, target) {
            Ok(p) => {
                let again = p.render(target);
                if again != rendered {
                    rec(
                        records,
                        format!("{id}/{tname}/parse"),
                        format!("Ok {again}"),
                    );
                }
            }
            Err(_) => rec(records, format!("{id}/{tname}/parse"), "Err"),
        }
    }
}

const ENDPOINT_HOSTILE: &[&str] = &[
    "plain",
    "comma",
    "pipe",
    "quote",
    "backslash",
    "space-in",
    "dollar",
    "dolldoll",
    "varref",
    "nonascii",
    "empty",
    "brackets",
];

fn endpoint_corpus(records: &mut Vec<String>) {
    let targets3: Vec<(&str, DialStringTarget)> = targets()
        .into_iter()
        .filter(|&(n, _)| n != "api-bang")
        .collect();

    for &name in ENDPOINT_HOSTILE {
        let v = hostile(name);
        let audio = || AudioEndpoint::new().with_destination(v);
        let cases: Vec<(&str, Result<Endpoint, OriginateError>)> = vec![
            ("sofia", SofiaEndpoint::new("pbx", v).map(Endpoint::from)),
            (
                "sofia-gateway",
                SofiaGateway::on_profile("internal", "gw1", v).map(Endpoint::from),
            ),
            (
                "loopback",
                LoopbackEndpoint::new(v)
                    .and_then(|ep| ep.with_context("ctx"))
                    .map(Endpoint::from),
            ),
            (
                "user",
                UserEndpoint::in_domain(v, "example.com").map(Endpoint::from),
            ),
            (
                "sofia-contact",
                SofiaContact::on_profile("internal", v, "example.com").map(Endpoint::from),
            ),
            (
                "group-call",
                GroupCall::new(v, "example.com").map(|ep| {
                    ep.with_order(GroupCallOrder::Enterprise)
                        .into()
                }),
            ),
            ("portaudio", audio().map(Endpoint::PortAudio)),
            ("pulseaudio", audio().map(Endpoint::PulseAudio)),
            ("alsa", audio().map(Endpoint::Alsa)),
        ];
        for (kind, ep) in cases {
            let id = format!("ep/{kind}/{name}");
            match ep {
                Ok(ep) => endpoint_case(records, &id, &ep, &targets3),
                Err(_) => rec(records, id, "Err"),
            }
        }
    }

    let app_ep: Endpoint = LoopbackEndpoint::new("app=lua:script.lua")
        .unwrap()
        .into();
    endpoint_case(records, "ep/loopback-app", &app_ep, &targets3);

    for cause_text in [
        "user_busy",
        "USER_BUSY",
        "no_route_destination",
        "123",
        "bogus_cause_xyz",
        "",
    ] {
        for &(tname, target) in &targets3 {
            let text = format!("error/{cause_text}");
            let repr = match Endpoint::parse_for(&text, target) {
                Ok(p) => format!("Ok {}", p.render(target)),
                Err(_) => "Err".to_string(),
            };
            rec(
                records,
                format!("ep/error-parse/{cause_text}/{tname}"),
                repr,
            );
        }
    }

    let attach_vars = |mut ep: Endpoint| -> Result<Endpoint, OriginateError> {
        let vars = Variables::with_vars(VariablesType::Channel, [("ov0", "a,b"), ("ov1", "x")])
            .expect("plain pairs build");
        ep.set_variables(Some(vars))?;
        Ok(ep)
    };
    let base_cases: Vec<(&str, Endpoint)> = vec![
        (
            "sofia",
            SofiaEndpoint::new("pbx", "192.0.2.1")
                .unwrap()
                .into(),
        ),
        (
            "sofia-gateway",
            SofiaGateway::new("gw1", "18005551234")
                .unwrap()
                .into(),
        ),
        (
            "loopback",
            LoopbackEndpoint::new("9199")
                .unwrap()
                .with_context("test")
                .unwrap()
                .into(),
        ),
        (
            "user",
            UserEndpoint::new("bob")
                .unwrap()
                .with_domain("example.com")
                .unwrap()
                .into(),
        ),
        (
            "sofia-contact",
            SofiaContact::new("bob", "example.com")
                .unwrap()
                .into(),
        ),
        (
            "group-call",
            GroupCall::new("support", "example.com")
                .unwrap()
                .into(),
        ),
        ("error", ErrorEndpoint::new(HangupCause::UserBusy).into()),
        ("portaudio", Endpoint::PortAudio(AudioEndpoint::new())),
        ("alsa", Endpoint::Alsa(AudioEndpoint::new())),
    ];
    for (kind, ep) in base_cases {
        match attach_vars(ep) {
            Ok(ep) => endpoint_case(records, &format!("ep/{kind}/with-vars"), &ep, &targets3),
            Err(_) => rec(records, format!("ep/{kind}/with-vars"), "Err"),
        }
    }
}

fn base_endpoint() -> Endpoint {
    LoopbackEndpoint::new("9199")
        .unwrap()
        .with_context("test")
        .unwrap()
        .into()
}

fn o_extension_plain() -> Result<Originate, OriginateError> {
    Ok(Originate::extension(base_endpoint(), "1000"))
}
fn o_extension_hostile() -> Result<Originate, OriginateError> {
    Ok(Originate::extension(base_endpoint(), "it's,weird|1000"))
}
fn o_application_park() -> Result<Originate, OriginateError> {
    Ok(Originate::application(base_endpoint(), Application::park()))
}
fn o_application_args() -> Result<Originate, OriginateError> {
    Ok(Originate::application(
        base_endpoint(),
        Application::new("set", Some("k=it's v")),
    ))
}
fn o_inline_two_apps() -> Result<Originate, OriginateError> {
    Originate::inline(
        base_endpoint(),
        vec![
            Application::simple("park"),
            Application::new("set", Some("v=a,b")),
        ],
    )
}
fn o_inline_with_delim() -> Result<Originate, OriginateError> {
    Originate::inline_with_delimiter(
        base_endpoint(),
        vec![
            Application::simple("park"),
            Application::new("set", Some("v=a;b")),
        ],
        ';',
    )
}
fn o_inline_xml_conflict() -> Result<Originate, OriginateError> {
    Originate::inline(base_endpoint(), vec![Application::simple("park")])?
        .dialplan(DialplanType::XML)
}
fn o_dialplan_xml() -> Result<Originate, OriginateError> {
    Originate::extension(base_endpoint(), "1000").dialplan(DialplanType::XML)
}
fn o_dialplan_named() -> Result<Originate, OriginateError> {
    Originate::extension(base_endpoint(), "1000")
        .dialplan(DialplanType::named("custom_dialplan").expect("not undef"))
}
fn o_positionals_none() -> Result<Originate, OriginateError> {
    Ok(Originate::extension(base_endpoint(), "1000"))
}
fn o_positionals_empty_context() -> Result<Originate, OriginateError> {
    let mut o = Originate::extension(base_endpoint(), "1000");
    o.set_context(Some(""));
    Ok(o)
}
fn o_positionals_spaced() -> Result<Originate, OriginateError> {
    let mut o = Originate::extension(base_endpoint(), "1000");
    o.set_cid_name(Some("Jane Doe"));
    o.set_cid_num(Some("5551234567"));
    Ok(o)
}
fn o_positionals_quoted() -> Result<Originate, OriginateError> {
    let mut o = Originate::extension(base_endpoint(), "1000");
    o.set_cid_name(Some("it's \"quoted\""));
    Ok(o)
}
fn o_positionals_backslash() -> Result<Originate, OriginateError> {
    let mut o = Originate::extension(base_endpoint(), "1000");
    o.set_cid_name(Some("a\\b"));
    Ok(o)
}
fn o_positionals_literal_undef() -> Result<Originate, OriginateError> {
    let mut o = Originate::extension(base_endpoint(), "1000");
    o.set_context(Some("undef"));
    Ok(o)
}
fn o_positionals_gap_undef() -> Result<Originate, OriginateError> {
    let mut o = Originate::extension(base_endpoint(), "1000");
    o.set_cid_num(Some("5551234567"));
    Ok(o)
}
fn o_timeout() -> Result<Originate, OriginateError> {
    let mut o = Originate::extension(base_endpoint(), "1000");
    o.set_timeout(Some(Duration::from_secs(45)));
    Ok(o)
}

type OriginateBuilder = fn() -> Result<Originate, OriginateError>;

const ORIGINATE_CASES: &[(&str, OriginateBuilder)] = &[
    ("extension-plain", o_extension_plain),
    ("extension-hostile", o_extension_hostile),
    ("application-park", o_application_park),
    ("application-args", o_application_args),
    ("inline-two-apps", o_inline_two_apps),
    ("inline-with-delim", o_inline_with_delim),
    ("inline-xml-conflict", o_inline_xml_conflict),
    ("dialplan-xml", o_dialplan_xml),
    ("dialplan-raw", o_dialplan_named),
    ("positionals-none", o_positionals_none),
    ("positionals-empty-context", o_positionals_empty_context),
    ("positionals-spaced", o_positionals_spaced),
    ("positionals-quoted", o_positionals_quoted),
    ("positionals-backslash", o_positionals_backslash),
    ("positionals-literal-undef", o_positionals_literal_undef),
    ("positionals-gap-undef", o_positionals_gap_undef),
    ("timeout", o_timeout),
];

fn originate_case(records: &mut Vec<String>, id: &str, o: &Originate) {
    let rendered = o.render(BlockParse::default());
    rec(records, format!("orig/{id}/display"), &rendered);
    roundtrip(
        records,
        format!("orig/{id}/roundtrip"),
        &rendered,
        Originate::parse_for(&rendered, BlockParse::default())
            .map(|o| o.render(BlockParse::default())),
    );
}

/// Records a re-render only where it differs from the text it was parsed from.
fn roundtrip<E>(records: &mut Vec<String>, id: String, rendered: &str, again: Result<String, E>) {
    match again {
        Ok(again) if again == rendered => {}
        Ok(again) => rec(records, id, format!("Ok {again}")),
        Err(_) => rec(records, id, "Err"),
    }
}

fn originate_corpus(records: &mut Vec<String>) {
    for &(id, build) in ORIGINATE_CASES {
        match build() {
            Ok(o) => originate_case(records, id, &o),
            Err(_) => rec(records, format!("orig/{id}/build-err"), "Err"),
        }
    }
    for &(id, build) in ORIGINATE_CASES {
        let id = format!("{id}-argv-tilde");
        match build().and_then(|o| o.with_argv_separator('~')) {
            Ok(o) => originate_case(records, &id, &o),
            Err(_) => rec(records, format!("orig/{id}/build-err"), "Err"),
        }
    }
}

fn bridge_corpus(records: &mut Vec<String>) {
    let vars = Variables::with_vars(VariablesType::Default, [("hangup_after_bridge", "true")])
        .expect("a plain pair builds");
    let ep1: Endpoint = SofiaGateway::new("gw-a", "it's,weird")
        .unwrap()
        .into();
    let ep2: Endpoint = SofiaGateway::new("gw-b", "a|b\\c")
        .unwrap()
        .into();
    let ep3: Endpoint = ErrorEndpoint::new(HangupCause::NoRouteDestination).into();
    let bridge = BridgeDialString::new(vec![vec![ep1, ep2], vec![ep3]])
        .unwrap()
        .with_variables(vars);
    let block_parse = BlockParse::default();
    let rendered = bridge.render(block_parse);
    rec(records, "bridge/basic/display", &rendered);
    roundtrip(
        records,
        "bridge/basic/roundtrip".to_string(),
        &rendered,
        BridgeDialString::parse_for(&rendered, block_parse).map(|b| b.render(block_parse)),
    );

    // An unescaped comma inside a channel-scope value: the leg-split comma scan protects
    // it (this stays one leg, not two), but the block's own pair split still reads it as
    // a separator, so the block fails to parse rather than the legs merging.
    let bad = "[k=a,b]sofia/internal/1000,sofia/internal/1001";
    let repr = match BridgeDialString::parse_for(bad, BlockParse::default()) {
        Ok(b) => format!("Ok {}", b.render(BlockParse::default())),
        Err(_) => "Err".to_string(),
    };
    rec(records, "bridge/unescaped-comma-in-block/parse", repr);
}

fn quoting_corpus(records: &mut Vec<String>) {
    for &(name, v) in VALUES {
        let quoted = originate_quote(v);
        rec(records, format!("quote/originate_quote/{name}"), &quoted);
        let unquoted = originate_unquote(&quoted);
        if unquoted != v {
            rec(records, format!("quote/originate_unquote/{name}"), unquoted);
        }
        rec(
            records,
            format!("quote/uuid_setvar/{name}"),
            quote_for_uuid_setvar(v),
        );
    }
    let lines: &[(&str, &str)] = &[
        (
            "quoted-app",
            "loopback/9199/test '&socket(127.0.0.1:8040 async full)'",
        ),
        ("argv-tilde", "^^~loopback/9199/test~&park()"),
        ("unterminated-quote", "unterminated 'quote"),
        ("empty", ""),
        ("trailing-backslash-space", "a\\ b"),
    ];
    for &(lid, line) in lines {
        for split_at in [' ', '~'] {
            let id = format!("quote/originate_split/{lid}/{split_at}");
            match originate_split(line, split_at) {
                Ok(tokens) => rec_data(records, id, tokens),
                Err(_) => rec(records, id, "Err"),
            }
        }
    }
}

fn escape_argument_corpus(records: &mut Vec<String>) {
    for &(tname, target) in &targets() {
        for &(name, v) in VALUES {
            let repr = match target.escape_argument(v) {
                Some(cow) => format!("Some {cow:?}"),
                None => "None".to_string(),
            };
            rec(records, format!("escape/{tname}/{name}"), repr);
        }
    }
}

/// `(suffix, debug-escaped value)` pairs for one carrier's reading of a fixture.
fn flattened_case(input: &str, target: DialStringTarget, detailed: bool) -> Vec<(String, String)> {
    let list = match FlattenedDialString::parse_for(input, target) {
        Ok(list) => list,
        Err(_) => return vec![("err".to_string(), format!("{:?}", "Err"))],
    };
    let mut out = vec![
        (
            "raw".to_string(),
            format!(
                "{:?}",
                list.display_raw()
                    .to_string()
            ),
        ),
        ("for".to_string(), format!("{:?}", list.render(target))),
    ];
    if detailed {
        for (i, leg) in list
            .legs()
            .enumerate()
        {
            out.push((format!("leg{i}"), format!("{:?}", leg_data(leg))));
        }
    }
    out
}

fn leg_data(leg: &FlattenedLeg) -> Vec<String> {
    let mut data = vec![format!("raw={}", leg.raw())];
    if let Some(presence) = leg.variable(ChannelVariable::PresenceId) {
        data.push(format!("presence_id={presence}"));
    }
    if let LegTarget::Error(error) = leg.target() {
        data.push(format!("cause={}", error.as_written()));
        data.push(match error.reading() {
            CauseReading::Name(cause) => format!("name={cause}/{}", cause.as_number()),
            CauseReading::Number(n) => format!("number={n}"),
            CauseReading::Unrecognized => "unrecognized".to_string(),
            other => format!("{other:?}"),
        });
    }
    data
}

fn flattened_corpus(records: &mut Vec<String>) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/flattened");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read fixtures dir")
        .filter_map(|e| e.ok())
        .map(|e| {
            e.file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|n| n.ends_with(".txt"))
        .collect();
    names.sort();

    let api = DialStringTarget::new(DialStringCarrier::EslApi);
    let dialplan = DialStringTarget::new(DialStringCarrier::Dialplan);
    let argv = api
        .with_argv_separator('~')
        .expect("~ is a usable argv separator");

    for fname in &names {
        let path = format!("{dir}/{fname}");
        let input = std::fs::read_to_string(&path).expect("read fixture");
        let api_records = flattened_case(&input, api, true);
        for (i, (suffix, value)) in api_records
            .iter()
            .enumerate()
        {
            // An absent api `for` equals api `raw`.
            if !(suffix == "for" && *value == api_records[i - 1].1) {
                records.push(format!("flat/{fname}/api/{suffix}\t{value}"));
            }
        }
        // Other carriers record only entries that differ from api's, `for` included.
        let mut others = vec![("dialplan", flattened_case(&input, dialplan, true))];
        match argv.escape_argument(&input) {
            Some(escaped) => others.push(("argv-tilde", flattened_case(&escaped, argv, false))),
            None => rec(records, format!("flat/{fname}/argv-tilde/escape"), "None"),
        }
        for (carrier, list) in others {
            for (suffix, value) in list {
                if !api_records.contains(&(suffix.clone(), value.clone())) {
                    records.push(format!("flat/{fname}/{carrier}/{suffix}\t{value}"));
                }
            }
        }
    }
}

fn json_result<T>(json: &str, render: impl Fn(&T) -> String) -> String
where
    T: serde::de::DeserializeOwned,
{
    match serde_json::from_str::<T>(json) {
        Ok(v) => format!("Ok {}", render(&v)),
        Err(_) => "Err".to_string(),
    }
}

fn serde_config_corpus(records: &mut Vec<String>) {
    let vars_cases: &[(&str, &str)] = &[
        ("flat-map", r#"{"k1":"v1","k2":"a,b"}"#),
        (
            "scoped-channel",
            r#"{"scope":"channel","vars":{"k":"it's"}}"#,
        ),
        (
            "scoped-with-sep",
            r#"{"scope":"default","vars":{"k":"a~b"},"separator":"~"}"#,
        ),
        ("bad-scope", r#"{"scope":"bogus","vars":{"k":"v"}}"#),
    ];
    for &(id, json) in vars_cases {
        rec(
            records,
            format!("serde/vars/{id}"),
            json_result(json, |v: &Variables| v.render(DialStringCarrier::EslApi)),
        );
    }

    let endpoint_cases: &[(&str, &str)] = &[
        (
            "sofia",
            r#"{"sofia":{"profile":"internal","destination":"1000@example.com"}}"#,
        ),
        (
            "loopback-app",
            r#"{"loopback":{"extension":"app=lua:script.lua"}}"#,
        ),
        ("error", r#"{"error":{"cause":"UserBusy"}}"#),
        (
            "error-wire-name-refused",
            r#"{"error":{"cause":"USER_BUSY"}}"#,
        ),
        ("error-bad-cause", r#"{"error":{"cause":"not_a_cause"}}"#),
        ("unknown-tag", r#"{"bogus_type":{}}"#),
        (
            "group-call",
            r#"{"group_call":{"group":"support","domain":"example.com","order":"Enterprise"}}"#,
        ),
    ];
    for &(id, json) in endpoint_cases {
        rec(
            records,
            format!("serde/endpoint/{id}"),
            json_result(json, |e: &Endpoint| e.render(DialStringCarrier::EslApi)),
        );
    }

    let originate_cases: &[(&str, &str)] = &[
        (
            "extension",
            r#"{"endpoint":{"loopback":{"extension":"9199"}},"target":{"extension":"1000"}}"#,
        ),
        (
            "extension-inline-dialplan-refused",
            r#"{"endpoint":{"loopback":{"extension":"9199"}},"target":{"extension":"1000"},"dialplan":"inline"}"#,
        ),
        (
            "empty-inline-refused",
            r#"{"endpoint":{"loopback":{"extension":"9199"}},"target":{"inline_applications":[]}}"#,
        ),
        (
            "inline-under-xml-refused",
            r#"{"endpoint":{"loopback":{"extension":"9199"}},"target":{"inline_applications":[{"name":"park"}]},"dialplan":"XML"}"#,
        ),
        ("missing-endpoint", r#"{"target":{"extension":"1000"}}"#),
    ];
    for &(id, json) in originate_cases {
        rec(
            records,
            format!("serde/originate/{id}"),
            json_result(json, |o: &Originate| o.render(BlockParse::default())),
        );
    }

    let bridge_cases: &[(&str, &str)] = &[
        (
            "basic",
            r#"{"groups":[[{"sofia":{"profile":"internal","destination":"1000"}}]]}"#,
        ),
        (
            "with-vars",
            r#"{"variables":{"hangup_after_bridge":"true"},"groups":[[{"sofia":{"profile":"internal","destination":"1000"}},{"sofia":{"profile":"internal","destination":"1001"}}]]}"#,
        ),
    ];
    for &(id, json) in bridge_cases {
        rec(
            records,
            format!("serde/bridge/{id}"),
            json_result(json, |b: &BridgeDialString| b.render(BlockParse::default())),
        );
    }
}

#[test]
fn render_golden() {
    let mut records = Vec::new();
    variables_corpus(&mut records);
    endpoint_corpus(&mut records);
    originate_corpus(&mut records);
    bridge_corpus(&mut records);
    quoting_corpus(&mut records);
    escape_argument_corpus(&mut records);
    flattened_corpus(&mut records);
    serde_config_corpus(&mut records);

    let mut seen = std::collections::HashSet::new();
    for line in &records {
        let id = line
            .split('\t')
            .next()
            .unwrap();
        assert!(seen.insert(id.to_string()), "duplicate record id: {id}");
    }

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/render_golden.txt"
    );
    let mut actual = records.join("\n");
    actual.push('\n');

    if std::env::var_os("GOLDEN_UPDATE").is_some() {
        std::fs::write(path, &actual).expect("write golden file");
        return;
    }

    let expected = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {path}: {e}; run with GOLDEN_UPDATE=1 to create it"));
    assert_eq!(
        actual, expected,
        "golden mismatch; rerun with GOLDEN_UPDATE=1 to update {path} after confirming the change is intended"
    );
}
