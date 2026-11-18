//! The SDP session layer: sofia-sip's `sdp_parse` at flags 0, as `switch_core_media.c` calls it,
//! reduced to what codec extraction reads and run leniently.
//!
//! sofia decides which bytes form a line (`next()`), which section a line belongs to, which lines
//! are skipped (`parse_descs` skips any media-level line it does not know), and where the parse
//! refuses the offer (`parse_message`, `parse_media`, `parse_media_attr`, `sdp_sanity_check`) or
//! stops reading it. This module computes all of that, in sofia's order, but parses the whole
//! input regardless: the first refusal and the stop come back as warnings beside the sections.
//!
//! Invariants a change has to keep:
//!
//! - Sofia's outcome is computed in sofia's order. Its parse ends at the first refusal or stop, so
//!   nothing after either is reported, and a one-character record in the session part returns
//!   from `parse_message` before `post_session`, so the sanity check does not run there.
//! - `&input[..stop]` re-parses to exactly the sections sofia saw: the stop offset is where a line
//!   starts, or the NUL `strlen` ends the switch's input at.
//! - Offsets are byte offsets into the caller's input, at the start of the line concerned.
//! - Only `m=` and `a=` lines are decoded and kept. The values of the other lines (`o=`, `c=`,
//!   `b=`, `t=`, `r=`, `z=`, `k=`) are not validated, so an offer sofia refuses for one of them
//!   alone parses here with no warning.

pub(crate) mod scan;

use std::fmt;

use crate::sdp::error::{SdpWarning, SwitchRejection};

use scan::{is_slash_token, is_token, lines, media_token, parse_ul, skip, token, Line, SP_TAB};

/// An `a=` line split as `token(&r, ":", TOKEN, SPACE TAB)` splits it.
#[derive(Debug, Clone)]
pub(crate) struct Attribute {
    pub(crate) attribute: String,
    pub(crate) value: Option<String>,
    /// Where the line starts in the input.
    pub(crate) offset: usize,
}

/// One `m=` line and the attributes below it.
#[derive(Debug, Clone)]
pub(crate) struct Media {
    pub(crate) media: String,
    pub(crate) port: u16,
    pub(crate) proto: String,
    /// The `m=` line past its proto, verbatim.
    pub(crate) fmt: String,
    /// The RTP payload types the format list reads as, for an RTP proto.
    pub(crate) payloads: Vec<u8>,
    pub(crate) attributes: Vec<Attribute>,
}

/// A parsed session and what the switch's own parse of it would report.
#[derive(Debug, Clone, Default)]
pub(crate) struct Session {
    pub(crate) attributes: Vec<Attribute>,
    pub(crate) medias: Vec<Media>,
    /// `SwitchStopsReading`, then `SwitchRejects`, each where sofia raises one.
    pub(crate) warnings: Vec<SdpWarning>,
}

/// Input extraction cannot read at all.
#[derive(Debug)]
pub(crate) struct SessionError {
    offset: usize,
    reason: &'static str,
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.reason, self.offset)
    }
}

impl std::error::Error for SessionError {}

/// Whether sofia reads this proto's format list, `a=rtpmap` and `a=fmtp` as RTP.
///
/// `sdp_media_transport` matches the proto against an exact set, so a name merely containing
/// `RTP` is not one; `sdp_media_has_rtp` then decides. The bare `RTP` is `parse_media`'s
/// non-strict spelling of `RTP/AVP`.
pub(crate) fn proto_has_rtp(proto: &str) -> bool {
    const RTP_PROTOS: [&str; 8] = [
        "RTP",
        "RTP/AVP",
        "RTP/AVPF",
        "RTP/SAVP",
        "RTP/SAVPF",
        "UDP/RTP/AVPF",
        "UDP/TLS/RTP/SAVP",
        "UDP/TLS/RTP/SAVPF",
    ];
    RTP_PROTOS
        .iter()
        .any(|p| p.eq_ignore_ascii_case(proto))
}

/// Parse `input` as far as extraction can go.
///
/// Fails only where the first line is not exactly `v=0`, which makes the input something other
/// than SDP; on an `m=` port above 65535; and on an `m=` or `a=` line that is not UTF-8.
pub(crate) fn parse(input: &[u8]) -> Result<Session, SessionError> {
    let all: Vec<Line<'_>> = lines(input).collect();
    match all.first() {
        Some(Line { offset: 0, text }) if *text == b"v=0" => {}
        _ => {
            return Err(SessionError {
                offset: 0,
                reason: "the first line is not v=0",
            })
        }
    }
    let nul = input
        .iter()
        .position(|&b| b == 0);
    let mut switch = Switch::default();
    let mut session = Session::default();
    // None while the lines belong to an m= line extraction could not read.
    let mut section: Option<usize> = None;
    let mut in_media = false;
    for line in &all[1..] {
        let read = Read::of(line)?;
        match nul {
            Some(nul) if line.offset > nul => switch.stop(nul, true),
            _ => switch.read(line, &read, in_media),
        }
        match read {
            Read::Media { section: found, .. } => {
                in_media = true;
                section = found.map(|media| {
                    session
                        .medias
                        .push(media);
                    session
                        .medias
                        .len()
                        - 1
                });
            }
            Read::Attribute(Some(attribute)) if in_media => {
                if let Some(at) = section {
                    session.medias[at]
                        .attributes
                        .push(attribute);
                }
            }
            Read::Attribute(Some(attribute)) => session
                .attributes
                .push(attribute),
            _ => {}
        }
    }
    let followed = switch
        .stopped_at()
        .is_some_and(|stop| {
            all.iter()
                .any(|line| {
                    line.offset > stop
                        && !line
                            .text
                            .is_empty()
                })
        });
    session.warnings = switch.finish(followed);
    Ok(session)
}

/// What one line holds, read once for both extraction and the model of sofia.
enum Read {
    /// Empty after its leading SP/TAB, or one byte.
    Short,
    /// Not key=value with a one-byte key.
    NotKeyValue,
    /// `m=`: the section extraction reads, and the refusal `parse_media` raises on the line.
    Media {
        section: Option<Media>,
        refusal: Option<SwitchRejection>,
    },
    /// `a=`, `None` where its name has no token byte.
    Attribute(Option<Attribute>),
    /// Any other key.
    Other(u8),
}

impl Read {
    fn of(line: &Line<'_>) -> Result<Self, SessionError> {
        let text = line.text;
        if text.len() < 2 {
            return Ok(Self::Short);
        }
        if text[1] != b'=' {
            return Ok(Self::NotKeyValue);
        }
        let rest = skip(&text[2..], SP_TAB);
        if matches!(text[0], b'm' | b'a') && std::str::from_utf8(rest).is_err() {
            return Err(SessionError {
                offset: line.offset,
                reason: "an m= or a= line is not UTF-8",
            });
        }
        Ok(match text[0] {
            b'm' => media(rest, line.offset)?,
            b'a' => Self::Attribute(token(rest, b":", Some(is_token), SP_TAB).map(
                |(name, value)| Attribute {
                    attribute: ascii(name),
                    value: (!value.is_empty()).then(|| ascii(value)),
                    offset: line.offset,
                },
            )),
            key => Self::Other(key),
        })
    }
}

/// Text cut from a line already checked as UTF-8, at a boundary the scanners only place beside
/// an ASCII byte.
fn ascii(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `parse_media` over an `m=` line past `m=`.
///
/// A refusal on the format list keeps the section, with the payload types that do read: only the
/// media, port and proto decide whether extraction has a section at all.
fn media(rest: &[u8], offset: usize) -> Result<Read, SessionError> {
    let Some(MediaHead {
        media,
        port,
        proto,
        formats,
    }) = media_head(rest)
    else {
        return Ok(Read::Media {
            section: None,
            refusal: Some(SwitchRejection::InvalidMediaLine),
        });
    };
    let port = u16::try_from(port).map_err(|_| SessionError {
        offset,
        reason: "an m= port is above 65535",
    })?;
    let proto = ascii(proto);
    let (payloads, refusal) = if proto_has_rtp(&proto) {
        payload_types(formats)
    } else {
        (Vec::new(), format_refusal(formats))
    };
    Ok(Read::Media {
        section: Some(Media {
            media: ascii(media),
            port,
            proto,
            fmt: ascii(formats),
            payloads,
            attributes: Vec::new(),
        }),
        refusal,
    })
}

/// The media, port and proto `parse_media` reads, and the format list after them.
struct MediaHead<'a> {
    media: &'a [u8],
    port: u64,
    proto: &'a [u8],
    formats: &'a [u8],
}

/// `None` where `parse_media` refuses the media, the port or the proto.
fn media_head(rest: &[u8]) -> Option<MediaHead<'_>> {
    let (media, r) = media_token(rest, is_token, &[])?;
    let (port, mut r) = parse_ul(r, 0)?;
    if let Some(after) = r.strip_prefix(b"/") {
        r = parse_ul(after, 0)?.1;
    }
    let (proto, formats) = media_token(r, is_slash_token, b" ")?;
    Some(MediaHead {
        media,
        port,
        proto,
        formats,
    })
}

/// `parse_payload`: each format through `parse_ul` below 128. Where sofia refuses a format, the
/// word holding it is skipped and the rest still read.
fn payload_types(formats: &[u8]) -> (Vec<u8>, Option<SwitchRejection>) {
    let mut payloads = Vec::new();
    let mut refusal = None;
    let mut r = formats;
    while !r.is_empty() {
        match parse_ul(r, 128) {
            Some((pt, after)) => {
                // parse_ul refused 128 and above.
                payloads.extend(u8::try_from(pt).ok());
                r = after;
            }
            None => {
                refusal = Some(SwitchRejection::InvalidRtpFormat);
                let word = r
                    .iter()
                    .take_while(|b| !SP_TAB.contains(b))
                    .count();
                r = skip(&r[word..], SP_TAB);
            }
        }
    }
    (payloads, refusal)
}

/// `parse_media`'s read of a format list that is not RTP: a token at a time, refused at a format
/// with no token byte.
fn format_refusal(formats: &[u8]) -> Option<SwitchRejection> {
    let mut r = formats;
    while !r.is_empty() {
        match token(r, SP_TAB, Some(is_token), SP_TAB) {
            Some((_, after)) => r = after,
            None => return Some(SwitchRejection::InvalidMediaLine),
        }
    }
    None
}

/// Where sofia's own parse of the input ended.
enum End {
    Refused(SwitchRejection, Option<usize>),
    Stopped { at: usize, sanity: bool },
}

/// One section as `sdp_sanity_check` sees it.
struct SwitchSection {
    offset: usize,
    live: bool,
    connection: bool,
}

/// sofia's parse of the same lines, held to the point where it ends.
#[derive(Default)]
struct Switch {
    end: Option<End>,
    origin: bool,
    connection: bool,
    times: bool,
    zone_open: bool,
    sections: Vec<SwitchSection>,
}

impl Switch {
    fn refuse(&mut self, reason: SwitchRejection, offset: Option<usize>) {
        self.end
            .get_or_insert(End::Refused(reason, offset));
    }

    fn stop(&mut self, at: usize, sanity: bool) {
        self.end
            .get_or_insert(End::Stopped { at, sanity });
    }

    fn stopped_at(&self) -> Option<usize> {
        match self.end {
            Some(End::Stopped { at, .. }) => Some(at),
            _ => None,
        }
    }

    /// One line through `parse_message` or `parse_descs`, whichever holds it.
    fn read(&mut self, line: &Line<'_>, read: &Read, in_media: bool) {
        if self
            .end
            .is_some()
        {
            return;
        }
        let at = Some(line.offset);
        match read {
            // next() returning NULL ends the loop, so post_session still runs; a one-byte record
            // returns from parse_message before it.
            Read::Short => self.stop(
                line.offset,
                line.text
                    .is_empty()
                    || in_media,
            ),
            Read::NotKeyValue if in_media => {}
            Read::NotKeyValue => self.refuse(SwitchRejection::BadLine, at),
            Read::Media {
                refusal: Some(reason),
                ..
            } => self.refuse(*reason, at),
            Read::Media { section, .. } => self
                .sections
                .push(SwitchSection {
                    offset: line.offset,
                    live: section
                        .as_ref()
                        .is_some_and(|media| media.port != 0),
                    connection: false,
                }),
            Read::Attribute(None) => self.refuse(SwitchRejection::InvalidAttributeName, at),
            Read::Attribute(Some(_)) => {}
            Read::Other(b'c') if in_media => {
                if let Some(section) = self
                    .sections
                    .last_mut()
                {
                    section.connection = true;
                }
            }
            Read::Other(_) if in_media => {}
            Read::Other(key) => self.session_field(*key, at),
        }
    }

    /// `parse_message`'s switch on a session-level key.
    fn session_field(&mut self, key: u8, at: Option<usize>) {
        match key {
            b'o' => self.origin = true,
            b'c' => self.connection = true,
            b't' => {
                self.times = true;
                self.zone_open = true;
            }
            b'r' if !self.times => self.refuse(SwitchRejection::RepeatWithoutTime, at),
            b'z' if self.zone_open => self.zone_open = false,
            b'z' => self.refuse(SwitchRejection::ZoneWithoutTime, at),
            b's' | b'i' | b'u' | b'e' | b'p' | b'b' | b'k' | b'r' => {}
            _ => self.refuse(SwitchRejection::UnknownField, at),
        }
    }

    /// The warnings sofia's parse raises; `followed` is whether any line comes after its stop.
    fn finish(self, followed: bool) -> Vec<SdpWarning> {
        let mut warnings = Vec::new();
        let sanity = match self.end {
            Some(End::Refused(reason, offset)) => {
                warnings.push(SdpWarning::SwitchRejects { reason, offset });
                false
            }
            Some(End::Stopped { at, sanity }) => {
                if followed {
                    warnings.push(SdpWarning::SwitchStopsReading { offset: at });
                }
                sanity
            }
            None => true,
        };
        if sanity {
            warnings.extend(self.sanity_check());
        }
        warnings
    }

    /// `sdp_sanity_check` at flags 0: `o=`, then `c=` at session level or on every live section.
    fn sanity_check(&self) -> Option<SdpWarning> {
        if !self.origin {
            return Some(SdpWarning::SwitchRejects {
                reason: SwitchRejection::MissingOrigin,
                offset: None,
            });
        }
        if self.connection {
            return None;
        }
        self.sections
            .iter()
            .find(|section| section.live && !section.connection)
            .map(|section| SdpWarning::SwitchRejects {
                reason: SwitchRejection::MissingConnection,
                offset: Some(section.offset),
            })
    }
}
