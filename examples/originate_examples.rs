//! Originate builder reference -- all endpoint types and targeting modes.
//!
//! Part 1 (no connection required): builds each endpoint type and prints the
//! resulting wire string. Covers variable scoping ({} default, [] channel,
//! <> enterprise), known and named dialplans, all OriginateTarget forms,
//! and JSON deserialization.
//!
//! Part 2: connects to FreeSWITCH, places a test call via bgapi, and reports
//! the BACKGROUND_JOB result and channel lifecycle events.
//!
//! Usage: FREESWITCH_VERSION=1.10.12 RUST_LOG=info cargo run --example originate_examples
//!   Configure via ESL_HOST, ESL_PORT, ESL_PASSWORD env vars (defaults from constants).
//!   FREESWITCH_VERSION picks the parser revision both parts render for, and is
//!   required; ESL_BLOCK_PARSE names one outright, for a dev build or unvouched version.

use freeswitch_esl_tokio::commands::endpoint::GroupCallOrder;
use freeswitch_esl_tokio::commands::{
    AudioEndpoint, BlockParse, BridgeDialString, ErrorEndpoint, GroupCall, LoopbackEndpoint,
    SofiaContact, SofiaEndpoint, SofiaGateway, UserEndpoint,
};
use std::time::Duration;

#[path = "common/block_parse.rs"]
mod block_parse;
mod common;

use freeswitch_esl_tokio::{
    Application, BgJobTracker, DialplanType, Endpoint, EslEventType, EventFormat, HeaderLookup,
    Originate, SipPassthroughHeader, Variables, VariablesType,
};
use tracing::{error, info};

/// The originate carries its own 10s timeout, so anything past this means no
/// result is coming.
const CALL_DEADLINE: Duration = Duration::from_secs(30);

fn print_endpoint_examples(block_parse: BlockParse) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Endpoint wire formats ===");

    // -----------------------------------------------------------------------
    // Direct SIP profile routing
    // -----------------------------------------------------------------------

    println!("\n-- SofiaEndpoint: sofia/profile/destination --");

    let cmd = Originate::extension(
        Endpoint::Sofia(SofiaEndpoint::new("internal", "1000@10.0.0.1")?),
        "1000",
    )?
    .with_cid_name("Alice")?
    .with_cid_num("5551234")?
    .with_timeout(Duration::from_secs(30));
    // originate sofia/internal/1000@10.0.0.1 1000 undef undef Alice 5551234 30
    println!("{}", cmd.render(block_parse));

    // -----------------------------------------------------------------------
    // SIP gateway routing
    // -----------------------------------------------------------------------

    println!("\n-- SofiaGateway: sofia/gateway/name/destination --");

    let cmd = Originate::application(
        // .with_profile("external") qualifies as profile::gateway
        Endpoint::SofiaGateway(SofiaGateway::new("my_provider", "18005551234")?),
        Application::simple("park"),
    )?
    .with_timeout(Duration::from_secs(60));
    // originate sofia/gateway/my_provider/18005551234 &park() undef undef undef undef 60
    println!("{}", cmd.render(block_parse));

    // With a profile qualifier: sofia/gateway/external::my_provider/destination
    let cmd = Originate::application(
        Endpoint::SofiaGateway(
            SofiaGateway::new("my_provider", "18005551234")?.with_profile("external")?,
        ),
        Application::simple("park"),
    )?;
    // originate sofia/gateway/external::my_provider/18005551234 &park()
    println!("{}", cmd.render(block_parse));

    // -----------------------------------------------------------------------
    // User endpoint -- FreeSWITCH resolves the contact via the directory
    // -----------------------------------------------------------------------

    println!("\n-- UserEndpoint: user/name@domain --");

    let cmd = Originate::extension(
        Endpoint::User(UserEndpoint::new("1000")?.with_domain("pbx.example.com")?),
        "5000",
    )?;
    // originate user/1000@pbx.example.com 5000
    println!("{}", cmd.render(block_parse));

    // -----------------------------------------------------------------------
    // sofia_contact -- runtime expression a dialplan application expands; `originate`
    // over the API never expands it, so it goes into a bridge dial string
    // -----------------------------------------------------------------------

    println!("\n-- SofiaContact: ${{sofia_contact([profile/]user@domain)}} --");

    let cmd = BridgeDialString::new(vec![vec![
        // "*" searches all profiles; use a profile name to limit the lookup
        Endpoint::SofiaContact(SofiaContact::new("bob", "pbx.example.com")?.with_profile("*")?),
    ]])?;
    // ${sofia_contact(*/bob@pbx.example.com)}
    println!("{}", cmd.render(block_parse));

    // -----------------------------------------------------------------------
    // group_call -- runtime expression resolving a directory group, expanded by a
    // dialplan application or the `expand` API
    // -----------------------------------------------------------------------

    println!("\n-- GroupCall: ${{group_call(group@domain[+order])}} --");

    let cmd = BridgeDialString::new(vec![vec![Endpoint::GroupCall(
        // A=all members simultaneously, F=first registered, E=enterprise
        GroupCall::new("support", "pbx.example.com")?.with_order(GroupCallOrder::All),
    )]])?;
    // ${group_call(support@pbx.example.com+A)}
    println!("{}", cmd.render(block_parse));

    // -----------------------------------------------------------------------
    // Loopback -- routes through the dialplan; useful for testing
    // -----------------------------------------------------------------------

    println!("\n-- LoopbackEndpoint: loopback/extension/context --");

    // 9196 = delay_echo in the default FreeSWITCH configuration
    let cmd = Originate::application(
        Endpoint::Loopback(LoopbackEndpoint::new("9196")?.with_context("default")?),
        Application::simple("park"),
    )?;
    // originate loopback/9196/default &park()
    println!("{}", cmd.render(block_parse));

    // -----------------------------------------------------------------------
    // error/ -- immediately fails with a specified hangup cause
    // -----------------------------------------------------------------------

    println!("\n-- ErrorEndpoint: error/cause --");

    let cmd = Originate::application(
        Endpoint::Error(ErrorEndpoint::new(
            freeswitch_esl_tokio::HangupCause::UserBusy,
        )),
        Application::simple("park"),
    )?;
    // originate error/USER_BUSY &park()
    println!("{}", cmd.render(block_parse));

    // -----------------------------------------------------------------------
    // Audio device endpoints (portaudio / pulseaudio / alsa)
    // -----------------------------------------------------------------------

    println!("\n-- AudioEndpoint: portaudio[/destination] --");

    let cmd = Originate::application(
        Endpoint::PortAudio(AudioEndpoint::new().with_destination("auto_answer")?),
        Application::simple("park"),
    )?;
    // originate portaudio/auto_answer &park()
    println!("{}", cmd.render(block_parse));

    // -----------------------------------------------------------------------
    // Variable scoping
    // -----------------------------------------------------------------------

    println!("\n=== Variable scoping ===");

    // Default scope {}: applies to all legs of this originate
    println!("\n-- Default scope {{}} --");

    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("hangup_after_bridge", "true")?;
    vars.insert("continue_on_fail", "true")?;
    let cmd = Originate::application(
        Endpoint::SofiaGateway(SofiaGateway::new("carrier", "15551234567")?.with_variables(vars)),
        Application::simple("bridge"),
    )?;
    // originate {hangup_after_bridge=true,continue_on_fail=true}sofia/gateway/carrier/15551234567 &bridge()
    println!("{}", cmd.render(block_parse));

    // Channel scope []: applies only to the immediately following endpoint
    println!("\n-- Channel scope [] --");

    let mut vars = Variables::new(VariablesType::Channel);
    vars.insert("originate_timeout", "20")?;
    // Typed SIP passthrough header -- produces "sip_h_X-Tenant" on the wire
    vars.insert(SipPassthroughHeader::request_raw("X-Tenant")?, "acme")?;
    let cmd = Originate::application(
        Endpoint::Sofia(
            SofiaEndpoint::new("external", "sip:alice@carrier.example.com")?.with_variables(vars),
        ),
        Application::simple("park"),
    )?;
    // originate [originate_timeout=20,sip_h_X-Tenant=acme]sofia/external/sip:alice@carrier.example.com &park()
    println!("{}", cmd.render(block_parse));

    // Values containing commas are auto-escaped as \, -- required by the FS variable parser
    println!("\n-- Comma-containing values (auto-escaped) --");

    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("absolute_codec_string", "PCMU,PCMA,G722")?;
    let cmd = Originate::application(
        Endpoint::SofiaGateway(SofiaGateway::new("gw1", "1234")?.with_variables(vars)),
        Application::simple("park"),
    )?;
    // originate {absolute_codec_string=PCMU\,PCMA\,G722}sofia/gateway/gw1/1234 &park()
    println!("{}", cmd.render(block_parse));

    // -----------------------------------------------------------------------
    // OriginateTarget variants
    // -----------------------------------------------------------------------

    println!("\n=== OriginateTarget variants ===");

    // Extension: routed through the XML dialplan engine
    println!("\n-- Extension (routes through XML dialplan) --");

    let cmd = Originate::extension(
        Endpoint::SofiaGateway(SofiaGateway::new("gw1", "18005551234")?),
        "1000",
    )?
    // Safe unwrap: DialplanType::XML is a valid dialplan for extensions
    .with_dialplan(DialplanType::XML)
    .unwrap()
    .with_context("default")?;
    // originate sofia/gateway/gw1/18005551234 1000 XML default
    println!("{}", cmd.render(block_parse));

    // Any other dialplan module goes by the name it registers; the switch hangs the
    // channel up if no module answers to it.
    let cmd = Originate::extension(
        Endpoint::SofiaGateway(SofiaGateway::new("gw1", "18005551234")?),
        "1000",
    )?
    // Safe unwraps: only "undef" is refused as a name, and a named dialplan fits an extension
    .with_dialplan(DialplanType::named("enum").unwrap())
    .unwrap();
    // originate sofia/gateway/gw1/18005551234 1000 enum
    println!("{}", cmd.render(block_parse));

    // Application: single &app(args) XML form
    println!("\n-- Application (&app(args)) with spaces auto-quoted --");

    // Args containing spaces are automatically single-quoted on the wire.
    // FreeSWITCH's originate parser requires this.
    let cmd = Originate::application(
        Endpoint::SofiaGateway(SofiaGateway::new("gw1", "18005551234")?),
        Application::new("socket", Some("127.0.0.1:8040 async full")),
    )?;
    // originate sofia/gateway/gw1/18005551234 '&socket(127.0.0.1:8040 async full)'
    println!("{}", cmd.render(block_parse));

    // Inline applications: comma-separated app:args list
    println!("\n-- InlineApplications (app:args,app:args) --");

    // Originate::inline() returns Result; empty app list is rejected
    let cmd = Originate::inline(
        Endpoint::SofiaGateway(SofiaGateway::new("gw1", "18005551234")?),
        vec![
            Application::new("conference", Some("test_room")),
            Application::simple("hangup"),
        ],
    )
    // Safe unwrap: non-empty application list is valid
    .unwrap()
    // DialplanType::INLINE is emitted as "inline" on the wire
    // Safe unwrap: DialplanType::INLINE is valid for inline applications
    .with_dialplan(DialplanType::INLINE)
    .unwrap();
    // originate sofia/gateway/gw1/18005551234 conference:test_room,hangup inline
    println!("{}", cmd.render(block_parse));

    // -----------------------------------------------------------------------
    // JSON deserialization -- config-driven originate
    // -----------------------------------------------------------------------

    println!("\n=== JSON deserialization ===");

    // Originate commands can live entirely in config files and be deserialized
    // at runtime. The endpoint uses snake_case variant names.
    // A flat variable map defaults to VariablesType::Default ({} scope).
    let json = r#"{
        "endpoint": {
            "sofia_gateway": {
                "gateway": "my_provider",
                "destination": "18005551234",
                "variables": {"originate_timeout": "60", "sip_h_X-Custom": "value"}
            }
        },
        "application": {"name": "park"},
        "timeout_secs": 30
    }"#;
    match serde_json::from_str::<Originate>(json) {
        Ok(cmd) => println!("from JSON: {}", cmd.render(block_parse)),
        Err(e) => println!("JSON parse error: {}", e),
    }

    // Wire format round-trip, parsed and rendered for the same revision
    let wire = "originate sofia/gateway/carrier/15551234567 &bridge(user/1000) XML default Alice 5551234 60";
    match Originate::parse_for(wire, block_parse) {
        Ok(cmd) => {
            let round_tripped = cmd.render(block_parse);
            if round_tripped == wire {
                println!("round-trip: {round_tripped}");
            } else {
                println!("round-trip changed the string:\n  in:  {wire}\n  out: {round_tripped}");
            }
        }
        Err(e) => println!("parse error: {}", e),
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Escaping depends on the switch's parser revision, which the crate never
    // asks the switch for; resolved first, so a bad version fails before a call.
    let block_parse = block_parse::block_parse_from_env()?;
    println!("parser revision: {block_parse}\n");

    print_endpoint_examples(block_parse)?;

    // -----------------------------------------------------------------------
    // Part 2: live call via bgapi
    // -----------------------------------------------------------------------

    let (client, mut events) = common::connect_from_env().await?;

    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::BackgroundJob,
                EslEventType::ChannelCreate,
                EslEventType::ChannelAnswer,
                EslEventType::ChannelHangup,
                EslEventType::ChannelDestroy,
            ],
        )
        .await?;

    // loopback/9196/default routes to the built-in delay_echo test in the
    // default FreeSWITCH configuration -- no registered phones required.
    let cmd = Originate::application(
        Endpoint::Loopback(LoopbackEndpoint::new("9196")?.with_context("default")?),
        Application::simple("park"),
    )?
    .with_cid_name("ESL Test")?
    .with_cid_num("0000000000")?
    .with_timeout(Duration::from_secs(10));

    println!("\n=== Live call via bgapi ===");
    // The wire form has to match the revision the switch runs.
    let wire = cmd.render(block_parse);
    println!("originate: {wire}");

    // bgapi returns immediately with a Job-UUID and the originate result
    // arrives later as a BACKGROUND_JOB event. BACKGROUND_JOB is a switch-wide
    // event, so the UUID is what makes a result ours; BgJobTracker owns that
    // bookkeeping, and reports a refused bgapi as the denial it is rather than
    // as a missing header.
    let mut jobs: BgJobTracker<()> = BgJobTracker::new();
    jobs.bgapi(&client, &wire, ())
        .await?;

    let mut call_uuid: Option<String> = None;
    // Nothing guarantees the call ever produces a channel, so the loop needs an
    // end of its own.
    let deadline = tokio::time::Instant::now() + CALL_DEADLINE;

    loop {
        let result = match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(result)) => result,
            Ok(None) => {
                error!("event stream closed before the call finished");
                break;
            }
            Err(_) => {
                error!("gave up after {CALL_DEADLINE:?}");
                break;
            }
        };
        let event = match result {
            Ok(event) => event,
            Err(e) => {
                error!("event error: {e}");
                continue;
            }
        };

        // A BACKGROUND_JOB that is not ours leaves the tracker untouched.
        if let Some(((), job)) = jobs.try_complete(&event) {
            match job.parse_body() {
                Ok(uuid) => {
                    call_uuid = Some(uuid.to_string());
                    info!("call created: {uuid}");
                }
                Err(e) => {
                    error!("originate failed: {e}");
                    break;
                }
            }
            continue;
        }

        // unique_id() falls back to Caller-Unique-ID, which the enum lookup
        // alone would miss.
        let Some(uuid) = event.unique_id() else {
            continue;
        };
        match event.event_type() {
            Some(EslEventType::ChannelCreate) => info!("channel created: {uuid}"),
            Some(EslEventType::ChannelAnswer) => info!("channel answered: {uuid}"),
            Some(EslEventType::ChannelHangup) => match event.hangup_cause() {
                Ok(Some(cause)) => info!("channel hangup: {uuid} cause={cause}"),
                Ok(None) => info!("channel hangup: {uuid}, no cause header"),
                Err(e) => error!("channel hangup: {uuid}, unparseable cause: {e}"),
            },
            Some(EslEventType::ChannelDestroy) => {
                info!("channel destroyed: {uuid}");
                // Stop once our specific channel is gone
                if call_uuid.as_deref() == Some(uuid) {
                    break;
                }
            }
            _ => {}
        }
    }

    client
        .disconnect()
        .await?;
    Ok(())
}
