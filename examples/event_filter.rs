//! ESL Event Filter - Filter events by header values
//!
//! Usage: cargo run --example event_filter -- [OPTIONS]
//!
//! Examples:
//!   # Filter CHANNEL_CREATE events where caller ID contains "ra232"
//!   cargo run --example event_filter -- -e CHANNEL_CREATE -f Caller-Caller-ID-Number -v '/ra232/'
//!
//!   # Exact match filter
//!   cargo run --example event_filter -- -e CHANNEL_CREATE -f Caller-Caller-ID-Number -v 1001
//!
//!   # Multiple events
//!   cargo run --example event_filter -- -e CHANNEL_CREATE -e CHANNEL_ANSWER -f Call-Direction -v inbound
//!
//!   # Either of two filters: an event passes when any one of them matches
//!   cargo run --example event_filter -- -e ALL -f Event-Subclass -v sofia::register -f Event-Name -v CHANNEL_HANGUP
//!
//!   # Exit after the first 5 matching events
//!   cargo run --example event_filter -- -e CHANNEL_CREATE -c 5
//!
//!   # With userauth (user@domain format required)
//!   cargo run --example event_filter -- -u admin@default -p secret -e ALL

#[path = "common/env.rs"]
mod env;

use freeswitch_esl_tokio::connection::{AuthMethod, EslConnectOptions};
use freeswitch_esl_tokio::{
    EslClient, EslError, EslEventType, EventFormat, EventSubscription, HeaderLookup,
};
use std::time::Duration;

fn print_usage() {
    eprintln!(
        r#"ESL Event Filter - Filter FreeSWITCH events by header values

Usage: event_filter [OPTIONS]

Connection Options:
  -H, --host <HOST>        FreeSWITCH host (default: localhost)
  -P, --port <PORT>        ESL port (default: 8021)
  -p, --password <PASS>    ESL password (default: ClueCon)
  -u, --user <USER>        Username for userauth (format: user@domain)

Filter Options:
  -e, --event <EVENT>      Event type to subscribe to (can be repeated)
                           Examples: CHANNEL_CREATE, CHANNEL_ANSWER, ALL
  -f, --filter <HEADER>    Header name to filter on (can be repeated)
  -v, --value <VALUE>      Value to match (use /regex/ for regex matching);
                           pairs with the -f before it. FreeSWITCH applies the
                           filters on the switch and passes an event when any
                           one of them matches, so a CUSTOM subclass needs
                           -e ALL -f Event-Subclass -v <subclass>.
  -c, --max-count <N>      Exit after N matching events (default: run forever)
  -U, --until <EVENT>      Exit once this arrives, however many came before it.
      --until <HDR>=<VAL>  A bare word matches Event-Name; the Header=Value
                           form reaches markers that are not event names.
                           CHANNEL_DESTROY fires before the CHANNEL_STATE
                           carrying CS_DESTROY and the final hangup cause, so
                           the end of a call is Channel-State=CS_DESTROY.
  -T, --timeout <SECS>     Exit if nothing matches for this long

Output Options (default: every header, in a delimited block)
  -j, --json               One JSON object per event, body included
  -r, --raw                The event exactly as it arrived on the wire
  -q, --quiet              One summary line per event

Examples:
  # Filter CHANNEL_CREATE events where caller contains "ra232"
  event_filter -e CHANNEL_CREATE -f Caller-Caller-ID-Number -v '/ra232/'

  # Exact match on call direction
  event_filter -e CHANNEL_CREATE -f Call-Direction -v inbound

  # Multiple events with JSON output
  event_filter -e CHANNEL_CREATE -e CHANNEL_ANSWER -f Caller-Context -v public -j

  # A CUSTOM subclass alongside a core event
  event_filter -e ALL -f Event-Subclass -v sofia::register -f Event-Name -v CHANNEL_HANGUP

  # Print the next 3 matching events then exit
  event_filter -e CHANNEL_CREATE -f Call-Direction -v inbound -c 3

  # Follow one call to its end without knowing how many events that takes
  event_filter -e ALL -f Unique-ID -v <uuid> -U Channel-State=CS_DESTROY -T 60

  # With userauth (user@domain format)
  event_filter -u admin@default -p secret -e ALL

Common Header Names:
  Caller-Caller-ID-Number   Calling party number
  Caller-Caller-ID-Name     Calling party name
  Caller-Destination-Number Destination number
  Call-Direction            inbound/outbound
  Caller-Context            Dialplan context
  variable_sip_from_user    SIP From username
  variable_sip_to_user      SIP To username
  Unique-ID                 Channel UUID
"#
    );
}

#[derive(Debug)]
struct Args {
    host: String,
    port: u16,
    user: Option<String>,
    password: String,
    events: Vec<String>,
    /// Header and value pairs; the switch passes an event matching any of them.
    filters: Vec<(String, String)>,
    max_count: Option<usize>,
    /// Header and value whose arrival ends the run.
    until: Option<(String, String)>,
    idle_timeout: Option<Duration>,
    json_output: bool,
    raw_output: bool,
    quiet: bool,
}

impl Args {
    /// Defaults from the environment, before the flags below override them.
    fn from_env() -> Result<Self, String> {
        let env = env::EslEnv::from_env()?;
        Ok(Self {
            host: env.host,
            port: env.port,
            user: None,
            password: env.password,
            events: vec!["CHANNEL_CREATE".to_string()],
            filters: Vec::new(),
            max_count: None,
            until: None,
            idle_timeout: None,
            json_output: false,
            raw_output: false,
            quiet: false,
        })
    }
}

fn parse_args() -> Result<Args, String> {
    let args: Vec<String> = std::env::args().collect();
    let mut result = Args::from_env()?;
    result
        .events
        .clear();

    // A header waiting for the -v that completes its pair.
    let mut pending_header: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "-H" | "--host" => {
                i += 1;
                result.host = args
                    .get(i)
                    .ok_or("Missing host value")?
                    .clone();
            }
            "-P" | "--port" => {
                i += 1;
                result.port = args
                    .get(i)
                    .ok_or("Missing port value")?
                    .parse()
                    .map_err(|_| "Invalid port number")?;
            }
            "-p" | "--password" => {
                i += 1;
                result.password = args
                    .get(i)
                    .ok_or("Missing password value")?
                    .clone();
            }
            "-u" | "--user" => {
                i += 1;
                let user = args
                    .get(i)
                    .ok_or("Missing user value")?
                    .clone();
                if !user.contains('@') {
                    return Err(format!(
                        "Invalid user format '{}': must be user@domain (e.g., admin@default)",
                        user
                    ));
                }
                result.user = Some(user);
            }
            "-e" | "--event" => {
                i += 1;
                result
                    .events
                    .push(
                        args.get(i)
                            .ok_or("Missing event value")?
                            .clone(),
                    );
            }
            "-f" | "--filter" => {
                i += 1;
                if let Some(header) = pending_header.replace(
                    args.get(i)
                        .ok_or("Missing filter header")?
                        .clone(),
                ) {
                    return Err(format!("--filter {header} has no --value"));
                }
            }
            "-v" | "--value" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or("Missing filter value")?
                    .clone();
                let header = pending_header
                    .take()
                    .ok_or_else(|| format!("--value {value} has no --filter before it"))?;
                result
                    .filters
                    .push((header, value));
            }
            "-c" | "--max-count" => {
                i += 1;
                let count: usize = args
                    .get(i)
                    .ok_or("Missing max-count value")?
                    .parse()
                    .map_err(|_| "Invalid max-count: expected a positive integer")?;
                if count == 0 {
                    return Err("--max-count must be at least 1".to_string());
                }
                result.max_count = Some(count);
            }
            "-U" | "--until" => {
                i += 1;
                let spec = args
                    .get(i)
                    .ok_or("Missing until value")?;
                // A bare word is the common case and means an event name; the
                // Header=Value form reaches the terminal markers that are not
                // ones, such as Channel-State=CS_DESTROY.
                result.until = Some(match spec.split_once('=') {
                    Some((header, value)) => (header.to_string(), value.to_string()),
                    None => ("Event-Name".to_string(), spec.to_uppercase()),
                });
            }
            "-T" | "--timeout" => {
                i += 1;
                let secs: u64 = args
                    .get(i)
                    .ok_or("Missing timeout value")?
                    .parse()
                    .map_err(|_| "Invalid timeout: expected whole seconds")?;
                if secs == 0 {
                    return Err("--timeout must be at least 1 second".to_string());
                }
                result.idle_timeout = Some(Duration::from_secs(secs));
            }
            "-j" | "--json" => {
                result.json_output = true;
            }
            "-r" | "--raw" => {
                result.raw_output = true;
            }
            "-q" | "--quiet" => {
                result.quiet = true;
            }
            arg => {
                return Err(format!("Unknown argument: {}", arg));
            }
        }
        i += 1;
    }

    if result
        .events
        .is_empty()
    {
        result
            .events
            .push("CHANNEL_CREATE".to_string());
    }

    if let Some(header) = pending_header {
        return Err(format!("--filter {header} has no --value"));
    }

    Ok(result)
}

/// Enough of the UUID to correlate lines, truncated by character: a value off
/// the wire may have decoded lossily, and slicing a replacement in half panics.
fn short_uuid(uuid: &str) -> String {
    uuid.chars()
        .take(8)
        .collect()
}

fn format_event_summary(event: &freeswitch_esl_tokio::EslEvent) -> String {
    let event_name = event
        .event_type()
        .map(|t| t.to_string())
        .unwrap_or_else(|| "UNKNOWN".into());
    let caller_id = event
        .caller_id_number()
        .unwrap_or("-");
    let dest = event
        .destination_number()
        .unwrap_or("-");
    let direction = match event.call_direction() {
        Ok(Some(d)) => d.to_string(),
        Ok(None) => "-".into(),
        Err(e) => format!("!ERR({e})"),
    };
    let uuid = event
        .unique_id()
        .map_or_else(|| "-".to_string(), short_uuid);

    format!("[{event_name}] {caller_id} -> {dest} ({direction}) uuid:{uuid}...")
}

fn format_event_full(event: &freeswitch_esl_tokio::EslEvent) -> String {
    use std::fmt::Write;

    let mut output = String::from("---EVENT---\n");
    for (key, value) in event.headers() {
        // Writing to a String cannot fail, so there is no error to handle.
        let _ = writeln!(output, "{key}: {value}");
    }
    if let Some(body) = event.body() {
        let _ = writeln!(output, "\n{body}");
    }
    output.push_str("-----------\n");
    output
}

/// The whole event, body included. Serializing only the headers would drop the
/// payload that DTMF and BACKGROUND_JOB events carry, which the other output
/// modes print.
fn format_event_json(event: &freeswitch_esl_tokio::EslEvent) -> Result<String, serde_json::Error> {
    serde_json::to_string(event)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("Error: {}", e);
            eprintln!();
            print_usage();
            std::process::exit(1);
        }
    };

    if let Some(ref user) = args.user {
        eprintln!(
            "Connecting to FreeSWITCH at {}:{} as {}...",
            args.host, args.port, user
        );
    } else {
        eprintln!("Connecting to FreeSWITCH at {}:{}...", args.host, args.port);
    }

    let auth = match args.user {
        Some(ref user) => AuthMethod::user(user, &args.password),
        None => AuthMethod::password(&args.password),
    };
    let connect_result =
        EslClient::connect_with_auth(&args.host, args.port, auth, EslConnectOptions::new()).await;

    let (client, mut events) = match connect_result {
        Ok(pair) => pair,
        Err(EslError::AuthenticationFailed { ref reason }) => {
            eprintln!("Authentication failed: {}", reason);
            std::process::exit(1);
        }
        // An ACL rejection arrives as text/rude-rejection before auth, so it
        // reads as a connection failure unless it is named.
        Err(EslError::AccessDenied { ref reason }) => {
            eprintln!("Rejected by the ESL ACL: {}", reason);
            std::process::exit(1);
        }
        Err(EslError::Io(ref e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            eprintln!(
                "Connection refused - is FreeSWITCH running on {}:{}?",
                args.host, args.port
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Failed to connect: {}", e);
            std::process::exit(1);
        }
    };

    eprintln!("Connected successfully");

    // FromStr carries the crate's own error rather than a stringified one.
    let event_types = match args
        .events
        .iter()
        .map(|name| name.parse::<EslEventType>())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(types) => types,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    let format = if args.json_output {
        EventFormat::Json
    } else {
        EventFormat::Plain
    };

    // Build an EventSubscription combining events and filters into one unit
    let mut sub = EventSubscription::new(format).events(event_types);
    for (header, value) in &args.filters {
        sub = sub.filter_raw(header, value)?;
    }
    eprintln!("Subscribing to events: {:?}", args.events);
    for (header, value) in &args.filters {
        eprintln!("  filter {header}={value}");
    }
    client
        .apply_subscription(&sub)
        .await?;

    // A terminal event that was never subscribed to can never arrive, so the
    // run would sit until its timeout for a reason the flags already show.
    if let Some((header, value)) = &args.until {
        if header.eq_ignore_ascii_case("Event-Name")
            && !args
                .events
                .iter()
                .any(|e| e.eq_ignore_ascii_case(value) || e.eq_ignore_ascii_case("ALL"))
        {
            eprintln!("Warning: --until {value} is not among the subscribed events");
        }
    }

    match (&args.max_count, &args.until) {
        (_, Some((header, value))) => {
            eprintln!("Listening until {header}={value}... (Ctrl+C to exit early)\n")
        }
        (Some(n), None) => eprintln!("Listening for {n} event(s)... (Ctrl+C to exit early)\n"),
        (None, None) => eprintln!("Listening for events... (Ctrl+C to exit)\n"),
    }

    let mut matched = 0usize;
    let mut reached_max = false;
    let mut saw_until = false;
    let mut timed_out = false;

    loop {
        let next = match args.idle_timeout {
            Some(limit) => match tokio::time::timeout(limit, events.recv()).await {
                Ok(next) => next,
                Err(_) => {
                    timed_out = true;
                    break;
                }
            },
            None => {
                events
                    .recv()
                    .await
            }
        };
        let Some(result) = next else {
            break;
        };
        let event = match result {
            Ok(event) => event,
            Err(e) => {
                eprintln!("Event error: {}", e);
                continue;
            }
        };
        let output = if args.json_output {
            format_event_json(&event)?
        } else if args.raw_output {
            // The wire form the reader would see on the socket, not a rendering
            // of it: that is what makes --raw worth having.
            event.to_plain_format()
        } else if args.quiet {
            format_event_summary(&event)
        } else {
            format_event_full(&event)
        };
        println!("{output}");

        matched += 1;
        if let Some((header, value)) = &args.until {
            if event
                .header_str(header)
                .is_some_and(|found| found.eq_ignore_ascii_case(value))
            {
                saw_until = true;
                break;
            }
        }
        if args
            .max_count
            .is_some_and(|max| matched >= max)
        {
            reached_max = true;
            break;
        }
    }

    if saw_until {
        eprintln!("Stopped on --until after {matched} event(s)");
    } else if reached_max {
        eprintln!("Reached max count of {} event(s)", matched);
    } else if timed_out {
        eprintln!("Nothing matched for the timeout; saw {matched} event(s)");
    } else {
        eprintln!("Connection closed by server");
    }
    client
        .disconnect()
        .await?;

    Ok(())
}
