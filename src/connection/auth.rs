use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tracing::{debug, trace, warn};

use super::{read_into_parser, ReadStep};
use crate::{
    command::{EslCommand, EslResponse, Secret},
    constants::{CONTENT_TYPE_COMMAND_REPLY, HEADER_CONTENT_TYPE},
    error::{EslError, EslResult},
    protocol::{EslMessage, EslParser, MessageType},
    HeaderLookup,
};

/// How an inbound connection authenticates.
///
/// The only carrier of a credential: every `connect` constructor builds one and
/// hands it to [`EslClient::connect_with_auth`](super::EslClient::connect_with_auth).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum AuthMethod {
    /// `auth <password>` — the shared password from `event_socket.conf.xml`.
    Password(Secret),
    /// `userauth <user>:<password>` — a directory user with its own ACL.
    User {
        /// Directory user, which FreeSWITCH requires in `user@domain` form.
        user: String,
        /// That user's password.
        password: Secret,
    },
}

impl AuthMethod {
    /// Authenticate with the shared password.
    pub fn password(password: impl Into<String>) -> Self {
        Self::Password(Secret::new(password))
    }

    /// Authenticate as a directory user, spelled `user@domain`.
    pub fn user(user: impl Into<String>, password: impl Into<String>) -> Self {
        Self::User {
            user: user.into(),
            password: Secret::new(password),
        }
    }
}

/// The handshake's I/O: the still-unsplit stream, the parser being seeded from
/// it, and the scratch buffer between the two.
pub(super) struct Handshake<'a> {
    pub(super) stream: &'a mut TcpStream,
    pub(super) parser: &'a mut EslParser,
    pub(super) read_buffer: &'a mut [u8],
}

impl Handshake<'_> {
    /// Read one ESL message, failing the whole handshake on an idle socket.
    async fn recv_message(&mut self, read_timeout: Duration) -> EslResult<EslMessage> {
        let timeout_ms = read_timeout.as_millis() as u64;
        loop {
            if let Some(message) = self
                .parser
                .parse_message()?
            {
                trace!(
                    "[RECV] Parsed message from buffer: {:?}",
                    message.message_type
                );
                return Ok(message);
            }

            trace!("[RECV] Buffer needs more data, reading from socket");
            match read_into_parser(self.stream, self.parser, self.read_buffer, read_timeout).await?
            {
                ReadStep::Fed => {}
                ReadStep::Eof => return Err(EslError::ConnectionClosed),
                ReadStep::Idle => return Err(EslError::Timeout { timeout_ms }),
            }
        }
    }
}

/// Perform authentication on the stream, returning the auth response.
///
/// For `userauth`, the response contains `Allowed-Events`, `Allowed-API`,
/// and `Allowed-LOG` headers describing the user's access policy.
pub(super) async fn authenticate(
    io: &mut Handshake<'_>,
    method: &AuthMethod,
    connect_timeout: Duration,
) -> EslResult<EslResponse> {
    debug!("[AUTH] Waiting for auth request from FreeSWITCH");
    let message = io
        .recv_message(connect_timeout)
        .await?;

    if message.message_type == MessageType::RudeRejection {
        let reason = message
            .body
            .unwrap_or_else(|| "rude-rejection without body".to_string());
        return Err(EslError::AccessDenied { reason });
    }

    if message.message_type != MessageType::AuthRequest {
        return Err(EslError::protocol_error("Expected auth request"));
    }

    let auth_cmd = match method {
        AuthMethod::Password(password) => EslCommand::Auth {
            password: password.clone(),
        },
        AuthMethod::User { user, password } => EslCommand::UserAuth {
            user: user.clone(),
            password: password.clone(),
        },
    };

    let command_str = auth_cmd.to_wire_format()?;
    debug!(">> {}", auth_cmd.redact_wire(&command_str));
    io.stream
        .write_all(command_str.as_bytes())
        .await
        .map_err(EslError::Io)?;

    let response_msg = match io
        .recv_message(connect_timeout)
        .await
    {
        Ok(msg) => msg,
        Err(EslError::Timeout { timeout_ms }) => {
            match salvage_truncated_auth_response(io.parser) {
                Ok(Some(msg)) => {
                    warn!(
                        "FreeSWITCH sent a truncated auth response \
                         (mod_event_socket.c reply[512] overflow). \
                         Allowed-API/Allowed-LOG headers may be incomplete or missing."
                    );
                    msg
                }
                // Nothing to salvage: the read timeout is the whole story. A
                // salvage that refused for a named reason is that reason.
                Ok(None) => return Err(EslError::Timeout { timeout_ms }),
                Err(e) => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let response = EslResponse::from_message(response_msg);

    if !response.is_success() {
        return Err(match response.reply_text() {
            Some(text) => EslError::auth_failed(text.to_string()),
            None => EslError::protocol_error("auth response missing Reply-Text header"),
        });
    }

    debug!("Authentication successful");
    Ok(response)
}

/// Salvage a truncated `userauth` response, on the auth path only.
///
/// `mod_event_socket.c` formats the reply into `char reply[512]`; a long
/// `Allowed-Events` list makes `switch_snprintf` truncate it, and the `\n\n`
/// the parser waits for is never written.
fn salvage_truncated_auth_response(parser: &mut EslParser) -> EslResult<Option<EslMessage>> {
    if !parser.is_waiting_for_headers() {
        return Ok(None);
    }

    let data = parser.remaining_bytes();
    if data.is_empty() {
        return Ok(None);
    }

    let data_str = std::str::from_utf8(data).map_err(|source| EslError::Utf8Error {
        context: "truncated auth response",
        source,
    })?;

    // Find the last newline. Everything before it is complete header lines.
    // Everything after may be a truncated line.
    let last_nl = match data_str.rfind('\n') {
        Some(pos) => pos,
        None => return Ok(None),
    };

    let trailing = &data_str[last_nl + 1..];

    // Build the header block to parse: all complete lines, plus the
    // trailing fragment only if it contains a colon (truncated value
    // is acceptable; truncated header name is not).
    let header_block = if trailing.is_empty() || trailing.contains(':') {
        data_str
    } else {
        &data_str[..last_nl]
    };

    let (headers, lossy_values) = parser.parse_headers(header_block)?;

    // Validate this is actually a command/reply (auth response).
    let content_type = headers
        .header_str(HEADER_CONTENT_TYPE)
        .ok_or_else(|| {
            EslError::protocol_error(
                "truncated response missing Content-Type header — not an auth response",
            )
        })?;

    if content_type != CONTENT_TYPE_COMMAND_REPLY {
        return Err(EslError::protocol_error(format!(
            "truncated response has Content-Type '{}', expected command/reply",
            content_type
        )));
    }

    let message_type = MessageType::from_content_type(content_type)?;
    let message = EslMessage::new(message_type, headers, None).with_lossy_values(lossy_values);

    parser.drain_buffer();

    Ok(Some(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[test]
    fn salvage_truncated_auth_response_realistic() {
        // Simulates FreeSWITCH reply[512] overflow: long Allowed-Events
        // header causes Allowed-API to be truncated and \n\n never sent.
        let wire_data = "\
            Content-Type: command/reply\n\
            Reply-Text: +OK accepted\n\
            Allowed-Events: HEARTBEAT BACKGROUND_JOB CHANNEL_CREATE \
            CHANNEL_ANSWER CHANNEL_HANGUP_COMPLETE CHANNEL_STATE \
            CHANNEL_DATA CHANNEL_CALLSTATE CHANNEL_EXECUTE \
            CHANNEL_EXECUTE_COMPLETE CHANNEL_BRIDGE NOTIFY_IN \
            CHANNEL_DESTROY CHANNEL_HANGUP CHANNEL_HOLD CHANNEL_UNHOLD \
            CHANNEL_UNBRIDGE CHANNEL_PROGRESS CHANNEL_PROGRESS_MEDIA \
            CHANNEL_OUTGOING CHANNEL_PARK CHANNEL_UNPARK \
            CHANNEL_APPLICATION CHANNEL_ORIGINATE CHANNEL_UUID CUSTOM \
            sofia::gateway_state sofia::gateway_delete\n\
            Allowed-API: show";

        let mut parser = EslParser::new();
        parser
            .add_data(wire_data.as_bytes())
            .unwrap();

        // Normal parse should return None — no \n\n terminator
        assert!(parser
            .parse_message()
            .unwrap()
            .is_none());

        let msg = salvage_truncated_auth_response(&mut parser)
            .unwrap()
            .expect("salvage should succeed");

        assert_eq!(msg.message_type, MessageType::CommandReply);
        assert_eq!(
            msg.headers
                .header_str("Reply-Text"),
            Some("+OK accepted")
        );
        assert!(msg
            .headers
            .header_str("Allowed-Events")
            .is_some());
        // Truncated value is preserved
        assert_eq!(
            msg.headers
                .header_str("Allowed-Api"),
            Some("show")
        );

        // Parser buffer is drained
        assert_eq!(
            parser
                .remaining_bytes()
                .len(),
            0
        );
    }

    #[test]
    fn salvage_truncated_mid_header_name() {
        // Truncation cut mid-header-name (no colon in trailing fragment)
        let wire_data = "Content-Type: command/reply\nReply-Text: +OK accepted\nAllo";

        let mut parser = EslParser::new();
        parser
            .add_data(wire_data.as_bytes())
            .unwrap();

        let msg = salvage_truncated_auth_response(&mut parser)
            .unwrap()
            .expect("salvage should succeed with partial line dropped");

        assert_eq!(msg.message_type, MessageType::CommandReply);
        assert_eq!(
            msg.headers
                .header_str("Reply-Text"),
            Some("+OK accepted")
        );
        // "Allo" fragment was dropped
        assert!(msg
            .headers
            .header_str("Allo")
            .is_none());
    }

    #[test]
    fn salvage_empty_buffer_returns_none() {
        let mut parser = EslParser::new();
        assert!(salvage_truncated_auth_response(&mut parser)
            .unwrap()
            .is_none());
    }

    #[test]
    fn salvage_no_newline_returns_none() {
        let mut parser = EslParser::new();
        parser
            .add_data(b"Content-Type: command/reply")
            .unwrap();
        assert!(salvage_truncated_auth_response(&mut parser)
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn salvage_failure_surfaces_its_own_reason() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("[::1]:0")
            .await
            .unwrap();
        let addr = listener
            .local_addr()
            .unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener
                .accept()
                .await
                .unwrap();
            sock.write_all(b"Content-Type: auth/request\n\n")
                .await
                .unwrap();
            let mut discard = [0u8; 128];
            let n = sock
                .read(&mut discard)
                .await
                .unwrap();
            assert!(n > 0, "client sent no auth command");
            // A truncated block that is not an auth reply at all: the salvage
            // must say so rather than let the read timeout stand as the cause.
            sock.write_all(b"Content-Type: text/event-plain\nReply-Text: +OK\n")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(2)).await;
        });

        let mut stream = TcpStream::connect(addr)
            .await
            .unwrap();
        let mut parser = EslParser::new();
        let mut read_buffer = [0u8; 1024];
        let err = authenticate(
            &mut Handshake {
                stream: &mut stream,
                parser: &mut parser,
                read_buffer: &mut read_buffer,
            },
            &AuthMethod::password("ClueCon"),
            Duration::from_millis(200),
        )
        .await
        .expect_err("a non-auth truncated reply must not authenticate");

        assert!(
            matches!(err, EslError::ProtocolError { .. }),
            "got: {err:?}"
        );
        assert!(
            err.to_string()
                .contains("command/reply"),
            "error must name the check that failed: {err}"
        );
        server.abort();
    }

    #[test]
    fn salvage_of_non_utf8_keeps_the_decoder_error() {
        let mut parser = EslParser::new();
        parser
            .add_data(b"Content-Type: command/reply\nReply-Text: +OK \xff\n")
            .unwrap();
        let err = salvage_truncated_auth_response(&mut parser).unwrap_err();
        assert!(err.is_connection_error(), "{err}");
        assert!(!err.is_recoverable(), "{err}");
        let source = std::error::Error::source(&err).expect("no source");
        assert!(
            source
                .downcast_ref::<std::str::Utf8Error>()
                .is_some(),
            "{source}"
        );
    }

    #[test]
    fn salvage_wrong_content_type_returns_error() {
        let wire_data = "Content-Type: auth/request\n";
        let mut parser = EslParser::new();
        parser
            .add_data(wire_data.as_bytes())
            .unwrap();

        let result = salvage_truncated_auth_response(&mut parser);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("command/reply"),);
    }
}
