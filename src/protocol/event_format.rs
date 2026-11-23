//! Plain-text, log/data, and JSON event body parsing.

use super::{decode_header_block, EslMessage, EslParser, MessageType};
use crate::{
    constants::HEADER_TERMINATOR,
    error::{EslError, EslResult},
    event::{EslEvent, EslEventType},
    headers::EventHeader,
    LossyValues,
};

/// How [`decode_serialized_event`] reads a payload.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DecodeOptions {
    /// Which body the payload is (`"event body"`, `"response body"`), named by
    /// [`EslError::InvalidBodyLine`].
    pub(crate) body: &'static str,
    /// Hard-fail on a value that is not UTF-8 once percent-decoded, instead of
    /// substituting U+FFFD and recording the key.
    pub(crate) strict_utf8: bool,
    /// Drop a header whose value is the empty-value sentinel, which suits a
    /// read-back (a channel dump) and not the pushed event stream, where the
    /// sentinel is what the switch sent.
    pub(crate) skip_undef: bool,
}

/// Decode a `switch_event_serialize(encode=TRUE)` payload: percent-encoded
/// `Name: value` lines, then either nothing or `Content-Length` and a `\n\n`
/// followed by the inner body.
///
/// The returned event carries no `raw_body`: the payload is already a decoded
/// `&str`, so there are no exact wire bytes left to hand back.
pub(crate) fn decode_serialized_event(
    payload: &str,
    options: DecodeOptions,
) -> EslResult<EslEvent> {
    let mut event = EslEvent::new();
    let mut lossy = LossyValues::default();

    // Headers are terminated by \n\n; anything after is the inner body.
    let (header_section, inner_body) = match payload.find(HEADER_TERMINATOR) {
        Some(pos) => (
            &payload[..pos],
            Some(&payload[pos + HEADER_TERMINATOR.len()..]),
        ),
        None => (payload, None),
    };

    decode_header_block(
        header_section,
        "event header",
        options.skip_undef,
        EslParser::lossy_sink(options.strict_utf8, &mut lossy),
        |key, value| event.set_header(key, value),
    )
    .map_err(|e| match e {
        // The message framing held, so a colonless line is this body's fault.
        EslError::InvalidHeader { header } => EslError::InvalidBodyLine {
            context: options.body,
            line: header,
        },
        other => other,
    })?;

    event.set_lossy_values(lossy);

    if let Some(body) = inner_body.filter(|b| !b.is_empty()) {
        event.set_body(body.to_string());
    }

    Ok(event)
}

impl EslParser {
    /// Parse log/data message.
    ///
    /// FreeSWITCH log/data wire format uses single-level framing, unlike
    /// normal events. Log metadata (Log-Level, Log-File, etc.) lives in the
    /// outer envelope headers and the body is raw log text.
    pub(super) fn parse_log_event(message: EslMessage) -> EslResult<EslEvent> {
        let mut event = EslEvent::new();
        for (key, value) in message
            .headers
            .as_map()
        {
            event.set_header(key.clone(), value.clone());
        }
        if let Some(body) = message.body {
            event.set_body(body);
        }
        // Synthesize Event-Name so downstream event_type() resolves to
        // EslEventType::Log; FreeSWITCH does not include this header on
        // log/data envelopes.
        event.set_header(EventHeader::EventName.as_str(), EslEventType::Log.as_str());
        Self::carry_lossy_signal(&mut event, message.lossy_values, message.raw_body);
        Ok(event)
    }

    /// Parse plain text event
    ///
    /// FreeSWITCH text/event-plain wire format uses a two-part structure:
    /// - Outer envelope: Content-Length + Content-Type headers
    /// - Body: URL-encoded key: value lines (the actual event headers)
    ///
    /// If the event body itself contains a Content-Length, there's an inner
    /// body after the event headers.
    pub(super) fn parse_plain_event(&self, mut message: EslMessage) -> EslResult<EslEvent> {
        if message.message_type != MessageType::Event {
            return Err(EslError::protocol_error("Not an event message"));
        }

        let body = message
            .body
            .as_deref()
            .ok_or_else(|| EslError::protocol_error("Plain event missing body"))?;

        let mut event = decode_serialized_event(
            body,
            DecodeOptions {
                body: "event body",
                strict_utf8: self.strict_header_utf8,
                skip_undef: false,
            },
        )?;

        // The inner body's wire bytes are the tail of raw_body: event headers
        // are percent-encoded ASCII and U+FFFD substitution never touches
        // newline bytes, so the first \n\n falls at the same offset in bytes
        // as in the decoded string.
        let mut inner_raw = None;
        if event
            .body()
            .is_some()
        {
            if let Some(mut raw) = message
                .raw_body
                .take()
            {
                if let Some(pos) = raw
                    .windows(2)
                    .position(|w| w == b"\n\n")
                {
                    raw.drain(..pos + 2);
                    inner_raw = Some(raw);
                }
            }
        }

        Self::carry_lossy_signal(&mut event, message.lossy_values, inner_raw);
        Ok(event)
    }

    /// Parse JSON event
    pub(super) fn parse_json_event(&self, message: EslMessage) -> EslResult<EslEvent> {
        let body = message
            .body
            .ok_or_else(|| EslError::protocol_error("JSON event missing body"))?;

        // Parse JSON body
        let json_value: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| EslError::JsonError(Box::new(e)))?;

        let serde_json::Value::Object(map) = json_value else {
            return Err(EslError::protocol_error("JSON event body is not an object"));
        };

        let mut event = EslEvent::new();
        for (key, value) in map {
            let value_str = match value {
                serde_json::Value::String(s) => s,
                _ => value.to_string(),
            };
            // FreeSWITCH puts the event body under a "_body" key in JSON events
            if key == "_body" {
                event.set_body(value_str);
            } else {
                event.set_header(key, value_str);
            }
        }

        Self::carry_lossy_signal(&mut event, message.lossy_values, message.raw_body);
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventFormat;
    use crate::lookup::HeaderLookup;
    use indexmap::IndexMap;

    #[test]
    fn test_parse_event_plain() {
        let mut parser = EslParser::new();
        // Correct two-part wire format: outer envelope + body with event headers
        let body = "Event-Name: CHANNEL_ANSWER\nUnique-ID: test-uuid\n\n";
        let envelope = format!(
            "Content-Length: {}\nContent-Type: text/event-plain\n\n",
            body.len()
        );
        let data = format!("{}{}", envelope, body);

        parser
            .add_data(data.as_bytes())
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        let event = parser
            .parse_event(message, EventFormat::Plain)
            .unwrap();

        assert_eq!(event.event_type(), Some(EslEventType::ChannelAnswer));
        assert_eq!(event.unique_id(), Some("test-uuid"));
    }

    #[test]
    fn test_parse_event_plain_percent_decoding() {
        let mut parser = EslParser::new();
        let body = "Event-Name: HEARTBEAT\nUp-Time: 0%20years%2C%200%20days\nEvent-Info: System%20Ready\n\n";
        let envelope = format!(
            "Content-Length: {}\nContent-Type: text/event-plain\n\n",
            body.len()
        );
        let data = format!("{}{}", envelope, body);

        parser
            .add_data(data.as_bytes())
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        let event = parser
            .parse_event(message, EventFormat::Plain)
            .unwrap();

        assert_eq!(event.event_type(), Some(EslEventType::Heartbeat));
        assert_eq!(event.header_str("Up-Time"), Some("0 years, 0 days"));
        assert_eq!(event.header_str("Event-Info"), Some("System Ready"));
    }

    #[test]
    fn test_parse_event_plain_with_inner_body() {
        let mut parser = EslParser::new();
        // Event with inner body (e.g., BACKGROUND_JOB result)
        let inner_body = "+OK Status\n";
        let event_headers = format!(
            "Event-Name: BACKGROUND_JOB\nJob-UUID: abc-123\nContent-Length: {}\n",
            inner_body.len()
        );
        let body = format!("{}\n{}", event_headers, inner_body);
        let envelope = format!(
            "Content-Length: {}\nContent-Type: text/event-plain\n\n",
            body.len()
        );
        let data = format!("{}{}", envelope, body);

        parser
            .add_data(data.as_bytes())
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        let event = parser
            .parse_event(message, EventFormat::Plain)
            .unwrap();

        assert_eq!(event.event_type(), Some(EslEventType::BackgroundJob));
        assert_eq!(event.header(EventHeader::JobUuid), Some("abc-123"));
        assert_eq!(event.body(), Some("+OK Status\n"));
    }

    /// log/data uses single-level framing: metadata in outer envelope,
    /// raw log text as body. This matches mod_event_socket.c's output.
    #[test]
    fn test_parse_log_data_event() {
        let mut parser = EslParser::new();
        let log_text = "2024-01-01 00:00:00.000000 [INFO] mod_sofia.c:1234 Registration ok\n";
        let envelope = format!(
            "Content-Type: log/data\nContent-Length: {}\nLog-Level: 6\nText-Channel: 0\nLog-File: mod_sofia.c\nLog-Func: sofia_reg_handle\nLog-Line: 1234\nUser-Data: \n\n{}",
            log_text.len(),
            log_text,
        );

        parser
            .add_data(envelope.as_bytes())
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();

        assert_eq!(message.message_type, MessageType::Event);

        let event = parser
            .parse_event(message, EventFormat::Plain)
            .unwrap();

        assert_eq!(event.event_type(), Some(EslEventType::Log));
        assert_eq!(event.header(EventHeader::LogLevel), Some("6"));
        assert_eq!(event.header_str("Content-Type"), Some("log/data"));
        assert_eq!(event.header_str("Log-File"), Some("mod_sofia.c"));
        assert_eq!(event.header_str("Log-Func"), Some("sofia_reg_handle"));
        assert_eq!(event.header_str("Log-Line"), Some("1234"));
        assert_eq!(event.body(), Some(log_text));
    }

    #[test]
    fn test_notify_in_event_with_pl_data() {
        let mut parser = EslParser::new();
        // NOTIFY_IN event with percent-encoded pl_data containing JSON
        let json_payload = r#"{"Invite":"INVITE urn:service:sos SIP/2.0","InviteTimestamp":"2025-01-15T12:00:00Z"}"#;
        let encoded_payload =
            percent_encoding::utf8_percent_encode(json_payload, percent_encoding::NON_ALPHANUMERIC)
                .to_string();
        let body = format!(
            "Event-Name: NOTIFY_IN\nevent: emergency-AbandonedCall\npl_data: {}\nsip_content_type: application%2Fjson\ngateway_name: ng911-bcf\n\n",
            encoded_payload
        );
        let envelope = format!(
            "Content-Length: {}\nContent-Type: text/event-plain\n\n",
            body.len()
        );
        let data = format!("{}{}", envelope, body);

        parser
            .add_data(data.as_bytes())
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        let event = parser
            .parse_event(message, EventFormat::Plain)
            .unwrap();

        assert_eq!(event.event_type(), Some(EslEventType::NotifyIn));
        assert_eq!(event.header_str("event"), Some("emergency-AbandonedCall"));
        // pl_data must be percent-decoded back to raw JSON
        assert_eq!(event.header_str("pl_data"), Some(json_payload));
        assert_eq!(
            event.header_str("sip_content_type"),
            Some("application/json")
        );
        assert_eq!(event.header_str("gateway_name"), Some("ng911-bcf"));
    }

    #[test]
    fn test_parse_plain_event_invalid_utf8_strict_error() {
        // Event-body header with invalid UTF-8 in strict mode
        let parser = EslParser::new().with_strict_header_utf8(true);
        let body = "Event-Name: HEARTBEAT\nX-Bad: %FF\n\n";
        let msg = EslMessage::new(
            MessageType::Event,
            {
                let mut h = IndexMap::new();
                h.insert("Content-Type".to_string(), "text/event-plain".to_string());
                h
            },
            Some(body.to_string()),
        );
        let result = parser.parse_event(msg, EventFormat::Plain);
        assert!(
            result.is_err(),
            "invalid UTF-8 in event header with strict mode must be an error"
        );
    }

    /// A colonless line inside a plain event body leaves the framing intact: the
    /// error names the body and the line's length, never the line.
    #[test]
    fn a_colonless_line_in_an_event_body_is_a_body_fault() {
        let mut parser = EslParser::new();
        let event_body = "Event-Name: HEARTBEAT\nno-colon-sentinel\n\n";
        let data = format!(
            "Content-Type: text/event-plain\nContent-Length: {}\n\n{event_body}",
            event_body.len()
        );
        parser
            .add_data(data.as_bytes())
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        let err = parser
            .parse_event(message, EventFormat::Plain)
            .expect_err("a colonless line is refused");
        assert!(!matches!(err, EslError::InvalidHeader { .. }), "{err:?}");
        assert!(!err.is_connection_error(), "{err:?}");
        assert!(!err.is_recoverable(), "{err:?}");
        let shown = err.to_string();
        assert!(
            shown.contains("event body") && shown.contains("bytes"),
            "{shown}"
        );
        assert!(!shown.contains("no-colon-sentinel"), "{shown}");
    }

    #[test]
    fn test_plain_event_non_utf8_inner_body_lossy() {
        // sendevent payloads ride as a raw inner body after the
        // percent-encoded event headers; the raw bytes must reach the event.
        let mut parser = EslParser::new();
        let event_body: &[u8] = b"Event-Name: NOTIFY_IN\nContent-Length: 4\n\ncaf\xE9";
        let mut data = format!(
            "Content-Type: text/event-plain\nContent-Length: {}\n\n",
            event_body.len()
        )
        .into_bytes();
        data.extend_from_slice(event_body);
        parser
            .add_data(&data)
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        let event = parser
            .parse_event(message, EventFormat::Plain)
            .unwrap();

        assert_eq!(event.body(), Some("caf\u{FFFD}"));
        assert_eq!(event.raw_body(), Some(&b"caf\xE9"[..]));
    }

    // switch_event_serialize writes _undef_ for an empty value on the
    // text/event-plain path too. Only a read-back (a channel dump) reads it
    // as absent; the pushed stream must keep the header it was sent.
    #[test]
    fn plain_event_keeps_undef_values() {
        let parser = EslParser::new();
        let msg = EslMessage::new(
            MessageType::Event,
            {
                let mut h = IndexMap::new();
                h.insert("Content-Type".to_string(), "text/event-plain".to_string());
                h
            },
            Some("Event-Name: HEARTBEAT\nvariable_empty: _undef_\n\n".to_string()),
        );
        let event = parser
            .parse_event(msg, EventFormat::Plain)
            .unwrap();
        assert_eq!(event.variable_str("empty"), Some("_undef_"));
    }

    #[test]
    fn test_parse_json_event_body_key() {
        let parser = EslParser::new();
        let json = r#"{"Event-Name":"BACKGROUND_JOB","Job-UUID":"abc-123","_body":"+OK result"}"#;
        let msg = EslMessage::new(
            MessageType::Event,
            {
                let mut h = IndexMap::new();
                h.insert("Content-Type".to_string(), "text/event-json".to_string());
                h
            },
            Some(json.to_string()),
        );
        let event = parser
            .parse_event(msg, EventFormat::Json)
            .unwrap();
        assert_eq!(event.event_type(), Some(EslEventType::BackgroundJob));
        assert_eq!(event.body(), Some("+OK result"));
        assert!(
            event
                .header_str("_body")
                .is_none(),
            "_body must be mapped to event body, not stored as a header"
        );
    }

    #[test]
    fn test_json_event_end_to_end() {
        let mut parser = EslParser::new();
        let json_body = r#"{"Event-Name":"CHANNEL_CREATE","Unique-ID":"test-uuid-123","Channel-Name":"sofia/internal/1000@example.com","variable_sip_call_id":"call-456"}"#;
        let envelope = format!(
            "Content-Length: {}\nContent-Type: text/event-json\n\n",
            json_body.len()
        );
        let data = format!("{}{}", envelope, json_body);

        parser
            .add_data(data.as_bytes())
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        assert_eq!(message.message_type, MessageType::Event);

        let event = parser
            .parse_event(message, EventFormat::Json)
            .unwrap();
        assert_eq!(event.event_type(), Some(EslEventType::ChannelCreate));
        assert_eq!(event.unique_id(), Some("test-uuid-123"));
        assert_eq!(
            event.header_str("Channel-Name"),
            Some("sofia/internal/1000@example.com")
        );
        assert_eq!(event.variable_str("sip_call_id"), Some("call-456"));
    }

    #[test]
    fn test_json_event_with_body_end_to_end() {
        let mut parser = EslParser::new();
        let json_body = r#"{"Event-Name":"BACKGROUND_JOB","Job-UUID":"job-789","_body":"+OK result data\nline 2"}"#;
        let envelope = format!(
            "Content-Length: {}\nContent-Type: text/event-json\n\n",
            json_body.len()
        );
        let data = format!("{}{}", envelope, json_body);

        parser
            .add_data(data.as_bytes())
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        let event = parser
            .parse_event(message, EventFormat::Json)
            .unwrap();
        assert_eq!(event.event_type(), Some(EslEventType::BackgroundJob));
        assert!(event
            .body()
            .is_some());
        assert!(event
            .header_str("_body")
            .is_none());
    }

    #[test]
    fn test_to_plain_format_round_trip() {
        use crate::event::{EslEvent, EslEventType, EventFormat};
        use indexmap::IndexMap;

        let mut original = EslEvent::with_type(EslEventType::Heartbeat);
        original.set_header("Event-Name", "HEARTBEAT");
        original.set_header("Core-UUID", "abc-123");
        original.set_header("Up-Time", "0 years, 0 days, 1 hour");
        original.set_header("Event-Info", "System Ready");

        let plain1 = original.to_plain_format();

        let msg1 = EslMessage::new(
            MessageType::Event,
            {
                let mut h = IndexMap::new();
                h.insert("Content-Type".to_string(), "text/event-plain".to_string());
                h
            },
            Some(plain1.clone()),
        );
        let parsed1 = EslParser::new()
            .parse_event(msg1, EventFormat::Plain)
            .unwrap();

        assert_eq!(parsed1.event_type(), original.event_type());
        assert_eq!(parsed1.headers(), original.headers());
        assert_eq!(parsed1.body(), original.body());

        let plain2 = parsed1.to_plain_format();
        let msg2 = EslMessage::new(
            MessageType::Event,
            {
                let mut h = IndexMap::new();
                h.insert("Content-Type".to_string(), "text/event-plain".to_string());
                h
            },
            Some(plain2),
        );
        let parsed2 = EslParser::new()
            .parse_event(msg2, EventFormat::Plain)
            .unwrap();

        assert_eq!(parsed2.event_type(), original.event_type());
        assert_eq!(parsed2.headers(), original.headers());
        assert_eq!(parsed2.body(), original.body());
    }

    #[test]
    fn test_to_plain_format_wire_round_trip() {
        use crate::event::EventFormat;
        use crate::headers::EventHeader;
        use indexmap::IndexMap;

        // Realistic wire payload as FreeSWITCH would send it (percent-encoded
        // values, headers in FS emission order)
        let wire_body = "\
Event-Name: CHANNEL_CREATE\n\
Core-UUID: 2bde6598-0f10-4b90-b70e-d21f4c9e270f\n\
FreeSWITCH-Hostname: fs01%2Eexample%2Ecom\n\
FreeSWITCH-IPv4: 10%2E0%2E0%2E1\n\
Event-Date-Local: 2025-06-15%2010%3A30%3A00\n\
Unique-ID: a1b2c3d4-5678-9abc-def0-123456789abc\n\
Channel-Name: sofia%2Finternal%2F1000%40example.com\n\
Caller-Caller-ID-Name: J%C3%A9r%C3%B4me%20Poulin\n\
Call-Direction: inbound\n\
Channel-State: CS_INIT\n\
\n";

        let msg = EslMessage::new(
            MessageType::Event,
            {
                let mut h = IndexMap::new();
                h.insert("Content-Type".to_string(), "text/event-plain".to_string());
                h
            },
            Some(wire_body.to_string()),
        );
        let event = EslParser::new()
            .parse_event(msg, EventFormat::Plain)
            .unwrap();

        assert_eq!(
            event.header(EventHeader::FreeswitchHostname),
            Some("fs01.example.com")
        );
        assert_eq!(
            event.header(EventHeader::CallerCallerIdName),
            Some("Jérôme Poulin")
        );

        let regenerated = event.to_plain_format();

        // Parse the regenerated output back and compare
        let msg2 = EslMessage::new(
            MessageType::Event,
            {
                let mut h = IndexMap::new();
                h.insert("Content-Type".to_string(), "text/event-plain".to_string());
                h
            },
            Some(regenerated.clone()),
        );
        let reparsed = EslParser::new()
            .parse_event(msg2, EventFormat::Plain)
            .unwrap();
        assert_eq!(event.headers(), reparsed.headers());
        assert_eq!(event.body(), reparsed.body());

        // Verify header order is preserved (wire order, not alphabetical)
        let keys: Vec<&str> = regenerated
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| {
                l.split(':')
                    .next()
                    .unwrap()
            })
            .collect();
        assert_eq!(keys[0], "Event-Name");
        assert_eq!(keys[1], "Core-UUID");
        assert_eq!(keys[2], "FreeSWITCH-Hostname");
        assert_eq!(keys[3], "FreeSWITCH-IPv4");
    }

    #[test]
    fn test_to_plain_format_round_trip_with_body() {
        use crate::event::{EslEvent, EslEventType, EventFormat};
        use indexmap::IndexMap;

        let body_text = "+OK Status\nLine 2\n";
        let mut original = EslEvent::with_type(EslEventType::BackgroundJob);
        original.set_header("Event-Name", "BACKGROUND_JOB");
        original.set_header("Job-UUID", "job-789");
        original.set_header(
            "Content-Length".to_string(),
            body_text
                .len()
                .to_string(),
        );
        original.set_body(body_text.to_string());

        let plain = original.to_plain_format();
        let msg = EslMessage::new(
            MessageType::Event,
            {
                let mut h = IndexMap::new();
                h.insert("Content-Type".to_string(), "text/event-plain".to_string());
                h
            },
            Some(plain),
        );
        let parsed = EslParser::new()
            .parse_event(msg, EventFormat::Plain)
            .unwrap();

        assert_eq!(parsed.event_type(), original.event_type());
        assert_eq!(parsed.headers(), original.headers());
        assert_eq!(parsed.body(), original.body());
    }

    #[test]
    fn test_event_body_lossy_decode_default() {
        // Event-body value with %E9 (invalid UTF-8) is decoded lossily by default
        let parser = EslParser::new();
        let body = "Event-Name: HEARTBEAT\nvariable_dp_match: %E9foo\nvalid_key: normal\n\n";
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

        // Lossy decode produced U+FFFD
        assert!(event
            .header_str("variable_dp_match")
            .unwrap()
            .contains('\u{FFFD}'));
        // LossyValues tracks the affected key
        let lossy = event.lossy_values();
        assert!(!lossy.is_empty());
        assert_eq!(
            lossy
                .iter()
                .count(),
            1
        );
        let entry = lossy
            .iter()
            .next()
            .unwrap();
        assert_eq!(entry.key(), "variable_dp_match");
        assert_eq!(entry.raw_value(), "%E9foo");
        // Well-formed header unaffected
        assert_eq!(event.header_str("valid_key"), Some("normal"));
    }

    #[test]
    fn test_event_body_strict_utf8_fails() {
        // Same input with strict option => error
        let parser = EslParser::new().with_strict_header_utf8(true);
        let body = "Event-Name: HEARTBEAT\nvariable_dp_match: %E9foo\n\n";
        let msg = EslMessage::new(
            MessageType::Event,
            {
                let mut h = IndexMap::new();
                h.insert("Content-Type".to_string(), "text/event-plain".to_string());
                h
            },
            Some(body.to_string()),
        );
        let result = parser.parse_event(msg, EventFormat::Plain);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EslError::InvalidUtf8InHeader { .. }),
            "expected InvalidUtf8InHeader, got: {:?}",
            err
        );
    }

    #[test]
    fn json_event_non_utf8_value_carries_lossy_signal() {
        // FreeSWITCH serializes raw header values into event-json with no
        // percent-encoding or UTF-8 validation (switch_event.c cJSON path),
        // so a Latin-1 value reaches the wire raw. The envelope body decodes
        // lossily; the JSON parses fine — but the lossy signal must ride on
        // the event, not vanish.
        let mut parser = EslParser::new();
        let json_body: &[u8] =
            b"{\"Event-Name\":\"CHANNEL_CREATE\",\"Caller-Caller-ID-Name\":\"Andr\xE9\"}";
        let mut data = format!(
            "Content-Length: {}\nContent-Type: text/event-json\n\n",
            json_body.len()
        )
        .into_bytes();
        data.extend_from_slice(json_body);

        parser
            .add_data(&data)
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        let event = parser
            .parse_event(message, EventFormat::Json)
            .unwrap();

        assert_eq!(event.event_type(), Some(EslEventType::ChannelCreate));
        assert_eq!(
            event.header_str("Caller-Caller-ID-Name"),
            Some("Andr\u{FFFD}")
        );
        // Whole-envelope wire bytes: JSON cannot map bytes back to the
        // decoded body, but the signal and the source bytes must be carried.
        assert_eq!(event.raw_body(), Some(json_body));
    }

    #[test]
    fn json_event_non_object_body_is_error() {
        // A framed event whose body is well-formed JSON of the wrong shape is
        // a protocol violation: Err, not a silent empty event that is
        // indistinguishable from a headerless one.
        let parser = EslParser::new();
        for body in ["[1,2,3]", "\"foo\"", "123", "null"] {
            let msg = EslMessage::new(
                MessageType::Event,
                {
                    let mut h = IndexMap::new();
                    h.insert("Content-Type".to_string(), "text/event-json".to_string());
                    h
                },
                Some(body.to_string()),
            );
            let result = parser.parse_event(msg, EventFormat::Json);
            assert!(
                matches!(result, Err(EslError::ProtocolError { .. })),
                "body {body:?} must be a protocol error, got: {result:?}"
            );
        }
    }
}
