//! Integration tests against a live FreeSWITCH instance: YAML-configured
//! loopback originate (see docs/originate-loopback-yaml.md), nested-bridge
//! scoped variables, and the bowout pair.
//!
//! These tests require a live FreeSWITCH ESL; see docs/live-test-switch.md.
//! Run with: cargo test --test 'live_*' -- --ignored

mod live_common;

use freeswitch_esl_tokio::commands::originate::{OriginateTarget, Variables, VariablesType};
use freeswitch_esl_tokio::commands::{LoopbackEndpoint, UuidSetVar, UuidTransfer};
use freeswitch_esl_tokio::variables::{LoopbackChannelName, LoopbackHangupCause, LoopbackVariable};
use freeswitch_esl_tokio::{
    Application, ChannelState, DialplanType, Endpoint, EslEventType, EventFormat, EventHeader,
    HeaderLookup, Originate,
};
use live_common::{channel_exists, connect, getvar, wait_for_var, ChannelReaper};
use std::time::Duration;
use tokio::time::Instant;

/// The same YAML the example and the docs use, so a drift in any of the three
/// breaks the build rather than only the prose.
const LOOPBACK_YAML: &str = include_str!("../examples/originate_loopback.yaml");
const LOOPBACK_BOWOUT_YAML: &str = include_str!("../examples/originate_loopback_bowout.yaml");
const LOOPBACK_SCOPED_YAML: &str = include_str!("../examples/originate_loopback_scoped_vars.yaml");

/// Channel variables the YAML sets. FreeSWITCH must expose each on *both*
/// loopback legs: `switch_ivr_originate` applies the originate variable block
/// to the A leg, and mod_loopback replays it onto the B leg.
const LOOPBACK_YAML_VARS: &[(&str, &str)] = &[
    ("customer_id", "CUST-42"),
    ("tenant", "acme"),
    // On the wire this is `'T-1001\, urgent'`: comma escaped, whole value
    // quoted for the space. FreeSWITCH unescapes both.
    ("sip_h_X-Ticket", "T-1001, urgent"),
];

fn loopback_yaml_originate() -> Originate {
    yaml_serde::from_str(LOOPBACK_YAML).expect("examples/originate_loopback.yaml must deserialize")
}

#[test]
fn yaml_loopback_originate_parses() {
    let cmd = loopback_yaml_originate();

    let Endpoint::Loopback(ref ep) = *cmd.endpoint() else {
        panic!("expected a loopback endpoint, got {:?}", cmd.endpoint());
    };
    assert_eq!(ep.extension, "9199");
    assert_eq!(
        ep.context
            .as_deref(),
        Some("test")
    );

    let vars = ep
        .variables
        .as_ref()
        .expect("endpoint must carry variables");
    // `scope: channel` in YAML -> [] brackets on the wire.
    assert_eq!(vars.scope(), VariablesType::Channel);
    for (name, value) in LOOPBACK_YAML_VARS {
        assert_eq!(vars.get(name), Some(*value), "variable {}", name);
    }
    assert_eq!(vars.get("origination_caller_id_name"), Some("Sales Desk"));

    assert!(matches!(
        cmd.target(),
        OriginateTarget::Application(app) if app.name() == "park" && app.args().is_none()
    ));
    assert_eq!(cmd.dialplan_type(), Some(&DialplanType::XML));
    assert_eq!(cmd.context_str(), Some("test"));
    assert_eq!(cmd.caller_id_name(), Some("Fallback CID"));
    assert_eq!(cmd.caller_id_number(), Some("5550199"));
    assert_eq!(cmd.timeout_seconds(), Some(30));

    assert_eq!(
        cmd.to_string(),
        "originate [origination_caller_id_name='Sales Desk',\
origination_caller_id_number=5550100,ignore_early_media=true,customer_id=CUST-42,\
tenant=acme,sip_h_X-Ticket='T-1001\\, urgent']loopback/9199/test \
&park() XML test 'Fallback CID' 5550199 30"
    );
}

#[test]
fn yaml_loopback_bowout_parses() {
    let cmd: Originate = yaml_serde::from_str(LOOPBACK_BOWOUT_YAML)
        .expect("examples/originate_loopback_bowout.yaml must deserialize");

    let Endpoint::Loopback(ref ep) = *cmd.endpoint() else {
        panic!("expected a loopback endpoint, got {:?}", cmd.endpoint());
    };
    // mod_loopback's `app=<application>[:<args>]` destination form.
    assert_eq!(ep.extension, "app=bridge:null/farend");
    assert!(ep
        .context
        .is_none());

    // A bare YAML mapping (no scope/vars wrapper) means Default scope -> {}.
    let vars = ep
        .variables
        .as_ref()
        .expect("endpoint must carry variables");
    assert_eq!(vars.scope(), VariablesType::Default);
    assert_eq!(vars.get("loopback_bowout"), Some("true"));

    assert!(matches!(
        cmd.target(),
        OriginateTarget::Application(app)
            if app.name() == "bridge" && app.args() == Some("null/nearend")
    ));

    assert_eq!(
        cmd.to_string(),
        "originate {loopback_bowout=true}loopback/app=bridge:null/farend &bridge(null/nearend)"
    );
}

/// Originate the YAML command against FreeSWITCH and confirm the pair really
/// comes up: both legs are created and answered, and every variable from the
/// originate block is readable on each leg.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_originate_loopback_from_yaml() {
    let (client, mut events, _permit) = connect().await;

    client
        .subscribe_events(
            EventFormat::Plain,
            &[EslEventType::ChannelCreate, EslEventType::ChannelAnswer],
        )
        .await
        .unwrap();

    // api originate blocks until the A leg answers, which happens once the B
    // leg's dialplan (9199 -> answer) answers. Events queue while it blocks.
    let cmd = loopback_yaml_originate();
    let resp = client
        .api(&cmd.to_string())
        .await
        .expect("originate transport error");
    let a_leg = resp
        .api_result()
        .expect("originate returned an error")
        .to_string();

    let mut reaper = ChannelReaper::new(&client);
    reaper.track(&a_leg);

    // mod_loopback cross-links the two legs with this variable.
    let b_leg = getvar(&client, &a_leg, "other_loopback_leg_uuid")
        .await
        .expect("A leg must expose other_loopback_leg_uuid");
    reaper.track(&b_leg);
    assert_ne!(a_leg, b_leg, "the two loopback legs must be distinct");

    assert_eq!(
        getvar(&client, &a_leg, "loopback_leg")
            .await
            .as_deref(),
        Some("A")
    );
    assert_eq!(
        getvar(&client, &b_leg, "loopback_leg")
            .await
            .as_deref(),
        Some("B")
    );

    // Every originate variable must be visible on both legs.
    for (name, expected) in LOOPBACK_YAML_VARS {
        for (leg, uuid) in [("A", &a_leg), ("B", &b_leg)] {
            assert_eq!(
                getvar(&client, uuid, name)
                    .await
                    .as_deref(),
                Some(*expected),
                "{} leg is missing variable {}",
                leg,
                name
            );
        }
    }

    // The B leg is the one the dialplan runs on, so it carries the caller ID.
    // `origination_caller_id_*` wins over the positional cid_name/cid_num,
    // which the YAML deliberately sets to different values.
    assert_eq!(
        getvar(&client, &b_leg, "caller_id_name")
            .await
            .as_deref(),
        Some("Sales Desk")
    );
    assert_eq!(
        getvar(&client, &b_leg, "caller_id_number")
            .await
            .as_deref(),
        Some("5550100")
    );

    // Both legs must have been created and answered.
    let mut created = std::collections::HashSet::new();
    let mut answered = std::collections::HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while (created.len() < 2 || answered.len() < 2) && Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => {
                let Some(uuid) = evt.unique_id() else {
                    continue;
                };
                if uuid != a_leg && uuid != b_leg {
                    continue;
                }
                match evt.event_type() {
                    Some(EslEventType::ChannelCreate) => {
                        created.insert(uuid.to_string());
                    }
                    Some(EslEventType::ChannelAnswer) => {
                        answered.insert(uuid.to_string());
                    }
                    _ => {}
                }
            }
            Ok(Some(Err(e))) => panic!("event error: {}", e),
            Ok(None) => panic!("event stream closed"),
            Err(_) => break,
        }
    }

    reaper
        .reap()
        .await;

    assert_eq!(created.len(), 2, "expected CHANNEL_CREATE for both legs");
    assert_eq!(answered.len(), 2, "expected CHANNEL_ANSWER for both legs");
}

#[test]
fn yaml_loopback_scoped_vars_parses() {
    let cmd: Originate = yaml_serde::from_str(LOOPBACK_SCOPED_YAML)
        .expect("examples/originate_loopback_scoped_vars.yaml must deserialize");
    assert_eq!(
        cmd.to_string(),
        "originate {leg_a_only=outer}loopback/9199/test &bridge({leg_b_only=inner}null/far)"
    );
}

/// A variable set in the originate's own block reaches the loopback pair and
/// stops there; a variable set in the bridge's dial string reaches only the
/// bridged leg. This is the only way to give the two sides of a call
/// different variables, since neither `{}` nor `[]` can address one loopback
/// leg on its own.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_originate_loopback_nested_bridge_scopes_vars() {
    let (client, _events, _permit) = connect().await;

    let cmd: Originate = yaml_serde::from_str(LOOPBACK_SCOPED_YAML).unwrap();
    let resp = client
        .api(&cmd.to_string())
        .await
        .expect("originate transport error");
    let a_leg = resp
        .api_result()
        .expect("originate returned an error")
        .to_string();

    let mut reaper = ChannelReaper::new(&client);
    reaper.track(&a_leg);

    let b_leg = getvar(&client, &a_leg, "other_loopback_leg_uuid")
        .await
        .expect("A leg must expose other_loopback_leg_uuid");
    reaper.track(&b_leg);

    // The A leg runs &bridge(...), so the far channel shows up as its bridge
    // partner once the bridge is established.
    let deadline = Instant::now() + Duration::from_secs(10);
    let far = wait_for_var(&client, &a_leg, "bridge_uuid", deadline).await;
    if let Some(far) = &far {
        reaper.track(far);
    }

    // Every read happens while the channels are alive; the reap below has to
    // run before the first assertion.
    let mut on_legs = Vec::new();
    for (leg, uuid) in [("A", &a_leg), ("B", &b_leg)] {
        on_legs.push((
            leg,
            getvar(&client, uuid, "leg_a_only").await,
            getvar(&client, uuid, "leg_b_only").await,
        ));
    }
    let mut on_far = None;
    if let Some(far) = &far {
        on_far = Some((
            getvar(&client, far, "leg_a_only").await,
            getvar(&client, far, "leg_b_only").await,
        ));
    }

    reaper
        .reap()
        .await;

    let (far_leg_a, far_leg_b) = on_far.expect("A leg never bridged to null/far");
    for (leg, leg_a_only, leg_b_only) in on_legs {
        // The originate block reaches both loopback legs...
        assert_eq!(
            leg_a_only.as_deref(),
            Some("outer"),
            "{leg} leg should carry the originate block variable"
        );
        // ...and the bridge dial string reaches only the leg it dials.
        assert_eq!(
            leg_b_only, None,
            "{leg} leg must not see the bridge dial string variable"
        );
    }
    assert_eq!(
        far_leg_a, None,
        "originate block variables must not leak across the bridge"
    );
    assert_eq!(far_leg_b.as_deref(), Some("inner"));
}

/// Drive a loopback pair through a bowout and confirm mod_loopback removed
/// itself: both loopback legs resign, and the two real channels end up
/// bridged straight to each other.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_originate_loopback_bowout_from_yaml() {
    let (client, mut events, _permit) = connect().await;

    // Subscribe first: the pair can bow out within milliseconds of the A leg's
    // bridge starting, and those hangups are the evidence.
    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::ChannelBridge,
                EslEventType::ChannelHangupComplete,
            ],
        )
        .await
        .unwrap();

    let cmd: Originate = yaml_serde::from_str(LOOPBACK_BOWOUT_YAML).unwrap();
    let resp = client
        .api(&cmd.to_string())
        .await
        .expect("originate transport error");
    let a_leg = resp
        .api_result()
        .expect("originate returned an error")
        .to_string();

    let mut resigned: Vec<(String, String)> = Vec::new();
    let mut legs: Vec<String> = Vec::new();
    // uuid_bridge fires before mod_loopback stamps the legs, so the splice can
    // arrive before the survivors are known. Keep every real-to-real bridge and
    // pick ours out once the resignations name them.
    let mut real_bridges: Vec<(String, String)> = Vec::new();

    // Our splice is the real-to-real bridge whose two ends are exactly the
    // channels our resignations handed over to. The bridge event can arrive on
    // either side of the hangups, so both have to be in hand before deciding.
    let spliced = |resigned: &[(String, String)], bridges: &[(String, String)]| {
        let survivors: std::collections::HashSet<&str> = resigned
            .iter()
            .map(|(_, s)| s.as_str())
            .collect();
        survivors.len() == 2
            && bridges
                .iter()
                .any(|(a, b)| survivors.contains(a.as_str()) && survivors.contains(b.as_str()))
    };

    let deadline = Instant::now() + Duration::from_secs(15);
    while !spliced(&resigned, &real_bridges) && Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => match evt.event_type() {
                Some(EslEventType::ChannelHangupComplete) => {
                    // Both legs carry the partner's uuid, so either one ties
                    // back to the leg this test originated. Without that, a
                    // foreign bowout on the shared switch lands here too.
                    let ours = evt.unique_id() == Some(a_leg.as_str())
                        || evt.variable(LoopbackVariable::OtherLoopbackLegUuid)
                            == Some(a_leg.as_str());
                    if !ours {
                        continue;
                    }
                    // mod_loopback stamps this on both legs right before it
                    // bridges the real channels together.
                    let Some(r) = evt.loopback_resignation() else {
                        continue;
                    };
                    // This YAML drives the frame-count path specifically.
                    assert_eq!(r.cause(), Ok(LoopbackHangupCause::Bridge));
                    // The marker is also copied onto the channel that continues
                    // the call, so only the emitter's own name says a loopback
                    // leg sent this.
                    let name = evt
                        .channel_name()
                        .expect("a channel event names its channel");
                    let parsed = LoopbackChannelName::parse(name)
                        .expect("a resigning leg's own name is a loopback name");
                    assert_eq!(evt.channel_driver(), Some("loopback"));
                    assert_eq!(
                        Some(parsed.leg()),
                        evt.loopback_leg()
                            .expect("loopback_leg parses")
                    );
                    let survivor = r
                        .other_uuid()
                        .expect("a resigning leg must name the real channel it hands over to");
                    if let Some(uuid) = evt.unique_id() {
                        legs.push(uuid.to_string());
                    }
                    resigned.push((
                        evt.header(EventHeader::ChannelName)
                            .unwrap_or_default()
                            .to_string(),
                        survivor.to_string(),
                    ));
                }
                Some(EslEventType::ChannelBridge) => {
                    // Three bridges occur: loopback-a=null/nearend,
                    // loopback-b=null/farend, then the bowout's uuid_bridge.
                    // Only the last has a real channel on both sides.
                    let (Some(this), Some(other)) = (
                        evt.header(EventHeader::ChannelName),
                        evt.header(EventHeader::OtherLegChannelName),
                    ) else {
                        continue;
                    };
                    if LoopbackChannelName::parse(this).is_some()
                        || LoopbackChannelName::parse(other).is_some()
                    {
                        continue;
                    }
                    if let (Some(a), Some(b)) =
                        (evt.unique_id(), evt.header(EventHeader::OtherLegUniqueId))
                    {
                        real_bridges.push((a.to_string(), b.to_string()));
                    }
                }
                _ => {}
            },
            Ok(Some(Err(e))) => panic!("event error: {}", e),
            Ok(None) => panic!("event stream closed"),
            Err(_) => break,
        }
    }

    // A resigned leg hung itself up, so it must already be gone -- one that is
    // still around means mod_loopback left the audio path without tearing the
    // leg down, which would strand a channel per bowout on a live switch.
    // Checked before the reap below, which would otherwise hide it.
    let mut zombies = Vec::new();
    for leg in &legs {
        if channel_exists(&client, leg).await {
            zombies.push(leg.clone());
        }
    }

    // The survivors are a live call by design, so nothing else will end them.
    let mut reaper = ChannelReaper::new(&client);
    reaper.track(&a_leg);
    for (_, survivor) in &resigned {
        reaper.track(survivor);
    }
    reaper
        .reap()
        .await;

    assert!(
        zombies.is_empty(),
        "resigned loopback legs must be gone, still present: {:?}",
        zombies
    );
    assert_eq!(
        resigned.len(),
        2,
        "both loopback legs must resign, saw {:?}",
        resigned
    );
    assert!(
        spliced(&resigned, &real_bridges),
        "the two real channels this test created must end up bridged to each other; \
         resigned {:?}, real bridges seen {:?}",
        resigned,
        real_bridges
    );
}

/// mod_loopback's other bowout trigger, which reports a different token.
///
/// `loopback_bowout_on_execute` resigns the leg as soon as it executes an
/// application, masquerading its extension onto the real channel behind its
/// partner instead of waiting for audio to flow. `loopback_bowout=false`
/// vetoes the frame-count path so only this one can fire.
///
/// This is the token a consumer bug matched against, so it earns live coverage
/// rather than a synthesized header map: the two paths must stay
/// indistinguishable to `loopback_resignation()` and distinguishable to
/// `cause()`.
///
/// Setting the trigger in the originate would be a race, not a test.
/// mod_loopback bows out only if the partner leg already carries a signal bond
/// to a non-loopback channel when this leg executes, and does nothing at all
/// when it does not — `switch_ivr_multi_threaded_bridge` writes that bond
/// *after* it fires `CHANNEL_BRIDGE`, while `originate` returns as soon as the
/// leg answers. So park the leg, wait for the far channel to reach
/// `CS_EXCHANGE_MEDIA` (which the switch sets only after writing the bond),
/// then arm the trigger and transfer. Every step is ordered by the switch.
#[tokio::test]
#[ignore = "needs a live FreeSWITCH ESL; see docs/live-test-switch.md"]
async fn live_originate_loopback_bowout_on_execute() {
    let (client, mut events, _permit) = connect().await;

    client
        .subscribe_events(
            EventFormat::Plain,
            &[
                EslEventType::ChannelState,
                EslEventType::ChannelHangupComplete,
            ],
        )
        .await
        .unwrap();

    // No trigger yet: parking means this leg's first execute pass cannot race.
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert("loopback_bowout", "false")
        .unwrap();
    let cmd = Originate::application(
        Endpoint::Loopback(LoopbackEndpoint::new("app=bridge:null/farend").with_variables(vars)),
        Application::park(),
    );

    let resp = client
        .api(&cmd.to_string())
        .await
        .expect("originate transport error");
    let a_leg = resp
        .api_result()
        .expect("originate returned an error")
        .to_string();

    let mut reaper = ChannelReaper::new(&client);
    reaper.track(&a_leg);

    // The leg is parked and alive, so this is safe to ask for.
    let b_leg = getvar(&client, &a_leg, "other_loopback_leg_uuid")
        .await
        .expect("a loopback leg must name its partner");
    reaper.track(&b_leg);

    // The far channel bonded to the partner is the one whose CS_EXCHANGE_MEDIA
    // proves the bond exists; keying on the bond value keeps a concurrent
    // test's identical topology out of it.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut bonded = false;
    while !bonded && Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => {
                if evt.event_type() != Some(EslEventType::ChannelState) {
                    continue;
                }
                // CHANNEL_STATE carries no channel variables, so the bond
                // itself is not observable here -- but the far channel names
                // the partner leg it was originated by, and the switch only
                // moves it to CS_EXCHANGE_MEDIA after writing that bond.
                if evt.header(EventHeader::OtherLegUniqueId) != Some(b_leg.as_str()) {
                    continue;
                }
                if evt.channel_state() == Ok(Some(ChannelState::CsExchangeMedia)) {
                    bonded = true;
                }
            }
            Ok(Some(Err(e))) => panic!("event error: {}", e),
            Ok(None) => panic!("event stream closed"),
            Err(_) => break,
        }
    }

    if bonded {
        client
            .api(
                &UuidSetVar::new(&a_leg, "loopback_bowout_on_execute", "true")
                    .unwrap()
                    .to_string(),
            )
            .await
            .expect("uuid_setvar transport error")
            .api_result()
            .expect("uuid_setvar rejected");

        // Re-entering CS_EXECUTE runs channel_on_execute again, this time with
        // the trigger armed and the partner's bond already in place.
        client
            .api(
                &UuidTransfer::new(&a_leg, "bridge:null/nearend")
                    .with_dialplan(DialplanType::INLINE)
                    .to_string(),
            )
            .await
            .expect("uuid_transfer transport error")
            .api_result()
            .expect("uuid_transfer rejected");
    }

    let mut resignation: Option<(String, Option<String>, Option<String>)> = None;
    while bonded && resignation.is_none() && Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Ok(evt))) => {
                if evt.event_type() != Some(EslEventType::ChannelHangupComplete) {
                    continue;
                }
                let ours = evt.unique_id() == Some(a_leg.as_str())
                    || evt.variable(LoopbackVariable::OtherLoopbackLegUuid) == Some(a_leg.as_str());
                if !ours {
                    continue;
                }
                if let Some(r) = evt.loopback_resignation() {
                    resignation = Some((
                        r.cause_raw()
                            .to_string(),
                        r.other_uuid()
                            .map(str::to_string),
                        evt.channel_name()
                            .map(str::to_string),
                    ));
                }
            }
            Ok(Some(Err(e))) => panic!("event error: {}", e),
            Ok(None) => panic!("event stream closed"),
            Err(_) => break,
        }
    }

    if let Some((_, Some(ref survivor), _)) = resignation {
        reaper.track(survivor);
    }
    reaper
        .reap()
        .await;

    assert!(
        bonded,
        "the partner leg never bonded to a real channel, so the execute path \
         could not be reached"
    );
    let (cause_raw, other_uuid, channel_name) =
        resignation.expect("the execute path must report a resignation on the leg that bowed out");
    assert_eq!(
        cause_raw.parse::<LoopbackHangupCause>(),
        Ok(LoopbackHangupCause::Bowout),
        "the execute path writes its own token, got {:?}",
        cause_raw
    );
    assert!(
        other_uuid.is_some(),
        "a resigning leg must name the real channel it hands over to"
    );
    // This path masquerades the leg onto the survivor, so the marker reaches a
    // real channel too. What stays true is the emitter's own name.
    let name = channel_name.expect("a channel event names its channel");
    assert!(
        LoopbackChannelName::parse(&name).is_some(),
        "the resignation came from a channel that is not a loopback leg: {name}"
    );
}
