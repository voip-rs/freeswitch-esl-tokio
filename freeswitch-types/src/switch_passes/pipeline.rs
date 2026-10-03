//! Every pass a dial string meets between its carrier and the channel, in the order the switch
//! runs them: the carrier's own pass, then `switch_ivr_originate`'s.

use std::ops::Range;

use super::api_argument::ArgvCut;
use super::expansion::expand_escapes;
use super::originate_legs::{dial_list, DialList};
use super::{byte_range, extent, trace, PipelineError, Traced};
use crate::commands::variables::{DialStringCarrier, DialStringTarget};

#[cfg(test)]
mod tests;

/// Run every pass `target` applies, from the text as given to what each leg's
/// channel receives.
pub(crate) fn read(input: &str, target: DialStringTarget) -> Result<DialList, PipelineError> {
    let input = trace(input);
    let (text, raw, carrier_expands) = match target.carrier() {
        DialStringCarrier::EslApi => {
            let (text, raw) = api_argument(&input, target)?;
            (text, raw, false)
        }
        DialStringCarrier::Dialplan => {
            let (text, references) = expand_escapes(&input);
            (text, extent(&input), !references.is_empty())
        }
        DialStringCarrier::Direct => {
            let raw = extent(&input);
            (input, raw, false)
        }
    };
    dial_list(&text, raw, carrier_expands, target.block_parse())
}

/// `originate`'s own `switch_separate_string`, which the dial string must survive as one
/// argument: that argument and the input bytes it covers.
fn api_argument(
    text: &[Traced],
    target: DialStringTarget,
) -> Result<(Vec<Traced>, Range<usize>), PipelineError> {
    let Some(token) = target.split_argument(text) else {
        return Ok((text.to_vec(), extent(text)));
    };
    token
        .map_err(|ArgvCut| PipelineError::ArgvSplit)?
        .map(|token| (token.text, byte_range(text, extent(text), token.raw)))
        .ok_or(PipelineError::Empty)
}
