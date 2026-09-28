//! Convenience re-exports for common types and traits.
//!
//! ```
//! use freeswitch_types::prelude::*;
//! ```
//!
//! This brings [`HeaderLookup`] into scope (required for typed accessors like
//! `unique_id()`, `channel_state()`, `hangup_cause()`, etc.) along with the
//! header and variable enums used with `header()` and `variable()`, and the
//! catalog's row traits every store here implements.

pub use crate::headers::EventHeader;
pub use crate::lookup::HeaderLookup;
pub use crate::variables::{ChannelVariable, VariableName};
pub use sip_header_catalog::{SipHeader, SipHeaderRows, SipHeaderRowsExt};
