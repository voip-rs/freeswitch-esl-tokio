//! Inspect a live call's SDP offer and compose a codec string that FreeSWITCH will honour.
//!
//! For each answered channel this example:
//! - reads `switch_r_sdp` via the typed `ChannelVariable`/`HeaderLookup` path (never
//!   `header_str()` -- a typo in a raw header name is a silent `None`, not a compile error),
//! - parses it with `SdpCodecs::parse` and prints what the peer offered,
//! - builds the audio codec string from the offer, appends a mandatory backup list,
//!   deduplicates, and drops entries no loaded implementation would accept,
//! - sets `absolute_codec_string` and `rtp_force_audio_fmtp` on the channel via ESL.
//!
//! Run with the `sdp` feature enabled:
//!   cargo run --example sdp_codec_string --features sdp
//!
//! Connection (all optional, these are the defaults):
//!   ESL_HOST=localhost  ESL_PORT=8021  ESL_PASSWORD=ClueCon

use freeswitch_esl_tokio::sdp::{
    default_rate, CodecImplementation, CodecString, CodecStringOptions, SdpCodecEntry, SdpCodecs,
    SdpWarning,
};
use freeswitch_esl_tokio::{
    ChannelVariable, EslClient, EslError, EslEventType, EventFormat, EventSubscription,
    HeaderLookup, UuidSetVar, DEFAULT_ESL_PASSWORD, DEFAULT_ESL_PORT,
};
use tracing::{error, info, warn};

// Codecs a regulated interface requires to always be present, appended after the
// offer rather than merged into it. This is the point of composing over merging:
// the peer's own order and qualifiers (its `PCMU@8000h@20i` beats a bare `PCMU`
// appended below it) are never displaced, only backfilled if genuinely missing.
const MANDATORY_BACKUP_CODECS: &str = "PCMU,PCMA";

// No ESL API exposes the loaded-implementation table, so retain_available() knows
// only what this list says -- keep it in sync with the box's modules.conf.
//
// "t38" is deliberately absent: no codec interface carries that name, so a T.38
// offer is dropped here unless you add it. See docs/codec-string-format.md.
fn loaded_implementations() -> Vec<CodecImplementation> {
    vec![
        CodecImplementation::new("PCMU"),
        CodecImplementation::new("PCMA"),
        CodecImplementation::new("G722"),
        CodecImplementation::new("opus"),
    ]
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let host = std::env::var("ESL_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port: u16 = match std::env::var("ESL_PORT") {
        Ok(value) => value.parse()?,
        Err(_) => DEFAULT_ESL_PORT,
    };
    let password =
        std::env::var("ESL_PASSWORD").unwrap_or_else(|_| DEFAULT_ESL_PASSWORD.to_string());

    let (client, mut events) = match EslClient::connect(&host, port, &password).await {
        Ok(pair) => {
            info!("Connected to {}:{}", host, port);
            pair
        }
        Err(EslError::Io(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            error!("Connection refused at {}:{}", host, port);
            return Err(e.into());
        }
        Err(e) => return Err(e.into()),
    };

    // EventSubscription is the typed builder for subscriptions. It bundles the event
    // format, typed event names, and optional subclass filters into a single call to
    // apply_subscription(). Never construct "event plain CHANNEL_ANSWER" by hand.
    let subscription =
        EventSubscription::new(EventFormat::Plain).with_event(EslEventType::ChannelAnswer);
    client
        .apply_subscription(&subscription)
        .await?;
    info!("Subscribed to CHANNEL_ANSWER");

    // Parse the backup list once; the same CodecString is appended to every offer.
    // expect() is safe here: MANDATORY_BACKUP_CODECS is a compile-time constant whose
    // syntax was verified against the grammar in docs/codec-string-format.md.
    let backup: CodecString = MANDATORY_BACKUP_CODECS
        .parse()
        .expect("MANDATORY_BACKUP_CODECS is a valid codec string constant");
    let inventory = loaded_implementations();

    while let Some(result) = events
        .recv()
        .await
    {
        let event = match result {
            Ok(event) => event,
            Err(e) => {
                error!("Event error: {}", e.display_chain());
                continue;
            }
        };

        let uuid = match event.unique_id() {
            Some(u) => u.to_string(),
            None => continue,
        };

        // variable() is HeaderLookup's typed channel-variable accessor. It prepends
        // "variable_" internally and returns None cleanly when the header is absent.
        // Using the ChannelVariable enum rather than a raw string means a typo is a
        // compile error, not a silent None at runtime.
        let sdp_str = match event.variable(ChannelVariable::SwitchRSdp) {
            Some(s) => s.to_string(),
            None => {
                let short = short_uuid(&uuid);
                warn!("{short}: no switch_r_sdp -- call was not an INVITE with body SDP");
                continue;
            }
        };

        // event is dropped here; its string data was already copied into sdp_str.
        handle_answered_channel(&client, &uuid, &sdp_str, &backup, &inventory).await;
    }

    client
        .disconnect()
        .await?;
    Ok(())
}

async fn handle_answered_channel(
    client: &EslClient,
    uuid: &str,
    sdp_str: &str,
    backup: &CodecString,
    inventory: &[CodecImplementation],
) {
    let short = short_uuid(uuid);

    // Fails only on input that is not SDP. What the switch itself would refuse, or
    // stop reading at, comes back in warnings() below instead.
    let parsed = match SdpCodecs::parse(sdp_str) {
        Ok(p) => p,
        Err(e) => {
            warn!("{}: SDP parse error: {}", short, e);
            return;
        }
    };

    // Print a table of what the peer offered in the order it offered them.
    println!("{}: offered codecs (SDP offer order):", short);
    println!(
        "  {:<14} {:>3}  {:>8}  {:>3}  {:>5}  fmtp",
        "name", "pt", "rate", "ch", "ptime"
    );
    for entry in parsed.entries() {
        match entry {
            SdpCodecEntry::Rtp(c) => {
                println!(
                    "  {:<14} {:>3}  {:>8}  {:>3}  {:>5}  {}",
                    c.name(),
                    c.payload_type(),
                    c.clock_rate(),
                    c.channels()
                        .map_or_else(|| "-".to_string(), |n| n.to_string()),
                    c.ptime()
                        .map_or_else(|| "-".to_string(), |p| p.to_string()),
                    c.fmtp()
                        .unwrap_or(""),
                );
            }
            SdpCodecEntry::T38 => println!("  t38 (image/udptl)"),
            // SdpCodecEntry is #[non_exhaustive]; future variants land here.
            _ => {}
        }
    }

    // sections() is every m= line the offer carried, in order, including the ones the
    // table above cannot show: a port-0 section is a held or declined stream, and its
    // codecs are exactly what you want in front of you on a "no audio" complaint.
    for section in parsed.sections() {
        if section.is_negotiable() {
            continue;
        }
        println!(
            "{}: m={} port {} {} {} (excluded from the codec string)",
            short,
            section.media_type(),
            section.port(),
            section.proto(),
            section.formats(),
        );
        for entry in section.entries() {
            if let SdpCodecEntry::Rtp(c) = entry {
                println!(
                    "    [{:>3}] {}/{} {}",
                    c.payload_type(),
                    c.name(),
                    c.clock_rate(),
                    c.fmtp()
                        .unwrap_or(""),
                );
            }
        }
        for payload in section.non_codec_payloads() {
            println!("    {}", payload);
        }
    }

    // Negotiated through FreeSWITCH's smh->mparams->te and cng_pt paths, never through
    // the codec string, so these never reach entries() or the string built below. This
    // is what the offer said, not what the switch picked: it keeps one of each per
    // session and never reads their fmtp. Display renders the a=rtpmap they came from.
    for payload in parsed.non_codec_payloads() {
        println!("{}: {}", short, payload);
    }

    // unmapped(): payload types in the m= format list with no a=rtpmap and no
    // RFC 3551 static-table entry. Surfaced as data rather than a parse error so
    // the caller can distinguish "never offered" from "offered but unresolvable".
    for u in parsed.unmapped() {
        warn!("{}: {}", short, u);
    }

    // warnings(): oddities the parse read past, such as an unparseable a=ptime or a
    // line where the switch's own parser refuses the offer (SwitchRejects).
    for w in parsed.warnings() {
        warn!("{}: SDP warning: {}", short, w);
    }

    // Build the audio codec string from the offer. Lenient mode: an SDP crossing
    // the network is not something we control, so an unrepresentable fmtp gets
    // cleared (with a warning) rather than failing the whole call.
    let mut build_warnings: Vec<SdpWarning> = Vec::new();
    let mut codec_string =
        match parsed.audio_codec_string(&CodecStringOptions::audio(), Some(&mut build_warnings)) {
            Ok(cs) => cs,
            Err(e) => {
                warn!("{}: audio_codec_string error: {}", short, e);
                return;
            }
        };
    for w in &build_warnings {
        warn!("{}: codec-string warning: {}", short, w);
    }
    println!("{}: from offer: {}", short, codec_string);

    // Append the mandatory backup list. This is composition, not a merge: nothing
    // from the offer is removed or reordered, the backup entries just land at the
    // tail as a floor. Concatenation order is the policy -- see dedup() below.
    codec_string.extend_from(backup);
    println!("{}: with backup appended: {}", short, codec_string);

    // dedup() uses FreeSWITCH's own key (name/rate/ptime/channels/fmtp, normalized
    // the way switch_loadable_module_get_codecs_sorted does). A bare "PCMU" and
    // "PCMU@8000h@20i" collapse to one entry because they normalize to the same
    // key. The FIRST occurrence wins -- which is exactly why the offer was
    // concatenated ahead of the backup list: the peer's qualifiers survive, the
    // backup entry for that name is silently absorbed rather than duplicated.
    codec_string.dedup();
    println!("{}: after dedup: {}", short, codec_string);

    // retain_available() is a caller-supplied check: no ESL API exposes the loaded
    // codec implementation table, so `inventory` is this deployment's own list.
    // An entry can still survive here (its name/modname matches) and yet be
    // dropped later by the switch if none of its numeric *qualifiers* match a
    // loaded implementation. codec_string.qualified() also yields fmtp-only
    // entries, which can't be dropped this way but do change codec behaviour.
    let removed = codec_string.retain_available(inventory);
    for entry in &removed {
        warn!(
            "{}: {} not in the loaded-implementation inventory, dropped",
            short, entry
        );
    }
    for entry in codec_string.qualified() {
        info!(
            "{}: {} carries a qualifier or fmtp worth double-checking",
            short, entry
        );
    }
    println!("{}: final codec string: {}", short, codec_string);

    // absolute_codec_string takes precedence over codec_string and
    // ep_codec_string, and takes effect on the next SDP offer this leg generates.
    let name = ChannelVariable::AbsoluteCodecString;
    match set_channel_var(client, uuid, name, &codec_string.to_string()).await {
        Ok(()) => info!("{short}: {name} set: {codec_string}"),
        Err(e) => warn!("{short}: {name} not set: {e}"),
    }

    // On a leg with no bridged partner at INVITE time, this variable is the only
    // thing that puts an audio fmtp in the generated offer. It applies to one
    // payload map, so the codec that ended up first is the one to look up --
    // routinely not the offer's first -- at the rate the switch would assume.
    if let Some(first) = codec_string
        .entries()
        .first()
    {
        let rate = first
            .rate()
            .unwrap_or_else(|| default_rate(first.name()));
        if let Some(fmtp) = parsed.fmtp_for(first.name(), rate) {
            let name = ChannelVariable::RtpForceAudioFmtp;
            match set_channel_var(client, uuid, name, fmtp).await {
                Ok(()) => info!("{short}: {name} set: {fmtp}"),
                Err(e) => warn!("{short}: {name} not set: {e}"),
            }
        }
    }

    // Originating instead of live-updating is a different carrier: an inline
    // `{var=value}` block splits on a bare comma, so an unescaped one ends the
    // assignment early and stores "PCMU" with no error and no warning. Originate's
    // own Variables builder (examples/originate_examples.rs) escapes for you.
    let correct_inline = format!(
        "{{{}={}}}",
        ChannelVariable::AbsoluteCodecString,
        codec_string
            .to_string()
            .replace(',', "\\,")
    );
    let broken_inline = format!(
        "{{{}={}}}",
        ChannelVariable::AbsoluteCodecString,
        codec_string
    );
    println!(
        "{}: correct inline originate form: {}",
        short, correct_inline
    );
    println!(
        "{}: WITHOUT escaping (only the first codec survives): {}",
        short, broken_inline
    );
}

/// Enough of the UUID to correlate log lines.
///
/// Truncation is by character, not by byte: a value that came off the wire may
/// have decoded lossily, and slicing a multi-byte replacement in half panics.
fn short_uuid(uuid: &str) -> String {
    uuid.chars()
        .take(8)
        .collect()
}

/// Set a channel variable through `uuid_setvar`; `UuidSetVar` quotes the value for
/// the API's blank split, so a codec string with spaces arrives whole.
///
/// The reply is read with `api_result()`: the channel can hang up between the
/// event and this write, and `-ERR No such channel!` arrives in the body.
async fn set_channel_var(
    client: &EslClient,
    uuid: &str,
    name: ChannelVariable,
    value: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let cmd = UuidSetVar::new(uuid, name.to_string(), value)?;
    client
        .api(&cmd.to_string())
        .await?
        .api_result()?;
    Ok(())
}
