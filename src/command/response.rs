//! FreeSWITCH API body parsing and the ESL command response type.

use crate::{
    constants::{HEADER_REPLY_TEXT, REPLY_PREFIX_ERR, REPLY_PREFIX_OK, REPLY_PREFIX_USAGE},
    error::{EslError, EslResult},
    event::EslEvent,
    headers::EventHeader,
    lookup::HeaderLookup,
    protocol::{decode_serialized_event, DecodeOptions, EslMessage},
    EslHeaders, LossyValues,
};
use freeswitch_types::{variable_key, RowError, SipHeaderRows};
use indexmap::IndexMap;

/// Parse a FreeSWITCH API response body into a result.
///
/// FreeSWITCH API commands return results in varying formats:
///
/// - **Action commands** (`originate`, `uuid_kill`, …) prefix success
///   with `+OK` -- this function strips it and returns the payload.
/// - **Query commands** (`show channels as json`, `uuid_dump`, …) return
///   raw data with no prefix -- returned as-is.
/// - **Errors** (`-ERR …`, `-USAGE: …`) produce [`EslError::CommandFailed`].
///
/// A trailing `\n` (the standard FreeSWITCH API output terminator) is
/// stripped; all other content is preserved verbatim.
/// Returns [`EslError::ProtocolError`] if the body is empty after
/// stripping.
///
/// A command that wrote nothing is given a fixed unprefixed message rather than
/// an empty body, so it arrives here as ordinary output. Nothing distinguishes
/// it from a query that legitimately answered with prose.
///
/// This is the same parser used by [`EslResponse::api_result`]. Use it
/// directly on [`EslEvent::body`](freeswitch_types::EslEvent::body) for
/// `BACKGROUND_JOB` results:
///
/// ```rust,no_run
/// # use freeswitch_esl_tokio::{parse_api_body, EslEvent, HeaderLookup};
/// # fn example(event: &EslEvent) {
/// if let Some(body) = event.body() {
///     match parse_api_body(body) {
///         Ok(data) => println!("result: {}", data),
///         Err(e) => eprintln!("command failed: {}", e),
///     }
/// }
/// # }
/// ```
pub fn parse_api_body(body: &str) -> EslResult<&str> {
    // Strip the trailing \n that FreeSWITCH appends to API output.
    let body = body
        .strip_suffix('\n')
        .unwrap_or(body);
    let body = body
        .strip_suffix('\r')
        .unwrap_or(body);
    if body.is_empty() {
        return Err(EslError::protocol_error("api response body is empty"));
    }
    if let Some(rest) = body.strip_prefix(REPLY_PREFIX_OK) {
        Ok(rest
            .strip_prefix(' ')
            .unwrap_or(rest))
    } else if body.starts_with(REPLY_PREFIX_ERR) || body.starts_with(REPLY_PREFIX_USAGE) {
        Err(EslError::CommandFailed {
            reply_text: body.to_string(),
        })
    } else {
        Ok(body)
    }
}

/// Options for [`parse_channel_dump_with_options`].
#[derive(Debug, Clone, Default)]
pub struct ChannelDumpOptions {
    strict_header_utf8: bool,
}

impl ChannelDumpOptions {
    /// Create default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Strict UTF-8 validation on percent-decoded header values.
    ///
    /// When `true`, an invalid sequence returns
    /// [`EslError::InvalidUtf8InHeader`]. When `false` (default) it is decoded
    /// lossily (U+FFFD) and recorded in
    /// [`EslEvent::lossy_values`](freeswitch_types::EslEvent::lossy_values).
    pub fn with_strict_header_utf8(mut self, strict: bool) -> Self {
        self.strict_header_utf8 = strict;
        self
    }

    /// Whether to fail on invalid UTF-8 in a header value. Default: false.
    pub fn strict_header_utf8(&self) -> bool {
        self.strict_header_utf8
    }
}

/// Parse a `uuid_dump <uuid>` body into the `CHANNEL_DATA` event it is.
///
/// The default `txt` format is `switch_event_serialize` output — the same
/// shape as a `text/event-plain` body — so this shares the event decoder and
/// therefore the crate's header-key normalization, rather than splitting the
/// lines a second way.
///
/// The body goes through [`parse_api_body`] first, so a channel that hung up
/// between being listed and being dumped comes back as
/// [`EslError::CommandFailed`] (`-ERR No such channel!`), readable through
/// [`EslError::command_failure`], instead of as a parse failure on a line
/// with no colon.
///
/// A header whose value is the empty-value sentinel is omitted: a dump is a
/// read-back, so an unset variable reads as absent. The pushed event stream
/// keeps the sentinel it was sent.
///
/// [`raw_body`](freeswitch_types::EslEvent::raw_body) on the result is always
/// `None`. The dump arrives as a `&str` the message parser already decoded,
/// possibly lossily; re-encoding it would put U+FFFD bytes in the one field
/// whose contract is exact wire bytes. Those live on the
/// [`EslResponse::raw_body`] the string came from.
///
/// Only the default `txt` format is accepted. `json` and `xml` are refused by
/// [`EslError::InvalidEventFormat`]. `uuid_dump`'s own `plain` format is not
/// supported and is not detectable: it does not percent-encode, so a value
/// containing a newline breaks line splitting, which is only sound because
/// `txt` encodes a newline as `%0A`.
pub fn parse_channel_dump(body: &str) -> EslResult<EslEvent> {
    parse_channel_dump_with_options(body, &ChannelDumpOptions::default())
}

/// [`parse_channel_dump`] with control over lossy UTF-8 decoding.
pub fn parse_channel_dump_with_options(
    body: &str,
    options: &ChannelDumpOptions,
) -> EslResult<EslEvent> {
    let payload = parse_api_body(body)?;
    // A serialized header name starts with neither, so this cannot misfire on
    // a txt dump, and it names the format instead of parsing garbage.
    let format = match payload
        .trim_start()
        .as_bytes()
        .first()
    {
        Some(b'{') => Some("json"),
        Some(b'<') => Some("xml"),
        _ => None,
    };
    if let Some(format) = format {
        return Err(EslError::InvalidEventFormat {
            format: format.to_string(),
        });
    }
    decode_serialized_event(
        payload,
        DecodeOptions {
            strict_utf8: options.strict_header_utf8(),
            skip_undef: true,
        },
    )
}

/// Reply-Text classification per the ESL wire protocol.
///
/// FreeSWITCH commands return `+OK …` on success and `-ERR …` on failure.
/// A handful of commands (`getvar`) return the raw value with no prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReplyStatus {
    /// Reply-Text starts with `+OK` or is absent/empty.
    Ok,
    /// Reply-Text starts with `-ERR`.
    Err,
    /// Reply-Text present but matches neither `+OK` nor `-ERR`.
    /// This is normal for `getvar` (which returns the bare variable value)
    /// but unexpected for most other commands.
    ///
    /// A `-USAGE` lands here and so becomes
    /// [`EslError::UnexpectedReply`], while the same text in an api body is a
    /// [`CommandFailed`](EslError::CommandFailed) — read either through
    /// [`EslError::command_failure`].
    Other,
}

/// Response from ESL command execution
///
/// `#[must_use]`: a reply carries the only report a command makes, and an `api`
/// reply carries it in either of two places, so dropping the value is
/// indistinguishable from the command having succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "an ESL reply can be -ERR: read it with api_result(), or check() if there is no payload to read"]
pub struct EslResponse {
    headers: EslHeaders,
    body: Option<String>,
    /// Exact wire bytes of a body that was not valid UTF-8; `body` then
    /// holds the U+FFFD-substituted string. `None` in the normal case.
    raw_body: Option<Vec<u8>>,
    status: ReplyStatus,
    /// Header keys whose percent-decoded value was not valid UTF-8 and was
    /// decoded lossily. Populated for serialized responses such as the
    /// outbound `connect` channel data; empty in the normal case.
    lossy_values: LossyValues,
}

impl EslResponse {
    /// Keys are canonicalized with
    /// [`normalize_header_key`](freeswitch_types::normalize_header_key), so a
    /// map carrying the same logical header in two casings collapses to one
    /// entry — the same way [`EslEvent`](crate::event::EslEvent) treats it.
    ///
    /// `ReplyStatus` is derived from the `Reply-Text` header. Header
    /// lookups via [`header()`](Self::header) are case-insensitive for
    /// FS framing headers but case-sensitive for `variable_*`,
    /// `sip_h_*`, and `sip_i_*` keys, which must preserve original SIP
    /// wire casing.
    pub fn new(headers: IndexMap<String, String>, body: Option<String>) -> Self {
        let headers = EslHeaders::from_map(headers);
        let status = match headers.header_str(HEADER_REPLY_TEXT) {
            None | Some("") => ReplyStatus::Ok,
            Some(t) if t.starts_with(REPLY_PREFIX_OK) => ReplyStatus::Ok,
            Some(t) if t.starts_with(REPLY_PREFIX_ERR) => ReplyStatus::Err,
            Some(_) => ReplyStatus::Other,
        };
        Self {
            headers,
            body,
            raw_body: None,
            status,
            lossy_values: LossyValues::default(),
        }
    }

    /// Build the reply from a parsed [`EslMessage`], carrying its lossy-decode
    /// signal and raw body bytes.
    pub(crate) fn from_message(message: EslMessage) -> Self {
        Self {
            raw_body: message.raw_body,
            lossy_values: message.lossy_values,
            ..Self::new(message.headers, message.body)
        }
    }

    /// Exact wire bytes of the response body when it was not valid UTF-8.
    ///
    /// `Some` is the lossy signal: [`body()`](Self::body) then holds the
    /// U+FFFD-substituted string and these are the original payload bytes,
    /// so the app can re-decode or audit them. `None` in the normal case.
    pub fn raw_body(&self) -> Option<&[u8]> {
        self.raw_body
            .as_deref()
    }

    /// Header keys whose percent-decoded value was not valid UTF-8 and was
    /// decoded lossily (U+FFFD). Each entry carries the on-wire value. Mainly
    /// relevant to the outbound `connect` response, whose channel-data values
    /// are percent-encoded by FreeSWITCH. Empty in the normal case.
    pub fn lossy_values(&self) -> &LossyValues {
        &self.lossy_values
    }

    /// `true` if Reply-Text is `+OK` or absent.
    pub fn is_success(&self) -> bool {
        self.status == ReplyStatus::Ok
    }

    /// Classification of the `Reply-Text` header.
    pub fn reply_status(&self) -> ReplyStatus {
        self.status
    }

    /// Response body (the `api/` response payload, or `bgapi` result).
    pub fn body(&self) -> Option<&str> {
        self.body
            .as_deref()
    }

    /// Look up a response header by name. Case-insensitive.
    pub fn header(&self, name: impl AsRef<str>) -> Option<&str> {
        self.lookup_header(name.as_ref())
    }

    fn lookup_header(&self, name: &str) -> Option<&str> {
        self.headers
            .header_str(name)
    }

    /// All response headers, keyed canonically.
    pub fn headers(&self) -> &IndexMap<String, String> {
        self.headers
            .as_map()
    }

    /// Raw `Reply-Text` header value (e.g. `+OK`, `-ERR invalid command`).
    pub fn reply_text(&self) -> Option<&str> {
        self.headers
            .header_str(HEADER_REPLY_TEXT)
    }

    /// `Job-UUID` header from `bgapi` responses.
    ///
    /// FreeSWITCH returns the Job-UUID both in Reply-Text (`+OK Job-UUID: <uuid>`)
    /// and as a separate `Job-UUID` header. This reads the dedicated header.
    pub fn job_uuid(&self) -> Option<&str> {
        self.headers
            .header(EventHeader::JobUuid)
    }

    /// UUID of the event fired by `sendevent`.
    ///
    /// FreeSWITCH returns `+OK <event-uuid>` in the Reply-Text for
    /// `sendevent` commands. Returns `None` if the reply doesn't
    /// contain a UUID after `+OK `.
    pub fn event_uuid(&self) -> Option<&str> {
        self.reply_text()
            .and_then(|t| t.strip_prefix(REPLY_PREFIX_OK))
            .and_then(|rest| rest.strip_prefix(' '))
            .filter(|s| !s.is_empty())
    }

    /// Parse the response body as an API result.
    ///
    /// FreeSWITCH `api` commands return the result in the response body.
    /// The format varies by command:
    ///
    /// - **Action commands** (`originate`, `uuid_kill`, `uuid_setvar`, …)
    ///   return `+OK <data>` on success -- this method strips the prefix
    ///   and returns the payload.
    /// - **Query commands** (`show channels as json`, `uuid_dump`,
    ///   `uuid_getvar`, `status`, …) return raw data with no prefix --
    ///   this method returns the body as-is.
    /// - **Error responses** (`-ERR <message>`, `-USAGE: <usage>`)
    ///   return [`EslError::CommandFailed`].
    ///
    /// On success, a single trailing `\n` (then a single trailing `\r`, for
    /// `\r\n` wire endings) is stripped along with the `+OK ` prefix on
    /// action commands. No leading-whitespace trimming is performed.
    ///
    /// Returns [`EslError::ProtocolError`] if the body is missing or empty.
    ///
    /// ```
    /// # use freeswitch_esl_tokio::EslResponse;
    /// # use indexmap::IndexMap;
    /// let headers = IndexMap::from([("Reply-Text".into(), "+OK".into())]);
    ///
    /// // Action command: +OK prefix stripped
    /// let resp = EslResponse::new(headers.clone(), Some("+OK d4f3a2b1-1234\n".into()));
    /// assert_eq!(resp.api_result().unwrap(), "d4f3a2b1-1234");
    ///
    /// // Query command: raw body returned as-is
    /// let resp = EslResponse::new(headers.clone(), Some(r#"{"rows":[]}"#.into()));
    /// assert_eq!(resp.api_result().unwrap(), r#"{"rows":[]}"#);
    ///
    /// // Error: Err variant
    /// let resp = EslResponse::new(headers, Some("-ERR no such channel\n".into()));
    /// assert!(resp.api_result().is_err());
    /// ```
    pub fn api_result(&self) -> EslResult<&str> {
        // A refused command is answered by mod_event_socket itself, as a
        // reply with no body, so the body parser would only see it as empty.
        if self.status == ReplyStatus::Err {
            return Err(self.command_failed());
        }
        let body = self
            .body
            .as_deref()
            .unwrap_or("");
        parse_api_body(body)
    }

    /// The failure a `-ERR` `Reply-Text` names, for the two entry points that
    /// both have to report it.
    fn command_failed(&self) -> EslError {
        EslError::CommandFailed {
            reply_text: self
                .reply_text()
                .unwrap_or(REPLY_PREFIX_ERR)
                .to_string(),
        }
    }

    /// Require a successful reply and keep nothing.
    ///
    /// The same check as [`into_result`](Self::into_result), for the common
    /// case of a command whose reply carries no payload worth reading. Handing
    /// the response back there leaves a `#[must_use]` value the caller has
    /// already finished with.
    ///
    /// ```
    /// # use freeswitch_esl_tokio::EslResponse;
    /// # use indexmap::IndexMap;
    /// let headers: IndexMap<String, String> = [("Reply-Text".into(), "+OK".into())].into();
    /// EslResponse::new(headers, None).check().unwrap();
    /// ```
    pub fn check(self) -> EslResult<()> {
        self.into_result()
            .map(|_| ())
    }

    /// Convert to result based on success status.
    ///
    /// ```
    /// # use freeswitch_esl_tokio::EslResponse;
    /// # use indexmap::IndexMap;
    /// let headers: IndexMap<String, String> = [("Reply-Text".into(), "+OK".into())].into();
    /// let resp = EslResponse::new(headers, None);
    /// assert!(resp.into_result().is_ok());
    /// ```
    pub fn into_result(self) -> EslResult<Self> {
        match self.status {
            ReplyStatus::Ok => Ok(self),
            ReplyStatus::Err => Err(self.command_failed()),
            ReplyStatus::Other => {
                let reply_text = self
                    .reply_text()
                    .unwrap_or("")
                    .to_string();
                Err(EslError::UnexpectedReply { reply_text })
            }
        }
    }
}

impl SipHeaderRows for EslResponse {
    fn sip_header_rows_str<'a>(&'a self, name: &str) -> Result<Vec<&'a str>, RowError> {
        self.headers
            .sip_header_rows_str(name)
    }
}

impl HeaderLookup for EslResponse {
    fn header_str(&self, name: &str) -> Option<&str> {
        self.lookup_header(name)
    }

    fn variable_str(&self, name: &str) -> Option<&str> {
        self.header_str(&variable_key(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EslEvent;
    use crate::protocol::{EslParser, MessageType};

    #[test]
    fn esl_response_header_lookup_is_case_insensitive() {
        let mut headers = IndexMap::new();
        headers.insert("Reply-Text".to_string(), "+OK".to_string());
        let r = EslResponse::new(headers, None);
        assert_eq!(r.header("Reply-Text"), Some("+OK"));
        assert_eq!(r.header("reply-text"), Some("+OK"));
        assert_eq!(r.header("REPLY-TEXT"), Some("+OK"));
    }

    #[test]
    fn esl_response_normalizes_keys_like_esl_event() {
        // A CODEC event carries the same logical header in two casings; both
        // types must collapse them onto the same canonical entry.
        let raw: IndexMap<String, String> = [
            ("unique-id".into(), "first".into()),
            ("Unique-ID".into(), "second".into()),
        ]
        .into();

        let mut event = EslEvent::new();
        for (key, value) in &raw {
            event.set_header(key.clone(), value.clone());
        }
        let resp = EslResponse::new(raw, None);

        assert_eq!(
            resp.headers()
                .keys()
                .collect::<Vec<_>>(),
            event
                .headers()
                .keys()
                .collect::<Vec<_>>()
        );
        assert_eq!(resp.header("Unique-ID"), event.header_str("Unique-ID"));
        assert_eq!(resp.header("unique-id"), event.header_str("unique-id"));
    }

    #[test]
    fn esl_response_underscored_keys_preserve_case() {
        // Underscore keys carry SIP wire casing, so the lowercase fallback
        // must miss them: X-Foo and X-foo are two headers.
        let mut headers = IndexMap::new();
        headers.insert(
            "variable_sip_h_X-MixedCase-Hdr".to_string(),
            "value".to_string(),
        );
        headers.insert("variable_MyVar".to_string(), "vv".to_string());
        let r = EslResponse::new(headers, None);

        assert_eq!(r.header("variable_sip_h_X-MixedCase-Hdr"), Some("value"));
        assert_eq!(r.header("variable_MyVar"), Some("vv"));
        assert_eq!(r.header("variable_sip_h_x-mixedcase-hdr"), None);
        assert_eq!(r.header("VARIABLE_SIP_H_X-MIXEDCASE-HDR"), None);
        assert_eq!(r.header("variable_myvar"), None);
    }

    #[test]
    fn test_event_uuid_from_sendevent_reply() {
        let headers: IndexMap<String, String> = [(
            "Reply-Text".into(),
            "+OK 7d54c1e6-4a31-11e9-b1e3-001a4a160100".into(),
        )]
        .into();
        let resp = EslResponse::new(headers, None);
        assert_eq!(
            resp.event_uuid(),
            Some("7d54c1e6-4a31-11e9-b1e3-001a4a160100")
        );
    }

    #[test]
    fn test_event_uuid_none_for_plain_ok() {
        let headers: IndexMap<String, String> = [("Reply-Text".into(), "+OK".into())].into();
        let resp = EslResponse::new(headers, None);
        assert_eq!(resp.event_uuid(), None);
    }

    #[test]
    fn test_reply_status_ok() {
        let headers: IndexMap<String, String> =
            [("Reply-Text".into(), "+OK accepted".into())].into();
        let resp = EslResponse::new(headers, None);
        assert_eq!(resp.reply_status(), ReplyStatus::Ok);
        assert!(resp.is_success());
        assert!(resp
            .into_result()
            .is_ok());
    }

    #[test]
    fn test_reply_status_ok_prefix_only() {
        let headers: IndexMap<String, String> = [("Reply-Text".into(), "+OK".into())].into();
        let resp = EslResponse::new(headers, None);
        assert_eq!(resp.reply_status(), ReplyStatus::Ok);
        assert!(resp.is_success());
    }

    #[test]
    fn test_reply_status_empty() {
        let headers: IndexMap<String, String> = [("Reply-Text".into(), String::new())].into();
        let resp = EslResponse::new(headers, None);
        assert_eq!(resp.reply_status(), ReplyStatus::Ok);
        assert!(resp.is_success());
    }

    #[test]
    fn test_reply_status_missing_header() {
        let resp = EslResponse::new(IndexMap::new(), None);
        assert_eq!(resp.reply_status(), ReplyStatus::Ok);
        assert!(resp.is_success());
    }

    #[test]
    fn test_reply_status_err() {
        let headers: IndexMap<String, String> =
            [("Reply-Text".into(), "-ERR invalid command".into())].into();
        let resp = EslResponse::new(headers, None);
        assert_eq!(resp.reply_status(), ReplyStatus::Err);
        assert!(!resp.is_success());
        let err = resp
            .into_result()
            .unwrap_err();
        assert!(
            matches!(err, EslError::CommandFailed { ref reply_text } if reply_text == "-ERR invalid command")
        );
    }

    #[test]
    fn test_reply_status_err_bare() {
        let headers: IndexMap<String, String> = [("Reply-Text".into(), "-ERR".into())].into();
        let resp = EslResponse::new(headers, None);
        assert_eq!(resp.reply_status(), ReplyStatus::Err);
        assert!(!resp.is_success());
    }

    #[test]
    fn test_reply_status_other_getvar() {
        let headers: IndexMap<String, String> =
            [("Reply-Text".into(), "sip_from_user".into())].into();
        let resp = EslResponse::new(headers, None);
        assert_eq!(resp.reply_status(), ReplyStatus::Other);
        assert!(!resp.is_success());
        let err = resp
            .into_result()
            .unwrap_err();
        assert!(
            matches!(err, EslError::UnexpectedReply { ref reply_text } if reply_text == "sip_from_user")
        );
    }

    #[test]
    fn test_reply_status_other_random() {
        let headers: IndexMap<String, String> =
            [("Reply-Text".into(), "something unexpected".into())].into();
        let resp = EslResponse::new(headers, None);
        assert_eq!(resp.reply_status(), ReplyStatus::Other);
        assert!(!resp.is_success());
    }

    #[test]
    fn test_response_header_lookup_trait() {
        use crate::headers::EventHeader;
        use crate::lookup::HeaderLookup;

        let headers: IndexMap<String, String> = [
            ("Reply-Text".into(), "+OK".into()),
            ("Channel-Name".into(), "sofia/internal/1000@test".into()),
            ("Channel-State".into(), "CS_EXECUTE".into()),
            ("variable_sip_call_id".into(), "abc-123".into()),
        ]
        .into();
        let resp = EslResponse::new(headers, None);

        assert_eq!(
            resp.header(EventHeader::ChannelName),
            Some("sofia/internal/1000@test")
        );
        assert_eq!(
            resp.channel_state()
                .unwrap(),
            Some(crate::channel::ChannelState::CsExecute)
        );
        assert_eq!(resp.variable_str("sip_call_id"), Some("abc-123"));
        assert_eq!(resp.variable_str("nonexistent"), None);
    }

    // --- parse_api_body() tests ---

    #[test]
    fn parse_api_body_ok_with_data() {
        let data = parse_api_body("+OK d4f3a2b1-1234-5678-abcd-ef0123456789\n").unwrap();
        assert_eq!(data, "d4f3a2b1-1234-5678-abcd-ef0123456789");
    }

    #[test]
    fn parse_api_body_ok_no_data() {
        let data = parse_api_body("+OK\n").unwrap();
        assert_eq!(data, "");
    }

    #[test]
    fn parse_api_body_ok_bare() {
        let data = parse_api_body("+OK").unwrap();
        assert_eq!(data, "");
    }

    #[test]
    fn parse_api_body_err() {
        let err = parse_api_body("-ERR invalid command\n").unwrap_err();
        assert!(matches!(
            err,
            EslError::CommandFailed { ref reply_text } if reply_text == "-ERR invalid command"
        ));
    }

    #[test]
    fn parse_api_body_usage() {
        let err = parse_api_body("-USAGE: originate <call_url> <exten>\n").unwrap_err();
        assert!(matches!(err, EslError::CommandFailed { .. }));
    }

    #[test]
    fn parse_api_body_raw_json() {
        let data = parse_api_body(r#"{"row_count":2,"rows":[]}"#).unwrap();
        assert_eq!(data, r#"{"row_count":2,"rows":[]}"#);
    }

    #[test]
    fn parse_api_body_raw_value() {
        let data = parse_api_body("hello_world\n").unwrap();
        assert_eq!(data, "hello_world");
    }

    #[test]
    fn parse_api_body_raw_multiline() {
        let dump = "Variable-Name: value\nOther-Name: other\n";
        let data = parse_api_body(dump).unwrap();
        assert_eq!(data, "Variable-Name: value\nOther-Name: other");
    }

    #[test]
    fn parse_api_body_empty() {
        let err = parse_api_body("").unwrap_err();
        assert!(matches!(err, EslError::ProtocolError { .. }));
    }

    #[test]
    fn parse_api_body_whitespace_only() {
        // Only trailing \n is stripped; other whitespace is preserved verbatim
        let data = parse_api_body("  \n").unwrap();
        assert_eq!(data, "  ");
    }

    // --- api_result() tests ---

    #[test]
    fn api_result_delegates_to_parse() {
        let headers: IndexMap<String, String> = [("Reply-Text".into(), "+OK".into())].into();
        let resp = EslResponse::new(headers, Some("+OK uuid-123\n".into()));
        assert_eq!(
            resp.api_result()
                .unwrap(),
            "uuid-123"
        );
    }

    #[test]
    fn api_result_no_body() {
        let headers: IndexMap<String, String> = [("Reply-Text".into(), "+OK".into())].into();
        let resp = EslResponse::new(headers, None);
        assert!(matches!(
            resp.api_result()
                .unwrap_err(),
            EslError::ProtocolError { .. }
        ));
    }

    // A denial never reaches the body: mod_event_socket answers the refused
    // command itself, so there is no api/response frame to carry one.
    #[test]
    fn api_result_reports_denial_from_reply_text() {
        let headers: IndexMap<String, String> =
            [("Reply-Text".into(), "-ERR permission denied".into())].into();
        let resp = EslResponse::new(headers, None);
        let err = resp
            .api_result()
            .unwrap_err();
        assert!(
            matches!(err, EslError::CommandFailed { .. }),
            "expected CommandFailed, got: {err:?}"
        );
        assert!(err.is_permission_denied());
    }

    #[test]
    fn api_result_keeps_unprefixed_reply_text_non_fatal() {
        let headers: IndexMap<String, String> =
            [("Reply-Text".into(), "some-variable-value".into())].into();
        let resp = EslResponse::new(headers, Some("body-payload\n".into()));
        assert_eq!(
            resp.api_result()
                .unwrap(),
            "body-payload"
        );
    }

    // --- sip-header accessors over FreeSWITCH's ARRAY encoding ---

    #[test]
    fn esl_response_call_info_array_encoding() {
        use sip_header::SipHeaderLookup;

        let headers: IndexMap<String, String> = [(
            "Call-Info".into(),
            "ARRAY::<urn:emergency:uid:callid:abc>;purpose=emergency-CallId\
             |:<urn:emergency:uid:incidentid:def>;purpose=emergency-IncidentId"
                .into(),
        )]
        .into();
        let resp = EslResponse::new(headers, None);
        let ci = resp
            .call_info()
            .expect("should parse")
            .expect("should be present");
        assert_eq!(
            ci.entries()
                .len(),
            2,
            "ARRAY:: entries should expand"
        );
    }

    #[test]
    fn esl_response_call_info_plain_value_unchanged() {
        use sip_header::SipHeaderLookup;

        let headers: IndexMap<String, String> = [(
            "Call-Info".into(),
            "<sip:pbx.example.com>;purpose=icon".into(),
        )]
        .into();
        let resp = EslResponse::new(headers, None);
        let ci = resp
            .call_info()
            .expect("plain value should parse")
            .expect("should be present");
        assert_eq!(
            ci.entries()
                .len(),
            1
        );
    }

    // --- parse_channel_dump() tests ---

    /// A `uuid_dump <uuid>` body as `switch_event_serialize(encode=TRUE)`
    /// writes it: percent-encoded values, `_undef_` for an empty one, and the
    /// bare `\n` that closes a dump carrying no inner body.
    const DUMP: &str = "Event-Name: CHANNEL_DATA\n\
         Core-UUID: 2bde6598-0f10-4b90-b70e-d21f4c9e270f\n\
         FreeSWITCH-Hostname: fs01%2Eexample%2Ecom\n\
         Channel-Name: sofia%2Finternal%2F1000%40example%2Ecom\n\
         Unique-ID: a1b2c3d4-5678-9abc-def0-123456789abc\n\
         Channel-State: CS_EXECUTE\n\
         Caller-Callee-ID-Name: _undef_\n\
         variable_sip_call_id: call-456\n\
         variable_rtp_use_codec_string: _undef_\n\
         \n";

    #[test]
    fn channel_dump_is_a_serialized_channel_data_event() {
        let event = parse_channel_dump(DUMP).unwrap();
        assert_eq!(
            event.event_type(),
            Some(freeswitch_types::EslEventType::ChannelData)
        );
        assert_eq!(
            event.unique_id(),
            Some("a1b2c3d4-5678-9abc-def0-123456789abc")
        );
        assert_eq!(
            event.header(EventHeader::ChannelName),
            Some("sofia/internal/1000@example.com")
        );
        assert_eq!(event.variable_str("sip_call_id"), Some("call-456"));
    }

    // A dump is a read-back, so the sentinel for "no value" must read as
    // absent rather than as a header whose value is the sentinel.
    #[test]
    fn channel_dump_skips_undef_values() {
        let event = parse_channel_dump(DUMP).unwrap();
        assert_eq!(event.header_str("Caller-Callee-ID-Name"), None);
        assert_eq!(event.variable_str("rtp_use_codec_string"), None);
        assert!(event
            .headers()
            .values()
            .all(|v| v != "_undef_"));
    }

    // The crate's own normalisation, which is the whole point of routing the
    // dump through the event decoder rather than an inline splitter.
    #[test]
    fn channel_dump_normalizes_header_keys() {
        let event = parse_channel_dump("unique-id: abc\nchannel-state: CS_EXECUTE\n\n").unwrap();
        assert_eq!(event.header_str("Unique-ID"), Some("abc"));
        assert_eq!(
            event
                .headers()
                .keys()
                .collect::<Vec<_>>(),
            vec!["Unique-ID", "Channel-State"]
        );
    }

    #[test]
    fn channel_dump_carries_inner_body() {
        let event =
            parse_channel_dump("Event-Name: CHANNEL_DATA\nContent-Length: 5\n\nhello\n").unwrap();
        assert_eq!(event.body(), Some("hello"));
    }

    // The channel hung up between the listing and the dump: a failure the
    // rebuild loop can skip, not an InvalidHeader from a line with no colon.
    #[test]
    fn channel_dump_of_a_dead_channel_is_a_command_failure() {
        let err = parse_channel_dump("-ERR No such channel!\n").unwrap_err();
        assert_eq!(
            err.command_failure(),
            Some(crate::error::CommandFailure::Err("No such channel!"))
        );
    }

    #[test]
    fn channel_dump_empty_body_is_a_protocol_error() {
        assert!(matches!(
            parse_channel_dump("").unwrap_err(),
            EslError::ProtocolError { .. }
        ));
    }

    #[test]
    fn channel_dump_rejects_json_and_xml_formats() {
        for (body, expected) in [
            (r#"{"Event-Name":"CHANNEL_DATA"}"#, "json"),
            ("  <event>\n  <headers/>\n</event>", "xml"),
        ] {
            let err = parse_channel_dump(body).unwrap_err();
            assert!(
                matches!(err, EslError::InvalidEventFormat { format: ref f } if f == expected),
                "body {body:?} should name the {expected} format, got: {err:?}"
            );
        }
    }

    #[test]
    fn channel_dump_lossy_value_rides_as_data_without_raw_body() {
        let event = parse_channel_dump("Event-Name: CHANNEL_DATA\nX-Bad: %E9foo\n\n").unwrap();
        let lossy = event.lossy_values();
        assert_eq!(
            lossy
                .iter()
                .map(|v| v.key())
                .collect::<Vec<_>>(),
            vec!["X-Bad"]
        );
        // The dump reached us as an already-decoded &str, so there are no
        // exact wire bytes to hand back.
        assert_eq!(event.raw_body(), None);
    }

    #[test]
    fn channel_dump_strict_option_rejects_invalid_utf8() {
        let options = ChannelDumpOptions::new().with_strict_header_utf8(true);
        assert!(options.strict_header_utf8());
        let err =
            parse_channel_dump_with_options("Event-Name: CHANNEL_DATA\nX-Bad: %E9\n\n", &options)
                .unwrap_err();
        assert!(matches!(err, EslError::InvalidUtf8InHeader { .. }));
    }

    #[test]
    fn esl_response_alert_info_array_encoding() {
        use sip_header::SipHeaderLookup;

        let headers: IndexMap<String, String> = [(
            "Alert-Info".into(),
            "ARRAY::<http://pbx.example.com/bell.wav>|:<http://pbx.example.com/siren.wav>".into(),
        )]
        .into();
        let resp = EslResponse::new(headers, None);
        let ai = resp
            .alert_info()
            .expect("should parse")
            .expect("should be present");
        assert_eq!(
            ai.entries()
                .len(),
            2,
            "ARRAY:: entries should expand"
        );
    }

    #[test]
    fn test_parse_connect_response() {
        let mut parser = EslParser::new();

        // FreeSWITCH serializes the outbound `connect` response with
        // switch_event_serialize(SWITCH_TRUE): every value is percent-encoded,
        // including the channel data. The parser must percent-decode them.
        let data = "Content-Type: command/reply\n\
             Reply-Text: +OK\n\
             Socket-Mode: async\n\
             Control: full\n\
             Event-Name: CHANNEL_DATA\n\
             Channel-Name: sofia/internal/1000%40example.com\n\
             Unique-ID: abcd-1234\n\
             Caller-Caller-ID-Name: Test%20User\n\
             \n";

        parser
            .add_data(data.as_bytes())
            .unwrap();
        let message = parser
            .parse_message()
            .unwrap()
            .unwrap();

        assert_eq!(message.message_type, MessageType::CommandReply);
        assert_eq!(
            message
                .headers
                .get("Channel-Name")
                .map(|s| s.as_str()),
            Some("sofia/internal/1000@example.com")
        );
        assert_eq!(
            message
                .headers
                .get("Caller-Caller-ID-Name")
                .map(|s| s.as_str()),
            Some("Test User")
        );
        assert_eq!(
            message
                .headers
                .get("Socket-Mode")
                .map(|s| s.as_str()),
            Some("async")
        );
        assert_eq!(
            message
                .headers
                .get("Control")
                .map(|s| s.as_str()),
            Some("full")
        );

        let response = EslResponse::from_message(message);
        assert!(response.is_success());
        assert_eq!(response.reply_text(), Some("+OK"));
        assert!(response
            .lossy_values()
            .is_empty());
    }

    #[test]
    fn test_connect_response_non_utf8_value_lossy() {
        // A channel-data value that is not valid UTF-8 after percent-decoding
        // (a Latin-1 caller name) is decoded lossily by default and surfaced
        // on the response, not a hard error.
        let mut parser = EslParser::new();
        let data = "Content-Type: command/reply\n\
             Reply-Text: +OK\n\
             Caller-Caller-ID-Name: Andr%E9\n\
             \n";
        parser
            .add_data(data.as_bytes())
            .unwrap();
        let response = EslResponse::from_message(
            parser
                .parse_message()
                .unwrap()
                .unwrap(),
        );

        assert!(response.is_success());
        assert_eq!(
            response.header("Caller-Caller-ID-Name"),
            Some("Andr\u{FFFD}")
        );
        let lossy = response.lossy_values();
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
        assert_eq!(entry.key(), "Caller-Caller-ID-Name");
        assert_eq!(entry.raw_value(), "Andr%E9");
    }

    #[test]
    fn test_api_response_non_utf8_body_raw_body() {
        let mut parser = EslParser::new();
        let mut data = b"Content-Type: api/response\nContent-Length: 4\n\n".to_vec();
        data.extend_from_slice(b"caf\xE9");
        parser
            .add_data(&data)
            .unwrap();
        let response = EslResponse::from_message(
            parser
                .parse_message()
                .unwrap()
                .unwrap(),
        );

        assert_eq!(response.body(), Some("caf\u{FFFD}"));
        assert_eq!(response.raw_body(), Some(&b"caf\xE9"[..]));
    }
}
