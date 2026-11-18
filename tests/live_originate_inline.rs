//! Integration tests against a live FreeSWITCH instance: an inline action list
//! whose arguments contain the separator.
//!
//! `inline_dialplan_hunt` splits the list on `,` — or on whatever an `m:<d>:`
//! prefix names — and a bare separator inside an argument is read as an action
//! boundary. The switch reports nothing when that happens: it builds the
//! applications the split produced and runs them. Only a live switch can prove
//! the escaping this crate emits is undone the way `cleanup_separated_string`
//! is expected to undo it, which is why these are not unit tests.
//!
//! These tests require a live FreeSWITCH ESL; see docs/live-test-switch.md.
//! Run with: cargo test --test 'live_*' -- --ignored

mod live_common;

use freeswitch_esl_tokio::commands::originate::{Variables, VariablesType};
use freeswitch_esl_tokio::commands::LoopbackEndpoint;
use freeswitch_esl_tokio::{
    Application, Endpoint, EslEventType, EventFormat, EventHeader, HeaderLookup, Originate,
};
use live_common::{
    bgapi_originate, connect, create_uuid, getvar, originate_job_reply, ChannelReaper,
};
use std::time::Duration;
use tokio::time::Instant;

/// Long enough for the list to reach `park`, short enough that a test which
/// panics before reaping does not strand the channel for the whole run.
const PARK_TIMEOUT_SECS: &str = "8";

/// Holds the leg while the test reads back what the application received.
fn parked_loopback(uuid: &str) -> Endpoint {
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("origination_uuid", uuid)
        .unwrap();
    vars.insert("park_timeout", PARK_TIMEOUT_SECS)
        .unwrap();
    Endpoint::Loopback(
        LoopbackEndpoint::new("9199")
            .unwrap()
            .with_context("test")
            .unwrap()
            .with_variables(vars),
    )
}

/// The value the application actually received, or `None` if it never ran.
///
/// Reads the variable rather than the event's `Application-Data`, because the
/// data header shows what the dialplan parsed while the variable shows what
/// reached the application — and it is the second one that a split in the
/// wrong place corrupts.
async fn run_and_read_back(
    build: impl FnOnce(Endpoint) -> Originate,
    variable: &str,
) -> Option<String> {
    let (client, mut events, _permit) = connect().await;

    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::BackgroundJob,
                EslEventType::ChannelExecuteComplete,
            ],
        )
        .await
        .expect("subscribe failed");

    // The inline list runs on the channel's own thread, so `set` can complete
    // before the originate's BACKGROUND_JOB: the uuid has to be known first.
    let uuid = create_uuid(&client).await;
    let mut reaper = ChannelReaper::new(&client);
    reaper.track(&uuid);
    let job_uuid = bgapi_originate(&client, &build(parked_loopback(&uuid))).await;

    // `set` completing is the point the variable exists; reading before that
    // races the application rather than the wire.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut reply = None;
    let mut ran = false;
    while !(ran && reply.is_some()) {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => {
                if let Some(result) = originate_job_reply(&evt, &job_uuid) {
                    let failed = result.is_err();
                    if let Ok(originated) = &result {
                        reaper.track(originated);
                    }
                    reply = Some(result);
                    if failed {
                        break;
                    }
                }
                ran |= evt.event_type() == Some(EslEventType::ChannelExecuteComplete)
                    && evt.unique_id() == Some(uuid.as_str())
                    && evt.header(EventHeader::Application) == Some("set");
            }
            Ok(Some(Err(e))) => panic!("event error: {e}"),
            Ok(None) => panic!("event stream closed"),
            Err(_) => break,
        }
    }

    let value = if ran {
        getvar(&client, &uuid, variable).await
    } else {
        None
    };

    reaper
        .reap()
        .await;
    let originated = reply
        .unwrap_or_else(|| panic!("no BACKGROUND_JOB for {job_uuid}"))
        .unwrap_or_else(|e| panic!("originate of {uuid} failed: {e}"));
    assert_eq!(originated, uuid, "origination_uuid was not honoured");
    assert!(ran, "the set application never ran on {uuid}");
    value
}

/// A comma in an argument is the case that shipped broken: the list would have
/// become three applications, two of them fragments of a tone spec.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_inline_argument_keeps_its_commas() {
    let spec = "tone_stream://%(500,0,800)";
    let cmd = |endpoint: Endpoint| {
        Originate::inline(
            endpoint,
            [
                Application::new("set", Some(format!("probe_comma={spec}"))),
                Application::park(),
            ],
        )
        .expect("inline builder rejected a valid list")
    };

    assert_eq!(
        run_and_read_back(cmd, "probe_comma").await,
        Some(spec.to_string())
    );
}

/// The same for a separator the caller named, which the arguments also use.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_inline_argument_keeps_a_named_separator() {
    let value = "a|b|c";
    let cmd = |endpoint: Endpoint| {
        Originate::inline_with_delimiter(
            endpoint,
            [
                Application::new("set", Some(format!("probe_pipe={value}"))),
                Application::park(),
            ],
            '|',
        )
        .expect("inline builder rejected a valid separator")
    };

    assert_eq!(
        run_and_read_back(cmd, "probe_pipe").await,
        Some(value.to_string())
    );
}

/// Every separator at once, since none of them can make a list unrenderable.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_inline_argument_keeps_a_hostile_value() {
    let value = ",|;~^!";
    let cmd = |endpoint: Endpoint| {
        Originate::inline(
            endpoint,
            [
                Application::new("set", Some(format!("probe_hostile={value}"))),
                Application::park(),
            ],
        )
        .expect("inline builder rejected a valid list")
    };

    assert_eq!(
        run_and_read_back(cmd, "probe_hostile").await,
        Some(value.to_string())
    );
}

/// The argument carries a space so the action list is wrapped in quotes, which
/// is the case where the escaping is load-bearing rather than incidental.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_inline_argument_keeps_a_single_quote() {
    let value = "it's one value";
    let cmd = |endpoint: Endpoint| {
        Originate::inline(
            endpoint,
            [
                Application::new("set", Some(format!("probe_quote={value}"))),
                Application::park(),
            ],
        )
        .expect("one quote is deliverable and must not be refused")
    };

    assert_eq!(
        run_and_read_back(cmd, "probe_quote").await,
        Some(value.to_string())
    );
}

/// The hunt's split trims a space at an action's end and reads escapes in it, so the
/// argument is escaped for that split. No backslash: `set` expands its value itself.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_inline_argument_keeps_an_edge_space_and_a_tab() {
    let value = "a\tb it's ";
    let cmd = |endpoint: Endpoint| {
        Originate::inline(
            endpoint,
            [
                Application::new("set", Some(format!("probe_edge={value}"))),
                Application::park(),
            ],
        )
        .expect("inline builder rejected a valid list")
    };

    assert_eq!(
        run_and_read_back(cmd, "probe_edge").await,
        Some(value.to_string())
    );
}

/// Two quotes in one value pair with each other at either split unless each is escaped
/// for both, and a `cond` over quoted operands is the expression that pairing breaks.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_inline_argument_keeps_two_quotes() {
    for (value, want) in [
        ("x'a'y z", "x'a'y z"),
        ("${cond('${probe_unset}' == '' ? empty : full)}", "empty"),
    ] {
        let cmd = |endpoint: Endpoint| {
            Originate::inline(
                endpoint,
                [
                    Application::new("set", Some(format!("probe_quotes={value}"))),
                    Application::park(),
                ],
            )
            .expect("inline builder rejected a valid list")
        };

        assert_eq!(
            run_and_read_back(cmd, "probe_quotes").await,
            Some(want.to_string()),
            "{value}"
        );
    }
}
