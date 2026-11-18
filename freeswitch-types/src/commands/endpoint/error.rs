use std::fmt;
use std::str::FromStr;

use crate::channel::HangupCause;
use crate::commands::originate::OriginateError;

/// Bridge to a specific hangup cause: `error/{cause}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ErrorEndpoint {
    /// Hangup cause code.
    cause: HangupCause,
}

impl ErrorEndpoint {
    /// Create a new error endpoint with the given hangup cause.
    pub fn new(cause: HangupCause) -> Self {
        Self { cause }
    }

    /// Hangup cause code.
    pub fn cause(&self) -> HangupCause {
        self.cause
    }
}

impl fmt::Display for ErrorEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error/{}", self.cause)
    }
}

impl FromStr for ErrorEndpoint {
    type Err = OriginateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let cause_str = s
            .strip_prefix("error/")
            .ok_or_else(|| OriginateError::ParseError("not an error endpoint".into()))?;
        let cause: HangupCause = cause_str
            .parse()
            .map_err(|source| OriginateError::UnknownHangupCause {
                value: cause_str.to_string(),
                source,
            })?;
        Ok(Self { cause })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_endpoint_display() {
        let ep = ErrorEndpoint::new(HangupCause::UserBusy);
        assert_eq!(ep.to_string(), "error/USER_BUSY");
    }

    #[test]
    fn error_endpoint_from_str() {
        let ep: ErrorEndpoint = "error/USER_BUSY"
            .parse()
            .unwrap();
        assert_eq!(ep.cause, HangupCause::UserBusy);
    }

    #[test]
    fn error_endpoint_from_str_rejects_wrong_case() {
        assert!("error/user_busy"
            .parse::<ErrorEndpoint>()
            .is_err());
    }

    #[test]
    fn error_endpoint_round_trip() {
        let ep = ErrorEndpoint::new(HangupCause::NoAnswer);
        let s = ep.to_string();
        let parsed: ErrorEndpoint = s
            .parse()
            .unwrap();
        assert_eq!(parsed, ep);
    }

    #[test]
    fn serde_error_endpoint() {
        let ep = ErrorEndpoint::new(HangupCause::UserBusy);
        let json = serde_json::to_string(&ep).unwrap();
        let parsed: ErrorEndpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ep);
    }
}
