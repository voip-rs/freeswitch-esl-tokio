//! Connection management for ESL

use std::sync::atomic::{AtomicBool, AtomicU64};
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

use auth::{authenticate, Handshake};
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
        }
    }
}

/// Establish a TCP connection with a timeout.
async fn tcp_connect_with_timeout(
    host: &str,
    port: u16,
    connect_timeout: Duration,
) -> EslResult<TcpStream> {
    let timeout_ms = connect_timeout.as_millis() as u64;
    let tcp_result = timeout(connect_timeout, TcpStream::connect((host, port))).await;

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
            Err(EslError::Timeout { timeout_ms })
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

/// Slot for the in-flight command waiter and its stale-reply counter.
///
/// ESL carries no correlation IDs, so a reply arriving after its command timed
/// out has to be discarded before the next waiter can consume it by mistake.
struct PendingReply {
    /// Waiter for the currently in-flight command (`None` between commands).
    waiting: Option<oneshot::Sender<EslMessage>>,
    /// Late replies the reader drains before dispatching to `waiting`. Non-zero
    /// coexists with `waiting: Some`: the next command installs while one is
    /// still in flight.
    stale_replies: u32,
    /// Set by the reader loop on exit and checked under this same lock before a
    /// waiter is installed, which closes the window after `is_connected()`.
    reader_dead: bool,
}

impl PendingReply {
    fn new() -> Self {
        Self {
            waiting: None,
            stale_replies: 0,
            reader_dead: false,
        }
    }
}

/// Shared state between EslClient and the reader task
struct SharedState {
    pending_reply: Mutex<PendingReply>,
    /// Connection status sender (shared so disconnect() can set ClientRequested)
    status_tx: watch::Sender<ConnectionStatus>,
    /// Liveness timeout in milliseconds (0 = disabled)
    liveness_timeout_ms: AtomicU64,
    /// Command response timeout in milliseconds
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
}

/// What the reader does when the event queue is full.
///
/// Waiting is only sound on a connection that issues no commands: the reader
/// serves replies from the same loop, and the switch's listener stops reading
/// while its write to us is stalled, so a command sent during a wait fails and
/// its late reply is discarded as stale. An events-only connection still issues
/// commands when it subscribes and re-subscribes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum EventOverflow {
    /// Drop the arriving event, count it, and arm an `EslError::QueueFull`
    /// notice for the consumer.
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
}

impl EslConnectOptions {
    /// Create default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the event queue capacity.
    pub fn with_event_queue_size(mut self, size: usize) -> Self {
        self.event_queue_size = size;
        self
    }

    /// Set the timeout for TCP connect and auth handshake.
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

    /// Capacity of the mpsc channel delivering events. Default: 1000.
    pub fn event_queue_size(&self) -> usize {
        self.event_queue_size
    }

    /// Timeout for TCP connect and each auth handshake read. Default: 2s.
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
}

impl Default for EslConnectOptions {
    fn default() -> Self {
        Self {
            event_queue_size: MAX_EVENT_QUEUE_SIZE,
            connect_timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            strict_header_utf8: false,
            event_overflow: EventOverflow::DropIncoming,
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

    /// Connect with user authentication
    ///
    /// The user must be in the format `user@domain` (e.g., `admin@default`).
    #[deprecated(since = "2.5.0", note = "use connect_with_auth with AuthMethod::user")]
    pub async fn connect_with_user(
        host: &str,
        port: u16,
        user: &str,
        password: &str,
    ) -> EslResult<(Self, EslEventStream)> {
        Self::connect_with_auth(
            host,
            port,
            AuthMethod::user(user, password),
            EslConnectOptions::default(),
        )
        .await
    }

    /// Connect with user authentication and custom options
    ///
    /// The user must be in the format `user@domain` (e.g., `admin@default`).
    #[deprecated(since = "2.5.0", note = "use connect_with_auth with AuthMethod::user")]
    pub async fn connect_with_user_and_options(
        host: &str,
        port: u16,
        user: &str,
        password: &str,
        options: EslConnectOptions,
    ) -> EslResult<(Self, EslEventStream)> {
        Self::connect_with_auth(host, port, AuthMethod::user(user, password), options).await
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
                return Err(EslError::auth_failed(format!(
                    "Invalid username format '{}': must be user@domain (e.g., admin@default)",
                    user
                )));
            }
        }

        info!("Connecting to FreeSWITCH at {}:{}", host, port);

        let connect_timeout = options.connect_timeout;
        let mut stream = tcp_connect_with_timeout(host, port, connect_timeout).await?;
        let mut parser = EslParser::new().with_strict_header_utf8(options.strict_header_utf8());
        let mut read_buffer = [0u8; SOCKET_BUF_SIZE];

        let auth_response = authenticate(
            &mut Handshake {
                stream: &mut stream,
                parser: &mut parser,
                read_buffer: &mut read_buffer,
            },
            &method,
            connect_timeout,
        )
        .await?;

        info!("Successfully connected and authenticated to FreeSWITCH");
        Ok(Self::split_and_spawn_with_options(
            stream,
            parser,
            options,
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
            options,
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
    /// See [`adopt_stream()`](Self::adopt_stream) for details.
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
            .max(1);

        let (read_half, write_half) = stream.into_split();

        let (status_tx, status_rx) = watch::channel(ConnectionStatus::Connected);
        let status_rx2 = status_tx.subscribe();

        #[cfg(unix)]
        let (stop_tx, stop_rx) = oneshot::channel();
        #[cfg(unix)]
        let (result_tx, result_rx) = oneshot::channel();

        let shared = Arc::new(SharedState {
            pending_reply: Mutex::new(PendingReply::new()),
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
            assert_eq!(pending.stale_replies, 0);
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
}
