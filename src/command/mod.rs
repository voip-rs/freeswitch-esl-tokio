//! Command execution and response handling

// qual:allow(coupling, sdp) reason: "channel dumps reuse the protocol decoder"
use crate::{
    constants::{FILTER_DELETE_ALL, HEADER_TERMINATOR, LINE_TERMINATOR},
    error::{EslError, EslResult},
    event::FilterHeaderFault,
    event::{EslEvent, EventFormat},
    headers::EventHeader,
    LogLevel,
};
pub(crate) use freeswitch_types::wire_safety::contains_wire_terminator;
use indexmap::IndexMap;
use std::borrow::Cow;
use std::fmt;
use std::time::Duration;

mod response;
pub use response::{
    parse_api_body, parse_channel_dump, parse_channel_dump_with_options, ChannelDumpOptions,
    EslResponse, ReplyStatus,
};

/// Wraps a string so `Debug` prints `[REDACTED]` instead of the value.
///
/// Used for password fields in [`EslCommand`] to prevent accidental exposure
/// in debug logs. The inner value is not accessible from external crates.
#[derive(Clone)]
pub struct Secret(pub(crate) String);

impl Secret {
    /// Wrap a password so [`EslCommand::Auth`] and [`EslCommand::UserAuth`] can
    /// be built for [`send_command`](crate::EslClient::send_command).
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Refuse a user-provided string carrying a line break, which injects a
/// command, or a NUL, which cuts one short.
fn validate_wire_text(s: &str, field: &'static str) -> EslResult<()> {
    if contains_wire_terminator(s) {
        return Err(EslError::WireTerminatorInField { field });
    }
    Ok(())
}

/// Refuse a filter header carrying the `fault` that [`FilterHeaderFault`] found.
fn refuse_filter_header(header: &str, fault: Option<FilterHeaderFault>) -> EslResult<()> {
    let Some(fault) = fault else {
        return Ok(());
    };
    Err(EslError::UnsplittableFilterHeader {
        header: header.to_string(),
        fault,
    })
}

/// Builder for custom ESL commands not covered by [`EslClient`](crate::EslClient) methods.
///
/// Produces the wire-format string including headers and optional body.
///
/// ```
/// use freeswitch_esl_tokio::CommandBuilder;
///
/// let cmd = CommandBuilder::new("mycommand")
///     .header("X-Custom", "value").unwrap()
///     .body("payload data")
///     .build();
/// assert!(cmd.starts_with("mycommand\n"));
/// assert!(cmd.contains("X-Custom: value"));
/// assert!(cmd.contains("Content-Length: 12"));
/// ```
#[derive(Debug)]
pub struct CommandBuilder {
    command: String,
    headers: IndexMap<String, String>,
    body: Option<String>,
}

impl CommandBuilder {
    /// Start building a command with the given command line.
    pub fn new(command: &str) -> Self {
        Self {
            command: command.to_string(),
            headers: IndexMap::new(),
            body: None,
        }
    }

    /// Add header to command.
    ///
    /// Returns an error if the name or value contains a line break or NUL.
    pub fn header(mut self, name: &str, value: &str) -> EslResult<Self> {
        validate_wire_text(name, "header name")?;
        validate_wire_text(value, "header value")?;
        self.headers
            .insert(name.to_string(), value.to_string());
        Ok(self)
    }

    /// Set command body.
    ///
    /// The body is length-delimited so it may contain newlines.
    pub fn body(mut self, body: &str) -> Self {
        self.body = Some(body.to_string());
        self
    }

    /// Build the command string
    pub fn build(self) -> String {
        let mut result = self.command;
        result.push_str(LINE_TERMINATOR);

        for (key, value) in &self.headers {
            result.extend([key.as_str(), ": ", value.as_str(), LINE_TERMINATOR]);
        }

        if let Some(body) = &self.body {
            let length = body
                .len()
                .to_string();
            result.extend([
                "Content-Length: ",
                &length,
                LINE_TERMINATOR,
                LINE_TERMINATOR,
                body,
            ]);
        } else {
            result.push_str(LINE_TERMINATOR);
        }

        result
    }
}

/// Options for `sendmsg execute` commands.
///
/// Controls optional headers that modify execution behavior in outbound
/// ESL mode (socket application with `async full`).
#[derive(Debug, Clone, Default)]
pub struct ExecuteOptions {
    event_lock: bool,
    async_mode: bool,
    loops: Option<u32>,
}

impl ExecuteOptions {
    /// Create default options (no event-lock, no async, no loops).
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable the `Event-Lock` header.
    pub fn with_event_lock(mut self) -> Self {
        self.event_lock = true;
        self
    }

    /// Enable the `async` header.
    pub fn with_async(mut self) -> Self {
        self.async_mode = true;
        self
    }

    /// Set the `loops` header.
    pub fn with_loops(mut self, count: u32) -> Self {
        self.loops = Some(count);
        self
    }

    /// Lock the event queue during execution so events are serialized
    /// with the application (prevents race conditions on fast-executing apps).
    pub fn event_lock(&self) -> bool {
        self.event_lock
    }

    /// Return immediately instead of waiting for the application to finish.
    /// Only meaningful in outbound `async full` mode.
    pub fn async_mode(&self) -> bool {
        self.async_mode
    }

    /// Repeat the application N times.
    pub fn loops(&self) -> Option<u32> {
        self.loops
    }
}

/// Argument of the outbound-mode `divert_events` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DivertMode {
    /// Hand the session's queued events to this socket instead of its running
    /// application, and queue its DTMF there rather than fire it to every listener.
    On,
    /// Restore both. The switch reads any value but `on` as this.
    Off,
}

impl DivertMode {
    fn as_str(self) -> &'static str {
        match self {
            DivertMode::On => "on",
            DivertMode::Off => "off",
        }
    }
}

/// ESL command types for the wire protocol.
///
/// Most users won't construct these directly -- use [`EslClient`](crate::EslClient)
/// methods or [`AppCommand`](crate::AppCommand) instead. This enum is public so
/// that [`send_command()`](crate::EslClient::send_command) callers can name the type.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum EslCommand {
    /// Authenticate with password.
    Auth {
        /// ESL password (from `event_socket.conf.xml`).
        password: Secret,
    },
    /// Authenticate with user and password.
    UserAuth {
        /// Username (e.g. `admin@default`).
        user: String,
        /// Password for this user.
        password: Secret,
    },
    /// Execute API command.
    Api {
        /// Full command string (e.g. `"status"`, `"sofia status"`).
        command: String,
    },
    /// Execute background API command.
    BgApi {
        /// Full command string.
        command: String,
    },
    /// Subscribe to events.
    Events {
        /// Event format.
        format: EventFormat,
        /// Space-separated event names or `ALL`.
        events: String,
    },
    /// Set event filters.
    Filter {
        /// Header name to filter on.
        header: String,
        /// Required header value.
        value: String,
    },
    /// Send message to channel.
    SendMsg {
        /// Target channel UUID (omit in outbound mode).
        uuid: Option<String>,
        /// Event containing sendmsg headers and optional body.
        event: EslEvent,
    },
    /// Execute application on channel.
    Execute {
        /// Application name (e.g. `"answer"`, `"playback"`).
        app: String,
        /// Application arguments.
        args: Option<String>,
        /// Target channel UUID (omit in outbound mode).
        uuid: Option<String>,
        /// Optional execution flags (event-lock, async, loops).
        options: ExecuteOptions,
    },
    /// Exit/logout.
    Exit,
    /// Enable log forwarding at the given level.
    Log {
        /// Lowest severity forwarded.
        level: LogLevel,
    },
    /// Disable log forwarding.
    NoLog,
    /// No operation / keepalive.
    NoOp,
    /// Fire an event into FreeSWITCH's event bus.
    SendEvent {
        /// Event to fire.
        event: EslEvent,
    },
    /// Subscribe to session events (outbound: no uuid, inbound: with uuid).
    MyEvents {
        /// Event format.
        format: EventFormat,
        /// Channel UUID for inbound mode; omit for outbound.
        uuid: Option<String>,
    },
    /// Keep socket open after channel hangup.
    Linger {
        /// Linger timeout, or `None` for indefinite.
        timeout: Option<Duration>,
    },
    /// Cancel linger mode.
    NoLinger,
    /// Resume dialplan execution on socket disconnect.
    Resume,
    /// Unsubscribe from specific events.
    NixEvent {
        /// Space-separated event names.
        events: String,
    },
    /// Unsubscribe from all events.
    NoEvents,
    /// Remove event filters.
    FilterDelete {
        /// Header name to remove filter from. `all` in any case is refused
        /// with [`EslError::FilterDeleteAllHeader`]: send
        /// [`FilterDeleteAll`](Self::FilterDeleteAll).
        header: String,
        /// Specific value to remove, or `None` for all values.
        value: Option<String>,
    },
    /// Redirect session events to ESL (outbound mode).
    DivertEvents {
        /// Whether to divert.
        mode: DivertMode,
    },
    /// Read a channel variable (outbound mode).
    GetVar {
        /// Variable name.
        name: String,
    },
    /// Request channel data in outbound mode.
    Connect,
    /// Remove all event filters.
    FilterDeleteAll,
}

impl EslCommand {
    /// Format a simple command with optional arguments
    fn format_simple_command(cmd: &str, args: &[&str]) -> String {
        let mut result = String::from(cmd);
        for arg in args {
            result.push(' ');
            result.push_str(arg);
        }
        result.push_str(HEADER_TERMINATOR);
        result
    }

    /// Build a command carrying an event as its header block and body.
    fn build_from_event(cmd: &str, event: &EslEvent) -> EslResult<String> {
        let mut builder = CommandBuilder::new(cmd);

        for (key, value) in event.headers() {
            builder = builder.header(key, value)?;
        }

        if let Some(body) = event.body() {
            builder = builder.body(body);
        }

        Ok(builder.build())
    }

    /// The `Event-Name` a `sendevent` puts on its command line.
    fn sendevent_name(event: &EslEvent) -> EslResult<String> {
        event
            .event_type()
            .map(|t| t.to_string())
            .or_else(|| {
                event
                    .header(EventHeader::EventName)
                    .map(|s| s.to_string())
            })
            .ok_or(EslError::MissingField {
                field: "sendevent event name (event type or Event-Name header)",
            })
    }

    /// Reject every user-supplied field that would break the line framing.
    ///
    /// Runs before any formatting so no half-built command reaches the wire.
    fn validate(&self) -> EslResult<()> {
        match self {
            EslCommand::Auth { password } => validate_wire_text(&password.0, "password"),
            EslCommand::UserAuth { user, password } => {
                validate_wire_text(user, "user")?;
                validate_wire_text(&password.0, "password")
            }
            EslCommand::Api { command } => validate_wire_text(command, "api command"),
            EslCommand::BgApi { command } => validate_wire_text(command, "bgapi command"),
            EslCommand::Events { events, .. } => validate_wire_text(events, "event list"),
            EslCommand::Filter { header, value } => {
                validate_wire_text(header, "filter header")?;
                validate_wire_text(value, "filter value")?;
                refuse_filter_header(header, FilterHeaderFault::of_filter(header))
            }
            EslCommand::SendMsg { uuid, .. } => match uuid {
                Some(u) => validate_wire_text(u, "sendmsg uuid"),
                None => Ok(()),
            },
            EslCommand::Execute {
                app, args, uuid, ..
            } => {
                validate_wire_text(app, "execute app")?;
                if let Some(a) = args {
                    validate_wire_text(a, "execute args")?;
                }
                match uuid {
                    Some(u) => validate_wire_text(u, "execute uuid"),
                    None => Ok(()),
                }
            }
            EslCommand::SendEvent { event } => {
                validate_wire_text(&Self::sendevent_name(event)?, "sendevent event name")
            }
            EslCommand::MyEvents { uuid, .. } => match uuid {
                Some(u) => validate_wire_text(u, "myevents uuid"),
                None => Ok(()),
            },
            EslCommand::NixEvent { events } => validate_wire_text(events, "nixevent list"),
            EslCommand::FilterDelete { header, value } => {
                validate_wire_text(header, "filter delete header")?;
                refuse_filter_header(header, FilterHeaderFault::of_filter_delete(header))?;
                if header.eq_ignore_ascii_case(FILTER_DELETE_ALL) {
                    return Err(EslError::FilterDeleteAllHeader {
                        header: header.clone(),
                    });
                }
                match value {
                    Some(v) => validate_wire_text(v, "filter delete value"),
                    None => Ok(()),
                }
            }
            EslCommand::GetVar { name } => validate_wire_text(name, "getvar name"),
            EslCommand::Exit
            | EslCommand::Log { .. }
            | EslCommand::NoLog
            | EslCommand::NoOp
            | EslCommand::Linger { .. }
            | EslCommand::NoLinger
            | EslCommand::Resume
            | EslCommand::NoEvents
            | EslCommand::DivertEvents { .. }
            | EslCommand::Connect
            | EslCommand::FilterDeleteAll => Ok(()),
        }
    }

    /// Validate all user-supplied fields, then convert to wire format.
    pub fn to_wire_format(&self) -> EslResult<String> {
        self.validate()?;
        match self {
            EslCommand::Auth { password } => {
                Ok(Self::format_simple_command("auth", &[&password.0]))
            }
            EslCommand::UserAuth { user, password } => Ok(Self::format_simple_command(
                "userauth",
                &[&format!("{}:{}", user, password.0)],
            )),
            EslCommand::Api { command } => Ok(Self::format_simple_command("api", &[command])),
            EslCommand::BgApi { command } => Ok(Self::format_simple_command("bgapi", &[command])),
            EslCommand::Events { format, events } => Ok(Self::format_simple_command(
                "event",
                &[format.as_str(), events],
            )),
            EslCommand::Filter { header, value } => {
                Ok(Self::format_simple_command("filter", &[header, value]))
            }
            EslCommand::SendMsg { uuid, event } => {
                let cmd_str = format!(
                    "sendmsg{}",
                    uuid.as_ref()
                        .map(|u| format!(" {}", u))
                        .unwrap_or_default()
                );
                Self::build_from_event(&cmd_str, event)
            }
            EslCommand::Execute {
                app,
                args,
                uuid,
                options,
            } => {
                let mut event = EslEvent::new();
                event.set_header("call-command", "execute");
                event.set_header("execute-app-name", app.clone());

                if let Some(args) = args {
                    event.set_header("execute-app-arg", args.clone());
                }

                if options.event_lock {
                    event.set_header("event-lock", "true");
                }
                if options.async_mode {
                    event.set_header("async", "true");
                }
                if let Some(loops) = options.loops {
                    event.set_header("loops", loops.to_string());
                }

                EslCommand::SendMsg {
                    uuid: uuid.clone(),
                    event,
                }
                .to_wire_format()
            }
            EslCommand::Exit => Ok(Self::format_simple_command("exit", &[])),
            EslCommand::Log { level } => Ok(Self::format_simple_command("log", &[level.as_str()])),
            EslCommand::NoLog => Ok(Self::format_simple_command("nolog", &[])),
            EslCommand::NoOp => Ok(Self::format_simple_command("noop", &[])),
            EslCommand::SendEvent { event } => Self::build_from_event(
                &format!("sendevent {}", Self::sendevent_name(event)?),
                event,
            ),
            EslCommand::MyEvents { format, uuid } => Ok(match uuid {
                Some(u) => Self::format_simple_command("myevents", &[u, format.as_str()]),
                None => Self::format_simple_command("myevents", &[format.as_str()]),
            }),
            EslCommand::Linger { timeout } => Ok(match timeout {
                Some(d) => Self::format_simple_command(
                    "linger",
                    &[&d.as_secs()
                        .to_string()],
                ),
                None => Self::format_simple_command("linger", &[]),
            }),
            EslCommand::NoLinger => Ok(Self::format_simple_command("nolinger", &[])),
            EslCommand::Resume => Ok(Self::format_simple_command("resume", &[])),
            EslCommand::NixEvent { events } => {
                Ok(Self::format_simple_command("nixevent", &[events]))
            }
            EslCommand::NoEvents => Ok(Self::format_simple_command("noevents", &[])),
            EslCommand::FilterDelete { header, value } => Ok(match value {
                Some(v) => Self::format_simple_command("filter", &["delete", header, v]),
                None => Self::format_simple_command("filter", &["delete", header]),
            }),
            EslCommand::FilterDeleteAll => Ok(Self::format_simple_command(
                "filter",
                &["delete", FILTER_DELETE_ALL],
            )),
            EslCommand::DivertEvents { mode } => Ok(Self::format_simple_command(
                "divert_events",
                &[mode.as_str()],
            )),
            EslCommand::GetVar { name } => Ok(Self::format_simple_command("getvar", &[name])),
            EslCommand::Connect => Ok(Self::format_simple_command("connect", &[])),
        }
    }

    /// Return a log-safe version of the wire-format string.
    ///
    /// Passwords are replaced with `[REDACTED]`. The mandatory `\n\n` wire
    /// terminator is stripped so the result fits on one log line; all other
    /// content is preserved verbatim. Non-sensitive commands borrow the input
    /// (zero allocation).
    pub fn redact_wire<'a>(&self, wire: &'a str) -> Cow<'a, str> {
        match self {
            EslCommand::Auth { .. } => Cow::Owned("auth [REDACTED]".into()),
            EslCommand::UserAuth { user, .. } => {
                Cow::Owned(format!("userauth {}:[REDACTED]", user))
            }
            _ => Cow::Borrowed(
                wire.strip_suffix(HEADER_TERMINATOR)
                    .unwrap_or(wire),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EslEventType;

    #[test]
    fn test_command_builder() {
        let cmd = CommandBuilder::new("api status")
            .header("Custom-Header", "value")
            .unwrap()
            .body("test body")
            .build();

        assert!(cmd.contains("api status"));
        assert!(cmd.contains("Custom-Header: value"));
        assert!(cmd.contains("Content-Length: 9"));
        assert!(cmd.contains("test body"));
    }

    /// Every command whose wire form is one line with no body.
    #[test]
    fn single_line_wire_formats() {
        let cases: &[(EslCommand, &str)] = &[
            (
                EslCommand::Auth {
                    password: Secret::new("test"),
                },
                "auth test\n\n",
            ),
            (
                EslCommand::UserAuth {
                    user: "admin@default".to_string(),
                    password: Secret::new("secret123"),
                },
                "userauth admin@default:secret123\n\n",
            ),
            (
                EslCommand::Api {
                    command: "status".to_string(),
                },
                "api status\n\n",
            ),
            (
                EslCommand::BgApi {
                    command: "status".to_string(),
                },
                "bgapi status\n\n",
            ),
            (
                EslCommand::Events {
                    format: EventFormat::Plain,
                    events: "ALL".to_string(),
                },
                "event plain ALL\n\n",
            ),
            (
                EslCommand::Filter {
                    header: "Event-Name".to_string(),
                    value: "CHANNEL_CREATE".to_string(),
                },
                "filter Event-Name CHANNEL_CREATE\n\n",
            ),
            (
                EslCommand::MyEvents {
                    format: EventFormat::Plain,
                    uuid: None,
                },
                "myevents plain\n\n",
            ),
            (
                EslCommand::MyEvents {
                    format: EventFormat::Json,
                    uuid: Some("abc-123".to_string()),
                },
                "myevents abc-123 json\n\n",
            ),
            (EslCommand::Linger { timeout: None }, "linger\n\n"),
            (
                EslCommand::Linger {
                    timeout: Some(Duration::from_secs(600)),
                },
                "linger 600\n\n",
            ),
            (EslCommand::NoLinger, "nolinger\n\n"),
            (EslCommand::Resume, "resume\n\n"),
            (
                EslCommand::NixEvent {
                    events: "CHANNEL_CREATE CHANNEL_DESTROY".to_string(),
                },
                "nixevent CHANNEL_CREATE CHANNEL_DESTROY\n\n",
            ),
            (EslCommand::NoEvents, "noevents\n\n"),
            (
                EslCommand::FilterDelete {
                    header: "Event-Name".to_string(),
                    value: None,
                },
                "filter delete Event-Name\n\n",
            ),
            (
                EslCommand::FilterDelete {
                    header: "Event-Name".to_string(),
                    value: Some("CHANNEL_CREATE".to_string()),
                },
                "filter delete Event-Name CHANNEL_CREATE\n\n",
            ),
            (
                EslCommand::DivertEvents {
                    mode: DivertMode::On,
                },
                "divert_events on\n\n",
            ),
            (
                EslCommand::DivertEvents {
                    mode: DivertMode::Off,
                },
                "divert_events off\n\n",
            ),
            (
                EslCommand::GetVar {
                    name: "caller_id_name".to_string(),
                },
                "getvar caller_id_name\n\n",
            ),
            (
                EslCommand::Log {
                    level: LogLevel::Debug,
                },
                "log debug\n\n",
            ),
            (EslCommand::NoLog, "nolog\n\n"),
            (EslCommand::NoOp, "noop\n\n"),
            (EslCommand::Exit, "exit\n\n"),
            (EslCommand::Connect, "connect\n\n"),
        ];

        for (cmd, expected) in cases {
            assert_eq!(
                cmd.to_wire_format()
                    .unwrap(),
                *expected,
                "{cmd:?}"
            );
        }
    }

    #[test]
    fn test_app_commands() {
        use crate::app::dptools::AppCommand;

        let answer = AppCommand::answer()
            .to_wire_format()
            .unwrap();
        assert!(answer.contains("Execute-App-Name: answer"));

        let hangup = AppCommand::hangup(Some(crate::channel::HangupCause::NormalClearing))
            .to_wire_format()
            .unwrap();
        assert!(hangup.contains("Execute-App-Name: hangup"));
        assert!(hangup.contains("Execute-App-Arg: NORMAL_CLEARING"));
    }

    #[test]
    fn test_execute_with_options_wire_format() {
        let cmd = EslCommand::Execute {
            app: "playback".to_string(),
            args: Some("tone_stream://%(200,100,440)".to_string()),
            uuid: Some("abc-123".to_string()),
            options: ExecuteOptions::new()
                .with_event_lock()
                .with_async()
                .with_loops(3),
        };
        let wire = cmd
            .to_wire_format()
            .unwrap();
        assert!(wire.contains("Event-Lock: true"));
        assert!(wire.contains("Async: true"));
        assert!(wire.contains("Loops: 3"));
        assert!(wire.contains("Execute-App-Name: playback"));
    }

    #[test]
    fn test_execute_default_options_no_extra_headers() {
        let cmd = EslCommand::Execute {
            app: "answer".to_string(),
            args: None,
            uuid: None,
            options: ExecuteOptions::default(),
        };
        let wire = cmd
            .to_wire_format()
            .unwrap();
        assert!(!wire.contains("event-lock"));
        assert!(!wire.contains("async"));
        assert!(!wire.contains("loops"));
    }

    #[test]
    fn test_sendevent_wire_format() {
        let mut event = EslEvent::with_type(EslEventType::Custom);
        event.set_header("Event-Name", "CUSTOM");
        event.set_header("Event-Subclass", "my::test_event");

        let cmd = EslCommand::SendEvent { event };
        let wire = cmd
            .to_wire_format()
            .unwrap();

        assert!(wire.starts_with("sendevent CUSTOM\n"));
        assert!(wire.contains("Event-Name: CUSTOM\n"));
        assert!(wire.contains("Event-Subclass: my::test_event\n"));
        assert!(wire.ends_with("\n\n"));
    }

    #[test]
    fn test_sendevent_wire_format_with_body() {
        let mut event = EslEvent::with_type(EslEventType::Custom);
        event.set_header("Event-Name", "CUSTOM");
        event.set_body("hello world".to_string());

        let cmd = EslCommand::SendEvent { event };
        let wire = cmd
            .to_wire_format()
            .unwrap();

        assert!(wire.starts_with("sendevent CUSTOM\n"));
        assert!(wire.contains("Content-Length: 11\n"));
        assert!(wire.ends_with("hello world"));
    }

    /// mod_event_socket compares the header to `all` with `strcasecmp` and
    /// clears every filter, whatever value follows.
    #[test]
    fn filter_delete_refuses_the_all_header_in_any_case() {
        for header in ["all", "ALL", "All"] {
            for value in [None, Some("CHANNEL_CREATE".to_string())] {
                let err = EslCommand::FilterDelete {
                    header: header.to_string(),
                    value: value.clone(),
                }
                .to_wire_format()
                .expect_err(header);
                assert!(
                    matches!(&err, EslError::FilterDeleteAllHeader { header: h } if h == header),
                    "{err}"
                );
                assert!(!err.is_recoverable(), "{header} {value:?}: {err}");
                assert!(!err.is_connection_error(), "{header} {value:?}: {err}");
            }
        }
    }

    /// mod_event_socket cuts a filter header at its first space, takes
    /// `add` and `delete` as keywords before it, and leaves an empty one
    /// empty: each would act on some other header than the one named.
    #[test]
    fn filter_headers_the_switch_would_split_are_refused() {
        let filters = [
            ("Event Name", "x"),
            (" Event-Name", "x"),
            ("", "x"),
            ("add", "Event-Name CHANNEL_CREATE"),
            ("DELETE", "Event-Name"),
        ];
        for (header, value) in filters {
            let err = EslCommand::Filter {
                header: header.to_string(),
                value: value.to_string(),
            }
            .to_wire_format()
            .expect_err(header);
            assert!(
                matches!(err, EslError::UnsplittableFilterHeader { .. }),
                "{header:?}: {err}"
            );
            assert!(!err.is_recoverable(), "{header:?}: {err}");
            assert!(!err.is_connection_error(), "{header:?}: {err}");
        }
        for header in ["Event Name", "", " Event-Name"] {
            let err = EslCommand::FilterDelete {
                header: header.to_string(),
                value: None,
            }
            .to_wire_format()
            .expect_err(header);
            assert!(
                matches!(err, EslError::UnsplittableFilterHeader { .. }),
                "{header:?}: {err}"
            );
            assert!(!err.is_recoverable(), "{header:?}: {err}");
            assert!(!err.is_connection_error(), "{header:?}: {err}");
        }
        for header in ["add", "delete"] {
            assert!(
                EslCommand::FilterDelete {
                    header: header.to_string(),
                    value: None,
                }
                .to_wire_format()
                .is_ok(),
                "after delete, {header} is a header name"
            );
        }
    }

    #[test]
    fn test_filter_delete_all_wire_format() {
        assert_eq!(
            EslCommand::FilterDeleteAll
                .to_wire_format()
                .unwrap(),
            "filter delete all\n\n"
        );
    }

    #[test]
    fn test_sendevent_event_name_only_no_typed_variant() {
        // Event-Name set as a raw header (no event_type) is accepted —
        // the wire-format serializer falls back to the header.
        let mut event = EslEvent::new();
        event.set_header("Event-Name", "CUSTOM");

        let cmd = EslCommand::SendEvent { event };
        let wire = cmd
            .to_wire_format()
            .unwrap();
        assert!(wire.starts_with("sendevent CUSTOM\n"));
    }

    #[test]
    fn test_sendevent_without_event_name_errors() {
        // Bare EslEvent with no event_type and no Event-Name header is
        // a hard error — we refuse to silently mislabel as "CUSTOM".
        let bare_event = EslEvent::new();
        let cmd = EslCommand::SendEvent { event: bare_event };
        let err = cmd
            .to_wire_format()
            .unwrap_err();
        assert!(matches!(err, EslError::MissingField { .. }), "{err}");
        assert!(!err.is_connection_error(), "{err}");
        assert!(!err.is_recoverable(), "{err}");
    }

    #[test]
    fn test_newline_injection_rejected() {
        let api = EslCommand::Api {
            command: "status\n\nevent plain ALL".to_string(),
        };
        assert!(api
            .to_wire_format()
            .is_err());

        let auth = EslCommand::Auth {
            password: Secret("test\napi status".to_string()),
        };
        assert!(auth
            .to_wire_format()
            .is_err());

        let filter = EslCommand::Filter {
            header: "Event-Name\r\n".to_string(),
            value: "CHANNEL_CREATE".to_string(),
        };
        assert!(filter
            .to_wire_format()
            .is_err());
    }

    #[test]
    fn test_debug_redacts_password() {
        let auth = EslCommand::Auth {
            password: Secret("secret".to_string()),
        };
        let debug_str = format!("{:?}", auth);
        assert!(!debug_str.contains("secret"));
        assert!(debug_str.contains("REDACTED"));

        let user_auth = EslCommand::UserAuth {
            user: "admin@default".to_string(),
            password: Secret("secret".to_string()),
        };
        let debug_str = format!("{:?}", user_auth);
        assert!(!debug_str.contains("secret"));
        assert!(debug_str.contains("admin@default"));
        assert!(debug_str.contains("REDACTED"));
    }

    #[test]
    fn test_user_auth_newline_in_user_rejected() {
        let cmd = EslCommand::UserAuth {
            user: "admin\n@default".to_string(),
            password: Secret("pass".to_string()),
        };
        assert!(cmd
            .to_wire_format()
            .is_err());
    }

    #[test]
    fn test_user_auth_newline_in_password_rejected() {
        let cmd = EslCommand::UserAuth {
            user: "admin@default".to_string(),
            password: Secret("pass\nword".to_string()),
        };
        assert!(cmd
            .to_wire_format()
            .is_err());
    }

    #[test]
    fn test_redact_wire_sendmsg() {
        let mut event = EslEvent::new();
        event.set_header("call-command", "execute");
        event.set_header("execute-app-name", "answer");
        let cmd = EslCommand::SendMsg {
            uuid: Some("abc-123".to_string()),
            event,
        };
        let wire = cmd
            .to_wire_format()
            .unwrap();
        let redacted = cmd.redact_wire(&wire);
        // SendMsg is not sensitive, wire content is preserved (minus terminator)
        assert!(redacted.contains("sendmsg"));
        assert!(redacted.contains("Execute-App-Name: answer"));
        assert!(!redacted.ends_with("\n\n"));
    }

    #[test]
    fn test_redact_wire_sendevent() {
        let mut event = EslEvent::with_type(EslEventType::Custom);
        event.set_header("Event-Name", "CUSTOM");
        event.set_header("Event-Subclass", "test::redact");
        let cmd = EslCommand::SendEvent { event };
        let wire = cmd
            .to_wire_format()
            .unwrap();
        let redacted = cmd.redact_wire(&wire);
        assert!(redacted.contains("sendevent"));
        assert!(!redacted.ends_with("\n\n"));
    }

    #[test]
    fn test_redact_wire_auth() {
        let cmd = EslCommand::Auth {
            password: Secret("secret".to_string()),
        };
        let wire = cmd
            .to_wire_format()
            .unwrap();
        let redacted = cmd.redact_wire(&wire);
        assert!(!redacted.contains("secret"));
        assert!(redacted.contains("REDACTED"));
    }

    #[test]
    fn test_redact_wire_user_auth() {
        let cmd = EslCommand::UserAuth {
            user: "admin@default".to_string(),
            password: Secret("secret".to_string()),
        };
        let wire = cmd
            .to_wire_format()
            .unwrap();
        let redacted = cmd.redact_wire(&wire);
        assert!(!redacted.contains("secret"));
        assert!(redacted.contains("admin@default"));
        assert!(redacted.contains("REDACTED"));
    }

    #[test]
    fn test_header_newline_rejected() {
        let result = CommandBuilder::new("test").header("X-Bad\n", "value");
        assert!(result.is_err());

        let result = CommandBuilder::new("test").header("X-Key", "bad\nvalue");
        assert!(result.is_err());
    }

    /// The caller's own input is at fault, so the connection stays usable and
    /// resending the same command cannot succeed. The value may be a secret.
    #[test]
    fn a_line_break_in_caller_input_is_neither_a_connection_fault_nor_retryable() {
        let cases = [
            (
                EslCommand::Api {
                    command: "status\n\nevent plain ALL".to_string(),
                },
                "api command",
            ),
            (
                EslCommand::Auth {
                    password: Secret("hunter2\0".to_string()),
                },
                "password",
            ),
            (
                EslCommand::Filter {
                    header: "Event-Name\r".to_string(),
                    value: "CHANNEL_CREATE".to_string(),
                },
                "filter header",
            ),
        ];
        for (cmd, field) in cases {
            let err = cmd
                .to_wire_format()
                .unwrap_err();
            assert!(
                matches!(err, EslError::WireTerminatorInField { field: f } if f == field),
                "{err}"
            );
            assert!(!err.is_connection_error(), "{err}");
            assert!(!err.is_recoverable(), "{err}");
            let shown = err.to_string();
            assert!(shown.contains(field), "{shown}");
            assert!(!shown.contains("hunter2"), "{shown}");
        }
    }

    /// The switch reads a command as a C string, so a NUL cuts it short.
    #[test]
    fn nul_rejected_at_the_wire() {
        let api = EslCommand::Api {
            command: "uuid_setvar abc sip_h_X-Tag a\0b".to_string(),
        };
        assert!(api
            .to_wire_format()
            .is_err());

        let auth = EslCommand::Auth {
            password: Secret("pass\0word".to_string()),
        };
        assert!(auth
            .to_wire_format()
            .is_err());

        assert!(CommandBuilder::new("test")
            .header("X-Key", "bad\0value")
            .is_err());
        assert!(CommandBuilder::new("test")
            .header("X-Bad\0", "value")
            .is_err());
    }
}
