//! Connection management for ESL

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::{
    command::EslResponse,
    constants::{DEFAULT_TIMEOUT_MS, MAX_EVENT_QUEUE_SIZE, SOCKET_BUF_SIZE},
    error::{EslError, EslResult},
    event::EslEvent,
    protocol::{EslMessage, EslParser},
};

mod auth;
mod client;
mod reader;
mod reexec;

pub use auth::AuthMethod;
pub use client::SwallowedEventTypes;
#[cfg(unix)]
pub use reexec::ReexecHandoff;

use auth::{authenticate, Handshake, HandshakeDeadline};
use reader::reader_loop;
#[cfg(unix)]
use reexec::ReexecCaller;
use reexec::ReexecReader;

/// Connection status for ESL client
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnectionStatus {
    /// ESL session is active.
    Connected,
    /// ESL session ended.
    Disconnected(DisconnectReason),
}

/// Reason for disconnection
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DisconnectReason {
    /// Server sent a `text/disconnect-notice` with `Content-Disposition: disconnect`.
    ///
    /// The notice may include a `Controlled-Session-UUID` header identifying
    /// the session and a body with a human-readable message.
    ServerNotice {
        /// UUID of the controlled session, if present in the notice.
        controlled_session_uuid: Option<String>,
        /// Body text from the disconnect notice (e.g. "Disconnected, goodbye.").
        body: Option<String>,
    },
    /// Liveness timeout exceeded without any inbound traffic
    HeartbeatExpired,
    /// TCP I/O error (io::Error is not Clone, so we store the message)
    IoError(String),
    /// Clean EOF on the TCP connection
    ConnectionClosed,
    /// Client called disconnect()
    ClientRequested,
    /// Client initiated re-exec teardown
    #[cfg(unix)]
    ReexecTeardown,
    /// Protocol-level desync (e.g. unsolicited auth/request after login)
    ProtocolError(String),
    /// Connection rejected by ACL (`text/rude-rejection`)
    AccessDenied(String),
    /// The application dropped its [`EslEventStream`]; the reader stopped at
    /// the next event, error or late reply it had to deliver.
    EventStreamDropped,
}

impl std::fmt::Display for DisconnectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DisconnectReason::ServerNotice {
                controlled_session_uuid,
                ..
            } => {
                if let Some(uuid) = controlled_session_uuid {
                    write!(f, "server sent disconnect notice (session {})", uuid)
                } else {
                    write!(f, "server sent disconnect notice")
                }
            }
            DisconnectReason::HeartbeatExpired => write!(f, "liveness timeout expired"),
            DisconnectReason::IoError(msg) => write!(f, "I/O error: {}", msg),
            DisconnectReason::ConnectionClosed => write!(f, "connection closed"),
            DisconnectReason::ClientRequested => write!(f, "client requested disconnect"),
            #[cfg(unix)]
            DisconnectReason::ReexecTeardown => write!(f, "re-exec teardown"),
            DisconnectReason::ProtocolError(msg) => write!(f, "protocol error: {}", msg),
            DisconnectReason::AccessDenied(msg) => write!(f, "access denied: {}", msg),
            DisconnectReason::EventStreamDropped => write!(f, "event stream dropped"),
        }
    }
}

/// Establish a TCP connection within the handshake's deadline.
async fn tcp_connect_with_timeout(
    host: &str,
    port: u16,
    deadline: &HandshakeDeadline,
) -> EslResult<TcpStream> {
    let timeout_ms = deadline.timeout_ms();
    let tcp_result = timeout(deadline.remaining(), TcpStream::connect((host, port))).await;

    match tcp_result {
        Ok(Ok(s)) => {
            debug!("[CONNECT] TCP connection established");
            Ok(s)
        }
        Ok(Err(e)) => {
            warn!("[CONNECT] TCP connect failed: {}", e);
            Err(EslError::Io(e))
        }
        Err(_) => {
            warn!("[CONNECT] TCP connect timed out after {}ms", timeout_ms);
            Err(EslError::ConnectTimeout { timeout_ms })
        }
    }
}

/// What one [`read_into_parser`] call observed on the socket.
enum ReadStep {
    /// Bytes arrived and were handed to the parser.
    Fed,
    /// The peer closed the connection cleanly.
    Eof,
    /// Nothing arrived before `read_timeout` elapsed.
    Idle,
}

/// Read once into `parser`, the step both the auth handshake and the reader
/// loop take between `parse_message` calls.
///
/// The caller owns what an idle tick means: a handshake deadline for one, a
/// liveness check for the other.
async fn read_into_parser<S: tokio::io::AsyncRead + Unpin>(
    stream: &mut S,
    parser: &mut EslParser,
    read_buffer: &mut [u8],
    read_timeout: Duration,
) -> EslResult<ReadStep> {
    use tokio::io::AsyncReadExt;

    let bytes_read = match timeout(read_timeout, stream.read(read_buffer)).await {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => return Err(EslError::Io(e)),
        Err(_) => return Ok(ReadStep::Idle),
    };
    if bytes_read == 0 {
        return Ok(ReadStep::Eof);
    }
    parser.add_data(&read_buffer[..bytes_read])?;
    Ok(ReadStep::Fed)
}

/// Connection mode for ESL
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnectionMode {
    /// Inbound connection - client connects to FreeSWITCH
    Inbound,
    /// Outbound connection - FreeSWITCH connects to client
    Outbound,
}

/// Default command timeout in milliseconds (5 seconds)
const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 5000;

/// How long a command may take from the start of its write to its reply.
#[derive(Debug, Clone, Copy)]
enum ReplyWait {
    /// The connection's [`EslClient::set_command_timeout`], read once the
    /// writer is held.
    ConnectionDefault,
    /// Until the reply or the end of the connection.
    Unbounded,
    For(Duration),
}

impl ReplyWait {
    /// A caller's timeout, where `None` and zero both mean none.
    fn per_call(timeout: Option<Duration>) -> Self {
        match timeout {
            Some(timeout) if !timeout.is_zero() => Self::For(timeout),
            _ => Self::Unbounded,
        }
    }
}

/// Milliseconds for an error or a log line, saturating rather than wrapping.
fn saturating_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Milliseconds for a stored timeout where 0 means none: a non-zero duration
/// under a millisecond rounds up instead of switching the timeout off.
fn stored_millis(duration: Duration) -> u64 {
    if duration.is_zero() {
        return 0;
    }
    saturating_millis(duration).max(1)
}

/// One command sent on a connection, numbered in send order.
///
/// [`EslError::CommandTimeout`] hands it out and the [`EslError::LateReply`]
/// answering the same command carries it back. Unique per connection only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandTicket(pub(crate) u64);

impl fmt::Display for CommandTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Slot for the in-flight command waiter and the replies still owed.
///
/// ESL carries no correlation IDs and the switch answers in order, so every
/// reply it owes belongs to the installed waiter or to a ticket in `stale`,
/// oldest first. A command that timed out, or whose future was dropped while
/// it waited, moves to `stale`; its reply is then delivered as
/// [`EslError::LateReply`] and never reaches the next waiter. A waiter found on
/// install is such a dropped command's. One dropped before its write completed
/// left an unknown prefix of its command on the wire: no reply can be assumed
/// owed and the next command cannot be framed after it, so the connection is
/// closed instead.
struct PendingReply {
    /// Waiter for the currently in-flight command (`None` between commands).
    waiting: Option<Waiter>,
    /// Commands whose reply is owed but nobody awaits. Non-empty coexists with
    /// `waiting: Some`: the next command installs while one is still in flight.
    stale: VecDeque<CommandTicket>,
    next_ticket: u64,
    /// Set by the reader loop on exit and checked under this same lock before a
    /// waiter is installed, which closes the window after `is_connected()`.
    reader_dead: bool,
}

impl PendingReply {
    /// `owed` replies a previous process image left unanswered, each under a
    /// ticket this connection never hands to a command.
    fn new(owed: usize) -> Self {
        let next_ticket = u64::try_from(owed).unwrap_or(u64::MAX);
        Self {
            waiting: None,
            stale: (0..next_ticket)
                .map(CommandTicket)
                .collect(),
            next_ticket,
            reader_dead: false,
        }
    }

    /// Installs the waiter for a command about to be written, or reports that
    /// a previous command was dropped mid-write. The returned flag is the
    /// writer's to set once its write completes.
    fn install(
        &mut self,
        tx: oneshot::Sender<EslMessage>,
    ) -> Result<Arc<AtomicBool>, AbandonedMidWrite> {
        if let Some(abandoned) = self
            .waiting
            .take()
        {
            if !abandoned
                .written
                .load(Ordering::Acquire)
            {
                return Err(AbandonedMidWrite);
            }
            self.stale
                .push_back(abandoned.ticket);
        }
        let written = Arc::new(AtomicBool::new(false));
        self.waiting = Some(Waiter {
            ticket: CommandTicket(self.next_ticket),
            tx,
            written: written.clone(),
        });
        self.next_ticket += 1;
        Ok(written)
    }
}

/// The command awaiting the next reply.
struct Waiter {
    ticket: CommandTicket,
    tx: oneshot::Sender<EslMessage>,
    /// Set with no await point after the write completes, so a future
    /// dropped after its write cannot leave it unset.
    written: Arc<AtomicBool>,
}

/// A command's future was dropped before its bytes all reached the socket.
struct AbandonedMidWrite;

/// Shared state between EslClient and the reader task
struct SharedState {
    pending_reply: Mutex<PendingReply>,
    /// Connection status sender (shared so disconnect() can set ClientRequested)
    status_tx: watch::Sender<ConnectionStatus>,
    /// Liveness timeout in milliseconds (0 = disabled)
    liveness_timeout_ms: AtomicU64,
    /// Command response timeout in milliseconds (0 = none)
    command_timeout_ms: AtomicU64,
    /// Set when events have been dropped due to a full queue
    queue_full_notice_armed: AtomicBool,
    /// Total count of dropped events
    dropped_event_count: AtomicU64,
    /// Dispatches that had to wait for queue capacity
    event_stall_count: AtomicU64,
    /// Accumulated nanoseconds spent waiting for queue capacity
    event_stall_nanos: AtomicU64,
    /// Auth response from inbound connect (None for outbound)
    auth_response: Option<EslResponse>,
    /// Whether this is an inbound or outbound ESL connection
    mode: ConnectionMode,
    /// Re-exec channel caller half (taken by teardown_for_reexec)
    #[cfg(unix)]
    reexec: Mutex<Option<ReexecCaller>>,
    /// Run by the reader before dispatching its next message.
    #[cfg(test)]
    induced_panic: std::sync::Mutex<Option<fn()>>,
    /// Run by the reader once a re-exec failure has reached the teardown
    /// caller, which may then be woken.
    #[cfg(test)]
    after_reexec_report: std::sync::Mutex<Option<fn()>>,
}

impl SharedState {
    /// Moves the status from `Connected` to `Disconnected(reason)`. The first
    /// reason stands: a later one, such as the EOF the switch answers a
    /// `disconnect()` with, is a consequence of it.
    fn publish_disconnect(&self, reason: DisconnectReason) {
        self.status_tx
            .send_if_modified(|status| {
                if !matches!(status, ConnectionStatus::Connected) {
                    return false;
                }
                *status = ConnectionStatus::Disconnected(reason);
                true
            });
    }
}

/// What the reader does when the event queue is full.
///
/// Waiting is only sound on a connection that issues no commands: the reader
/// serves replies from the same loop, and the switch's listener stops reading
/// while its write to us is stalled, so a command sent during a wait times out
/// and its reply arrives as `EslError::LateReply`. An events-only connection still issues
/// commands when it subscribes and re-subscribes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum EventOverflow {
    /// Drop the arriving event, count it, and arm an `EslError::QueueFull`
    /// notice for the consumer. A reply delivered on the stream
    /// (`LateReply`, `UnsolicitedReply`) is dropped the same way.
    #[default]
    DropIncoming,
    /// Wait up to this long for capacity, then fall back to `DropIncoming`.
    ///
    /// A budget near the switch's own send-retry window trades a counted loss
    /// for a truncated event on the wire: `switch_socket_send` gives up after
    /// it, and the event-write site in `mod_event_socket.c` does not check the
    /// result. Keep it well short of that.
    BlockFor(Duration),
}

/// Options for ESL connection configuration.
///
/// Controls parameters that are fixed at connection time, such as the event
/// queue capacity and connect timeout. Use [`Default::default()`] for standard settings.
#[derive(Debug, Clone)]
pub struct EslConnectOptions {
    event_queue_size: usize,
    connect_timeout: Duration,
    strict_header_utf8: bool,
    event_overflow: EventOverflow,
    owed_replies: usize,
}

impl EslConnectOptions {
    /// Create default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the event queue capacity. Zero is taken as one, and a size past the
    /// largest tokio's channel holds as that size.
    pub fn with_event_queue_size(mut self, size: usize) -> Self {
        self.event_queue_size = size;
        self
    }

    /// Set the deadline for the TCP connect and the auth handshake together.
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Strict UTF-8 validation on event-body header values.
    ///
    /// When `true`, invalid UTF-8 in percent-decoded event-body values returns
    /// `EslError::InvalidUtf8InHeader` and stops the stream. When `false`
    /// (default), invalid bytes are decoded lossily (U+FFFD) and surfaced via
    /// `EslEvent::lossy_values()` for inspection/recovery.
    pub fn with_strict_header_utf8(mut self, strict: bool) -> Self {
        self.strict_header_utf8 = strict;
        self
    }

    /// Set what the reader does when the event queue is full.
    pub fn with_event_overflow(mut self, overflow: EventOverflow) -> Self {
        self.event_overflow = overflow;
        self
    }

    /// Replies the switch still owes the process image that handed this
    /// socket over, from [`ReexecHandoff::owed_replies`](crate::ReexecHandoff::owed_replies).
    ///
    /// Read only by [`EslClient::adopt_stream_with_options`]: the first `n`
    /// replies then arrive as [`EslError::LateReply`] under tickets no command
    /// here was given, instead of being taken as the next commands' own. A
    /// connection this process opened owes nothing, so `connect` and
    /// `accept_outbound` ignore it.
    pub fn with_owed_replies(mut self, n: usize) -> Self {
        self.owed_replies = n;
        self
    }

    /// Capacity of the mpsc channel delivering events. Default: 1000.
    pub fn event_queue_size(&self) -> usize {
        self.event_queue_size
    }

    /// One deadline from the start of the TCP connect through the auth reply,
    /// the auth command's write included; a timeout too large to add to the
    /// current time is none. Default: 2s.
    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    /// Whether to fail on invalid UTF-8 in event-body values. Default: false.
    pub fn strict_header_utf8(&self) -> bool {
        self.strict_header_utf8
    }

    /// Full-queue behaviour. Default: [`EventOverflow::DropIncoming`].
    pub fn event_overflow(&self) -> EventOverflow {
        self.event_overflow
    }

    /// Replies owed on an adopted socket. Default: 0.
    pub fn owed_replies(&self) -> usize {
        self.owed_replies
    }
}

impl Default for EslConnectOptions {
    fn default() -> Self {
        Self {
            event_queue_size: MAX_EVENT_QUEUE_SIZE,
            connect_timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            strict_header_utf8: false,
            event_overflow: EventOverflow::DropIncoming,
            owed_replies: 0,
        }
    }
}

/// ESL client handle (Clone + Send)
///
/// Commands are serialized through the writer mutex. The reader task
/// routes replies to the pending oneshot channel.
#[derive(Clone)]
pub struct EslClient {
    writer: Arc<Mutex<OwnedWriteHalf>>,
    shared: Arc<SharedState>,
    status_rx: watch::Receiver<ConnectionStatus>,
}

impl std::fmt::Debug for EslClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EslClient")
            .field("connected", &self.is_connected())
            .finish()
    }
}

/// Event stream receiver (!Clone)
///
/// Receives events from the background reader task via an mpsc channel.
///
/// Events are delivered as `Result<EslEvent, EslError>`. An `Err(EslError::QueueFull)`
/// indicates that one or more events were dropped because the application fell behind.
/// It rides the next successful dispatch, so a burst that ends with the queue still
/// full reports nothing in-band: [`EslClient::dropped_event_count`] is the reliable
/// count. [`EventOverflow`] trades those drops for a bounded stall.
pub struct EslEventStream {
    rx: mpsc::Receiver<Result<EslEvent, EslError>>,
    status_rx: watch::Receiver<ConnectionStatus>,
}

impl std::fmt::Debug for EslEventStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EslEventStream")
            .field("connected", &self.is_connected())
            .finish()
    }
}

impl EslClient {
    /// Connect to FreeSWITCH (inbound mode) with the shared `auth` password.
    ///
    /// Shorthand for [`connect_with_auth`](Self::connect_with_auth) with
    /// [`AuthMethod::password`] and default options.
    pub async fn connect(
        host: &str,
        port: u16,
        password: &str,
    ) -> EslResult<(Self, EslEventStream)> {
        Self::connect_with_auth(
            host,
            port,
            AuthMethod::password(password),
            EslConnectOptions::default(),
        )
        .await
    }

    /// Connect with the shared `auth` password and custom options.
    ///
    /// Shorthand for [`connect_with_auth`](Self::connect_with_auth) with
    /// [`AuthMethod::password`].
    pub async fn connect_with_options(
        host: &str,
        port: u16,
        password: &str,
        options: EslConnectOptions,
    ) -> EslResult<(Self, EslEventStream)> {
        Self::connect_with_auth(host, port, AuthMethod::password(password), options).await
    }

    /// Connect to FreeSWITCH (inbound mode), authenticating with `method`.
    ///
    /// The credential lives only in the [`AuthMethod`]; every other connect
    /// constructor is a shorthand for this one. A directory user must be
    /// spelled `user@domain`.
    pub async fn connect_with_auth(
        host: &str,
        port: u16,
        method: AuthMethod,
        options: EslConnectOptions,
    ) -> EslResult<(Self, EslEventStream)> {
        if let AuthMethod::User { user, .. } = &method {
            if !user.contains('@') {
                return Err(EslError::auth_failed(
                    "userauth user has no @domain (e.g. admin@default)",
                ));
            }
        }

        info!("Connecting to FreeSWITCH at {}:{}", host, port);

        let deadline = HandshakeDeadline::starting_now(options.connect_timeout);
        let mut stream = tcp_connect_with_timeout(host, port, &deadline).await?;
        let mut parser = EslParser::new().with_strict_header_utf8(options.strict_header_utf8());
        let mut read_buffer = [0u8; SOCKET_BUF_SIZE];

        let auth_response = authenticate(
            &mut Handshake {
                stream: &mut stream,
                parser: &mut parser,
                read_buffer: &mut read_buffer,
            },
            &method,
            &deadline,
        )
        .await?;

        info!("Successfully connected and authenticated to FreeSWITCH");
        Ok(Self::split_and_spawn_with_options(
            stream,
            parser,
            options.with_owed_replies(0),
            Some(auth_response),
            ConnectionMode::Inbound,
        ))
    }

    /// Accept outbound connection from FreeSWITCH.
    ///
    /// After `accept_outbound`, you MUST call [`Self::connect_session`] before
    /// any other command. Calling [`Self::api`], [`Self::subscribe_events`],
    /// etc. before `connect_session()` will leave the channel in an undefined
    /// state. See [`docs/outbound-esl-quirks.md`](https://github.com/voip-rs/freeswitch-esl-tokio/blob/master/docs/outbound-esl-quirks.md)
    /// for full context.
    pub async fn accept_outbound(listener: &TcpListener) -> EslResult<(Self, EslEventStream)> {
        Self::accept_outbound_with_options(listener, EslConnectOptions::default()).await
    }

    /// Accept outbound connection from FreeSWITCH with custom options.
    ///
    /// After accepting, you MUST call [`Self::connect_session`] before any
    /// other command. Calling [`Self::api`], [`Self::subscribe_events`], etc.
    /// before `connect_session()` will leave the channel in an undefined
    /// state. See [`docs/outbound-esl-quirks.md`](https://github.com/voip-rs/freeswitch-esl-tokio/blob/master/docs/outbound-esl-quirks.md)
    /// for full context.
    pub async fn accept_outbound_with_options(
        listener: &TcpListener,
        options: EslConnectOptions,
    ) -> EslResult<(Self, EslEventStream)> {
        info!("Waiting for outbound connection from FreeSWITCH");

        let (stream, addr) = listener
            .accept()
            .await
            .map_err(EslError::Io)?;
        info!("Accepted outbound connection from {}", addr);

        Ok(Self::accept_outbound_stream_with_options(stream, options))
    }

    /// Create an outbound-mode client from an already-accepted `TcpStream`.
    ///
    /// Use this when you need control over the accept step (e.g. for
    /// timeouts, TLS wrapping, or custom accept logic).
    ///
    /// After this call, you MUST call [`Self::connect_session`] before any
    /// other command. Calling [`Self::api`], [`Self::subscribe_events`], etc.
    /// before `connect_session()` will leave the channel in an undefined
    /// state. See [`docs/outbound-esl-quirks.md`](https://github.com/voip-rs/freeswitch-esl-tokio/blob/master/docs/outbound-esl-quirks.md)
    /// for full context.
    pub fn accept_outbound_stream(stream: TcpStream) -> (Self, EslEventStream) {
        Self::accept_outbound_stream_with_options(stream, EslConnectOptions::default())
    }

    /// Create an outbound-mode client from an already-accepted `TcpStream`
    /// with custom options.
    ///
    /// After this call, you MUST call [`Self::connect_session`] before any
    /// other command. Calling [`Self::api`], [`Self::subscribe_events`], etc.
    /// before `connect_session()` will leave the channel in an undefined
    /// state. See [`docs/outbound-esl-quirks.md`](https://github.com/voip-rs/freeswitch-esl-tokio/blob/master/docs/outbound-esl-quirks.md)
    /// for full context.
    pub fn accept_outbound_stream_with_options(
        stream: TcpStream,
        options: EslConnectOptions,
    ) -> (Self, EslEventStream) {
        Self::split_and_spawn_with_options(
            stream,
            EslParser::new().with_strict_header_utf8(options.strict_header_utf8()),
            options.with_owed_replies(0),
            None,
            ConnectionMode::Outbound,
        )
    }

    /// Create an `EslClient` from an already-authenticated TCP stream.
    ///
    /// Used after re-exec: the previous process already authenticated and
    /// subscribed to events. The `residual_bytes` are any unparsed data
    /// left in the previous parser buffer (from
    /// [`teardown_for_reexec()`](Self::teardown_for_reexec)).
    ///
    /// FreeSWITCH server-side event subscriptions survive across `exec()`.
    /// Events will arrive immediately, so the caller should be ready to
    /// consume them before calling this method.
    pub fn adopt_stream(
        stream: TcpStream,
        residual_bytes: &[u8],
    ) -> EslResult<(Self, EslEventStream)> {
        Self::adopt_stream_with_options(stream, residual_bytes, EslConnectOptions::default())
    }

    /// Create an `EslClient` from an already-authenticated TCP stream
    /// with custom options.
    ///
    /// See [`adopt_stream()`](Self::adopt_stream) for details. Pass the
    /// handoff's owed reply count through
    /// [`EslConnectOptions::with_owed_replies`], which `adopt_stream` cannot.
    pub fn adopt_stream_with_options(
        stream: TcpStream,
        residual_bytes: &[u8],
        options: EslConnectOptions,
    ) -> EslResult<(Self, EslEventStream)> {
        let mut parser = EslParser::new().with_strict_header_utf8(options.strict_header_utf8());
        if !residual_bytes.is_empty() {
            parser.add_data(residual_bytes)?;
        }
        Ok(Self::split_and_spawn_with_options(
            stream,
            parser,
            options,
            None,
            ConnectionMode::Inbound,
        ))
    }

    fn split_and_spawn_with_options(
        stream: TcpStream,
        parser: EslParser,
        options: EslConnectOptions,
        auth_response: Option<EslResponse>,
        mode: ConnectionMode,
    ) -> (Self, EslEventStream) {
        let queue_size = options
            .event_queue_size
            .clamp(1, tokio::sync::Semaphore::MAX_PERMITS);

        let (read_half, write_half) = stream.into_split();

        let (status_tx, status_rx) = watch::channel(ConnectionStatus::Connected);
        let status_rx2 = status_tx.subscribe();

        #[cfg(unix)]
        let (stop_tx, stop_rx) = oneshot::channel();
        #[cfg(unix)]
        let (result_tx, result_rx) = oneshot::channel();

        let shared = Arc::new(SharedState {
            pending_reply: Mutex::new(PendingReply::new(options.owed_replies)),
            status_tx,
            liveness_timeout_ms: AtomicU64::new(0),
            command_timeout_ms: AtomicU64::new(DEFAULT_COMMAND_TIMEOUT_MS),
            queue_full_notice_armed: AtomicBool::new(false),
            dropped_event_count: AtomicU64::new(0),
            event_stall_count: AtomicU64::new(0),
            event_stall_nanos: AtomicU64::new(0),
            auth_response,
            mode,
            #[cfg(unix)]
            reexec: Mutex::new(Some(ReexecCaller { stop_tx, result_rx })),
            #[cfg(test)]
            induced_panic: std::sync::Mutex::new(None),
            #[cfg(test)]
            after_reexec_report: std::sync::Mutex::new(None),
        });
        let (event_tx, event_rx) = mpsc::channel(queue_size);

        #[cfg(unix)]
        let reexec_reader = ReexecReader {
            stop_rx,
            result_tx: Some(result_tx),
        };
        #[cfg(not(unix))]
        let reexec_reader = ReexecReader {};

        tokio::spawn(reader_loop(
            read_half,
            parser,
            shared.clone(),
            event_tx,
            options.event_overflow,
            reexec_reader,
        ));

        let client = EslClient {
            writer: Arc::new(Mutex::new(write_half)),
            shared,
            status_rx,
        };

        let stream = EslEventStream {
            rx: event_rx,
            status_rx: status_rx2,
        };

        (client, stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Refused before any socket opens, without quoting the user back.
    #[tokio::test]
    async fn a_user_without_a_domain_is_refused_unquoted() {
        let method = AuthMethod::user("nodomain-sentinel", "pass").expect("no colon");
        let err = EslClient::connect_with_auth("localhost", 1, method, EslConnectOptions::new())
            .await
            .expect_err("a user without @ is refused");
        assert!(
            matches!(err, EslError::AuthenticationFailed { .. }),
            "{err:?}"
        );
        assert!(
            !err.to_string()
                .contains("nodomain-sentinel"),
            "{err}"
        );
    }

    /// The disconnect reason after the reader panics on its next message.
    async fn reason_after_reader_panic(hook: fn()) -> String {
        use tokio::io::AsyncWriteExt;

        let listener = TcpListener::bind("[::1]:0")
            .await
            .unwrap();
        let addr = listener
            .local_addr()
            .unwrap();
        let (client_stream, accept_result) =
            tokio::join!(TcpStream::connect(addr), listener.accept());
        let (mut server_stream, _) = accept_result.unwrap();
        let (client, mut events) = EslClient::split_and_spawn_with_options(
            client_stream.unwrap(),
            EslParser::new(),
            EslConnectOptions::default(),
            None,
            ConnectionMode::Inbound,
        );
        *client
            .shared
            .induced_panic
            .lock()
            .unwrap() = Some(hook);
        server_stream
            .write_all(b"Content-Type: command/reply\nReply-Text: +OK\n\n")
            .await
            .unwrap();
        let end = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("the reader exits");
        assert!(end.is_none(), "{end:?}");
        match client.status() {
            ConnectionStatus::Disconnected(DisconnectReason::IoError(reason)) => reason,
            other => panic!("expected IoError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_reader_panic_reports_its_str_payload() {
        let reason = reason_after_reader_panic(|| panic!("static-payload-sentinel")).await;
        assert!(reason.contains("static-payload-sentinel"), "{reason}");
    }

    #[tokio::test]
    async fn a_reader_panic_reports_its_string_payload() {
        let reason = reason_after_reader_panic(|| {
            let n = 42;
            panic!("formatted-payload-sentinel {n}")
        })
        .await;
        assert!(reason.contains("formatted-payload-sentinel 42"), "{reason}");
    }

    #[tokio::test]
    async fn a_reader_panic_without_a_message_says_so() {
        let reason = reason_after_reader_panic(|| std::panic::panic_any(7u8)).await;
        assert!(reason.contains("non-string payload"), "{reason}");
    }

    #[tokio::test]
    async fn write_failure_clears_pending_waiting() {
        use tokio::io::AsyncWriteExt;

        let listener = TcpListener::bind("[::1]:0")
            .await
            .unwrap();
        let addr = listener
            .local_addr()
            .unwrap();
        let (client_stream, accept_result) =
            tokio::join!(TcpStream::connect(addr), listener.accept());
        let (_server_stream, _) = accept_result.unwrap();

        let (client, _events) = EslClient::split_and_spawn_with_options(
            client_stream.unwrap(),
            EslParser::new(),
            EslConnectOptions::default(),
            None,
            ConnectionMode::Inbound,
        );

        // Shut down the write half directly: the status watch stays Connected
        // and the reader stays alive, isolating send_command's write-error
        // path (reachable via the public API only through a disconnect()
        // race, since disconnect() flips the status before the shutdown).
        client
            .writer
            .lock()
            .await
            .shutdown()
            .await
            .unwrap();
        assert!(client.is_connected());

        let err = client
            .noop()
            .await
            .expect_err("write on a shut-down half must fail");
        assert!(matches!(err, EslError::Io(_)), "got: {err}");
        // An I/O error may follow a partial write, so the stream is closed.
        assert!(
            matches!(
                client.status(),
                ConnectionStatus::Disconnected(DisconnectReason::IoError(_))
            ),
            "{:?}",
            client.status()
        );

        // The failed command must not leave its waiter installed, and no
        // stale reply can be in flight for a command that never hit the wire.
        {
            let pending = client
                .shared
                .pending_reply
                .lock()
                .await;
            assert!(
                pending
                    .waiting
                    .is_none(),
                "waiting slot must be cleared on write failure"
            );
            assert!(pending
                .stale
                .is_empty());
        }

        // User-visible symptom: teardown_for_reexec must not report a
        // phantom "command still in-flight".
        #[cfg(unix)]
        {
            let result = client
                .teardown_for_reexec()
                .await;
            assert!(
                result.is_ok(),
                "teardown saw a phantom in-flight command: {:?}",
                result.err()
            );
        }
    }

    /// The reader takes the waiter after the timeout fired but before
    /// `send_command` re-locks: the reply was delivered, so the command
    /// succeeds instead of timing out with its reply lost.
    #[tokio::test]
    async fn reply_delivered_inside_the_timeout_window_is_returned() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = TcpListener::bind("[::1]:0")
            .await
            .unwrap();
        let addr = listener
            .local_addr()
            .unwrap();
        let (client_stream, accept_result) =
            tokio::join!(TcpStream::connect(addr), listener.accept());
        let (mut server_stream, _) = accept_result.unwrap();
        let (client, _events) = EslClient::split_and_spawn_with_options(
            client_stream.unwrap(),
            EslParser::new(),
            EslConnectOptions::default(),
            None,
            ConnectionMode::Inbound,
        );
        let timeout = Duration::from_millis(300);
        client.set_command_timeout(timeout);

        let cmd = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .api("status")
                    .await
            }
        });
        let mut written = Vec::new();
        while !written.ends_with(b"\n\n") {
            let mut buf = [0u8; 256];
            let n = server_stream
                .read(&mut buf)
                .await
                .unwrap();
            assert!(n > 0, "client closed before writing its command");
            written.extend_from_slice(&buf[..n]);
        }

        // tokio's Mutex grants in request order: the reader queues on the lock
        // first, the timed-out send_command second.
        let held = client
            .shared
            .pending_reply
            .lock()
            .await;
        server_stream
            .write_all(b"Content-Type: api/response\nContent-Length: 6\n\nwindow")
            .await
            .unwrap();
        tokio::time::sleep(timeout + Duration::from_millis(200)).await;
        drop(held);

        let response = cmd
            .await
            .unwrap()
            .expect("the reply arrived, so the command did not time out");
        assert_eq!(response.body(), Some("window"));
    }

    /// A client over a socket whose server end the test drives by hand.
    async fn hand_driven_pair() -> (TcpStream, EslClient, EslEventStream) {
        let listener = TcpListener::bind("[::1]:0")
            .await
            .unwrap();
        let addr = listener
            .local_addr()
            .unwrap();
        let (client_stream, accept_result) =
            tokio::join!(TcpStream::connect(addr), listener.accept());
        let (server_stream, _) = accept_result.unwrap();
        let (client, events) = EslClient::split_and_spawn_with_options(
            client_stream.unwrap(),
            EslParser::new(),
            EslConnectOptions::default(),
            None,
            ConnectionMode::Inbound,
        );
        (server_stream, client, events)
    }

    /// A command that reaches the writer while teardown is between its
    /// in-flight check and the stop signal must not reach the wire: its reply
    /// would cross the exec with nobody to pair it with.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_racing_teardown_is_not_written() {
        use tokio::io::AsyncReadExt;

        let (mut server, client, _events) = hand_driven_pair().await;
        let held = client
            .shared
            .reexec
            .lock()
            .await;
        let teardown = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .teardown_for_reexec()
                    .await
            }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let command = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .noop()
                    .await
            }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(held);

        let handoff = tokio::time::timeout(Duration::from_secs(10), teardown)
            .await
            .expect("teardown returns")
            .unwrap();
        assert!(handoff.is_ok(), "{:?}", handoff.err());
        let err = tokio::time::timeout(Duration::from_secs(5), command)
            .await
            .expect("the command returns")
            .unwrap()
            .expect_err("the connection was handed off");
        assert!(err.is_connection_error(), "{err:?}");
        let mut buf = [0u8; 64];
        let read = tokio::time::timeout(Duration::from_millis(300), server.read(&mut buf)).await;
        assert!(read.is_err(), "the command reached the wire: {read:?}");
    }

    /// Starts a teardown mid-body, lets the server end the drain with `ending`
    /// (closing the socket after it when `close`), and returns the error with
    /// the status read the moment teardown returned. The reader stalls once it
    /// has reported, so a status it publishes later is not there yet.
    #[cfg(unix)]
    async fn status_when_teardown_fails(
        ending: &'static [u8],
        close: bool,
    ) -> (EslError, ConnectionStatus) {
        use tokio::io::AsyncWriteExt;

        let (mut server, client, mut events) = hand_driven_pair().await;
        *client
            .shared
            .after_reexec_report
            .lock()
            .unwrap() = Some(|| {
            // Hands the worker over first, so the woken teardown runs meanwhile.
            tokio::task::block_in_place(|| std::thread::sleep(Duration::from_millis(500)))
        });
        server
            .write_all(
                concat!(
                    "Content-Type: text/event-plain\nContent-Length: 21\n\nEvent-Name: HEARTBEAT\n",
                    "Content-Type: text/event-plain\nContent-Length: 4\n\n",
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("the heartbeat arrives");
        assert!(matches!(first, Some(Ok(_))), "{first:?}");

        let teardown = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .teardown_for_reexec()
                    .await
            }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        server
            .write_all(ending)
            .await
            .unwrap();
        if close {
            drop(server);
        }
        let err = tokio::time::timeout(Duration::from_secs(10), teardown)
            .await
            .expect("teardown returns")
            .unwrap()
            .expect_err("the drain cannot finish");
        (err, client.status())
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_drain_cut_by_eof_is_published_before_teardown_returns() {
        let (err, status) = status_when_teardown_fails(b"ab", true).await;
        assert!(matches!(err, EslError::ReexecReaderLost { .. }), "{err:?}");
        assert_eq!(
            status,
            ConnectionStatus::Disconnected(DisconnectReason::ConnectionClosed)
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_reader_exit_mid_drain_is_published_before_teardown_returns() {
        let (err, status) =
            status_when_teardown_fails(b"abcdContent-Type: text/garbage\n\n", false).await;
        assert!(matches!(err, EslError::ReexecReaderLost { .. }), "{err:?}");
        assert!(
            matches!(
                status,
                ConnectionStatus::Disconnected(DisconnectReason::ProtocolError(_))
            ),
            "{status:?}"
        );
    }

    /// Teardown switches liveness off before it signals the reader; a teardown
    /// refused at that signal must leave the timeout as the caller set it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_teardown_refused_at_the_stop_signal_restores_liveness() {
        let (server, client, mut events) = hand_driven_pair().await;
        client.set_liveness_timeout(Duration::from_secs(7));
        drop(server);
        let end = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("the reader exits on EOF");
        assert!(end.is_none(), "{end:?}");

        let err = client
            .teardown_for_reexec()
            .await
            .expect_err("the reader is gone");
        assert!(matches!(err, EslError::ReexecReaderLost { .. }), "{err:?}");
        assert_eq!(
            client
                .shared
                .liveness_timeout_ms
                .load(Ordering::Relaxed),
            7000
        );
    }
}
