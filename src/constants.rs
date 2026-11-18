//! Protocol constants and configuration values

/// Socket buffer size for reading from TCP stream (64KB) - standard TCP receive window
pub const SOCKET_BUF_SIZE: usize = 65536;

/// Buffer allocation size (64KB) - used for both initial allocation and growth increments.
/// Sized to handle most ESL messages without reallocation.
pub const BUF_CHUNK: usize = 64 * 1024;

/// Maximum single message size (8MB) - validates Content-Length header.
/// Safety limit for protocol sanity; large responses (e.g. sofia status) fit well within this.
pub const MAX_MESSAGE_SIZE: usize = 8 * 1024 * 1024;

/// Maximum total buffer size (16MB) - safety limit to prevent runaway memory
/// Should hold 2 max messages + overhead. Indicates a bug if exceeded.
pub const MAX_BUFFER_SIZE: usize = 16 * 1024 * 1024;

/// Double newline separating header blocks in ESL wire format.
pub const HEADER_TERMINATOR: &str = "\n\n";
/// Single newline separating individual headers in ESL wire format.
pub const LINE_TERMINATOR: &str = "\n";

/// Content-Type for auth challenge from FreeSWITCH.
pub const CONTENT_TYPE_AUTH_REQUEST: &str = "auth/request";
/// Content-Type for command reply messages.
pub const CONTENT_TYPE_COMMAND_REPLY: &str = "command/reply";
/// Content-Type for api/bgapi response bodies.
pub const CONTENT_TYPE_API_RESPONSE: &str = "api/response";
/// Content-Type for plain-text event format.
pub const CONTENT_TYPE_TEXT_EVENT_PLAIN: &str = "text/event-plain";
/// Content-Type for JSON event format.
pub const CONTENT_TYPE_TEXT_EVENT_JSON: &str = "text/event-json";
/// Content-Type for XML event format.
pub const CONTENT_TYPE_TEXT_EVENT_XML: &str = "text/event-xml";
/// Content-Type for log/data messages (FreeSWITCH log forwarding).
pub const CONTENT_TYPE_LOG_DATA: &str = "log/data";

/// What `switch_event_serialize` writes in place of an empty value, and what
/// `uuid_getvar` answers for a variable that is not set.
pub const UNDEF_VALUE: &str = "_undef_";

/// Success prefix on a `Reply-Text` value or an api response body.
pub const REPLY_PREFIX_OK: &str = "+OK";
/// Failure prefix on a `Reply-Text` value or an api response body.
pub const REPLY_PREFIX_ERR: &str = "-ERR";
/// Usage-synopsis prefix on an api response body. Stored without the trailing
/// `:` so a `starts_with` test matches whatever separator the command wrote.
pub const REPLY_PREFIX_USAGE: &str = "-USAGE";

/// What `mod_event_socket` puts in a reply that a command left empty, which is
/// not always a command it failed to recognise.
pub const REPLY_COMMAND_NOT_FOUND: &str = "-ERR command not found";

/// Protocol framing header names (not event payload -- these stay as constants).
pub const HEADER_CONTENT_TYPE: &str = "Content-Type";
/// Protocol framing header: body length.
pub const HEADER_CONTENT_LENGTH: &str = "Content-Length";
/// Protocol framing header: command reply status.
pub const HEADER_REPLY_TEXT: &str = "Reply-Text";

/// Connection timeout in milliseconds
pub const DEFAULT_TIMEOUT_MS: u64 = 2000;

/// Maximum number of queued events before dropping
pub const MAX_EVENT_QUEUE_SIZE: usize = 1000;

/// How long the reader waits on a silent socket before waking to re-check the
/// liveness threshold and the re-exec stop signal.
pub const READER_TICK_MS: u64 = 2000;

/// Maximum time (ms) to drain an in-progress message body during re-exec teardown
#[cfg(unix)]
pub const REEXEC_DRAIN_TIMEOUT_MS: u64 = 5000;

/// The `filter delete` header mod_event_socket reads, in any case, as
/// "every filter".
pub const FILTER_DELETE_ALL: &str = "all";
