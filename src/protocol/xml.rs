//! XML event body parsing (quick_xml is confined to this module).

use super::{EslMessage, EslParser};
use crate::error::{EslError, EslResult};
use crate::event::EslEvent;

impl EslParser {
    /// Box a quick_xml error as the source of `EslError::XmlError`, keeping the
    /// dependency type out of the signature.
    fn xml_err<E: std::error::Error + Send + Sync + 'static>(e: E) -> EslError {
        EslError::XmlError(Box::new(e))
    }

    /// Parse XML event using quick_xml.
    ///
    /// FreeSWITCH XML event format:
    /// ```xml
    /// <event>
    ///   <headers>
    ///     <Event-Name>HEARTBEAT</Event-Name>
    ///     <Core-UUID>abc-123</Core-UUID>
    ///   </headers>
    ///   <body>...</body>
    /// </event>
    /// ```
    // qual:allow(complexity, max_cyclomatic=20) reason: "one pull-parser state machine over shared flags"
    // qual:allow(srp, slm) reason: "one pull-parser state machine over shared flags"
    pub(super) fn parse_xml_event(&self, message: EslMessage) -> EslResult<EslEvent> {
        use quick_xml::events::Event as XmlEvent;
        use quick_xml::Reader;

        let EslMessage {
            body,
            raw_body,
            lossy_values,
            ..
        } = message;
        let body = body.ok_or_else(|| EslError::protocol_error("XML event missing body"))?;

        let mut reader = Reader::from_str(&body);
        let mut event = EslEvent::new();
        let mut in_headers = false;
        let mut current_tag: Option<String> = None;
        let mut in_body = false;
        // quick-xml splits text around entity references ("Smith &amp; Jones"
        // arrives as Text, GeneralRef, Text), so fragments accumulate here and
        // flush on the End tag.
        let mut text_buf = String::new();

        loop {
            match reader.read_event() {
                Ok(XmlEvent::Start(ref e)) => {
                    let tag = e
                        .name()
                        .as_ref()
                        .to_string();
                    match tag.as_str() {
                        "headers" => in_headers = true,
                        "body" => in_body = true,
                        _ if in_headers => {
                            text_buf.clear();
                            current_tag = Some(tag);
                        }
                        _ => {}
                    }
                }
                Ok(XmlEvent::End(ref e)) => {
                    let tag = e
                        .name()
                        .as_ref()
                        .to_string();
                    match tag.as_str() {
                        "headers" => in_headers = false,
                        "body" => {
                            if !text_buf.is_empty() {
                                event.set_body(std::mem::take(&mut text_buf));
                            }
                            in_body = false;
                        }
                        _ if in_headers => {
                            if let Some(ref tag) = current_tag {
                                if !text_buf.is_empty() {
                                    event.set_header(tag.clone(), std::mem::take(&mut text_buf));
                                }
                            }
                            current_tag = None;
                        }
                        _ => {}
                    }
                }
                Ok(XmlEvent::Text(ref e)) => {
                    if in_body || current_tag.is_some() {
                        text_buf.push_str(e);
                    }
                }
                Ok(XmlEvent::GeneralRef(ref e)) if in_body || current_tag.is_some() => {
                    let resolved = Self::resolve_entity(e)?;
                    text_buf.push_str(&resolved);
                }
                Ok(XmlEvent::Eof) => break,
                Err(e) => return Err(Self::xml_err(e)),
                _ => {}
            }
        }

        Self::carry_lossy_signal(&mut event, lossy_values, raw_body);
        Ok(event)
    }

    /// Resolve an XML entity reference (`&name;` or `&#num;`) to its string value.
    fn resolve_entity(entity: &quick_xml::events::BytesRef<'_>) -> EslResult<String> {
        if let Some(ch) = entity
            .resolve_char_ref()
            .map_err(Self::xml_err)?
        {
            return Ok(ch.to_string());
        }
        let name: &str = entity;
        match quick_xml::escape::resolve_xml_entity(name) {
            Some(s) => Ok(s.to_string()),
            None => Err(EslError::protocol_error(format!(
                "unknown XML entity: &{name};"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EslEventType, EventFormat};
    use crate::headers::EventHeader;

    #[test]
    fn test_parse_event_xml_heartbeat() {
        let mut parser = EslParser::new();
        let xml_body = "\
<event>\n\
  <headers>\n\
    <Event-Name>HEARTBEAT</Event-Name>\n\
    <Core-UUID>abc-123</Core-UUID>\n\
    <Up-Time>0 years, 1 day</Up-Time>\n\
  </headers>\n\
</event>";
        let envelope = format!(
            "Content-Length: {}\nContent-Type: text/event-xml\n\n",
            xml_body.len()
        );
        let data = format!("{}{}", envelope, xml_body);

        parser
            .add_data(data.as_bytes())
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        let event = parser
            .parse_event(message, EventFormat::Xml)
            .unwrap();

        assert_eq!(event.event_type(), Some(EslEventType::Heartbeat));
        assert_eq!(event.header(EventHeader::CoreUuid), Some("abc-123"));
        assert_eq!(event.header_str("Up-Time"), Some("0 years, 1 day"));
    }

    #[test]
    fn test_parse_event_xml_with_body() {
        let mut parser = EslParser::new();
        let xml_body = "\
<event>\n\
  <headers>\n\
    <Event-Name>BACKGROUND_JOB</Event-Name>\n\
    <Job-UUID>def-456</Job-UUID>\n\
  </headers>\n\
  <body>+OK result data</body>\n\
</event>";
        let envelope = format!(
            "Content-Length: {}\nContent-Type: text/event-xml\n\n",
            xml_body.len()
        );
        let data = format!("{}{}", envelope, xml_body);

        parser
            .add_data(data.as_bytes())
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        let event = parser
            .parse_event(message, EventFormat::Xml)
            .unwrap();

        assert_eq!(event.event_type(), Some(EslEventType::BackgroundJob));
        assert_eq!(event.header(EventHeader::JobUuid), Some("def-456"));
        assert_eq!(event.body(), Some("+OK result data"));
    }

    #[test]
    fn test_parse_event_xml_ampersand_escaped() {
        let mut parser = EslParser::new();
        let xml_body = "\
<event>\n\
  <headers>\n\
    <Event-Name>CHANNEL_CREATE</Event-Name>\n\
    <Caller-Caller-ID-Name>Smith &amp; Jones</Caller-Caller-ID-Name>\n\
    <variable_sip_h_Subject>Test &lt;1&gt; &amp; Test &lt;2&gt;</variable_sip_h_Subject>\n\
  </headers>\n\
</event>";
        let envelope = format!(
            "Content-Length: {}\nContent-Type: text/event-xml\n\n",
            xml_body.len()
        );
        let data = format!("{}{}", envelope, xml_body);

        parser
            .add_data(data.as_bytes())
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        let event = parser
            .parse_event(message, EventFormat::Xml)
            .unwrap();

        assert_eq!(event.event_type(), Some(EslEventType::ChannelCreate));
        assert_eq!(
            event.header_str("Caller-Caller-ID-Name"),
            Some("Smith & Jones")
        );
        assert_eq!(
            event.variable_str("sip_h_Subject"),
            Some("Test <1> & Test <2>")
        );
    }

    #[test]
    fn xml_event_non_utf8_text_carries_lossy_signal() {
        let mut parser = EslParser::new();
        let xml_body: &[u8] = b"<event>\n  <headers>\n    <Event-Name>CHANNEL_CREATE</Event-Name>\n    <Caller-Caller-ID-Name>Andr\xE9</Caller-Caller-ID-Name>\n  </headers>\n</event>";
        let mut data = format!(
            "Content-Length: {}\nContent-Type: text/event-xml\n\n",
            xml_body.len()
        )
        .into_bytes();
        data.extend_from_slice(xml_body);

        parser
            .add_data(&data)
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();
        let event = parser
            .parse_event(message, EventFormat::Xml)
            .unwrap();

        assert_eq!(event.event_type(), Some(EslEventType::ChannelCreate));
        assert_eq!(
            event.header_str("Caller-Caller-ID-Name"),
            Some("Andr\u{FFFD}")
        );
        assert_eq!(event.raw_body(), Some(xml_body));
    }
}
