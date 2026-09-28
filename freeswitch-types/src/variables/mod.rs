//! Channel variable types: format parsers (`ARRAY::`, SIP multipart) and typed
//! variable name enums.

mod conference;
mod core;
mod core_media;
mod esl_array;
#[cfg(feature = "esl")]
mod esl_headers;
mod loopback;
mod sip_multipart;
mod sip_passthrough;
#[cfg(feature = "esl")]
pub(crate) use sip_passthrough::names_outgoing_header;
mod sofia;

pub use self::core::{ChannelVariable, ParseChannelVariableError};
pub use conference::{ConferenceVariable, ParseConferenceVariableError};
pub use core_media::{CoreMediaVariable, ParseCoreMediaVariableError, RtpStatUnit};
pub use esl_array::{EslArray, EslArrayError, MAX_ARRAY_ITEMS};
#[cfg(feature = "esl")]
pub use esl_headers::EslHeaders;
pub use loopback::{
    LoopbackChannelName, LoopbackHangupCause, LoopbackLeg, LoopbackResignation, LoopbackVariable,
    ParseLoopbackHangupCauseError, ParseLoopbackLegError, ParseLoopbackVariableError,
};
pub use sip_multipart::{MultipartBody, MultipartBodyError, MultipartItem};
pub use sip_passthrough::{
    InvalidHeaderName, ParseSipPassthroughError, SipHeaderPrefix, SipPassthroughHeader,
};
pub use sofia::{CarriedHeader, ParseSofiaVariableError, SofiaVariable};

/// Trait for typed channel variable name enums.
///
/// Implement this on variable name enums to use them with
/// [`HeaderLookup::variable()`](crate::HeaderLookup::variable) and
/// [`variable_str()`](crate::HeaderLookup::variable_str).
/// For variables not covered by any typed enum, use `variable_str()`.
pub trait VariableName {
    /// Wire-format variable name (e.g. `"sip_call_id"`).
    fn as_str(&self) -> &str;

    /// Full event header name, prefixed (e.g. `"variable_sip_call_id"`).
    ///
    /// For callers naming a variable with no event in hand -- a config entry, a
    /// filter, a request field. Reading one out of a store is
    /// [`HeaderLookup::variable()`](crate::HeaderLookup::variable), which takes
    /// the bare name.
    fn header_name(&self) -> String {
        crate::lookup::variable_key(self.as_str())
    }
}

/// Forwards to the inherent `as_str()` a `define_header_enum!` enum already has.
macro_rules! impl_variable_name {
    ($($enum:ident),+ $(,)?) => {
        $(
            impl VariableName for $enum {
                fn as_str(&self) -> &str {
                    $enum::as_str(self)
                }
            }
        )+
    };
}

impl_variable_name!(
    ChannelVariable,
    SofiaVariable,
    CoreMediaVariable,
    LoopbackVariable,
    ConferenceVariable,
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HeaderLookup, VARIABLE_PREFIX};
    use sip_header::SipHeader;
    use std::collections::HashMap;

    #[test]
    fn header_name_composes_the_wire_key() {
        assert_eq!(
            ChannelVariable::ReadCodec.header_name(),
            "variable_read_codec"
        );
        assert_eq!(
            LoopbackVariable::LoopbackLeg.header_name(),
            "variable_loopback_leg"
        );
        assert!(ChannelVariable::ReadCodec
            .header_name()
            .starts_with(VARIABLE_PREFIX));
    }

    // SipPassthroughHeader's as_str borrows from self, so the provided method
    // must stay &self-borrowing to cover it.
    #[test]
    fn header_name_covers_a_borrowed_as_str() {
        let h = SipPassthroughHeader::invite(SipHeader::CallInfo);
        assert_eq!(h.header_name(), "variable_sip_i_call_info");
    }

    // What a downstream crate defining its own variable enum gets for free.
    #[test]
    fn an_external_implementor_supplies_only_as_str() {
        struct TenantVariable;

        impl VariableName for TenantVariable {
            fn as_str(&self) -> &str {
                "tenant_id"
            }
        }

        assert_eq!(TenantVariable.header_name(), "variable_tenant_id");
    }

    #[test]
    fn header_name_is_the_key_a_header_store_holds() {
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert(ChannelVariable::ReadCodec.header_name(), "PCMU".into());
        assert_eq!(map.variable(ChannelVariable::ReadCodec), Some("PCMU"));
    }
}
