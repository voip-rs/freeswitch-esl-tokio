//! Integration tests against a live FreeSWITCH instance: originate targets
//! (application/extension/inline), the channel timetable, uuid_setvar/getvar
//! and uuid_kill round trips, and dial-string escaping per carrier.
//!
//! These tests require a live FreeSWITCH ESL; see docs/live-test-switch.md.
//! Run with: cargo test --test 'live_*' -- --ignored

mod live_common;

use freeswitch_esl_tokio::commands::originate::{Variables, VariablesType};
use freeswitch_esl_tokio::commands::{
    DialStringCarrier, ExecuteOn, LoopbackEndpoint, SofiaEndpoint, UserEndpoint, UuidGetVar,
    UuidKill, UuidSetVar,
};
use freeswitch_esl_tokio::variables::SofiaVariable;
use freeswitch_esl_tokio::ExecuteOptions;
use freeswitch_esl_tokio::{
    parse_channel_dump, Application, BridgeDialString, ChannelTimetable, ChannelVariable,
    CommandFailure, DialplanType, Endpoint, EslClient, EslEventStream, EslEventType, EventFormat,
    EventHeader, HeaderLookup, LogLevel, Originate, TimetableField, TimetablePrefix,
};
use live_common::{
    bgapi_originate_ok, block_parse_under_test, carried_in, channel_exists, connect, create_uuid,
    escaping_block, getvar, kill_channel, parked_anchor, percent_escape_tree, switch_version,
    target_under_test, wait_for_bridge, wait_for_own_event, BridgeOutcome, ChannelReaper,
    PercentEscape, ESCAPING_CASES, ESCAPING_SEPARATOR,
};
use std::collections::HashSet;
use std::time::Duration;
use tokio::time::Instant;

/// The dialplan extension every originate in this file dials.
fn test_9199() -> Endpoint {
    Endpoint::Loopback(LoopbackEndpoint::new("9199").with_context("test"))
}

/// [`test_9199`] with application arguments delivered as the originate line left them.
fn unexpanded_9199() -> Endpoint {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("app_disable_expand_variables", "true");
    Endpoint::Loopback(
        LoopbackEndpoint::new("9199")
            .with_context("test")
            .with_variables(vars),
    )
}

/// Originate over bgapi and reap what it created. The originate is the
/// assertion: a dial string FreeSWITCH will not take comes back `-ERR`.
async fn originate_and_reap(cmd: &Originate) {
    let (client, mut events, _permit) = connect().await;

    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::BackgroundJob])
        .await
        .unwrap();

    let uuid = bgapi_originate_ok(&client, &mut events, cmd).await;
    kill_channel(&client, &uuid).await;
}

/// `&park()` holds the channel, so bgapi returns while it is still up.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_originate_application_target() {
    originate_and_reap(&Originate::application(
        test_9199(),
        Application::simple("park"),
    ))
    .await;
}

/// Extension target: routed through the XML dialplan rather than an app.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_originate_extension_target() {
    let cmd = Originate::extension(test_9199(), "9199")
        .dialplan(DialplanType::Xml)
        .unwrap()
        .context("test");

    originate_and_reap(&cmd).await;
}

/// An inline dialplan, which answers and hangs up before bgapi reports.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_originate_inline_target() {
    let cmd = Originate::inline(
        test_9199(),
        vec![
            Application::simple("answer"),
            Application::new("hangup", Some("NORMAL_CLEARING")),
        ],
    )
    .unwrap();

    originate_and_reap(&cmd).await;
}

/// A timeout without cid_name/cid_num forces `undef` placeholders into the
/// positional tail, which FreeSWITCH has to accept as NULL arguments.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_originate_timeout_fills_positional_gaps() {
    let cmd = Originate::application(test_9199(), Application::simple("park"))
        .timeout(Duration::from_secs(5));

    originate_and_reap(&cmd).await;
}

/// The first CHANNEL_EXECUTE of `app` on `uuid`, or `None` once the deadline passes.
async fn wait_for_own_execute(
    events: &mut freeswitch_esl_tokio::EslEventStream,
    uuid: &str,
    app: &str,
    deadline: Instant,
) -> Option<freeswitch_esl_tokio::EslEvent> {
    while let Some(evt) =
        wait_for_own_event(events, uuid, EslEventType::ChannelExecute, deadline).await
    {
        if evt.header(EventHeader::Application) == Some(app) {
            return Some(evt);
        }
    }
    None
}

type HeaderValues<'a> = &'a [(EventHeader, Option<&'a str>)];
type VariableValues<'a> = &'a [(&'a str, Option<&'a str>)];

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_originate_argv_separator_positionals() {
    originate_positionals(Some('~')).await;
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_originate_blank_positionals() {
    originate_positionals(None).await;
}

/// `originate_function` answers usage past seven arguments on either split.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_originate_refuses_an_eighth_argument() {
    let (client, _events, _permit) = connect().await;
    for line in [
        "originate loopback/9199/test &park() XML test a b 5 extra",
        "originate ^^~loopback/9199/test~&park()~XML~test~a~b~5~extra",
    ] {
        assert!(
            line.parse::<Originate>()
                .is_err(),
            "{line} parsed"
        );
        let resp = client
            .api(line)
            .await
            .unwrap_or_else(|e| panic!("{line}: transport error: {e}"));
        let reply = resp
            .api_result()
            .map(str::to_owned);
        if let Ok(uuid) = &reply {
            kill_channel(&client, uuid).await;
        }
        let err = reply.expect_err(line);
        assert!(
            err.to_string()
                .contains("USAGE"),
            "{line} answered {err}"
        );
    }
}

async fn originate_positionals(separator: Option<char>) {
    let (client, mut events, _permit) = connect().await;
    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::ChannelExecute])
        .await
        .expect("subscribe CHANNEL_EXECUTE");

    let separated = |cmd: Originate| match separator {
        Some(sep) => cmd
            .with_argv_separator(sep)
            .expect("the separator splits originate's arguments"),
        None => cmd,
    };
    let park = || {
        Originate::inline(test_9199(), vec![Application::simple("park")])
            .expect("park is deliverable")
    };
    let socket = Originate::application(
        test_9199(),
        Application::new("socket", Some("127.0.0.1:1 async full")),
    )
    .dialplan(DialplanType::Xml)
    .expect("an application target takes XML")
    .context("test")
    .cid_name("  Front  Desk  ")
    .cid_num("5551234")
    .timeout(Duration::from_secs(6));
    let empty_last = separated(
        park()
            .context("test")
            .cid_name("Last Empty")
            .cid_num(""),
    );
    assert!(
        empty_last
            .to_string()
            .ends_with(&format!("{}''", separator.unwrap_or(' '))),
        "{empty_last}"
    );

    let cases: Vec<(&str, Originate, &str, HeaderValues, VariableValues)> = vec![
        (
            "socket target",
            separated(socket),
            "socket",
            &[(EventHeader::ApplicationData, Some("127.0.0.1:1 async full"))],
            &[
                ("origination_caller_id_name", Some("  Front  Desk  ")),
                ("origination_caller_id_number", Some("5551234")),
            ],
        ),
        (
            "absent context forced by a later slot",
            separated(park().cid_name("Ctx Undef")),
            "park",
            &[
                (EventHeader::CallerContext, Some("default")),
                (EventHeader::CallerDialplan, Some("inline")),
            ],
            &[("origination_caller_id_name", Some("Ctx Undef"))],
        ),
        (
            "empty context",
            separated(
                park()
                    .context("")
                    .cid_name("Ctx Empty"),
            ),
            "park",
            &[(EventHeader::CallerContext, Some("test"))],
            &[("origination_caller_id_name", Some("Ctx Empty"))],
        ),
        (
            "spaced context",
            separated(
                park()
                    .context("ctx with space")
                    .cid_name("Ctx Named"),
            ),
            "park",
            &[(EventHeader::CallerContext, Some("ctx with space"))],
            &[("origination_caller_id_name", Some("Ctx Named"))],
        ),
        (
            "empty last slot",
            empty_last,
            "park",
            &[(EventHeader::CallerContext, Some("test"))],
            &[
                ("origination_caller_id_name", Some("Last Empty")),
                ("origination_caller_id_number", None),
            ],
        ),
        (
            "single quote in an application argument and a caller id",
            separated(
                Originate::application(test_9199(), Application::new("set", Some("quoted=it's")))
                    .cid_name("it's"),
            ),
            "set",
            &[(EventHeader::ApplicationData, Some("quoted=it's"))],
            &[("origination_caller_id_name", Some("it's"))],
        ),
        (
            "single quote beside a comma in an inline argument",
            separated(
                Originate::inline(
                    test_9199(),
                    vec![
                        Application::new("set", Some("quoted=it's,b")),
                        Application::simple("park"),
                    ],
                )
                .expect("one quote is deliverable")
                .cid_num("it's"),
            ),
            "set",
            &[(EventHeader::ApplicationData, Some("quoted=it's,b"))],
            &[("origination_caller_id_number", Some("it's"))],
        ),
        (
            "backslash escapes in an application argument and a caller id",
            separated(
                Originate::application(
                    unexpanded_9199(),
                    Application::new("set", Some(r"lit=a\nb\\c\sd")),
                )
                .cid_name(r"x\ny")
                .cid_num(r"\t"),
            ),
            "set",
            &[(EventHeader::ApplicationData, Some(r"lit=a\nb\\c\sd"))],
            &[
                ("origination_caller_id_name", Some(r"x\ny")),
                ("origination_caller_id_number", Some(r"\t")),
            ],
        ),
    ];

    for (label, cmd, app, headers, variables) in cases {
        let line = cmd.to_string();
        let reply = client
            .api(&line)
            .await
            .and_then(|resp| {
                resp.api_result()
                    .map(str::to_owned)
            });
        let mut reaper = ChannelReaper::new(&client);
        let executed = match &reply {
            Ok(uuid) => {
                reaper.track(uuid);
                let deadline = Instant::now() + Duration::from_secs(10);
                wait_for_own_execute(&mut events, uuid, app, deadline).await
            }
            Err(_) => None,
        };
        reaper
            .reap()
            .await;

        let uuid = reply.unwrap_or_else(|e| panic!("{label}: {line} rejected: {e}"));
        let evt = executed
            .unwrap_or_else(|| panic!("{label}: no CHANNEL_EXECUTE {app} on {uuid} from {line}"));
        for (header, want) in headers {
            assert_eq!(evt.header(*header), *want, "{label}: {header} from {line}");
        }
        for (name, want) in variables {
            assert_eq!(evt.variable_str(name), *want, "{label}: {name} from {line}");
        }
    }
}

/// A leg that never answers, so the originate ends when its timeout does.
async fn unanswered_originate_elapsed(
    client: &freeswitch_esl_tokio::EslClient,
    seconds: u64,
) -> (
    Result<String, freeswitch_esl_tokio::EslError>,
    String,
    Duration,
) {
    let marker = format!("argv-timeout-{}-{seconds}", std::process::id());
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("fp_marker", marker.as_str());
    vars.insert("ignore_early_media", "true");
    let cmd = Originate::application(
        Endpoint::Loopback(LoopbackEndpoint::new("app=park").with_variables(vars)),
        Application::simple("park"),
    )
    .timeout(Duration::from_secs(seconds))
    .with_argv_separator('~')
    .expect("'~' separates originate's arguments");
    let line = cmd.to_string();
    let started = Instant::now();
    let reply = client
        .api(&line)
        .await
        .and_then(|resp| {
            resp.api_result()
                .map(str::to_owned)
        });
    let elapsed = started.elapsed();
    if let Ok(uuid) = &reply {
        kill_channel(client, uuid).await;
    }
    let hupall = format!("hupall NORMAL_CLEARING fp_marker {marker}");
    match client
        .api(&hupall)
        .await
    {
        Ok(resp) => {
            if let Err(e) = resp.api_result() {
                eprintln!("cleanup: {hupall}: {e}");
            }
        }
        Err(e) => eprintln!("cleanup: {hupall}: transport error: {e}"),
    }
    (reply, line, elapsed)
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_originate_argv_separator_timeout_is_read() {
    let (client, _events, _permit) = connect().await;

    let (short, short_line, short_elapsed) = unanswered_originate_elapsed(&client, 1).await;
    let (long, long_line, long_elapsed) = unanswered_originate_elapsed(&client, 3).await;

    for (reply, line) in [(&short, &short_line), (&long, &long_line)] {
        assert!(
            matches!(reply, Err(e) if !matches!(e, freeswitch_esl_tokio::EslError::Timeout { .. })),
            "{line} answered {reply:?}"
        );
    }
    // Load on a shared switch only lengthens both, so a floor and their gap hold under it.
    assert!(
        long_elapsed >= Duration::from_millis(2800),
        "{long_line} took {long_elapsed:?}"
    );
    assert!(
        long_elapsed >= short_elapsed + Duration::from_millis(1000),
        "{short_line} took {short_elapsed:?}, {long_line} took {long_elapsed:?}"
    );
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_channel_timetable_on_create() {
    let (client, mut events, _permit) = connect().await;

    client
        .subscribe_events(
            EventFormat::Plain,
            &[EslEventType::ChannelCreate, EslEventType::ChannelDestroy],
        )
        .await
        .unwrap();

    // Originate a call to &park() — creates a channel that immediately parks
    let resp = client
        .api("originate null/test &park()")
        .await
        .unwrap();
    let uuid = resp
        .api_result()
        .expect("originate failed")
        .to_string();

    // `park` holds the channel open indefinitely, so nothing reaps it for us.
    let mut reaper = ChannelReaper::new(&client);
    reaper.track(&uuid);

    let deadline = Instant::now() + Duration::from_secs(5);
    let created_event =
        wait_for_own_event(&mut events, &uuid, EslEventType::ChannelCreate, deadline).await;

    reaper
        .reap()
        .await;

    let evt = created_event
        .unwrap_or_else(|| panic!("did not receive CHANNEL_CREATE with timetable for {}", uuid));
    let tt = evt
        .caller_timetable()
        .expect("timetable should parse without error")
        .expect("CHANNEL_CREATE should have Caller timetable");

    // created must be a positive epoch-microsecond timestamp
    let created = tt
        .created
        .expect("created should be present on CHANNEL_CREATE");
    assert!(
        created > 1_000_000_000_000_000,
        "created timestamp should be a recent epoch-us value: {}",
        created
    );
    let profile_created = tt
        .profile_created
        .expect("profile_created should be present on CHANNEL_CREATE");
    assert!(
        profile_created > 1_000_000_000_000_000,
        "profile_created should be a recent epoch-us value: {}",
        profile_created
    );
    // answered/hungup should be 0 at creation time
    assert_eq!(tt.answered, Some(0), "answered should be 0 at create");
    assert_eq!(tt.hungup, Some(0), "hungup should be 0 at create");
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_uuid_setvar_getvar_round_trip() {
    let (client, mut events, _permit) = connect().await;

    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::BackgroundJob])
        .await
        .unwrap();

    // Create a channel to work with
    let cmd = Originate::application(test_9199(), Application::simple("park"));
    let uuid = bgapi_originate_ok(&client, &mut events, &cmd).await;

    // Set a variable on the channel
    let set_cmd = UuidSetVar::new(&uuid, "esl_test_var", "hello_world");
    let resp = client
        .api(&set_cmd.to_string())
        .await
        .unwrap();
    resp.api_result()
        .expect("uuid_setvar failed");

    // Get the variable back (uuid_getvar returns the raw value, no +OK prefix)
    let get_cmd = UuidGetVar::new(&uuid, "esl_test_var");
    let resp = client
        .api(&get_cmd.to_string())
        .await
        .unwrap();
    assert_eq!(
        resp.api_result()
            .unwrap(),
        "hello_world",
        "uuid_getvar should return the value we set"
    );

    kill_channel(&client, &uuid).await;
}

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_uuid_kill_with_cause() {
    let (client, mut events, _permit) = connect().await;

    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::BackgroundJob,
                EslEventType::ChannelHangupComplete,
            ],
        )
        .await
        .unwrap();

    let cmd = Originate::application(test_9199(), Application::simple("park"));
    let uuid = bgapi_originate_ok(&client, &mut events, &cmd).await;

    // Kill with a specific hangup cause
    let kill_cmd = UuidKill::with_cause(&uuid, freeswitch_esl_tokio::HangupCause::UserBusy);
    let resp = client
        .api(&kill_cmd.to_string())
        .await
        .unwrap();
    resp.api_result()
        .expect("uuid_kill failed");

    let deadline = Instant::now() + Duration::from_secs(5);
    let evt = wait_for_own_event(
        &mut events,
        &uuid,
        EslEventType::ChannelHangupComplete,
        deadline,
    )
    .await
    .unwrap_or_else(|| panic!("did not receive CHANNEL_HANGUP_COMPLETE for {uuid}"));

    let cause = evt
        .hangup_cause()
        .expect("should parse hangup cause")
        .expect("should have hangup cause");
    assert_eq!(
        cause,
        freeswitch_esl_tokio::HangupCause::UserBusy,
        "hangup cause should be USER_BUSY"
    );
}

// --- Endpoint text, per carrier ---

const CARRIERS: [DialStringCarrier; 2] = [DialStringCarrier::EslApi, DialStringCarrier::Dialplan];

/// A space, a quote, a backslash and a comma after `lead`, which tells one dial from another.
fn hostile_field(lead: &str) -> String {
    format!(r"{lead} it's C:\x,y")
}

/// Dial `endpoint` without waiting for the outcome: `bgapi originate` over the API carrier, an
/// async `bridge` from a parked anchor over the dialplan one. Returns the anchor to reap.
async fn dial_without_waiting(
    client: &EslClient,
    endpoint: &Endpoint,
    carrier: DialStringCarrier,
) -> Option<String> {
    let block_parse = block_parse_under_test();
    if carrier == DialStringCarrier::EslApi {
        let cmd = Originate::application(endpoint.clone(), Application::park())
            .display_with(block_parse)
            .to_string();
        client
            .bgapi(&cmd)
            .await
            .unwrap_or_else(|e| panic!("{cmd}: transport error: {e}"))
            .check()
            .unwrap_or_else(|e| panic!("{cmd} rejected: {e}"));
        return None;
    }
    let anchor = parked_anchor(client, "null/anchor").await;
    let dial = BridgeDialString::new(vec![vec![endpoint.clone()]])
        .display_with(block_parse)
        .to_string();
    client
        .execute_with_options(
            "bridge",
            Some(&dial),
            Some(&anchor),
            ExecuteOptions::new().with_async(),
        )
        .await
        .unwrap_or_else(|e| panic!("bridge {dial}: transport error: {e}"))
        .check()
        .unwrap_or_else(|e| panic!("bridge {dial} rejected: {e}"));
    Some(anchor)
}

/// `{origination_uuid=<uuid>}` plus `extra`, so the dialled channel is found by its own uuid.
fn uuid_block(uuid: &str, extra: &[(&str, &str)]) -> Variables {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("origination_uuid", uuid);
    for (key, value) in extra {
        vars.insert(*key, *value);
    }
    vars
}

/// mod_loopback's A leg reports the extension and context it split the destination into.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_loopback_fields_arrive_on_either_carrier() {
    let (client, mut events, _permit) = connect().await;
    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::ChannelCreate])
        .await
        .unwrap();

    for carrier in CARRIERS {
        let uuid = create_uuid(&client).await;
        let extension = hostile_field("ext");
        let context = hostile_field("ctx");
        let endpoint = Endpoint::Loopback(
            LoopbackEndpoint::new(extension.as_str())
                .with_context(context.as_str())
                .with_variables(uuid_block(&uuid, &[])),
        );

        let mut reaper = ChannelReaper::new(&client);
        reaper.track(&uuid);
        if let Some(anchor) = dial_without_waiting(&client, &endpoint, carrier).await {
            reaper.track(anchor);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let created =
            wait_for_own_event(&mut events, &uuid, EslEventType::ChannelCreate, deadline).await;
        reaper
            .reap()
            .await;

        let created =
            created.unwrap_or_else(|| panic!("{carrier:?}: {endpoint} was never created"));
        assert_eq!(
            created.header(EventHeader::CallerDestinationNumber),
            Some(extension.as_str()),
            "{carrier:?}: {endpoint}"
        );
        assert_eq!(
            created.header(EventHeader::CallerContext),
            Some(context.as_str()),
            "{carrier:?}: {endpoint}"
        );
    }
}

/// mod_sofia records the URL it dials; nothing listens on the discard port, so the leg ends by
/// itself.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_sofia_destination_arrives_on_either_carrier() {
    let (client, mut events, _permit) = connect().await;
    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::ChannelCreate])
        .await
        .unwrap();

    for carrier in CARRIERS {
        let uuid = create_uuid(&client).await;
        let destination = format!("sip:{}@127.0.0.1:9", hostile_field("dest"));
        let vars = uuid_block(&uuid, &[("sofia_suppress_url_encoding", "true")]);
        let endpoint = Endpoint::Sofia(
            SofiaEndpoint::new("lab-lo", destination.as_str()).with_variables(vars),
        );

        let mut reaper = ChannelReaper::new(&client);
        reaper.track(&uuid);
        if let Some(anchor) = dial_without_waiting(&client, &endpoint, carrier).await {
            reaper.track(anchor);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let created =
            wait_for_own_event(&mut events, &uuid, EslEventType::ChannelCreate, deadline).await;
        reaper
            .reap()
            .await;

        let created =
            created.unwrap_or_else(|| panic!("{carrier:?}: {endpoint} was never created"));
        assert_eq!(
            created.variable(SofiaVariable::SipDestinationUrl),
            Some(destination.as_str()),
            "{carrier:?}: {endpoint}"
        );
    }
}

/// With URL encoding left on, `protect_dest_uri` encodes the user part of a destination carrying an
/// unsafe character. Upstream keeps a valid `%XX` there; the 1.10.13 fork encodes its `%`.
///
/// Read off a JSON event: a plain one carries the value through `switch_event_serialize`, which
/// upstream leaves ambiguous for a valid `%XX`.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_sofia_destination_encodes_the_user_part_by_tree() {
    let (client, mut events, _permit) = connect().await;
    client
        .subscribe_events(EventFormat::Json, &[EslEventType::ChannelCreate])
        .await
        .unwrap();

    let version = switch_version(&client).await;
    let user = match percent_escape_tree(version) {
        PercentEscape::KeepsValidEscapes => "a%41%20b",
        PercentEscape::EncodesEveryPercent => "a%2541%20b",
    };

    for carrier in CARRIERS {
        let uuid = create_uuid(&client).await;
        let endpoint = Endpoint::Sofia(
            SofiaEndpoint::new("lab-lo", "sip:a%41 b@127.0.0.1:9")
                .with_variables(uuid_block(&uuid, &[])),
        );

        let mut reaper = ChannelReaper::new(&client);
        reaper.track(&uuid);
        if let Some(anchor) = dial_without_waiting(&client, &endpoint, carrier).await {
            reaper.track(anchor);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let created =
            wait_for_own_event(&mut events, &uuid, EslEventType::ChannelCreate, deadline).await;
        reaper
            .reap()
            .await;

        let created =
            created.unwrap_or_else(|| panic!("{carrier:?}: {endpoint} was never created"));
        assert_eq!(
            created.variable(SofiaVariable::SipDestinationUrl),
            Some(format!("sip:{user}@127.0.0.1:9").as_str()),
            "{carrier:?}: {endpoint} on FreeSWITCH {version}"
        );
    }
}

/// The log line `user_outgoing_channel` writes when the directory holds no such user, found by
/// the uuid the name opens with.
async fn wait_for_user_lookup(
    events: &mut EslEventStream,
    marker: &str,
    deadline: Instant,
) -> Option<String> {
    while Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => {
                if let Some(body) = evt
                    .body()
                    .filter(|body| body.contains("Can't find user") && body.contains(marker))
                {
                    return Some(body.to_owned());
                }
            }
            Ok(Some(Err(e))) => panic!("event error waiting for the lookup of {marker}: {e}"),
            Ok(None) => panic!("event stream closed waiting for the lookup of {marker}"),
            Err(_) => break,
        }
    }
    None
}

/// mod_dptools names the user and domain it looked up.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_user_fields_arrive_on_either_carrier() {
    let (client, mut events, _permit) = connect().await;
    client
        .log(LogLevel::Warning)
        .await
        .expect("log transport error")
        .check()
        .expect("log rejected");

    for carrier in CARRIERS {
        let marker = create_uuid(&client).await;
        let name = hostile_field(&marker);
        let endpoint = Endpoint::User(UserEndpoint::new(name.as_str()).with_domain("example.com"));

        let mut reaper = ChannelReaper::new(&client);
        if let Some(anchor) = dial_without_waiting(&client, &endpoint, carrier).await {
            reaper.track(anchor);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let logged = wait_for_user_lookup(&mut events, &marker, deadline).await;
        reaper
            .reap()
            .await;

        let logged =
            logged.unwrap_or_else(|| panic!("{carrier:?}: {endpoint} was never looked up"));
        assert!(
            logged.contains(&format!("[{name}@example.com]")),
            "{carrier:?}: {endpoint} logged {logged:?}"
        );
    }
}

/// Through `bridge`, endpoint text naming a variable is left to the expansion of the executing
/// channel, as a block value is, while a `$$` beside no reference still arrives whole.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_endpoint_text_leaves_a_reference_to_the_bridge_expansion() {
    let (client, mut events, _permit) = connect().await;
    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::ChannelCreate])
        .await
        .unwrap();

    let marker = create_uuid(&client).await;
    let expanded = format!("ext-{marker}");
    let cases = [
        ("${some_var}".to_owned(), expanded.clone()),
        (format!("pa$${marker}"), format!("pa$${marker}")),
    ];
    for (extension, want) in cases {
        let uuid = create_uuid(&client).await;
        let anchor = parked_anchor(&client, &format!("{{some_var={expanded}}}null/anchor")).await;
        let endpoint = Endpoint::Loopback(
            LoopbackEndpoint::new(extension.as_str())
                .with_context("default")
                .with_variables(uuid_block(&uuid, &[])),
        );
        let dial = BridgeDialString::new(vec![vec![endpoint]])
            .display_with(block_parse_under_test())
            .to_string();

        let mut reaper = ChannelReaper::new(&client);
        reaper.track(&anchor);
        reaper.track(&uuid);
        client
            .execute_with_options(
                "bridge",
                Some(&dial),
                Some(&anchor),
                ExecuteOptions::new().with_async(),
            )
            .await
            .unwrap_or_else(|e| panic!("bridge {dial}: transport error: {e}"))
            .check()
            .unwrap_or_else(|e| panic!("bridge {dial} rejected: {e}"));
        let deadline = Instant::now() + Duration::from_secs(10);
        let created =
            wait_for_own_event(&mut events, &uuid, EslEventType::ChannelCreate, deadline).await;
        reaper
            .reap()
            .await;

        let created = created.unwrap_or_else(|| panic!("{dial} was never created"));
        assert_eq!(
            created.header(EventHeader::CallerDestinationNumber),
            Some(want.as_str()),
            "{dial}"
        );
    }
}

// --- Dial-string escaping, per carrier ---

/// Every case through `originate`, whose argument list the switch splits before
/// the block is parsed.
async fn escaping_over_the_api_carrier(separator: Option<char>, scope: VariablesType) {
    let (client, _events, permit) = connect().await;

    for (label, pairs) in ESCAPING_CASES {
        if !carried_in(scope, pairs) {
            continue;
        }
        let vars = escaping_block(&[], pairs, separator, scope);
        let block = vars.display_for(target_under_test(DialStringCarrier::EslApi));
        let resp = client
            .api(&format!("originate {block}null/escaping &park()"))
            .await
            .unwrap_or_else(|e| panic!("{label}: originate transport error: {e}"));
        let uuid = resp
            .api_result()
            .unwrap_or_else(|e| panic!("{label}: originate rejected {block}: {e}"))
            .to_string();

        let mut reaper = ChannelReaper::new(&client);
        reaper.track(&uuid);
        let mut results = Vec::new();
        for (key, want) in *pairs {
            results.push((*key, *want, getvar(&client, &uuid, key).await));
        }
        reaper
            .reap()
            .await;

        for (key, want, got) in results {
            assert_eq!(
                got.as_deref(),
                Some(want),
                "{label}: {key} arrived wrong from {block}"
            );
        }
    }

    drop(permit);
}

/// Every case through a dialplan application, which receives its argument
/// whole and so needs one escaping level less.
async fn escaping_over_the_dialplan_carrier(separator: Option<char>, scope: VariablesType) {
    let (client, _events, permit) = connect().await;

    for (label, pairs) in ESCAPING_CASES {
        if !carried_in(scope, pairs) {
            continue;
        }
        let b_uuid = create_uuid(&client).await;
        let a_uuid = parked_anchor(&client, "null/anchor").await;

        // Pre-assigning the B leg's uuid is what makes the far side findable.
        let vars = escaping_block(&[("origination_uuid", &b_uuid)], pairs, separator, scope);

        let mut reaper = ChannelReaper::new(&client);
        reaper.track(&a_uuid);
        reaper.track(&b_uuid);

        let dial_string = format!(
            "{}null/escaping",
            vars.display_for(target_under_test(DialStringCarrier::Dialplan))
        );
        client
            .execute_with_options(
                "bridge",
                Some(&dial_string),
                Some(&a_uuid),
                ExecuteOptions::new().with_async(),
            )
            .await
            .unwrap_or_else(|e| panic!("{label}: execute bridge transport error: {e}"))
            .check()
            .unwrap_or_else(|e| panic!("{label}: execute bridge rejected: {e}"));

        // The bridge is async: the far leg carries the block once the A leg
        // names it as its bridge partner.
        let deadline = Instant::now() + Duration::from_secs(10);
        let outcome = wait_for_bridge(&client, &a_uuid, deadline).await;
        let mut results = Vec::new();
        if matches!(outcome, Some(BridgeOutcome::Bridged(_))) {
            for (key, want) in *pairs {
                results.push((*key, *want, getvar(&client, &b_uuid, key).await));
            }
        }
        reaper
            .reap()
            .await;

        match outcome {
            Some(BridgeOutcome::Bridged(_)) => {}
            Some(BridgeOutcome::Failed(status)) => {
                panic!("{label}: the anchor's bridge failed: DIALSTATUS={status}")
            }
            None => panic!("{label}: the anchor never bridged"),
        }
        for (key, want, got) in results {
            assert_eq!(
                got.as_deref(),
                Some(want),
                "{label}: {key} arrived wrong from {dial_string}"
            );
        }
    }

    drop(permit);
}

/// The `originate` API splits its argument list before the block is parsed, so
/// this carrier needs one escaping level more than a dialplan application.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_escaping_survives_the_api_carrier() {
    escaping_over_the_api_carrier(None, VariablesType::Default).await;
}

/// A dialplan application receives its argument whole, so the same values need
/// one level less. Rendering with the API default here would corrupt a quoted
/// value, which is the whole reason the carrier is nameable.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_escaping_survives_the_dialplan_carrier() {
    escaping_over_the_dialplan_carrier(None, VariablesType::Default).await;
}

/// A separator the values do not contain is the only way a `${...}`-expanded
/// value carries a comma, since substitution happens before the block is parsed.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_chosen_separator_carries_commas_unescaped() {
    let (client, _events, permit) = connect().await;

    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("codecs", "PCMA,PCMU,G729");
    vars.insert("tenant", "acme");
    let vars = vars
        .with_separator(':')
        .expect("':' does not appear in any value here");

    let uuid = client
        .api(&format!("originate {vars}null/escaping &park()"))
        .await
        .expect("originate transport error")
        .api_result()
        .unwrap_or_else(|e| panic!("originate rejected {vars}: {e}"))
        .to_string();

    let mut reaper = ChannelReaper::new(&client);
    reaper.track(&uuid);
    let codecs = getvar(&client, &uuid, "codecs").await;
    let tenant = getvar(&client, &uuid, "tenant").await;
    reaper
        .reap()
        .await;

    assert_eq!(codecs.as_deref(), Some("PCMA,PCMU,G729"));
    assert_eq!(tenant.as_deref(), Some("acme"));

    drop(permit);
}

/// A `^^` block reaches the switch's tokenizer the same number of times as the
/// comma form, so every value that needed escaping there still needs it here.
/// Only the comma stops being special.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_separated_escaping_survives_the_api_carrier() {
    escaping_over_the_api_carrier(Some(ESCAPING_SEPARATOR), VariablesType::Default).await;
}

/// The dialplan half of the pair above: one escaping level less, same block
/// shape, so a rule that only holds on one carrier shows up as a difference
/// between these two tests rather than as a passing suite.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_separated_escaping_survives_the_dialplan_carrier() {
    escaping_over_the_dialplan_carrier(Some(ESCAPING_SEPARATOR), VariablesType::Default).await;
}

/// A `[]` block is not parsed where `{}` is: it rides through the peer split,
/// preceded by a scan that toggles its quote state on every `'` whatever
/// precedes it, so the same values at the same depth measure differently.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_escaping_survives_the_api_carrier_in_channel_scope() {
    escaping_over_the_api_carrier(None, VariablesType::Channel).await;
}

/// The dialplan half of the channel-scope pair.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_escaping_survives_the_dialplan_carrier_in_channel_scope() {
    escaping_over_the_dialplan_carrier(None, VariablesType::Channel).await;
}

/// The refusal of a quote in channel scope rests on this: the scan ahead of the
/// peer split pairs the quote in `p1` with the one in `p2`, escaped or not, and
/// the separator between them stops being one. The block is written by hand
/// because the typed API will not build it.
///
/// Failing here is good news — it would mean the switch started honouring the
/// escape in that scan, and the refusal should then be revisited rather than
/// kept.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_channel_scope_pairs_quotes_across_values() {
    let (client, _events, permit) = connect().await;

    let uuid = client
        .api(r"originate [p1=it\\\'s,p2=don\\\'t,p3=SENTINEL]null/escaping &park()")
        .await
        .expect("originate transport error")
        .api_result()
        .expect("originate rejected")
        .to_string();

    let mut reaper = ChannelReaper::new(&client);
    reaper.track(&uuid);
    let p1 = getvar(&client, &uuid, "p1").await;
    let p2 = getvar(&client, &uuid, "p2").await;
    let p3 = getvar(&client, &uuid, "p3").await;
    reaper
        .reap()
        .await;

    assert_eq!(
        p1.as_deref(),
        Some("its,p2=dont"),
        "the switch now splits a channel-scope block on a separator between \
         two quoted values; Variables refuses such a value, so that refusal is \
         no longer justified"
    );
    assert_eq!(p2, None, "p2 was set on its own, so p1 did not absorb it");
    assert_eq!(
        p3.as_deref(),
        Some("SENTINEL"),
        "the block itself must be well-formed, or the assertions above prove nothing"
    );

    drop(permit);
}

/// `execute_on_originate` runs its application on the new channel before that
/// channel's session thread starts, so a variable set there is on the channel
/// before anything the leg does — the INVITE, for a SIP leg. The value carries
/// a space, so this also proves the hook survives the block's quoting.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_execute_on_originate_runs_before_the_leg_does() {
    let (client, _events, permit) = connect().await;

    let hook = ExecuteOn::new("set", Some("probe=hooked")).expect("a plain set is representable");
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert(
        ChannelVariable::ExecuteOnOriginate.as_str(),
        hook.to_string(),
    );

    let uuid = client
        .api(&format!("originate {vars}null/hook &park()"))
        .await
        .expect("originate transport error")
        .api_result()
        .expect("originate rejected")
        .to_string();

    let mut reaper = ChannelReaper::new(&client);
    reaper.track(&uuid);
    let probe = getvar(&client, &uuid, "probe").await;
    reaper
        .reap()
        .await;

    assert_eq!(
        probe.as_deref(),
        Some("hooked"),
        "the hook did not run from {vars}"
    );

    drop(permit);
}

/// The refusal in `Variables` rests on this: an empty value never reaches the
/// channel. The block is written by hand because the typed API will not build
/// one, and the point is to check the switch rather than the crate.
///
/// Failing here is good news — it would mean FreeSWITCH started honouring an
/// empty pair, and the refusal should then be revisited rather than kept.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_empty_value_still_never_reaches_the_channel() {
    let (client, _events, permit) = connect().await;

    let uuid = client
        .api("originate {p1=,p2=SENTINEL}null/escaping &park()")
        .await
        .expect("originate transport error")
        .api_result()
        .expect("originate rejected")
        .to_string();

    let mut reaper = ChannelReaper::new(&client);
    reaper.track(&uuid);
    let empty = getvar(&client, &uuid, "p1").await;
    let sentinel = getvar(&client, &uuid, "p2").await;
    reaper
        .reap()
        .await;

    assert_eq!(
        empty, None,
        "the switch now keeps an empty pair; Variables refuses to build one, so \
         that refusal is no longer justified"
    );
    assert_eq!(
        sentinel.as_deref(),
        Some("SENTINEL"),
        "the block itself must be well-formed, or the assertion above proves nothing"
    );

    drop(permit);
}

/// The `variable_*` headers of `uuid`'s dump whose name matches `name` ignoring case, as written.
async fn dumped_spellings(client: &EslClient, uuid: &str, name: &str) -> Vec<(String, String)> {
    let body = client
        .api(&format!("uuid_dump {uuid}"))
        .await
        .expect("uuid_dump transport error")
        .body()
        .expect("uuid_dump must return a body")
        .to_owned();
    let dump = parse_channel_dump(&body).unwrap_or_else(|e| panic!("uuid_dump {uuid}: {e}"));
    let wanted = format!("variable_{name}");
    dump.headers()
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case(&wanted))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// The refusals of a key carrying `[` and of two keys differing only in case rest on this: the
/// block's event and the channel's variables both replace a name by `strcasecmp`, and
/// `switch_event_base_add_header` reads `[` as an array index. Written by hand because `Variables`
/// will not build either. A `_body` key is not among them: it arrives as a variable.
///
/// Failing here is good news — the refusal it names should then be revisited rather than kept.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_keys_the_switch_collapses_or_reads_as_an_index() {
    type Spellings<'a> = &'a [(&'a str, &'a str)];
    type Dialled<'a> = (&'a str, &'a [(&'a str, Spellings<'a>)]);

    let (client, _events, permit) = connect().await;

    let cases: &[Dialled] = &[
        (
            "{k=lower,K=upper,a[1]=arr,b[x=idx,_body=text,sentinel=S}null/escaping",
            &[
                ("k", &[("variable_K", "upper")]),
                ("a", &[("variable_a", "arr")]),
                ("a[1]", &[]),
                ("b", &[("variable_b", "idx")]),
                ("_body", &[("variable__body", "text")]),
                ("sentinel", &[("variable_sentinel", "S")]),
            ],
        ),
        ("{k=g}[K=l]null/escaping", &[("k", &[("variable_k", "g")])]),
        (
            "{local_var_clobber=true,k=g}[K=l]null/escaping",
            &[("k", &[("variable_K", "l")])],
        ),
    ];

    for (dial, want) in cases {
        let uuid = client
            .api(&format!("originate {dial} &park()"))
            .await
            .expect("originate transport error")
            .api_result()
            .unwrap_or_else(|e| panic!("originate {dial} rejected: {e}"))
            .to_string();
        let mut reaper = ChannelReaper::new(&client);
        reaper.track(&uuid);
        let mut got = Vec::new();
        for (name, _) in *want {
            got.push(dumped_spellings(&client, &uuid, name).await);
        }
        reaper
            .reap()
            .await;

        for ((name, want), got) in want
            .iter()
            .zip(got)
        {
            let want: Vec<(String, String)> = want
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            assert_eq!(got, want, "{name} from {dial}");
        }
    }

    drop(permit);
}

// --- Channel dump parsing: the connect-time state rebuild loop ---

/// Headers that identify the channel and so must read the same from a dump as
/// from the event stream; anything time-varying (state, timestamps) will not.
const DUMP_IDENTITY_HEADERS: &[&str] = &[
    "Unique-ID",
    "Channel-Name",
    "Core-UUID",
    "FreeSWITCH-Hostname",
    "Call-Direction",
    "Caller-Destination-Number",
];

#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_channel_dump_rebuild_loop() {
    let (client, mut events, _permit) = connect().await;

    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::ChannelCreate])
        .await
        .unwrap();

    // Two channels, so the loop below is a loop and not a single dump.
    // Originated over `api` rather than `bgapi`: waiting for a BACKGROUND_JOB
    // drains the event stream, and these channels' CHANNEL_CREATE is what the
    // dump is compared against.
    let mut reaper = ChannelReaper::new(&client);
    let mut uuids = Vec::new();
    for _ in 0..2 {
        let cmd = Originate::application(test_9199(), Application::simple("park"));
        let uuid = client
            .api(&cmd.to_string())
            .await
            .expect("originate: transport error")
            .api_result()
            .expect("originate rejected")
            .to_string();
        reaper.track(&uuid);
        uuids.push(uuid);
    }

    // Every CHANNEL_CREATE is correlated to one of our own UUIDs; the switch is
    // shared, so anything else on the stream belongs to another test.
    let mut created: Vec<(String, freeswitch_esl_tokio::EslEvent)> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while created.len() < uuids.len() && Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => {
                if evt.event_type() != Some(EslEventType::ChannelCreate) {
                    continue;
                }
                if let Some(uuid) = evt
                    .unique_id()
                    .filter(|u| {
                        uuids
                            .iter()
                            .any(|ours| ours == u)
                    })
                    .map(|u| u.to_string())
                {
                    created.push((uuid, evt));
                }
            }
            Ok(Some(Err(e))) => panic!("event error: {}", e),
            Ok(None) => panic!("event stream closed"),
            Err(_) => break,
        }
    }

    // Collect everything before reaping; the assertions run after.
    let mut dumps = Vec::new();
    for uuid in &uuids {
        let body = client
            .api(&format!("uuid_dump {}", uuid))
            .await
            .expect("uuid_dump: transport error")
            .body()
            .expect("uuid_dump must return a body")
            .to_string();
        let parsed = parse_channel_dump(&body);
        dumps.push((uuid.clone(), parsed));
    }

    reaper
        .reap()
        .await;

    assert_eq!(
        created.len(),
        uuids.len(),
        "did not observe CHANNEL_CREATE for every originated channel"
    );
    for (uuid, parsed) in dumps {
        let dump = parsed.unwrap_or_else(|e| panic!("uuid_dump {} did not parse: {}", uuid, e));

        assert_eq!(
            dump.event_type(),
            Some(EslEventType::ChannelData),
            "a dump is a serialized CHANNEL_DATA event"
        );
        assert_eq!(dump.raw_body(), None, "a dump arrives already decoded");

        let event = &created
            .iter()
            .find(|(u, _)| *u == uuid)
            .expect("every uuid has a CHANNEL_CREATE")
            .1;
        for name in DUMP_IDENTITY_HEADERS {
            assert_eq!(
                dump.header_str(name),
                event.header_str(name),
                "{} disagrees between the dump and the event for {}",
                name,
                uuid
            );
        }

        // Channel variables reach through the one normalised convention, which
        // is what an inline splitter over the same body loses.
        assert_eq!(
            dump.variable_str("uuid"),
            Some(uuid.as_str()),
            "the dump's channel variables must resolve through variable_str"
        );

        // An unset variable reads as absent. A channel this test can build
        // carries no empty value -- the switch deletes a variable set to one --
        // so the skip itself is exercised by the fixture in
        // src/command/response.rs; here the sentinel must simply never survive.
        assert!(
            dump.headers()
                .values()
                .all(|v| v != "_undef_"),
            "an empty value must read as absent, not as the sentinel, for {}",
            uuid
        );
    }
}

/// The race the rebuild loop hits in production: the channel hung up before
/// its dump.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_channel_dump_of_reaped_uuid_is_skippable() {
    let (client, mut events, _permit) = connect().await;

    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::BackgroundJob])
        .await
        .unwrap();

    let cmd = Originate::application(test_9199(), Application::simple("park"));
    let uuid = bgapi_originate_ok(&client, &mut events, &cmd).await;
    kill_channel(&client, &uuid).await;

    let deadline = Instant::now() + Duration::from_secs(10);
    while channel_exists(&client, &uuid).await {
        assert!(Instant::now() < deadline, "{} never went away", uuid);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let body = client
        .api(&format!("uuid_dump {}", uuid))
        .await
        .expect("uuid_dump: transport error")
        .body()
        .expect("uuid_dump must return a body")
        .to_string();

    let err =
        parse_channel_dump(&body).expect_err("a dump of a dead channel must not parse as an event");
    let failure = err
        .command_failure()
        .expect("the loop has to recognise this as a command failure");
    let payload = match failure {
        CommandFailure::Err(payload) => payload,
        other => panic!("expected an -ERR reply, got {:?}", other),
    };
    assert!(
        payload.contains("No such channel"),
        "unexpected -ERR payload: {:?}",
        payload
    );
    assert!(err.is_recoverable(), "the loop skips this and carries on");
}

/// `switch_event_serialize` writes this for an empty value. `parse_channel_dump`
/// reads it back as absent while the live decoder keeps it, so a create-side
/// sentinel is not a header the dump lost.
const UNDEF_VALUE: &str = "_undef_";

/// Headers the switch moves on between a channel's CHANNEL_CREATE and a dump
/// taken later: `originate` returns on answer, so the dump is past the state
/// the create reported.
const VOLATILE_HEADERS: &[&str] = &[
    "Channel-State",
    "Channel-State-Number",
    "Channel-Call-State",
    "Answer-State",
    // Reads back as the session UUID until something sets `call_uuid`.
    "Channel-Call-UUID",
    // Flips once the channel reaches the dialplan.
    "Channel-HIT-Dialplan",
    // An originate stamps a placeholder callee id and clears it once the call
    // is up; the dialplan rewrites the caller id it routes on.
    "Caller-Callee-ID-Name",
    "Caller-Callee-ID-Number",
    "Caller-Caller-ID-Name",
    "Caller-Caller-ID-Number",
];

/// [`VOLATILE_HEADERS`] plus the timetable, which fills in as the call
/// progresses. The two creation stamps are fixed by the time the channel is
/// announced, so they stay in the comparison.
fn volatile_headers() -> HashSet<String> {
    let mut set: HashSet<String> = VOLATILE_HEADERS
        .iter()
        .map(|h| h.to_string())
        .collect();
    let fixed = [
        TimetableField::ProfileCreated.as_str(),
        TimetableField::Created.as_str(),
    ];
    for prefix in [TimetablePrefix::Caller, TimetablePrefix::OtherLeg] {
        for suffix in ChannelTimetable::SUFFIXES {
            if fixed.contains(suffix) {
                continue;
            }
            set.insert(format!("{}-{}", prefix.as_str(), suffix));
        }
    }
    set
}

/// Every row's `uuid` -- the one field a bootstrap reads out of the listing.
/// A row without it is a broken contract, not a row to skip.
fn show_channel_uuids(body: &str) -> Vec<String> {
    let json: serde_json::Value =
        serde_json::from_str(body).expect("show channels as json must answer JSON");
    let Some(rows) = json
        .get("rows")
        .and_then(|v| v.as_array())
    else {
        // An empty result carries a row count and no rows key at all.
        return Vec::new();
    };
    rows.iter()
        .map(|row| {
            row.get("uuid")
                .and_then(|v| v.as_str())
                .expect("every listed channel must carry a uuid")
                .to_string()
        })
        .collect()
}

/// The bootstrap the `channel_tracker` example runs: the listing hands over a
/// UUID, the dump hands over the channel. What that rebuilds has to be the
/// CHANNEL_CREATE the switch already sent, or the example is inventing state
/// no consumer could have got from the wire.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_show_bootstrap_rebuilds_channel_create() {
    let (client, mut events, _permit) = connect().await;

    client
        .subscribe_events(EventFormat::Plain, &[EslEventType::ChannelCreate])
        .await
        .unwrap();

    // `api`, not `bgapi_originate_ok`: waiting on a BACKGROUND_JOB drains the
    // stream this test reads its CHANNEL_CREATE off.
    let mut reaper = ChannelReaper::new(&client);
    let cmd = Originate::application(test_9199(), Application::simple("park"));
    let uuid = client
        .api(&cmd.to_string())
        .await
        .expect("originate: transport error")
        .api_result()
        .expect("originate rejected")
        .to_string();
    reaper.track(&uuid);

    let deadline = Instant::now() + Duration::from_secs(10);
    let created =
        wait_for_own_event(&mut events, &uuid, EslEventType::ChannelCreate, deadline).await;

    // The core writes its channel row through an async queue, so the listing
    // trails the event by however long that queue takes to drain.
    let mut listed = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !listed && Instant::now() < deadline {
        let body = client
            .api("show channels as json")
            .await
            .expect("show channels: transport error")
            .api_result()
            .expect("show channels rejected")
            .to_string();
        listed = show_channel_uuids(&body).contains(&uuid);
        if !listed {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    let dump = client
        .api(&format!("uuid_dump {}", uuid))
        .await
        .expect("uuid_dump: transport error")
        .body()
        .expect("uuid_dump must return a body")
        .to_string();

    reaper
        .reap()
        .await;

    let created = created.unwrap_or_else(|| panic!("no CHANNEL_CREATE for {}", uuid));
    assert!(listed, "{} never reached the channel listing", uuid);

    let mut rebuilt = parse_channel_dump(&dump)
        .unwrap_or_else(|e| panic!("uuid_dump {} did not parse: {}", uuid, e));
    rebuilt.set_header(
        EventHeader::EventName.as_str(),
        EslEventType::ChannelCreate.as_str(),
    );
    assert_eq!(
        rebuilt.event_type(),
        Some(EslEventType::ChannelCreate),
        "renaming the dump is the whole translation a bootstrap performs"
    );

    let volatile = volatile_headers();
    // A comparison that skipped its way past the channel's identity would pass
    // on an empty dump.
    for name in DUMP_IDENTITY_HEADERS {
        assert!(
            !volatile.contains(*name)
                && created
                    .header_str(name)
                    .is_some(),
            "{} has to be inside the comparison",
            name
        );
    }

    let mut absent = Vec::new();
    let mut differs = Vec::new();
    for (key, value) in created.headers() {
        if key.starts_with("Event-") || volatile.contains(key.as_str()) || value == UNDEF_VALUE {
            continue;
        }
        match rebuilt.header_str(key) {
            None => absent.push(format!("{key}={value:?}")),
            Some(dumped) if dumped != value => {
                differs.push(format!("{key} create={value:?} dump={dumped:?}"))
            }
            Some(_) => {}
        }
    }
    assert!(
        absent.is_empty(),
        "the rebuilt channel is missing {:?}",
        absent
    );
    assert!(
        differs.is_empty(),
        "the rebuilt channel disagrees: {:?}",
        differs
    );
}
