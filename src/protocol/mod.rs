//! ESL protocol parsing and message handling

// qual:allow(coupling, sdp) reason: "the parser owns its read buffer"
use crate::{
    buffer::EslBuffer,
    constants::{
        CONTENT_TYPE_API_RESPONSE, CONTENT_TYPE_AUTH_REQUEST, CONTENT_TYPE_COMMAND_REPLY,
        CONTENT_TYPE_LOG_DATA, CONTENT_TYPE_TEXT_EVENT_JSON, CONTENT_TYPE_TEXT_EVENT_PLAIN,
        CONTENT_TYPE_TEXT_EVENT_XML, HEADER_CONTENT_LENGTH, HEADER_CONTENT_TYPE, HEADER_TERMINATOR,
        MAX_MESSAGE_SIZE, UNDEF_VALUE,
    },
    error::{EslError, EslResult},
    event::{EslEvent, EventFormat},
    headers::normalize_header_key,
    LossyValue, LossyValues,
};
use indexmap::IndexMap;
use percent_encoding::percent_decode_str;

mod event_format;
mod xml;

pub(crate) use event_format::{decode_serialized_event, DecodeOptions};

/// Decode one `Key: value` block, handing every decoded pair to `sink`.
///
/// `skip_undef` drops the empty-value sentinel before the sink sees it, which
/// suits a read-back and not the pushed event stream, where the sentinel is
/// what the switch sent.
fn decode_header_block(
    block: &str,
    context: &'static str,
    skip_undef: bool,
    mut lossy: Option<&mut LossyValues>,
    mut sink: impl FnMut(String, String),
) -> EslResult<()> {
    for line in block.lines() {
        let Some((key, raw_value)) = EslParser::parse_header_line(line)? else {
            continue;
        };
        let value = EslParser::decode_value(&key, &raw_value, context, lossy.as_deref_mut())?;
        if skip_undef && value == UNDEF_VALUE {
            continue;
        }
        sink(key, value);
    }
    Ok(())
}

/// ESL message types.
///
/// An unrecognized `Content-Type` is a protocol error rather than an `Unknown`
/// variant; see [`MessageType::from_content_type`].
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub(crate) enum MessageType {
    /// Authentication request from server
    AuthRequest,
    /// Command reply
    CommandReply,
    /// API response
    ApiResponse,
    /// Event message
    Event,
    /// Disconnect notice
    Disconnect,
    /// Connection rejected by ACL (text/rude-rejection)
    RudeRejection,
}

impl MessageType {
    /// Parse message type from a `Content-Type` header value.
    ///
    /// Returns [`EslError::ProtocolError`] if the value is not one of
    /// the recognized wire content-types — there is no `Unknown` fallback
    /// since correctness over recovery is preferred (see project
    /// CLAUDE.md). Mirrors the precedent set for `EventFormat`.
    pub fn from_content_type(content_type: &str) -> EslResult<Self> {
        match content_type {
            CONTENT_TYPE_AUTH_REQUEST => Ok(MessageType::AuthRequest),
            CONTENT_TYPE_COMMAND_REPLY => Ok(MessageType::CommandReply),
            CONTENT_TYPE_API_RESPONSE => Ok(MessageType::ApiResponse),
            CONTENT_TYPE_TEXT_EVENT_PLAIN
            | CONTENT_TYPE_TEXT_EVENT_JSON
            | CONTENT_TYPE_TEXT_EVENT_XML
            | CONTENT_TYPE_LOG_DATA => Ok(MessageType::Event),
            "text/disconnect-notice" => Ok(MessageType::Disconnect),
            "text/rude-rejection" => Ok(MessageType::RudeRejection),
            other => Err(EslError::protocol_error(format!(
                "Unrecognized Content-Type: {other}"
            ))),
        }
    }
}

/// Parsed ESL message: the parser's intermediate, consumed by the connection
/// module. What callers see is a command reply or an [`EslEvent`].
#[derive(Debug, Clone)]
pub(crate) struct EslMessage {
    /// Message type
    pub message_type: MessageType,
    /// Message headers
    pub headers: IndexMap<String, String>,
    /// Message body (optional)
    pub body: Option<String>,
    /// Exact wire bytes of a body that was not valid UTF-8; `body` then
    /// holds the U+FFFD-substituted string. `None` in the normal case.
    pub raw_body: Option<Vec<u8>>,
    /// Header keys whose percent-decoded value was not valid UTF-8 and was
    /// decoded lossily. Empty for framing-only envelopes; populated for
    /// serialized responses such as the outbound `connect` channel data.
    pub lossy_values: LossyValues,
}

impl EslMessage {
    /// Create new message
    pub fn new(
        message_type: MessageType,
        headers: IndexMap<String, String>,
        body: Option<String>,
    ) -> Self {
        Self {
            message_type,
            headers,
            body,
            raw_body: None,
            lossy_values: LossyValues::default(),
        }
    }

    /// Attach the lossy-decode signal recorded while parsing the headers.
    pub fn with_lossy_values(mut self, lossy_values: LossyValues) -> Self {
        self.lossy_values = lossy_values;
        self
    }
}

/// A message whose headers are consumed and whose body has not all arrived.
#[derive(Debug)]
struct PendingBody {
    message_type: MessageType,
    headers: IndexMap<String, String>,
    body_length: usize,
    lossy_values: LossyValues,
}

/// Parser state for handling incomplete messages
#[derive(Debug)]
enum ParseState {
    WaitingForHeaders,
    WaitingForBody(PendingBody),
}

/// ESL protocol parser
pub(crate) struct EslParser {
    buffer: EslBuffer,
    state: ParseState,
    strict_header_utf8: bool,
}

impl EslParser {
    /// Create new parser
    pub fn new() -> Self {
        Self {
            buffer: EslBuffer::new(),
            state: ParseState::WaitingForHeaders,
            strict_header_utf8: false,
        }
    }

    /// Set strict UTF-8 validation on event-body header values.
    pub fn with_strict_header_utf8(mut self, strict: bool) -> Self {
        self.strict_header_utf8 = strict;
        self
    }

    /// Unconsumed bytes remaining in the parser buffer.
    pub fn remaining_bytes(&self) -> &[u8] {
        self.buffer
            .data()
    }

    /// Returns `true` if the parser is between messages (not mid-body).
    pub fn is_waiting_for_headers(&self) -> bool {
        matches!(self.state, ParseState::WaitingForHeaders)
    }

    /// Discard all buffered data and reset to `WaitingForHeaders`.
    ///
    /// Used after salvaging a truncated auth response where the data
    /// was parsed externally via `parse_headers()`.
    pub(crate) fn drain_buffer(&mut self) {
        debug_assert!(
            self.is_waiting_for_headers(),
            "drain_buffer called outside WaitingForHeaders state"
        );
        self.buffer
            .clear();
        self.buffer
            .compact();
    }

    /// Add data to the parser buffer
    pub fn add_data(&mut self, data: &[u8]) -> EslResult<()> {
        self.buffer
            .extend_from_slice(data);
        self.buffer
            .check_size_limits()?;
        Ok(())
    }

    /// Try to parse a complete message from the buffer
    pub fn parse_message(&mut self) -> EslResult<Option<EslMessage>> {
        // Take the state so the body frame can move its fields into the
        // finished message; every path below assigns the next state.
        match std::mem::replace(&mut self.state, ParseState::WaitingForHeaders) {
            ParseState::WaitingForHeaders => self.parse_header_frame(),
            ParseState::WaitingForBody(pending) => self.parse_body_frame(pending),
        }
    }

    /// Consume a `\n\n`-terminated header block, then either finish the message
    /// or hand a `Content-Length` on to [`parse_body_frame`](Self::parse_body_frame).
    fn parse_header_frame(&mut self) -> EslResult<Option<EslMessage>> {
        let Some(headers_data) = self
            .buffer
            .extract_until_pattern(HEADER_TERMINATOR.as_bytes())
        else {
            return Ok(None);
        };
        self.buffer
            .compact();

        let headers_str = String::from_utf8(headers_data)
            .map_err(|_| EslError::protocol_error("Invalid UTF-8 in headers"))?;
        let (headers, lossy_values) = self.parse_headers(&headers_str)?;

        // Every ESL message must have Content-Type. Missing means
        // protocol desync (e.g. from a corrupted Content-Length).
        let content_type = headers
            .get(HEADER_CONTENT_TYPE)
            .ok_or_else(|| {
                EslError::protocol_error("Missing Content-Type header -- likely protocol desync")
            })?;
        let message_type = MessageType::from_content_type(content_type)?;

        let body_length = match headers.get(HEADER_CONTENT_LENGTH) {
            Some(length_str) => length_str
                .parse()
                .map_err(|_| EslError::InvalidHeader {
                    header: format!("Content-Length: {}", length_str),
                })?,
            // The outbound `connect` response arrives without one: a serialized
            // event whose values are percent-encoded, so lossy_values may be set.
            None => 0,
        };

        if body_length > MAX_MESSAGE_SIZE {
            return Err(EslError::protocol_error(format!(
                "Message too large: Content-Length {} exceeds limit {}. Protocol error or corrupted data.",
                body_length, MAX_MESSAGE_SIZE
            )));
        }

        if body_length == 0 {
            return Ok(Some(
                EslMessage::new(message_type, headers, None).with_lossy_values(lossy_values),
            ));
        }

        self.parse_body_frame(PendingBody {
            message_type,
            headers,
            body_length,
            lossy_values,
        })
    }

    /// Complete a message whose headers are already consumed, or park it back
    /// in the parser state until the framed byte count has arrived.
    fn parse_body_frame(&mut self, pending: PendingBody) -> EslResult<Option<EslMessage>> {
        let Some(body_data) = self
            .buffer
            .extract_bytes(pending.body_length)
        else {
            self.state = ParseState::WaitingForBody(pending);
            return Ok(None);
        };
        self.buffer
            .compact();

        // Content-Length frames the body in bytes, so non-UTF-8 is no desync:
        // decode lossily and keep the wire bytes, unless strict says otherwise.
        let mut raw_body = None;
        let body_str = match String::from_utf8(body_data) {
            Ok(s) => s,
            Err(e) if self.strict_header_utf8 => {
                return Err(EslError::protocol_error(format!(
                    "Invalid UTF-8 in body: {}",
                    e.utf8_error()
                )));
            }
            Err(e) => {
                let bytes = e.into_bytes();
                let lossy = String::from_utf8_lossy(&bytes).into_owned();
                raw_body = Some(bytes);
                lossy
            }
        };

        let mut message = EslMessage::new(pending.message_type, pending.headers, Some(body_str))
            .with_lossy_values(pending.lossy_values);
        message.raw_body = raw_body;
        Ok(Some(message))
    }

    /// Parse a single `Key: value` line, stripping `\r` and normalizing the key.
    ///
    /// Returns the on-wire `(key, value)` without percent-decoding. Returns
    /// `Ok(None)` for blank lines (caller keeps looping), `Ok(Some((k, v)))`
    /// on success, `Err` when the line is non-blank but lacks a colon.
    fn parse_header_line(line: &str) -> EslResult<Option<(String, String)>> {
        let line = line
            .strip_suffix('\r')
            .unwrap_or(line);
        if line.is_empty() {
            return Ok(None);
        }
        let Some(colon_pos) = line.find(':') else {
            return Err(EslError::InvalidHeader {
                header: line.to_string(),
            });
        };
        let key = normalize_header_key(&line[..colon_pos]);
        let raw_value = line[colon_pos + 1..]
            .strip_prefix(' ')
            .unwrap_or(&line[colon_pos + 1..]);
        Ok(Some((key, raw_value.to_string())))
    }

    /// Parse a header block, percent-decoding each value.
    ///
    /// FreeSWITCH percent-encodes serialized-event values, including the
    /// outbound `connect` response channel data that flows through this path.
    /// Pushed-event framing headers (`Content-Length`/`Content-Type`) are not
    /// encoded, but decoding them is a no-op. Non-UTF-8 values are decoded
    /// lossily and recorded in the returned `LossyValues` unless
    /// `strict_header_utf8` is set, in which case they error.
    pub(crate) fn parse_headers(
        &self,
        headers_str: &str,
    ) -> EslResult<(IndexMap<String, String>, LossyValues)> {
        let mut headers = IndexMap::new();
        let mut lossy = LossyValues::default();
        decode_header_block(
            headers_str,
            "header",
            false,
            Self::lossy_sink(self.strict_header_utf8, &mut lossy),
            |key, value| {
                headers.insert(key, value);
            },
        )?;
        Ok((headers, lossy))
    }

    /// Where [`decode_value`](Self::decode_value) records, or `None` under
    /// `strict`. No `self`: [`decode_serialized_event`] holds no parser.
    fn lossy_sink(strict: bool, acc: &mut LossyValues) -> Option<&mut LossyValues> {
        if strict {
            None
        } else {
            Some(acc)
        }
    }

    /// Parse event from message, handling different formats.
    ///
    /// log/data messages use single-level framing (metadata in outer envelope,
    /// raw log text as body) unlike normal events which use two-level framing.
    pub fn parse_event(&self, message: EslMessage, format: EventFormat) -> EslResult<EslEvent> {
        if message
            .headers
            .get(HEADER_CONTENT_TYPE)
            .map(|s| s.as_str())
            == Some(CONTENT_TYPE_LOG_DATA)
        {
            return Self::parse_log_event(message);
        }

        let event = match format {
            EventFormat::Plain => self.parse_plain_event(message),
            EventFormat::Json => self.parse_json_event(message),
            EventFormat::Xml => self.parse_xml_event(message),
            _ => {
                return Err(EslError::ProtocolError {
                    message: format!("unsupported event format: {format}"),
                })
            }
        }?;

        Ok(event)
    }

    /// Percent-decode a header value; `context` labels the error site. A
    /// `lossy` accumulator records U+FFFD substitutions instead of erroring.
    fn decode_value(
        key: &str,
        raw_value: &str,
        context: &'static str,
        lossy: Option<&mut LossyValues>,
    ) -> EslResult<String> {
        match percent_decode_str(raw_value).decode_utf8() {
            Ok(cow) => Ok(cow.into_owned()),
            Err(source) => match lossy {
                None => Err(EslError::InvalidUtf8InHeader {
                    context,
                    key: key.to_string(),
                    source,
                }),
                Some(acc) => {
                    acc.push(LossyValue::new(key.to_string(), raw_value.to_string()));
                    Ok(percent_decode_str(raw_value)
                        .decode_utf8_lossy()
                        .into_owned())
                }
            },
        }
    }

    /// Merge the envelope's lossy signal with the event's own; every format's
    /// parser ends here, so neither half can shadow the other.
    fn carry_lossy_signal(event: &mut EslEvent, lossy: LossyValues, raw_body: Option<Vec<u8>>) {
        if !lossy.is_empty() {
            let mut merged = lossy;
            for value in event
                .lossy_values()
                .iter()
            {
                merged.push(value.clone());
            }
            event.set_lossy_values(merged);
        }
        if let Some(raw) = raw_body {
            event.set_raw_body(raw);
        }
    }
}

impl Default for EslParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EslEventType;
    use crate::headers::EventHeader;
    use crate::lookup::HeaderLookup;

    #[test]
    fn test_parse_headers() {
        let parser = EslParser::new();
        let headers_str = "Content-Type: auth/request\r\nContent-Length: 0";
        let (headers, _lossy) = parser
            .parse_headers(headers_str)
            .unwrap();

        assert_eq!(
            headers
                .get("Content-Type")
                .map(|s| s.as_str()),
            Some("auth/request")
        );
        assert_eq!(
            headers
                .get("Content-Length")
                .map(|s| s.as_str()),
            Some("0")
        );
    }

    #[test]
    fn parsed_headers_preserve_insertion_order() {
        let parser = EslParser::new();
        let headers_str = "Alpha: 1\r\nBravo: 2\r\nCharlie: 3\r\nDelta: 4";
        let (headers, _lossy) = parser
            .parse_headers(headers_str)
            .unwrap();
        let keys: Vec<&str> = headers
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(keys, vec!["Alpha", "Bravo", "Charlie", "Delta"]);
    }

    #[test]
    fn test_parse_auth_request() {
        let mut parser = EslParser::new();
        let data = b"Content-Type: auth/request\n\n";

        parser
            .add_data(data)
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();

        assert_eq!(message.message_type, MessageType::AuthRequest);
        assert!(message
            .body
            .is_none());
    }

    #[test]
    fn test_parse_api_response() {
        let mut parser = EslParser::new();
        let data = b"Content-Type: api/response\nContent-Length: 2\n\nOK";

        parser
            .add_data(data)
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();

        assert_eq!(message.message_type, MessageType::ApiResponse);
        assert_eq!(message.body, Some("OK".to_string()));
    }

    #[test]
    fn test_incomplete_message() {
        let mut parser = EslParser::new();
        let data = b"Content-Type: api/response\nContent-Length: 10\n\ntest"; // Only 4 bytes instead of 10

        parser
            .add_data(data)
            .unwrap();
        let result = parser
            .parse_message()
            .unwrap();

        assert!(result.is_none()); // Should return None for incomplete message
    }

    #[test]
    fn test_crlf_header_terminator_not_matched() {
        // ESL uses \n\n, not \r\n\r\n. If something injects \r\n line endings,
        // the parser must not hang -- but it won't find the terminator either.
        // This documents the current behavior: \r\n\r\n is NOT recognized as
        // a header terminator, so the message stays incomplete.
        let mut parser = EslParser::new();
        let data = b"Content-Type: auth/request\r\n\r\n";

        parser
            .add_data(data)
            .unwrap();
        let result = parser
            .parse_message()
            .unwrap();
        assert!(
            result.is_none(),
            "\\r\\n\\r\\n should not match \\n\\n terminator"
        );
    }

    #[test]
    fn test_crlf_in_header_values_parsed_correctly() {
        // If \r\n appears within a \n\n-framed message, parse_headers()
        // uses .lines() which strips \r, so header values stay clean.
        let mut parser = EslParser::new();
        let data = b"Content-Type: auth/request\r\nSome-Header: some-value\n\n";

        parser
            .add_data(data)
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        assert_eq!(message.message_type, MessageType::AuthRequest);
        assert_eq!(
            message
                .headers
                .get("Some-Header")
                .map(|s| s.as_str()),
            Some("some-value")
        );
    }

    #[test]
    fn test_oversized_content_length_rejected() {
        let mut parser = EslParser::new();
        let data = format!(
            "Content-Type: api/response\nContent-Length: {}\n\n",
            MAX_MESSAGE_SIZE + 1
        );

        parser
            .add_data(data.as_bytes())
            .unwrap();
        let result = parser.parse_message();
        assert!(
            result.is_err(),
            "Content-Length exceeding MAX_MESSAGE_SIZE must be rejected"
        );
    }

    #[test]
    fn test_undersized_content_length_corrupts_next_message() {
        // Content-Length: 2 but body is "Hello" (5 bytes). The parser trusts
        // Content-Length and reads only 2 bytes, leaving "llo" in the buffer.
        // The next parse attempt sees "llo" as the start of a new message,
        // which won't have a valid header terminator -- so it returns None.
        let mut parser = EslParser::new();
        let data = b"Content-Type: api/response\nContent-Length: 2\n\nHello";

        parser
            .add_data(data)
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        assert_eq!(message.message_type, MessageType::ApiResponse);
        assert_eq!(message.body, Some("He".to_string()));

        // Leftover "llo" is now junk in the buffer -- next parse finds nothing
        let next = parser
            .parse_message()
            .unwrap();
        assert!(
            next.is_none(),
            "Leftover bytes should not form a valid message"
        );
    }

    #[test]
    fn test_undersized_content_length_followed_by_valid_message() {
        // Same scenario but a valid second message follows the junk.
        // The leftover bytes merge with the next message's headers,
        // making recovery impossible without reconnecting.
        let mut parser = EslParser::new();
        let msg1 = b"Content-Type: api/response\nContent-Length: 2\n\nHello";
        let msg2 = b"Content-Type: auth/request\n\n";

        parser
            .add_data(msg1)
            .unwrap();
        let first = parser
            .parse_message()
            .unwrap()
            .unwrap();
        assert_eq!(first.body, Some("He".to_string()));

        parser
            .add_data(msg2)
            .unwrap();
        let second = parser.parse_message();
        // "llo" + msg2 bytes = "lloContent-Type: auth/request\n\n"
        // The parser finds \n\n and parses "lloContent-Type: auth/request"
        // as key="lloContent-Type" value="auth/request". No real Content-Type
        // header exists, so the parser returns a protocol error -- signaling
        // the caller to disconnect.
        assert!(
            second.is_err(),
            "Desync must be detected as a protocol error"
        );
    }

    #[test]
    fn test_non_numeric_content_length_rejected() {
        let mut parser = EslParser::new();
        let data = b"Content-Type: api/response\nContent-Length: abc\n\n";

        parser
            .add_data(data)
            .unwrap();
        let result = parser.parse_message();
        assert!(
            result.is_err(),
            "Non-numeric Content-Length must be rejected"
        );
    }

    #[test]
    fn test_parse_headers_percent_decodes_values() {
        // parse_headers decodes values: the outbound connect response is a
        // serialized event (switch_event_serialize SWITCH_TRUE) whose values
        // are percent-encoded, and that response flows through this path.
        let parser = EslParser::new();
        let (headers, lossy) = parser
            .parse_headers("Channel-Name: sofia/internal/1000%40example.com\nX-Space: a%20b")
            .unwrap();

        assert_eq!(
            headers
                .get("Channel-Name")
                .map(|s| s.as_str()),
            Some("sofia/internal/1000@example.com")
        );
        assert_eq!(
            headers
                .get("X-Space")
                .map(|s| s.as_str()),
            Some("a b")
        );
        assert!(lossy.is_empty());
    }

    /// A raw non-UTF-8 byte in the envelope is a framing fault, so it ends the
    /// session, and the decoder's offset survives as the error's source.
    #[test]
    fn non_utf8_envelope_keeps_the_decoder_error() {
        let mut parser = EslParser::new();
        parser
            .add_data(b"Content-Type: command/reply\nReply-Text: +OK \xff\n\n")
            .unwrap();
        let err = parser
            .parse_message()
            .unwrap_err();
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
    fn test_connect_response_non_utf8_value_strict_error() {
        let mut parser = EslParser::new().with_strict_header_utf8(true);
        let data = "Content-Type: command/reply\nCaller-Caller-ID-Name: Andr%E9\n\n";
        parser
            .add_data(data.as_bytes())
            .unwrap();
        let result = parser.parse_message();
        assert!(matches!(result, Err(EslError::InvalidUtf8InHeader { .. })));
    }

    #[test]
    fn test_message_body_non_utf8_lossy_default() {
        // A raw event body is framed by Content-Length in bytes and is not
        // percent-encoded; non-UTF-8 bytes there must not kill the connection.
        let mut parser = EslParser::new();
        let mut data = b"Content-Type: text/event-plain\nContent-Length: 4\n\n".to_vec();
        data.extend_from_slice(b"caf\xE9");
        parser
            .add_data(&data)
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();

        assert_eq!(message.message_type, MessageType::Event);
        assert_eq!(
            message
                .body
                .as_deref(),
            Some("caf\u{FFFD}")
        );
        assert_eq!(
            message
                .raw_body
                .as_deref(),
            Some(&b"caf\xE9"[..])
        );
    }

    #[test]
    fn test_message_body_non_utf8_strict_error() {
        let mut parser = EslParser::new().with_strict_header_utf8(true);
        let mut data = b"Content-Type: text/event-plain\nContent-Length: 4\n\n".to_vec();
        data.extend_from_slice(b"caf\xE9");
        parser
            .add_data(&data)
            .unwrap();
        assert!(matches!(
            parser.parse_message(),
            Err(EslError::ProtocolError { .. })
        ));
    }

    #[test]
    fn test_waiting_for_body_multi_chunk() {
        let mut parser = EslParser::new();

        // Send headers first (with body length)
        let headers = b"Content-Type: api/response\nContent-Length: 20\n\n";
        parser
            .add_data(headers)
            .unwrap();

        // Parser transitions to WaitingForBody, returns None
        let result = parser
            .parse_message()
            .unwrap();
        assert!(result.is_none(), "should be waiting for body data");

        // Send first chunk (10 of 20 bytes)
        parser
            .add_data(b"0123456789")
            .unwrap();
        let result = parser
            .parse_message()
            .unwrap();
        assert!(result.is_none(), "still waiting for remaining body data");

        // Send remaining 10 bytes
        parser
            .add_data(b"abcdefghij")
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        assert_eq!(message.message_type, MessageType::ApiResponse);
        assert_eq!(message.body, Some("0123456789abcdefghij".to_string()));
    }

    #[test]
    fn test_rude_rejection_message_type() {
        let mt = MessageType::from_content_type("text/rude-rejection").unwrap();
        assert_eq!(mt, MessageType::RudeRejection);
    }

    #[test]
    fn test_unknown_content_type_is_protocol_error() {
        let err = MessageType::from_content_type("text/something-new").unwrap_err();
        assert!(matches!(err, EslError::ProtocolError { .. }));
    }

    #[test]
    fn test_missing_colon_invalid_header() {
        let result = EslParser::parse_header_line("NoColonHere");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EslError::InvalidHeader { .. }
        ));
    }

    #[test]
    fn test_lossy_values_serde_roundtrip() {
        let parser = EslParser::new();
        let body = "Event-Name: HEARTBEAT\nkey1: %E9value\n\n";
        let msg = EslMessage::new(
            MessageType::Event,
            {
                let mut h = IndexMap::new();
                h.insert("Content-Type".to_string(), "text/event-plain".to_string());
                h
            },
            Some(body.to_string()),
        );
        let event = parser
            .parse_event(msg, EventFormat::Plain)
            .unwrap();

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: EslEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(
            deserialized
                .lossy_values()
                .iter()
                .count(),
            1
        );
        assert_eq!(
            deserialized
                .lossy_values()
                .iter()
                .next()
                .unwrap()
                .key(),
            "Key1"
        );
        assert_eq!(
            deserialized
                .lossy_values()
                .iter()
                .next()
                .unwrap()
                .raw_value(),
            "%E9value"
        );
    }

    #[test]
    fn test_lossy_values_old_json_without_field() {
        // Old JSON without lossy_values field deserializes to empty
        let json = r#"{"headers":{"Event-Name":"HEARTBEAT"},"body":null}"#;
        let event: EslEvent = serde_json::from_str(json).unwrap();
        assert!(event
            .lossy_values()
            .is_empty());
    }

    #[test]
    fn plain_event_merges_envelope_and_body_lossy_values() {
        let parser = EslParser::new();
        let mut envelope = LossyValues::default();
        envelope.push(LossyValue::new(
            "User-Data".to_string(),
            "Andr%E9".to_string(),
        ));
        let msg = EslMessage::new(
            MessageType::Event,
            {
                let mut h = IndexMap::new();
                h.insert("Content-Type".to_string(), "text/event-plain".to_string());
                h
            },
            Some("Event-Name: HEARTBEAT\nkey1: %E9foo\n\n".to_string()),
        )
        .with_lossy_values(envelope);

        let event = parser
            .parse_event(msg, EventFormat::Plain)
            .unwrap();

        let keys: Vec<&str> = event
            .lossy_values()
            .iter()
            .map(|v| v.key())
            .collect();
        assert_eq!(keys, vec!["User-Data", "Key1"]);
    }

    #[test]
    fn envelope_lossy_value_survives_a_framed_body() {
        // log/data is the one envelope carrying more than framing headers, so
        // it is where an outer header can decode lossily on a body-bearing
        // frame -- the signal must reach the event, not stop at the transition.
        let mut parser = EslParser::new();
        let log_text = "some log line\n";
        let data = format!(
            "Content-Type: log/data\nContent-Length: {}\nLog-Level: 7\nUser-Data: Andr%E9\n\n{}",
            log_text.len(),
            log_text
        );
        parser
            .add_data(data.as_bytes())
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        assert!(!message
            .lossy_values
            .is_empty());

        let event = parser
            .parse_event(message, EventFormat::Plain)
            .unwrap();

        assert_eq!(event.body(), Some(log_text));
        assert_eq!(event.header_str("User-Data"), Some("Andr\u{FFFD}"));
        let entry = event
            .lossy_values()
            .iter()
            .next()
            .unwrap();
        assert_eq!(entry.key(), "User-Data");
        assert_eq!(entry.raw_value(), "Andr%E9");
    }

    #[test]
    fn test_lossy_values_display_keys_only() {
        let parser = EslParser::new();
        let body = "Event-Name: HEARTBEAT\nkey1: %E9foo\nkey2: %FFbar\n\n";
        let msg = EslMessage::new(
            MessageType::Event,
            {
                let mut h = IndexMap::new();
                h.insert("Content-Type".to_string(), "text/event-plain".to_string());
                h
            },
            Some(body.to_string()),
        );
        let event = parser
            .parse_event(msg, EventFormat::Plain)
            .unwrap();

        let display = event
            .lossy_values()
            .to_string();
        assert_eq!(display, "Key1, Key2");
        assert!(!display.contains("%E9"));
        assert!(!display.contains("foo"));
    }

    /// Feed `data` split at `split`, driving parse_message across the two
    /// chunks, and parse the resulting frame as an event of `format`.
    fn parse_wire_split(data: &[u8], split: usize, format: EventFormat) -> EslEvent {
        let mut parser = EslParser::new();
        parser
            .add_data(&data[..split])
            .unwrap();
        let mut message = parser
            .parse_message()
            .unwrap();
        assert!(
            message.is_none(),
            "frame must be incomplete at split {split}"
        );
        parser
            .add_data(&data[split..])
            .unwrap();
        message = parser
            .parse_message()
            .unwrap();
        let message =
            message.unwrap_or_else(|| panic!("frame incomplete after both chunks (split {split})"));
        parser
            .parse_event(message, format)
            .unwrap()
    }

    #[test]
    fn plain_event_frame_split_all_points() {
        // Inner body included so splits land mid-envelope-header, between the
        // two terminator \n bytes, mid-event-header, and mid-inner-body.
        let inner_body = "+OK result\n";
        let body = format!(
            "Event-Name: BACKGROUND_JOB\nJob-UUID: abc-123\nContent-Length: {}\n\n{}",
            inner_body.len(),
            inner_body
        );
        let data = format!(
            "Content-Length: {}\nContent-Type: text/event-plain\n\n{}",
            body.len(),
            body
        )
        .into_bytes();

        for split in 1..data.len() {
            let event = parse_wire_split(&data, split, EventFormat::Plain);
            assert_eq!(
                event.event_type(),
                Some(EslEventType::BackgroundJob),
                "split {split}"
            );
            assert_eq!(
                event.header(EventHeader::JobUuid),
                Some("abc-123"),
                "split {split}"
            );
            assert_eq!(event.body(), Some(inner_body), "split {split}");
        }
    }

    #[test]
    fn json_event_frame_split_all_points() {
        let json_body =
            r#"{"Event-Name":"BACKGROUND_JOB","Job-UUID":"abc-123","_body":"+OK result"}"#;
        let data = format!(
            "Content-Length: {}\nContent-Type: text/event-json\n\n{}",
            json_body.len(),
            json_body
        )
        .into_bytes();

        for split in 1..data.len() {
            let event = parse_wire_split(&data, split, EventFormat::Json);
            assert_eq!(
                event.event_type(),
                Some(EslEventType::BackgroundJob),
                "split {split}"
            );
            assert_eq!(event.body(), Some("+OK result"), "split {split}");
        }
    }

    #[test]
    fn xml_event_frame_split_all_points() {
        let xml_body = "<event>\n  <headers>\n    <Event-Name>BACKGROUND_JOB</Event-Name>\n    <Job-UUID>abc-123</Job-UUID>\n  </headers>\n  <body>+OK result</body>\n</event>";
        let data = format!(
            "Content-Length: {}\nContent-Type: text/event-xml\n\n{}",
            xml_body.len(),
            xml_body
        )
        .into_bytes();

        for split in 1..data.len() {
            let event = parse_wire_split(&data, split, EventFormat::Xml);
            assert_eq!(
                event.event_type(),
                Some(EslEventType::BackgroundJob),
                "split {split}"
            );
            assert_eq!(event.body(), Some("+OK result"), "split {split}");
        }
    }

    #[test]
    fn multiple_event_frames_in_single_read() {
        let mut parser = EslParser::new();

        let bodies = [
            "Event-Name: CHANNEL_CREATE\nUnique-ID: uuid-1\n\n",
            "Event-Name: CHANNEL_ANSWER\nUnique-ID: uuid-2\n\n",
            "Event-Name: CHANNEL_DESTROY\nUnique-ID: uuid-3\n\n",
        ];
        let mut data = Vec::new();
        for body in &bodies {
            data.extend_from_slice(
                format!(
                    "Content-Length: {}\nContent-Type: text/event-plain\n\n{}",
                    body.len(),
                    body
                )
                .as_bytes(),
            );
        }

        parser
            .add_data(&data)
            .unwrap();

        let expected = [
            (EslEventType::ChannelCreate, "uuid-1"),
            (EslEventType::ChannelAnswer, "uuid-2"),
            (EslEventType::ChannelDestroy, "uuid-3"),
        ];
        for (event_type, uuid) in expected {
            let message = parser
                .parse_message()
                .unwrap()
                .expect("all frames arrived in one read");
            let event = parser
                .parse_event(message, EventFormat::Plain)
                .unwrap();
            assert_eq!(event.event_type(), Some(event_type));
            assert_eq!(event.unique_id(), Some(uuid));
        }
        assert!(parser
            .parse_message()
            .unwrap()
            .is_none());
    }
}
