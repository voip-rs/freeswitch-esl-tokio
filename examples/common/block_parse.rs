//! The parser revision an example renders its dial strings for. Included by
//! path, like `env.rs`, by the examples that render an originate or a bridge.

use freeswitch_esl_tokio::{BlockParse, FreeswitchVersion};
use std::env::VarError;

/// `ESL_BLOCK_PARSE` names a revision outright, for a development build or a
/// version the crate does not vouch for; otherwise `FREESWITCH_VERSION` states
/// the release the switch runs. How a variable value is escaped depends on that
/// revision, and the crate never asks the switch, so with neither set this is
/// an error rather than a guess that is right for one build and wrong on another.
pub fn block_parse_from_env() -> Result<BlockParse, String> {
    match std::env::var("ESL_BLOCK_PARSE") {
        Ok(name) => {
            return name
                .parse()
                .map_err(|e| format!("ESL_BLOCK_PARSE: {e}"))
        }
        Err(VarError::NotPresent) => {}
        Err(e) => return Err(format!("ESL_BLOCK_PARSE: {e}")),
    }
    match std::env::var("FREESWITCH_VERSION") {
        Ok(value) => {
            let version: FreeswitchVersion = value
                .parse()
                .map_err(|e| format!("FREESWITCH_VERSION: {e}"))?;
            BlockParse::for_version(&version).map_err(|e| e.to_string())
        }
        Err(VarError::NotPresent) => Err(
            "set FREESWITCH_VERSION to the release the switch runs (e.g. 1.10.12), \
             or ESL_BLOCK_PARSE to a parser revision for a development build"
                .into(),
        ),
        Err(e) => Err(format!("FREESWITCH_VERSION: {e}")),
    }
}
