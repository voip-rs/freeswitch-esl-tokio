wire_enum! {
    /// Event format types supported by FreeSWITCH ESL.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum EventFormat {
        /// Plain text format (default)
        Plain => "plain",
        /// JSON format
        Json => "json",
        /// XML format
        Xml => "xml",
    }
    error ParseEventFormatError("event format");
}

impl EventFormat {
    /// Determine event format from a Content-Type header value.
    ///
    /// Returns `Err` for unrecognized content types to avoid silently
    /// misparsing events if FreeSWITCH adds a new format.
    pub fn from_content_type(ct: &str) -> Result<Self, ParseEventFormatError> {
        match ct {
            "text/event-json" => Ok(Self::Json),
            "text/event-xml" => Ok(Self::Xml),
            "text/event-plain" => Ok(Self::Plain),
            _ => Err(ParseEventFormatError(ct.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_format_from_str() {
        assert_eq!("plain".parse::<EventFormat>(), Ok(EventFormat::Plain));
        assert_eq!("json".parse::<EventFormat>(), Ok(EventFormat::Json));
        assert_eq!("xml".parse::<EventFormat>(), Ok(EventFormat::Xml));
        assert!("foo"
            .parse::<EventFormat>()
            .is_err());
    }

    #[test]
    fn test_event_format_from_str_rejects_wrong_case() {
        for s in ["PLAIN", "Json", "XML", "Xml"] {
            assert!(
                s.parse::<EventFormat>()
                    .is_err(),
                "{s}"
            );
        }
    }

    #[test]
    fn test_event_format_from_content_type() {
        assert_eq!(
            EventFormat::from_content_type("text/event-json"),
            Ok(EventFormat::Json)
        );
        assert_eq!(
            EventFormat::from_content_type("text/event-xml"),
            Ok(EventFormat::Xml)
        );
        assert_eq!(
            EventFormat::from_content_type("text/event-plain"),
            Ok(EventFormat::Plain)
        );
        assert!(EventFormat::from_content_type("unknown").is_err());
    }
}
