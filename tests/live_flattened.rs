//! Integration tests against a live FreeSWITCH instance: `group_call` output
//! against the committed fixtures, and `FlattenedDialString`'s typed view
//! against what each leg's channel receives, per carrier.
//!
//! These tests require a live FreeSWITCH ESL
//! and the flattened-probe directory groups; see docs/live-test-switch.md.
//! Run with: cargo test --test live_flattened -- --ignored

mod live_common;

use freeswitch_esl_tokio::channel::CallDirection;
use freeswitch_esl_tokio::commands::originate::VariablesType;
use freeswitch_esl_tokio::commands::{
    originate_split, CauseReading, DialStringCarrier, DialStringTarget, FlattenedDialString,
    FlattenedDialStringError, FlattenedLeg, LegTarget, LegWarning, LoopbackEndpoint,
    OriginateError, UuidGetVar,
};
use freeswitch_esl_tokio::variables::VariableName;
use freeswitch_esl_tokio::{
    parse_channel_dump, CommandFailure, Endpoint, EslClient, EslEvent, EslEventStream,
    EslEventType, EslResult, EventFormat, ExecuteOptions, HangupCause, HeaderLookup, UNDEF_VALUE,
};
use live_common::{
    carried_in, channel_exists, connect, create_uuid, escaping_block, getvar, kill_channel,
    parked_anchor, target_under_test, wait_for_bridge, BridgeOutcome, ChannelReaper,
    ESCAPING_CASES, ESCAPING_SEPARATOR,
};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::time::Instant;

const API: DialStringCarrier = DialStringCarrier::EslApi;
const DIALPLAN: DialStringCarrier = DialStringCarrier::Dialplan;

/// The directory domain the flattened-probe groups live in.
const DOMAIN: &str = "default";

/// Every key the flattened-probe groups and these tests set.
const PROBE_KEYS: &[&str] = &[
    "fp_marker",
    "originate_timeout",
    "presence_id",
    "sip_invite_domain",
    "k",
    "pe",
    "pa",
    "b",
    "local_var_clobber",
    "sentinel",
    "nv",
    "path",
    "codecs",
    "q",
    "from",
    "v",
    "j",
];

/// Names outside every typed variable enum.
struct Var<'a>(&'a str);

impl VariableName for Var<'_> {
    fn as_str(&self) -> &str {
        self.0
    }
}

fn parse(input: &str, carrier: DialStringCarrier) -> FlattenedDialString {
    FlattenedDialString::parse_for(input, target_under_test(carrier))
        .unwrap_or_else(|e| panic!("{input:?} at {carrier:?}: {e}"))
}

/// Unique across the run, so a `hupall` on it reaps this case's channels only.
fn marker(case: &str) -> String {
    format!("fp-{}-{case}", std::process::id())
}

fn fixture_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("freeswitch-types/tests/fixtures/flattened")
}

fn group_call_expr(group: &str, flag: &str) -> String {
    match flag {
        "none" => format!("${{group_call({group}@{DOMAIN})}}"),
        flag => format!("${{group_call({group}@{DOMAIN}+{flag})}}"),
    }
}

async fn live_group_call(client: &EslClient, group: &str, flag: &str) -> String {
    let expr = group_call_expr(group, flag);
    client
        .api(&format!("eval {expr}"))
        .await
        .unwrap_or_else(|e| panic!("eval {expr}: transport error: {e}"))
        .body()
        .unwrap_or_else(|| panic!("eval {expr} answered no body"))
        .to_owned()
}

/// Replace `from` wherever the next character does not continue a word.
fn replace_at_word_end(text: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(from) {
        out.push_str(&rest[..at]);
        let after = &rest[at + from.len()..];
        let continues = after
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        out.push_str(if continues { from } else { to });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// The SIP port the fixtures' registered contacts were captured on.
const CAPTURED_SIP_PORT: &str = "5080";

/// The SIP port of the profile the `fp-reg` gateways register to, from its status URL.
async fn registrar_port(client: &EslClient) -> String {
    let resp = client
        .api("sofia xmlstatus profile lab-lo")
        .await
        .expect("sofia xmlstatus transport error");
    let body = resp
        .api_result()
        .expect("sofia xmlstatus rejected");
    body.split_once("<url>")
        .and_then(|(_, rest)| rest.split_once("</url>"))
        .and_then(|(url, _)| url.rsplit_once(':'))
        .map(|(_, port)| port.to_owned())
        .unwrap_or_else(|| panic!("no lab-lo profile URL in {body:?}"))
}

/// The fixture README's substitution, in its order.
fn sanitise(live: &str, registrar_port: &str) -> String {
    let text = live
        .replace(
            &format!("127.0.0.1:{registrar_port}"),
            &format!("127.0.0.1:{CAPTURED_SIP_PORT}"),
        )
        .replace("127.0.0.1", "192.0.2.1")
        .replace("[::1]", "[2001:db8::1]");
    replace_at_word_end(&text, "@default", "@pbx.example.com").replace(
        "sip_invite_domain=default",
        "sip_invite_domain=pbx.example.com",
    )
}

fn is_registered_contact(leg: &FlattenedLeg) -> bool {
    matches!(leg.target(), LegTarget::Endpoint(Endpoint::Sofia(_)))
}

/// Registered-contact legs sorted in place, every other byte kept, since the
/// registration rows come back in no stable order.
fn registered_legs_sorted(text: &str) -> String {
    let Ok(list) = FlattenedDialString::parse_for(text, target_under_test(API)) else {
        return text.to_owned();
    };
    let mut spans = Vec::new();
    let mut cursor = 0;
    for leg in list.legs() {
        let at = cursor
            + text[cursor..]
                .find(leg.raw())
                .unwrap_or_else(|| panic!("leg {:?} is not in {text:?}", leg.raw()));
        let end = at
            + leg
                .raw()
                .len();
        spans.push((at, end, is_registered_contact(leg)));
        cursor = end;
    }
    let mut contacts: Vec<&str> = spans
        .iter()
        .filter(|(_, _, registered)| *registered)
        .map(|&(at, end, _)| &text[at..end])
        .collect();
    contacts.sort_unstable();
    let mut contacts = contacts.into_iter();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (at, end, registered) in spans {
        out.push_str(&text[last..at]);
        match registered {
            true => out.push_str(
                contacts
                    .next()
                    .expect("one sorted contact per registered span"),
            ),
            false => out.push_str(&text[at..end]),
        }
        last = end;
    }
    out.push_str(&text[last..]);
    out
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_group_call_output_matches_the_fixtures() {
    let (client, _events, _permit) = connect().await;

    let mut names: Vec<String> = std::fs::read_dir(fixture_dir())
        .expect("fixture directory")
        .map(|entry| {
            entry
                .expect("fixture directory entry")
                .file_name()
                .into_string()
                .expect("fixture names are UTF-8")
        })
        .filter_map(|name| {
            name.strip_suffix(".txt")
                .map(str::to_owned)
        })
        .filter(|name| !name.starts_with("pbx-"))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no fixture found in {:?}", fixture_dir());

    let registrar_port = registrar_port(&client).await;
    let mut drifted = Vec::new();
    let mut unrendered = Vec::new();
    let mut parsed = 0;
    for name in &names {
        let (group, flag) = name
            .rsplit_once('.')
            .unwrap_or_else(|| panic!("{name} is not <group>.<flag>"));
        let fixture = std::fs::read_to_string(fixture_dir().join(format!("{name}.txt")))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let live = live_group_call(&client, group, flag).await;

        if registered_legs_sorted(&sanitise(&live, &registrar_port))
            != registered_legs_sorted(&fixture)
        {
            drifted.push(format!("{name}: live {live:?}, fixture {fixture:?}"));
        }
        // A list the switch itself cannot read on a carrier has nothing to render there.
        for carrier in [API, DIALPLAN] {
            let Ok(list) = FlattenedDialString::parse_for(&live, target_under_test(carrier)) else {
                continue;
            };
            parsed += 1;
            let raw = list
                .display_raw()
                .to_string();
            if raw != live {
                unrendered.push(format!("{name} at {carrier:?}: {live:?} rendered {raw:?}"));
            }
        }
    }

    assert!(parsed > 0, "no live body parsed on either carrier");
    assert!(
        drifted.is_empty(),
        "group_call output drifted: {drifted:#?}"
    );
    assert!(
        unrendered.is_empty(),
        "display_raw is not the input: {unrendered:#?}"
    );
}

/// Captures whose legs the typed view is checked against, as `(group, flag)`.
const TYPED_CAPTURES: &[(&str, &str)] = &[
    ("g-fp-static", "A"),
    ("g-fp-scopes-all", "A"),
    ("g-fp-scopes-both", "A"),
    ("g-fp-scopes-ent", "A"),
    ("g-fp-scopes-noleg", "A"),
    ("g-fp-scopes-none", "A"),
    ("g-fp-pipe", "A"),
    ("g-fp-ent", "none"),
    ("g-fp-esc-bs-exp", "A"),
    ("g-fp-esc-comma", "A"),
    ("g-fp-esc-emptyval", "A"),
    ("g-fp-esc-quotedempty", "A"),
    ("g-fp-esc-nested", "A"),
    ("g-fp-quote-member", "A"),
    ("g-fp-quote-pair", "A"),
    ("g-fp-gds", "A"),
    ("g-fp-reg", "A"),
    ("fp-one-empty", "A"),
];

/// The legs that get a channel. A later group rings only once every leg of the
/// one before has failed, and each first group here has a leg that answers.
fn dialed_legs(list: &FlattenedDialString) -> Vec<&FlattenedLeg> {
    list.threads()
        .filter_map(|thread| {
            thread
                .groups()
                .next()
        })
        .flat_map(|group| group.legs())
        .filter(|leg| matches!(leg.target(), LegTarget::Endpoint(_)))
        .collect()
}

type LegVars = BTreeMap<&'static str, Option<String>>;

/// What the typed view says the leg's channel carries for every probe key.
fn expected_vars(leg: &FlattenedLeg) -> LegVars {
    for warning in leg.warnings() {
        let key = match warning {
            LegWarning::PairCleared { key, .. }
            | LegWarning::NestedVarsRefused { key, .. }
            | LegWarning::PairIgnored { key, .. } => key,
            other => panic!("{:?}: unexpected warning {other}", leg.raw()),
        };
        assert!(
            leg.variable(Var("sentinel"))
                .is_some(),
            "{:?} warns about {key} but has no sentinel to prove the leg exists",
            leg.raw()
        );
        assert!(
            PROBE_KEYS.contains(&key.as_str()),
            "{key} is not a probe key"
        );
    }
    PROBE_KEYS
        .iter()
        .map(|key| {
            (
                *key,
                leg.variable(Var(key))
                    .map(str::to_owned),
            )
        })
        .collect()
}

fn received_vars(event: &EslEvent) -> LegVars {
    PROBE_KEYS
        .iter()
        .map(|key| {
            (
                *key,
                event
                    .variable_str(key)
                    .map(str::to_owned),
            )
        })
        .collect()
}

/// CHANNEL_CREATE and CHANNEL_HANGUP_COMPLETE of each outbound leg carrying a marker: a loopback
/// pair's `-a` half and a sofia leg's outbound side, never the inbound legs they cause.
#[derive(Default)]
struct MarkedLegs {
    created: Vec<EslEvent>,
    completed: Vec<EslEvent>,
}

impl MarkedLegs {
    async fn collect(
        &mut self,
        events: &mut EslEventStream,
        marker: &str,
        done: impl Fn(&Self) -> bool,
        deadline: Instant,
    ) {
        while !done(self) && Instant::now() < deadline {
            match tokio::time::timeout_at(deadline, events.recv()).await {
                Ok(Some(Ok(evt))) => {
                    if evt.variable_str("fp_marker") != Some(marker)
                        || evt.call_direction() != Ok(Some(CallDirection::Outbound))
                    {
                        continue;
                    }
                    match evt.event_type() {
                        Some(EslEventType::ChannelCreate) => self
                            .created
                            .push(evt),
                        Some(EslEventType::ChannelHangupComplete) => self
                            .completed
                            .push(evt),
                        _ => {}
                    }
                }
                Ok(Some(Err(e))) => panic!("event error collecting {marker}: {e}"),
                Ok(None) => panic!("event stream closed collecting {marker}"),
                Err(_) => break,
            }
        }
    }

    /// Until `want` legs are seen, then a short settle that catches a leg the typed view did not
    /// expect. A leg losing before its thread runs completes without being announced.
    async fn collect_dialled(&mut self, events: &mut EslEventStream, marker: &str, want: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        self.collect(
            events,
            marker,
            |legs| {
                legs.uuids()
                    .len()
                    >= want
            },
            deadline,
        )
        .await;
        let settle = Instant::now() + Duration::from_millis(500);
        self.collect(events, marker, |_| false, settle)
            .await;
    }

    /// Until every leg seen has completed.
    async fn collect_completed(&mut self, events: &mut EslEventStream, marker: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        self.collect(
            events,
            marker,
            |legs| {
                legs.uuids()
                    .len()
                    == uuids_of(&legs.completed).len()
            },
            deadline,
        )
        .await;
    }

    fn uuids(&self) -> Vec<String> {
        let mut uuids = uuids_of(&self.created);
        uuids.extend(uuids_of(&self.completed));
        uuids.sort();
        uuids.dedup();
        uuids
    }
}

async fn hupall(client: &EslClient, variable: &str, value: &str) {
    let cmd = format!("hupall NORMAL_CLEARING {variable} {value}");
    match client
        .api(&cmd)
        .await
    {
        Ok(resp) => {
            if let Err(e) = resp.api_result() {
                eprintln!("cleanup: {cmd}: {e}");
            }
        }
        Err(e) => eprintln!("cleanup: {cmd}: transport error: {e}"),
    }
}

/// The Call-ID tying a sofia leg to the inbound leg it caused, while it is up.
async fn call_id(client: &EslClient, uuid: &str) -> Option<String> {
    let cmd = UuidGetVar::new(uuid, "sip_call_id");
    let resp = match client
        .api(&cmd.to_string())
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            eprintln!("cleanup: {cmd}: transport error: {e}");
            return None;
        }
    };
    match resp.api_result() {
        Ok(UNDEF_VALUE) => None,
        Ok(value) => Some(value.to_owned()),
        // A losing leg is already gone, and its CANCEL took the inbound leg.
        Err(e)
            if e.command_failure()
                .and_then(CommandFailure::payload)
                .is_some_and(|p| p.contains("No such channel")) =>
        {
            None
        }
        Err(e) => {
            eprintln!("cleanup: {cmd}: {e}");
            None
        }
    }
}

fn uuids_of(created: &[EslEvent]) -> Vec<String> {
    created
        .iter()
        .filter_map(|evt| {
            evt.unique_id()
                .map(str::to_owned)
        })
        .collect()
}

/// Kill every leg `marker` tagged, and the inbound legs its sofia legs caused.
async fn reap_marked(client: &EslClient, marker: &str, created: &[EslEvent]) {
    for evt in created {
        let Some(uuid) = evt.unique_id() else {
            continue;
        };
        if evt
            .channel_name()
            .is_some_and(|name| name.starts_with("sofia/"))
        {
            if let Some(id) = call_id(client, uuid).await {
                hupall(client, "sip_call_id", &id).await;
            }
        }
    }
    hupall(client, "fp_marker", marker).await;
}

/// Compared on completion, which every leg reaches whether or not its creation was announced.
fn assert_legs_match(label: &str, list: &FlattenedDialString, legs: &MarkedLegs) {
    let mut expected: Vec<LegVars> = dialed_legs(list)
        .into_iter()
        .map(expected_vars)
        .collect();
    let mut received: Vec<LegVars> = legs
        .completed
        .iter()
        .map(received_vars)
        .collect();
    assert!(!expected.is_empty(), "{label}: the typed view dials no leg");
    expected.sort();
    received.sort();
    assert_eq!(
        received, expected,
        "{label}: legs disagree with the typed view"
    );
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_typed_view_equals_what_each_leg_received_over_the_api() {
    let (client, mut events, _permit) = connect().await;
    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::ChannelCreate,
                EslEventType::ChannelHangupComplete,
            ],
        )
        .await
        .expect("subscribe CHANNEL_CREATE and CHANNEL_HANGUP_COMPLETE");

    for (group, flag) in TYPED_CAPTURES {
        let label = format!("{group}.{flag} over the API");
        let m = marker(&format!("api-{group}-{flag}"));
        let body = live_group_call(&client, group, flag).await;
        let dial_string = format!("<fp_marker={m},originate_timeout=4>{body}");
        let list = parse(&dial_string, API);
        let want = dialed_legs(&list).len();

        let reply = client
            .api(&format!("originate {dial_string} &park()"))
            .await
            .unwrap_or_else(|e| panic!("{label}: originate transport error: {e}"));
        let mut legs = MarkedLegs::default();
        legs.collect_dialled(&mut events, &m, want)
            .await;
        reap_marked(&client, &m, &legs.created).await;
        legs.collect_completed(&mut events, &m)
            .await;
        wait_gone(&client, &label, &legs.uuids()).await;

        reply
            .api_result()
            .unwrap_or_else(|e| panic!("{label}: originate {dial_string} rejected: {e}"));
        assert_legs_match(&label, &list, &legs);
    }
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_typed_view_equals_what_each_leg_received_over_the_dialplan() {
    let (client, mut events, _permit) = connect().await;
    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::ChannelCreate,
                EslEventType::ChannelHangupComplete,
            ],
        )
        .await
        .expect("subscribe CHANNEL_CREATE and CHANNEL_HANGUP_COMPLETE");

    for (group, flag) in TYPED_CAPTURES {
        let label = format!("{group}.{flag} over the dialplan");
        let m = marker(&format!("dp-{group}-{flag}"));
        let body = live_group_call(&client, group, flag).await;
        let dial_string = format!("<fp_marker={m}>{{fp_marker={m}}}{body}");
        let list = parse(&dial_string, DIALPLAN);
        let want = dialed_legs(&list).len();

        let anchor = parked_anchor(&client, "null/anchor").await;
        let mut reaper = ChannelReaper::new(&client);
        reaper.track(&anchor);

        let bridge = client
            .execute_with_options(
                "bridge",
                Some(&dial_string),
                Some(&anchor),
                ExecuteOptions::new().with_async(),
            )
            .await
            .and_then(|resp| resp.into_result());
        let mut legs = MarkedLegs::default();
        if bridge.is_ok() {
            legs.collect_dialled(&mut events, &m, want)
                .await;
        }
        reap_marked(&client, &m, &legs.created).await;
        reaper
            .reap()
            .await;
        legs.collect_completed(&mut events, &m)
            .await;
        let mut channels = legs.uuids();
        channels.push(anchor);
        wait_gone(&client, &label, &channels).await;

        if let Err(e) = bridge {
            panic!("{label}: execute bridge {dial_string} rejected: {e}");
        }
        assert_legs_match(&label, &list, &legs);
    }
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_error_legs_end_with_the_cause_the_reading_predicts() {
    let (client, _events, _permit) = connect().await;

    let cases: &[(&str, HangupCause)] = &[
        ("g-fp-err-upper", HangupCause::UserBusy),
        ("g-fp-err-lower", HangupCause::UserBusy),
        ("g-fp-err-num", HangupCause::UserBusy),
        ("g-fp-err-prefix", HangupCause::UserBusy),
        ("g-fp-err-zero", HangupCause::DestinationOutOfOrder),
        ("g-fp-err-bogus", HangupCause::NormalClearing),
        ("g-fp-err-empty", HangupCause::NormalClearing),
        ("fp-none", HangupCause::NoRouteDestination),
        ("g-fp-unreg", HangupCause::UserNotRegistered),
    ];

    for (group, measured) in cases {
        let m = marker(&format!("err-{group}"));
        let body = live_group_call(&client, group, "A").await;
        let dial_string = format!("<fp_marker={m},originate_timeout=4>{body}");
        let list = parse(&dial_string, API);
        let legs: Vec<&FlattenedLeg> = list
            .legs()
            .collect();
        let [leg] = legs[..] else {
            panic!("{group}: {body:?} is not a single leg");
        };
        let LegTarget::Error(error) = leg.target() else {
            panic!("{group}: {body:?} is not an error leg");
        };
        let predicted = match error.reading() {
            CauseReading::Name(cause) => cause,
            CauseReading::Number(0) => HangupCause::DestinationOutOfOrder,
            CauseReading::Number(n) => u16::try_from(n)
                .ok()
                .and_then(HangupCause::from_number)
                .unwrap_or_else(|| panic!("{group}: {n} names no cause")),
            CauseReading::Unrecognized => HangupCause::NormalClearing,
            other => panic!("{group}: unexpected reading {other:?}"),
        };

        let reply: EslResult<String> = client
            .api(&format!("originate {dial_string} &park()"))
            .await
            .and_then(|resp| {
                resp.api_result()
                    .map(str::to_owned)
            });
        hupall(&client, "fp_marker", &m).await;
        if let Ok(uuid) = &reply {
            kill_channel(&client, uuid).await;
        }

        let err = reply.expect_err(&format!("{group}: originate {dial_string} succeeded"));
        let Some(CommandFailure::Err(cause)) = err.command_failure() else {
            panic!("{group}: expected -ERR <cause>, got {err}");
        };
        assert_eq!(
            cause,
            predicted.as_str(),
            "{group}: reading {:?}",
            error.reading()
        );
        assert_eq!(cause, measured.as_str(), "{group}");
    }
}

/// Wait until each channel is destroyed, so a leg the reap missed fails here
/// rather than burning a later test's session budget.
async fn wait_gone(client: &EslClient, label: &str, uuids: &[String]) {
    let deadline = Instant::now() + Duration::from_secs(5);
    for uuid in uuids {
        while channel_exists(client, uuid).await {
            assert!(
                Instant::now() < deadline,
                "{label}: {uuid} survived its reap"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// The values of `keys` on the channel `tagged` originates over `carrier`,
/// and every channel that took, for the caller to reap.
async fn received_over(
    client: &EslClient,
    carrier: DialStringCarrier,
    tagged: &str,
    keys: &[&str],
) -> (Result<Vec<Option<String>>, String>, Vec<String>) {
    match carrier {
        DialStringCarrier::EslApi => received_over_the_api(client, tagged, keys).await,
        _ => received_over_the_dialplan(client, tagged, keys).await,
    }
}

async fn received_over_the_api(
    client: &EslClient,
    tagged: &str,
    keys: &[&str],
) -> (Result<Vec<Option<String>>, String>, Vec<String>) {
    let reply = client
        .api(&format!("originate {tagged} &park()"))
        .await
        .map_err(|e| format!("originate transport error: {e}"))
        .and_then(|resp| {
            resp.api_result()
                .map(str::to_owned)
                .map_err(|e| format!("originate rejected: {e}"))
        });
    match reply {
        Ok(uuid) => (Ok(read_vars(client, &uuid, keys).await), vec![uuid]),
        Err(e) => (Err(e), Vec::new()),
    }
}

async fn received_over_the_dialplan(
    client: &EslClient,
    tagged: &str,
    keys: &[&str],
) -> (Result<Vec<Option<String>>, String>, Vec<String>) {
    let anchor = parked_anchor(client, "null/anchor").await;
    let bridged = client
        .execute_with_options(
            "bridge",
            Some(tagged),
            Some(&anchor),
            ExecuteOptions::new().with_async(),
        )
        .await
        .and_then(|resp| resp.into_result());
    if let Err(e) = bridged {
        return (Err(format!("execute bridge rejected: {e}")), vec![anchor]);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    match wait_for_bridge(client, &anchor, deadline).await {
        Some(BridgeOutcome::Bridged(peer)) => {
            let values = read_vars(client, &peer, keys).await;
            (Ok(values), vec![anchor, peer])
        }
        Some(BridgeOutcome::Failed(status)) => (
            Err(format!("the anchor's bridge failed: DIALSTATUS={status}")),
            vec![anchor],
        ),
        None => (Err("the anchor never bridged".to_owned()), vec![anchor]),
    }
}

async fn read_vars(client: &EslClient, uuid: &str, keys: &[&str]) -> Vec<Option<String>> {
    let mut values = Vec::with_capacity(keys.len());
    for key in keys {
        values.push(getvar(client, uuid, key).await);
    }
    values
}

/// `input` with `[fp_marker=<marker>]` ahead of its loopback endpoint.
fn tag(input: &str, marker: &str) -> String {
    let at = input
        .find("loopback/")
        .unwrap_or_else(|| panic!("{input:?} dials no loopback leg"));
    format!("{}[fp_marker={marker}]{}", &input[..at], &input[at..])
}

fn carrier_name(carrier: DialStringCarrier) -> &'static str {
    match carrier {
        DialStringCarrier::EslApi => "api",
        _ => "dialplan",
    }
}

/// Dial `input` over `carrier` and compare `keys` between the typed view of its
/// single leg and the channel.
async fn typed_view_against_channel(
    client: &EslClient,
    case: &str,
    input: &str,
    carrier: DialStringCarrier,
    keys: &[&str],
) -> Vec<Option<String>> {
    let m = marker(&format!("{case}-{}", carrier_name(carrier)));
    let tagged = tag(input, &m);
    let list = parse(&tagged, carrier);
    let legs: Vec<&FlattenedLeg> = list
        .legs()
        .collect();
    let [leg] = legs[..] else {
        panic!("{tagged:?} at {carrier:?} is not a single leg");
    };
    let typed: Vec<Option<String>> = keys
        .iter()
        .map(|key| {
            leg.variable(Var(key))
                .map(str::to_owned)
        })
        .collect();

    let (received, channels) = received_over(client, carrier, &tagged, keys).await;
    for uuid in &channels {
        kill_channel(client, uuid).await;
    }
    hupall(client, "fp_marker", &m).await;
    wait_gone(client, case, &channels).await;

    let received = received.unwrap_or_else(|e| panic!("{case}: {tagged:?} at {carrier:?}: {e}"));
    assert_eq!(
        received, typed,
        "{case}: {keys:?} of {tagged:?} at {carrier:?} disagree with the typed view"
    );
    received
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_empty_pairs_ignore_or_clear_by_arrival_depth() {
    let (client, _events, _permit) = connect().await;

    let cases: &[(&str, DialStringCarrier, bool)] = &[
        (r"{k=1}{sentinel=s,k=\\\'\\\'}loopback/9199/test", API, true),
        (r"{k=1}{sentinel=s,k=''}loopback/9199/test", API, false),
        (r"{k=1}{sentinel=s,k=}loopback/9199/test", API, false),
        (
            r"[k=1][sentinel=s,k=\\\\\\\\\\\\\\\'\\\\\\\\\\\\\\\']loopback/9199/test",
            API,
            true,
        ),
        (
            r"[k=1][sentinel=s,k=\\\\\\\'\\\\\\\']loopback/9199/test",
            API,
            false,
        ),
        (
            r"{k=1}{sentinel=s,k=\\'\\'}loopback/9199/test",
            DIALPLAN,
            true,
        ),
        (r"{k=1}{sentinel=s,k=''}loopback/9199/test", DIALPLAN, false),
        (
            r"[k=1][sentinel=s,k=\\\\\\\\\\\\\\'\\\\\\\\\\\\\\']loopback/9199/test",
            DIALPLAN,
            true,
        ),
        (
            r"[k=1][sentinel=s,k=\\\\\\'\\\\\\']loopback/9199/test",
            DIALPLAN,
            false,
        ),
    ];

    for (n, (input, carrier, cleared)) in cases
        .iter()
        .enumerate()
    {
        let received = typed_view_against_channel(
            &client,
            &format!("depth{n}"),
            input,
            *carrier,
            &["k", "sentinel"],
        )
        .await;
        let want_k = (!cleared).then(|| "1".to_owned());
        assert_eq!(
            received,
            [want_k, Some("s".to_owned())],
            "{input:?} at {carrier:?}"
        );
    }
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_wider_scope_wins_unless_the_leg_clobbers() {
    let (client, _events, _permit) = connect().await;

    let cases: &[(&str, Option<&str>)] = &[
        ("{k=g}[k=l]loopback/9199/test", Some("g")),
        (
            "{local_var_clobber=true,k=g}[k=l]loopback/9199/test",
            Some("l"),
        ),
        ("<k=e>[k=l]loopback/9199/test", Some("e")),
        ("<k=e>{k=g}loopback/9199/test", Some("g")),
        ("<k=e>{k=g}[k=l]loopback/9199/test", None),
    ];

    for (n, (input, measured)) in cases
        .iter()
        .enumerate()
    {
        for carrier in [API, DIALPLAN] {
            let received =
                typed_view_against_channel(&client, &format!("scope{n}"), input, carrier, &["k"])
                    .await;
            if let Some(measured) = measured {
                assert_eq!(
                    received,
                    [Some((*measured).to_owned())],
                    "{input:?} at {carrier:?}"
                );
            }
        }
    }
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_argv_split_matches_originate_split() {
    let (client, _events, _permit) = connect().await;

    let unclosed = r"originate {v=x\\'y z}loopback/9199/test &park()";
    assert!(
        matches!(
            originate_split(unclosed, ' '),
            Err(OriginateError::UnclosedQuote(_))
        ),
        "{unclosed:?}"
    );
    assert_eq!(
        FlattenedDialString::parse_for(r"{v=x\\'y z}loopback/9199/test", target_under_test(API)),
        Err(FlattenedDialStringError::ArgvSplit)
    );
    let reply: EslResult<String> = client
        .api(unclosed)
        .await
        .and_then(|resp| {
            resp.api_result()
                .map(str::to_owned)
        });
    if let Ok(uuid) = &reply {
        kill_channel(&client, uuid).await;
    }
    let err = reply.expect_err("an unclosed quote must not originate");
    assert!(
        matches!(err.command_failure(), Some(CommandFailure::Usage(_))),
        "{unclosed:?} answered {err}"
    );

    for (case, dial_string, value) in [
        ("argv-space", r"{v=a\ b}loopback/9199/test", r"a\ b"),
        ("argv-tab", "{v=a\tb}loopback/9199/test", "a\tb"),
    ] {
        let line = format!("originate {dial_string} &park()");
        assert_eq!(
            originate_split(&line, ' ').unwrap_or_else(|e| panic!("{line:?}: {e}")),
            ["originate", dial_string, "&park()"],
            "{case}"
        );
        let received = typed_view_against_channel(&client, case, dial_string, API, &["v"]).await;
        assert_eq!(received, [Some(value.to_owned())], "{case}");
    }
}

/// Every escaping case the renderer carries in `scope`, rendered for `carrier`,
/// read through the typed view and off the channel the same dial string creates.
async fn escaping_through_the_typed_view(carrier: DialStringCarrier, scope: VariablesType) {
    let (client, _events, _permit) = connect().await;

    for separator in [None, Some(ESCAPING_SEPARATOR)] {
        for (case, pairs) in ESCAPING_CASES {
            if !carried_in(scope, pairs) {
                continue;
            }
            let label =
                format!("{case} in {scope:?} scope, separator {separator:?}, at {carrier:?}");
            // The dialplan carrier finds its far leg by a pre-assigned uuid.
            let b_uuid = match carrier {
                DialStringCarrier::EslApi => None,
                _ => Some(create_uuid(&client).await),
            };
            let lead: Vec<(&str, &str)> = b_uuid
                .iter()
                .map(|uuid| ("origination_uuid", uuid.as_str()))
                .collect();
            let vars = escaping_block(&lead, pairs, separator, scope);
            let dial_string = format!(
                "{}null/escaping",
                vars.display_for(target_under_test(carrier))
            );

            let list = parse(&dial_string, carrier);
            let legs: Vec<&FlattenedLeg> = list
                .legs()
                .collect();
            let [leg] = legs[..] else {
                panic!("{label}: {dial_string:?} is not a single leg");
            };
            let keys: Vec<&str> = pairs
                .iter()
                .map(|(key, _)| *key)
                .collect();
            let typed: Vec<Option<String>> = keys
                .iter()
                .map(|key| {
                    leg.variable(Var(key))
                        .map(str::to_owned)
                })
                .collect();

            let (received, channels) = received_over(&client, carrier, &dial_string, &keys).await;
            let mut reaper = ChannelReaper::new(&client);
            for uuid in channels
                .iter()
                .chain(&b_uuid)
            {
                reaper.track(uuid);
            }
            reaper
                .reap()
                .await;

            let received = received.unwrap_or_else(|e| panic!("{label}: {dial_string:?}: {e}"));
            if let Some(b_uuid) = &b_uuid {
                assert_eq!(
                    channels.last(),
                    Some(b_uuid),
                    "{label}: the far leg of {dial_string:?} is not its origination_uuid"
                );
            }
            for (((key, want), typed), got) in pairs
                .iter()
                .zip(typed)
                .zip(received)
            {
                assert!(
                    typed.as_deref() == Some(*want) && got.as_deref() == Some(*want),
                    "{label}: {key} of {dial_string:?}: expected {want:?}, typed {typed:?}, switch {got:?}"
                );
            }
        }
    }
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_typed_view_reads_escaping_depths_over_the_api() {
    escaping_through_the_typed_view(API, VariablesType::Default).await;
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_typed_view_reads_escaping_depths_over_the_dialplan() {
    escaping_through_the_typed_view(DIALPLAN, VariablesType::Default).await;
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_typed_view_reads_escaping_depths_over_the_api_in_enterprise_scope() {
    escaping_through_the_typed_view(API, VariablesType::Enterprise).await;
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_typed_view_reads_escaping_depths_over_the_dialplan_in_enterprise_scope() {
    escaping_through_the_typed_view(DIALPLAN, VariablesType::Enterprise).await;
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_typed_view_reads_escaping_depths_over_the_api_in_channel_scope() {
    escaping_through_the_typed_view(API, VariablesType::Channel).await;
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_typed_view_reads_escaping_depths_over_the_dialplan_in_channel_scope() {
    escaping_through_the_typed_view(DIALPLAN, VariablesType::Channel).await;
}

// --- The `^^<sep>` argument separator ---

const ARGV_SEPARATORS: [char; 2] = ['~', '!'];

fn argv_target(sep: char) -> DialStringTarget {
    target_under_test(API)
        .with_argv_separator(sep)
        .unwrap_or_else(|e| panic!("{sep:?}: {e}"))
}

/// Runs of three or more backslashes: the depths a quote or a literal backslash is written at,
/// the one escaping `sep`, a comma or a pipe not counted.
fn deep_escapes(text: &str, sep: Option<char>) -> Vec<usize> {
    let text = match sep {
        Some(sep) => text.replace(&format!("\\{sep}"), &sep.to_string()),
        None => text.to_owned(),
    };
    // A literal run is a power of two; an escaped comma or pipe adds one, doubled at a separator.
    let escape = if sep.is_some() { 2 } else { 1 };
    let mut runs = Vec::new();
    let mut run = 0;
    for c in text
        .chars()
        .chain(['\0'])
    {
        if c == '\\' {
            run += 1;
            continue;
        }
        let deep = if matches!(c, ',' | '|') && run % (2 * escape) == escape {
            run - escape
        } else {
            run
        };
        if deep >= 3 {
            runs.push(deep);
        }
        run = 0;
    }
    runs
}

/// `keys` off the dump of the channel `originate ^^<sep><dial><sep>&park()` returns, and that
/// channel with its loopback partner for the caller to reap.
async fn received_at_argv(
    client: &EslClient,
    sep: char,
    dial: &str,
    keys: &[&str],
) -> (Result<Vec<Option<String>>, String>, Vec<String>) {
    let reply = client
        .api(&format!("originate ^^{sep}{dial}{sep}&park()"))
        .await
        .map_err(|e| format!("originate transport error: {e}"))
        .and_then(|resp| {
            resp.api_result()
                .map(str::to_owned)
                .map_err(|e| format!("originate rejected: {e}"))
        });
    let uuid = match reply {
        Ok(uuid) => uuid,
        Err(e) => return (Err(e), Vec::new()),
    };
    let dump = client
        .api(&format!("uuid_dump {uuid}"))
        .await
        .map_err(|e| format!("uuid_dump transport error: {e}"))
        .and_then(|resp| {
            resp.body()
                .map(str::to_owned)
                .ok_or_else(|| "uuid_dump answered no body".to_owned())
        })
        .and_then(|body| parse_channel_dump(&body).map_err(|e| format!("uuid_dump: {e}")));
    let mut channels = vec![uuid];
    let values = dump.map(|dump| {
        channels.extend(
            dump.variable_str("other_loopback_leg_uuid")
                .map(str::to_owned),
        );
        keys.iter()
            .map(|key| {
                dump.variable_str(key)
                    .map(str::to_owned)
            })
            .collect()
    });
    (values, channels)
}

/// Every escaping case rendered by `Variables` and by `Endpoint` at an argv separator target,
/// read through the typed view and off the channel `originate ^^<sep>…<sep>&park()` creates.
async fn escaping_at_an_argv_separator(scope: VariablesType) {
    let (client, _events, _permit) = connect().await;

    for sep in ARGV_SEPARATORS {
        let target = argv_target(sep);
        for block_separator in [None, Some(ESCAPING_SEPARATOR)] {
            for (case, pairs) in ESCAPING_CASES {
                if !carried_in(scope, pairs) {
                    continue;
                }
                let label = format!(
                    "{case} in {scope:?} scope, block separator {block_separator:?}, at ^^{sep}"
                );
                let vars = escaping_block(&[], pairs, block_separator, scope);
                let at_api = vars
                    .display_for(target_under_test(API))
                    .to_string();
                let at_sep = vars
                    .display_for(target)
                    .to_string();
                assert_eq!(
                    deep_escapes(&at_sep, Some(sep)),
                    deep_escapes(&at_api, None),
                    "{label}: {at_sep} is not escaped as deep as {at_api}"
                );
                let endpoint = Endpoint::Loopback(
                    LoopbackEndpoint::new("9199")
                        .with_context("test")
                        .with_variables(vars),
                );
                let keys: Vec<&str> = pairs
                    .iter()
                    .map(|(key, _)| *key)
                    .collect();

                for dial in [
                    format!("{at_sep}null/escaping"),
                    endpoint
                        .display_for(target)
                        .to_string(),
                ] {
                    let list = FlattenedDialString::parse_for(&dial, target)
                        .unwrap_or_else(|e| panic!("{label}: {dial:?}: {e}"));
                    let legs: Vec<&FlattenedLeg> = list
                        .legs()
                        .collect();
                    let [leg] = legs[..] else {
                        panic!("{label}: {dial:?} is not a single leg");
                    };
                    let typed: Vec<Option<String>> = keys
                        .iter()
                        .map(|key| {
                            leg.variable(Var(key))
                                .map(str::to_owned)
                        })
                        .collect();

                    let (received, channels) = received_at_argv(&client, sep, &dial, &keys).await;
                    for uuid in &channels {
                        kill_channel(&client, uuid).await;
                    }
                    wait_gone(&client, &label, &channels).await;

                    let received = received.unwrap_or_else(|e| panic!("{label}: {dial:?}: {e}"));
                    for (((key, want), typed), got) in pairs
                        .iter()
                        .zip(typed)
                        .zip(received)
                    {
                        assert!(
                            typed.as_deref() == Some(*want) && got.as_deref() == Some(*want),
                            "{label}: {key} of {dial:?}: expected {want:?}, typed {typed:?}, switch {got:?}"
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_escaping_depths_at_an_argv_separator() {
    escaping_at_an_argv_separator(VariablesType::Default).await;
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_escaping_depths_at_an_argv_separator_in_enterprise_scope() {
    escaping_at_an_argv_separator(VariablesType::Enterprise).await;
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_escaping_depths_at_an_argv_separator_in_channel_scope() {
    escaping_at_an_argv_separator(VariablesType::Channel).await;
}

const ARGV_CAPTURES: &[&str] = &[
    "g-fp-argv-apos2",
    "g-fp-argv-bsbs",
    "g-fp-argv-bsn",
    "g-fp-argv-bsq",
    "g-fp-argv-bss",
    "g-fp-argv-bstilde",
    "g-fp-argv-caret",
    "g-fp-argv-lastesc",
    "g-fp-argv-leg2",
    "g-fp-argv-space",
    "g-fp-argv-tilde",
    "g-fp-argv-trail-last",
    "g-fp-argv-trail-mid",
];

/// `group_call` output tagged with `marker`, escaped as one argument of `target`'s split.
async fn argv_capture(
    client: &EslClient,
    group: &str,
    marker: &str,
    target: DialStringTarget,
) -> String {
    let body = live_group_call(client, group, "A").await;
    target
        .escape_argument(&format!("<fp_marker={marker},originate_timeout=4>{body}"))
        .expect("the API carrier escapes")
        .into_owned()
}

/// Originate `list`'s raw text under `^^<sep>`, or on a plain line without one, and compare each
/// leg it dials with its channel.
async fn dial_at_argv(
    client: &EslClient,
    events: &mut EslEventStream,
    label: &str,
    sep: Option<char>,
    list: &FlattenedDialString,
    marker: &str,
) {
    let line = match sep {
        Some(sep) => format!("originate ^^{sep}{}{sep}&park()", list.display_raw()),
        None => format!("originate {} &park()", list.display_raw()),
    };
    let reply = client
        .api(&line)
        .await
        .unwrap_or_else(|e| panic!("{label}: originate transport error: {e}"));
    let mut legs = MarkedLegs::default();
    legs.collect_dialled(events, marker, dialed_legs(list).len())
        .await;
    reap_marked(client, marker, &legs.created).await;
    legs.collect_completed(events, marker)
        .await;
    wait_gone(client, label, &legs.uuids()).await;

    reply
        .api_result()
        .unwrap_or_else(|e| panic!("{label}: {line} rejected: {e}"));
    assert_legs_match(label, list, &legs);
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_typed_view_equals_what_each_leg_received_at_an_argv_separator() {
    let (client, mut events, _permit) = connect().await;
    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::ChannelCreate,
                EslEventType::ChannelHangupComplete,
            ],
        )
        .await
        .expect("subscribe CHANNEL_CREATE and CHANNEL_HANGUP_COMPLETE");

    for (s, sep) in ARGV_SEPARATORS
        .into_iter()
        .enumerate()
    {
        let target = argv_target(sep);
        for group in ARGV_CAPTURES {
            let label = format!("{group}.A at ^^{sep}");
            let m = marker(&format!("argv{s}-{group}"));
            let escaped = argv_capture(&client, group, &m, target).await;
            let list = FlattenedDialString::parse_for(&escaped, target)
                .unwrap_or_else(|e| panic!("{label}: {escaped:?}: {e}"));
            assert_eq!(
                list.display_raw()
                    .to_string(),
                escaped,
                "{label}"
            );
            dial_at_argv(&client, &mut events, &label, Some(sep), &list, &m).await;
        }
    }
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_blank_split_forwards_the_legs_retain_keeps() {
    let (client, mut events, _permit) = connect().await;
    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::ChannelCreate,
                EslEventType::ChannelHangupComplete,
            ],
        )
        .await
        .expect("subscribe CHANNEL_CREATE and CHANNEL_HANGUP_COMPLETE");

    let target = target_under_test(API);
    for (group, drops) in [
        ("g-fp-argv-space", &[None][..]),
        ("g-fp-argv-leg2", &[None, Some(0), Some(1)][..]),
    ] {
        for &dropped in drops {
            let drop_name = dropped.map_or_else(|| "none".to_owned(), |leg| leg.to_string());
            let label = format!("{group}.A without leg {drop_name} at the blank split");
            let m = marker(&format!("blank-{group}-drop-{drop_name}"));
            let escaped = argv_capture(&client, group, &m, target).await;
            let mut list = FlattenedDialString::parse_for(&escaped, target)
                .unwrap_or_else(|e| panic!("{label}: {escaped:?}: {e}"));
            assert_eq!(
                list.display_raw()
                    .to_string(),
                escaped,
                "{label}"
            );
            if let Some(dropped) = dropped {
                let mut index = 0;
                list.retain(|_| {
                    index += 1;
                    index - 1 != dropped
                });
            }
            dial_at_argv(&client, &mut events, &label, None, &list, &m).await;
        }
    }
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_argv_separator_forwards_the_legs_retain_keeps() {
    let (client, mut events, _permit) = connect().await;
    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::ChannelCreate,
                EslEventType::ChannelHangupComplete,
            ],
        )
        .await
        .expect("subscribe CHANNEL_CREATE and CHANNEL_HANGUP_COMPLETE");

    for (s, sep) in ARGV_SEPARATORS
        .into_iter()
        .enumerate()
    {
        let target = argv_target(sep);
        for group in ["g-fp-argv-leg2", "g-fp-argv-lastesc"] {
            for dropped in 0..2 {
                let label = format!("{group}.A without leg {dropped} at ^^{sep}");
                let m = marker(&format!("argv{s}-{group}-drop{dropped}"));
                let escaped = argv_capture(&client, group, &m, target).await;
                let mut list = FlattenedDialString::parse_for(&escaped, target)
                    .unwrap_or_else(|e| panic!("{label}: {escaped:?}: {e}"));
                assert_eq!(
                    list.legs()
                        .count(),
                    2,
                    "{label}: {escaped:?}"
                );
                let mut index = 0;
                list.retain(|_| {
                    index += 1;
                    index - 1 != dropped
                });
                dial_at_argv(&client, &mut events, &label, Some(sep), &list, &m).await;
            }
        }
    }
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_argv_separator_dial_string_edge_spaces() {
    let (client, mut events, _permit) = connect().await;
    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::ChannelCreate,
                EslEventType::ChannelHangupComplete,
            ],
        )
        .await
        .expect("subscribe CHANNEL_CREATE and CHANNEL_HANGUP_COMPLETE");

    for (s, sep) in ARGV_SEPARATORS
        .into_iter()
        .enumerate()
    {
        let target = argv_target(sep);
        for escaped in [true, false] {
            let label = format!("edge spaces, escaped {escaped}, at ^^{sep}");
            let m = marker(&format!("argv{s}-edge-{escaped}"));
            let spaced = format!("  <fp_marker={m}>{{k=a b,sentinel=s}}loopback/9199/test ");
            let dial = match escaped {
                true => target
                    .escape_argument(&spaced)
                    .expect("a separator target escapes")
                    .into_owned(),
                false => spaced,
            };
            let list = FlattenedDialString::parse_for(&dial, target)
                .unwrap_or_else(|e| panic!("{label}: {dial:?}: {e}"));
            assert_eq!(
                list.display_raw()
                    .to_string(),
                dial,
                "{label}"
            );
            dial_at_argv(&client, &mut events, &label, Some(sep), &list, &m).await;
        }
    }
}

/// The session thread announces CHANNEL_CREATE from `CS_INIT`, so a losing leg the originate hangs
/// up before its thread runs that state is never announced, while CHANNEL_HANGUP_COMPLETE follows
/// every leg. `leg_delay_start` holds a leg's thread until the other leg has won.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_a_leg_that_loses_before_its_thread_runs_announces_no_create() {
    let (client, mut events, _permit) = connect().await;
    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::ChannelCreate,
                EslEventType::ChannelHangupComplete,
            ],
        )
        .await
        .expect("subscribe CHANNEL_CREATE and CHANNEL_HANGUP_COMPLETE");

    let m = marker("lose-before-init");
    let dial = format!(
        "<fp_marker={m},originate_timeout=4>[leg_delay_start=2]loopback/9199/test,loopback/9199/test"
    );
    let reply = client
        .api(&format!("originate {dial} &park()"))
        .await
        .and_then(|resp| {
            resp.api_result()
                .map(str::to_owned)
        });
    hupall(&client, "fp_marker", &m).await;

    let mut legs = MarkedLegs::default();
    let deadline = Instant::now() + Duration::from_secs(10);
    legs.collect(
        &mut events,
        &m,
        |legs| {
            legs.completed
                .len()
                == 2
        },
        deadline,
    )
    .await;
    wait_gone(&client, &m, &legs.uuids()).await;

    reply.unwrap_or_else(|e| panic!("originate {dial}: {e}"));
    assert_eq!(
        legs.completed
            .len(),
        2,
        "{dial}: legs that completed"
    );
    assert_eq!(
        legs.created
            .len(),
        1,
        "{dial}: legs announced"
    );
}
