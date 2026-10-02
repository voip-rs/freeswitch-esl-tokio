//! FreeSWITCH Event Socket Library (ESL) client for Rust
//!
//! This crate provides an async Rust client for FreeSWITCH's Event Socket Library (ESL),
//! allowing applications to connect to FreeSWITCH, execute commands, and receive events.
//!
//! # Architecture
//!
//! The library uses a split reader/writer design:
//! - [`EslClient`] (Clone + Send) -- send commands from any task
//! - [`EslEventStream`] -- receive events from a background reader task
//!
//! # Examples
//!
//! ## Inbound Connection
//!
//! ```rust,no_run
//! use freeswitch_esl_tokio::{EslClient, EslError, DEFAULT_ESL_PASSWORD, DEFAULT_ESL_PORT};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), EslError> {
//!     let (client, mut events) = EslClient::connect("localhost", DEFAULT_ESL_PORT, DEFAULT_ESL_PASSWORD).await?;
//!
//!     let response = client.api("status").await?;
//!     println!("Status: {}", response.api_result()?);
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Outbound Mode
//!
//! In outbound mode, FreeSWITCH connects to *your* application via the
//! [`socket`](https://developer.signalwire.com/freeswitch/FreeSWITCH-Explained/Modules/mod_event_socket_1048924/)
//! dialplan application. You run a TCP listener and accept connections:
//!
//! ```rust,no_run
//! use freeswitch_esl_tokio::{EslClient, EslError, AppCommand, EventFormat, EventHeader};
//! use tokio::net::TcpListener;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), EslError> {
//!     let listener = TcpListener::bind("[::]:8040").await
//!         .map_err(EslError::from)?;
//!
//!     let (client, mut events) = EslClient::accept_outbound(&listener).await?;
//!
//!     // First command must be connect_session -- establishes the session
//!     // and returns all channel variables as headers.
//!     let channel_data = client.connect_session().await?;
//!     // HeaderLookup trait provides typed header access via EventHeader enum
//!     println!("Channel: {}", channel_data.header(EventHeader::ChannelName).unwrap_or("?"));
//!
//!     client.myevents(EventFormat::Plain).await?;
//!     client.linger(None).await?; // keep socket open after hangup
//!     client.resume().await?;     // resume dialplan on disconnect
//!
//!     client.send_command(AppCommand::answer()).await?;
//!     client.send_command(AppCommand::playback("ivr/ivr-welcome.wav")).await?;
//!
//!     while let Some(event) = events.try_next().await? {
//!         println!("{:?}", event.event_type());
//!     }
//!     Ok(())
//! }
//! ```
//!
//! Configure FreeSWITCH to connect to your app:
//! ```xml
//! <action application="socket" data="127.0.0.1:8040 async full"/>
//! ```
//!
//! See [`docs/outbound-esl-quirks.md`](https://github.com/voip-rs/freeswitch-esl-tokio/blob/master/docs/outbound-esl-quirks.md)
//! for protocol details and command availability by mode.
//!
//! ## Command Builders
//!
//! Typed builders for common API commands -- no raw string assembly needed:
//!
//! ```rust
//! use std::time::Duration;
//! use freeswitch_esl_tokio::{Originate, Endpoint, Application};
//! use freeswitch_esl_tokio::commands::SofiaGateway;
//!
//! let cmd = Originate::application(
//!     Endpoint::SofiaGateway(SofiaGateway::new("my_provider", "18005551234")),
//!     Application::simple("park"),
//! )
//! .cid_name("Outbound Call")
//! .cid_num("5551234")
//! .timeout(Duration::from_secs(30));
//!
//! // Use with client.api(&cmd.to_string()) or client.bgapi(&cmd.to_string())
//! assert!(cmd.to_string().contains("sofia/gateway/my_provider/18005551234"));
//! ```
//!
//! See the [`commands`] module for `Originate`, `UuidBridge`, `UuidTransfer`,
//! and other builders.
//!
//! ## Event Subscription
//!
//! [`EventSubscription`] captures format, event types, custom subclasses, and
//! filters as a single reusable unit. Build one from code or deserialize from
//! YAML/JSON, then apply it to any connection:
//!
//! ```rust,no_run
//! use freeswitch_esl_tokio::{
//!     EslClient, EslEventType, EventFormat, EventHeader, EventSubscription,
//!     HeaderLookup, DEFAULT_ESL_PASSWORD, DEFAULT_ESL_PORT,
//! };
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Build once, reuse on every (re)connection
//!     let subscription = EventSubscription::new(EventFormat::Plain)
//!         .event(EslEventType::ChannelAnswer)
//!         .event(EslEventType::ChannelHangup)
//!         .event(EslEventType::Heartbeat)
//!         .custom_subclass("sofia::register")?
//!         .filter(EventHeader::CallDirection, "inbound")?;
//!
//!     let (client, mut events) = EslClient::connect(
//!         "localhost", DEFAULT_ESL_PORT, DEFAULT_ESL_PASSWORD,
//!     ).await?;
//!
//!     client.apply_subscription(&subscription).await?;
//!
//!     while let Some(event) = events.try_next().await? {
//!         if let Some(state) = event.channel_state()? {
//!             println!("{:?}: {}", event.event_type(), state);
//!         }
//!     }
//!
//!     Ok(())
//! }
//! ```

pub mod app;
pub mod bgjob;
pub mod connection;
pub mod error;

pub(crate) mod buffer;
pub(crate) mod command;
pub(crate) mod constants;
pub(crate) mod protocol;

// Re-export sub-modules from freeswitch-types for path-based access
#[cfg(feature = "sdp")]
pub use freeswitch_types::sdp;
pub use freeswitch_types::{
    channel, commands, event, headers, lookup, lossy_values, prelude, sofia, variables,
};
pub use freeswitch_types::{sip_header, sip_uri};

// Re-export domain types from freeswitch-types
pub use freeswitch_types::{
    case_alias_key, channel_driver, extract_header, normalize_header_key, AnswerState, Application,
    BlockParse, BridgeDialString, BridgeDialStringDisplay, CallDirection, CallState, CarriedHeader,
    ChannelState, ChannelTimetable, ChannelVariable, CoreMediaVariable, DialString,
    DialStringCarrier, DialStringTarget, DialplanType, Endpoint, EndpointDisplay,
    EndpointFieldFault, EslArray, EslArrayError, EslEvent, EslEventPriority, EslEventType,
    EslHeaders, EventFormat, EventHeader, EventSubscription, EventSubscriptionError, ExecuteOn,
    ExecuteOnFault, FreeswitchVersion, GatewayPingStatus, GatewayRegState, GroupCallOrder,
    HangupCause, HeaderLookup, HistoryInfo, HistoryInfoEntry, HistoryInfoError, HistoryInfoReason,
    InvalidArgvSeparator, InvalidHeaderName, LogLevel, LossyValue, LossyValues, MultipartBody,
    MultipartBodyError, MultipartItem, Originate, OriginateDisplay, OriginateError,
    OriginateTarget, ParseAnswerStateError, ParseBlockParseError, ParseCallDirectionError,
    ParseCallStateError, ParseChannelStateError, ParseChannelVariableError,
    ParseCoreMediaVariableError, ParseDialplanTypeError, ParseEventFormatError,
    ParseEventHeaderError, ParseEventTypeError, ParseFreeswitchVersionError,
    ParseGatewayPingStatusError, ParseGatewayRegStateError, ParseGroupCallOrderError,
    ParseHangupCauseError, ParseHeaderError, ParseHoldActionError, ParseLogLevelError,
    ParseMuteActionError, ParsePriorityError, ParseSipHeaderAddrError, ParseSipHeaderError,
    ParseSipPassthroughError, ParseSipUserPingStatusError, ParseSofiaEventSubclassError,
    ParseTimetableError, RtpStatUnit, SipGeolocation, SipGeolocationRef, SipHeader, SipHeaderAddr,
    SipHeaderLookup, SipHeaderPrefix, SipPassthroughHeader, SipUserPingStatus, SofiaChannelName,
    SofiaEventSubclass, TimetableField, TimetablePrefix, UnvouchedVersion, UriInfo, UriInfoEntry,
    UriInfoError, UuidAnswer, UuidBridge, UuidDeflect, UuidGetVar, UuidHold, UuidKill,
    UuidSendDtmf, UuidSetVar, UuidTransfer, VariableName, Variables, VariablesDisplay,
    VariablesType, DEFAULT_ESL_PASSWORD, DEFAULT_ESL_PORT, MAX_ARRAY_ITEMS, VARIABLE_PREFIX,
};

#[cfg(doctest)]
mod readme {
    #![doc = include_str!("../README.md")]
}

#[cfg(doctest)]
mod guide {
    mod commands {
        #![doc = include_str!("../docs/guide/commands.md")]
    }
    mod config {
        #![doc = include_str!("../docs/guide/config.md")]
    }
    mod connecting {
        #![doc = include_str!("../docs/guide/connecting.md")]
    }
    mod events {
        #![doc = include_str!("../docs/guide/events.md")]
    }
    mod sdp {
        #![doc = include_str!("../docs/guide/sdp.md")]
    }
}

pub use app::dptools::AppCommand;
pub use bgjob::{BgJobResult, BgJobTracker};
pub use command::{
    parse_api_body, parse_channel_dump, parse_channel_dump_with_options, ChannelDumpOptions,
    CommandBuilder, EslCommand, EslResponse, ExecuteOptions, ReplyStatus, Secret,
};
pub use connection::{
    AuthMethod, ConnectionMode, ConnectionStatus, DisconnectReason, EslClient, EslConnectOptions,
    EslEventStream, EventOverflow,
};
pub use constants::UNDEF_VALUE;
pub use error::{CommandFailure, EslError, EslResult};
