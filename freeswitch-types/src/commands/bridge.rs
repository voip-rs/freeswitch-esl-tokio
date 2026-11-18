//! Bridge dial string builder for multi-endpoint bridge commands.
//!
//! Supports simultaneous ring (`,`) and sequential failover (`|`)
//! with per-endpoint channel variables and global default variables.

use std::fmt;

use super::endpoint::{DialString, Endpoint};
use super::originate::OriginateError;
use super::variables::{
    installed_variables, read_error, BlockParse, DialStringCarrier, DialStringTarget, Variables,
};
use crate::switch_passes::brackets::Block;
use crate::switch_passes::originate_legs::{Leg, MAX_PEERS};
use crate::switch_passes::pipeline;

/// A bridge dial string is the argument of a dialplan application, which
/// receives it whole.
const CARRIER: DialStringCarrier = DialStringCarrier::Dialplan;

/// Typed bridge dial string.
///
/// Format: `{global_vars}[ep1_vars]ep1,[ep2_vars]ep2|[ep3_vars]ep3`
///
/// - `,` separates endpoints rung simultaneously (within a group)
/// - `|` separates groups tried sequentially (failover)
/// - Each endpoint may have channel-scope `[variables]`
/// - Global `{variables}` apply to all endpoints
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "config::BridgeDialString"))]
pub struct BridgeDialString {
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    variables: Option<Variables>,
    groups: Vec<Vec<Endpoint>>,
}

impl BridgeDialString {
    /// Create a new bridge dial string with the given failover groups, refusing what
    /// [`parse_for`](Self::parse_for) refuses at the revision the escaping was measured against.
    pub fn new(groups: Vec<Vec<Endpoint>>) -> Result<Self, OriginateError> {
        let bridge = Self {
            variables: None,
            groups,
        };
        bridge.check_legs(BlockParse::MEASURED)?;
        Ok(bridge)
    }

    /// Set global default-scope variables.
    pub fn with_variables(mut self, variables: Variables) -> Self {
        self.variables = Some(variables);
        self
    }

    /// Default-scope variables applied to all endpoints.
    pub fn variables(&self) -> Option<&Variables> {
        self.variables
            .as_ref()
    }

    /// Sequential failover groups (`|`-separated). Within each group,
    /// endpoints ring simultaneously (`,`-separated).
    pub fn groups(&self) -> &[Vec<Endpoint>] {
        &self.groups
    }

    /// Mutable reference to the default-scope variables.
    pub fn variables_mut(&mut self) -> &mut Option<Variables> {
        &mut self.variables
    }
}

/// A [`BridgeDialString`] written for one parser revision.
struct Rendered<'a> {
    bridge: &'a BridgeDialString,
    block_parse: BlockParse,
}

impl fmt::Display for Rendered<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.bridge
            .write_with(f, self.block_parse)
    }
}

impl BridgeDialString {
    /// The dial string for a switch running `block_parse`, as the `bridge` application
    /// receives it.
    pub fn render(&self, block_parse: BlockParse) -> String {
        Rendered {
            bridge: self,
            block_parse,
        }
        .to_string()
    }

    fn write_with(&self, f: &mut fmt::Formatter<'_>, block_parse: BlockParse) -> fmt::Result {
        let target = DialStringTarget::new(CARRIER, block_parse);
        if let Some(vars) = &self.variables {
            if !vars.is_empty() {
                write!(f, "{}", vars.display_for(target))?;
            }
        }
        for (gi, group) in self
            .groups
            .iter()
            .enumerate()
        {
            if gi > 0 {
                f.write_str("|")?;
            }
            for (ei, ep) in group
                .iter()
                .enumerate()
            {
                if ei > 0 {
                    f.write_str(",")?;
                }
                write!(f, "{}", ep.display_for(target))?;
            }
        }
        Ok(())
    }

    /// Parse a dial string written for a switch running `block_parse`, mirroring
    /// [`render`](Self::render).
    pub fn parse_for(s: &str, block_parse: BlockParse) -> Result<Self, OriginateError> {
        let target = DialStringTarget::new(CARRIER, block_parse);
        let s = s.trim_matches(' ');
        if s.is_empty() {
            return Err(OriginateError::ParseError(
                "empty bridge dial string".into(),
            ));
        }

        let list = pipeline::read(s, target).map_err(read_error)?;
        let [thread] = &list.threads[..] else {
            return Err(OriginateError::ParseError(
                "a bridge dial string has no enterprise threads".into(),
            ));
        };
        if !list
            .blocks
            .is_empty()
            || thread
                .blocks
                .len()
                > 1
        {
            return Err(OriginateError::ParseError(
                "a bridge dial string carries one leading variable block".into(),
            ));
        }
        let variables = installed_variables(
            thread
                .blocks
                .first(),
        )?;

        let past_limit = |group| {
            list.past_limit
                .iter()
                .any(|cut| cut.group == group)
        };
        if past_limit(None) {
            return Err(OriginateError::TooManyLegs {
                group: None,
                max: MAX_PEERS,
            });
        }
        let mut groups = Vec::new();
        for (k, group) in thread
            .groups
            .iter()
            .enumerate()
        {
            if past_limit(Some(k)) {
                return Err(OriginateError::TooManyLegs {
                    group: Some(groups.len()),
                    max: MAX_PEERS,
                });
            }
            let legs: Vec<&Leg> = group
                .iter()
                .filter(|leg| {
                    !leg.blocks
                        .is_empty()
                        || !leg
                            .endpoint
                            .is_empty()
                })
                .collect();
            let texts: Option<Vec<&str>> = legs
                .iter()
                .map(|leg| {
                    s.get(
                        leg.raw
                            .clone(),
                    )
                })
                .collect();
            if let Some(texts) = texts {
                let joined = legs
                    .iter()
                    .map(|leg| reading(leg))
                    .collect();
                if legs_interfere(texts, Some(joined), target) {
                    return Err(OriginateError::BracketSpansLegs {
                        group: groups.len(),
                    });
                }
            }
            let mut endpoints = Vec::new();
            for leg in legs {
                if leg
                    .blocks
                    .len()
                    > 1
                {
                    return Err(OriginateError::ParseError(
                        "an endpoint carries one variable block".into(),
                    ));
                }
                let vars = installed_variables(
                    leg.blocks
                        .first(),
                )?;
                let mut endpoint = Endpoint::parse_bare(&leg.endpoint)?;
                endpoint.set_variables(vars)?;
                endpoints.push(endpoint);
            }
            if !endpoints.is_empty() {
                groups.push(endpoints);
            }
        }

        let bridge = Self { variables, groups };
        bridge.check_legs(block_parse)?;
        Ok(bridge)
    }

    /// Refuse more legs or groups than the switch splits off, then a group whose legs the
    /// switch's comma scan reads across.
    fn check_legs(&self, block_parse: BlockParse) -> Result<(), OriginateError> {
        let too_many = |group| OriginateError::TooManyLegs {
            group,
            max: MAX_PEERS,
        };
        if self
            .groups
            .len()
            > MAX_PEERS
        {
            return Err(too_many(None));
        }
        if let Some(group) = self
            .groups
            .iter()
            .position(|group| group.len() > MAX_PEERS)
        {
            return Err(too_many(Some(group)));
        }
        let target = DialStringTarget::new(CARRIER, block_parse);
        match self
            .groups
            .iter()
            .position(|group| endpoints_interfere(group, target))
        {
            Some(group) => Err(OriginateError::BracketSpansLegs { group }),
            None => Ok(()),
        }
    }
}

/// What the switch installs on a leg and the endpoint text it dials.
type LegReading = (Vec<Block>, String);

fn reading(leg: &Leg) -> LegReading {
    (
        leg.blocks
            .clone(),
        leg.endpoint
            .clone(),
    )
}

/// Each leg the switch reads of `text`, as one group.
fn read_group(text: &str, target: DialStringTarget) -> Option<Vec<LegReading>> {
    let list = pipeline::read(text, target).ok()?;
    let [thread] = &list.threads[..] else {
        return None;
    };
    let [group] = &thread.groups[..] else {
        return None;
    };
    Some(
        group
            .iter()
            .map(reading)
            .collect(),
    )
}

/// Whether the switch reads a group otherwise than each of its leg `texts` dialled alone, as a
/// bracket range `switch_ivr_originate`'s comma scan runs from one leg into another makes it.
fn legs_interfere<'a>(
    texts: impl IntoIterator<Item = &'a str>,
    joined: Option<Vec<LegReading>>,
    target: DialStringTarget,
) -> bool {
    let Some(alone) = texts
        .into_iter()
        .map(|text| read_group(text, target))
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    joined != Some(alone.concat())
}

/// [`legs_interfere`] for a group's endpoints as they render.
fn endpoints_interfere(group: &[Endpoint], target: DialStringTarget) -> bool {
    let rendered: Vec<String> = group
        .iter()
        .map(|endpoint| {
            endpoint
                .display_for(target)
                .to_string()
        })
        .collect();
    let joined = read_group(&rendered.join(","), target);
    legs_interfere(
        rendered
            .iter()
            .map(String::as_str),
        joined,
        target,
    )
}

#[cfg(feature = "serde")]
mod config {
    use crate::commands::endpoint::Endpoint;
    use crate::commands::variables::Variables;

    #[derive(serde::Deserialize)]
    pub(super) struct BridgeDialString {
        #[serde(default)]
        pub(super) variables: Option<Variables>,
        pub(super) groups: Vec<Vec<Endpoint>>,
    }
}

#[cfg(feature = "serde")]
impl TryFrom<config::BridgeDialString> for BridgeDialString {
    type Error = OriginateError;

    fn try_from(config: config::BridgeDialString) -> Result<Self, Self::Error> {
        let mut bridge = Self::new(config.groups)?;
        bridge.variables = config.variables;
        Ok(bridge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::endpoint::{
        AudioEndpoint, DialString, ErrorEndpoint, LoopbackEndpoint, SofiaEndpoint, SofiaGateway,
    };
    use crate::commands::variables::{BlockParse, VariablesType};

    /// The comma scan runs after the application's argument is expanded, which here unescapes the
    /// quote that kept the `[` and `]` apart, so the switch dials both endpoints as one leg.
    #[cfg(feature = "serde")]
    #[test]
    fn a_bracket_spanning_legs_after_expansion_is_refused() {
        let group: Vec<Endpoint> = vec![
            crate::commands::endpoint::UserEndpoint::new("['")
                .unwrap()
                .into(),
            SofiaGateway::new("", "]")
                .unwrap()
                .with_profile(" ")
                .unwrap()
                .into(),
        ];
        let json = serde_json::json!({ "groups": [group] });
        assert!(matches!(
            serde_json::from_value::<BridgeDialString>(json)
                .map_err(|e| e.to_string()),
            Err(e) if e.contains("group 0")
        ));
    }

    /// The builder refuses what parse and config load refuse, rather than render a group the
    /// switch dials as one leg.
    #[test]
    fn new_refuses_what_parse_refuses() {
        let spanning: Vec<Endpoint> = vec![
            crate::commands::endpoint::UserEndpoint::new("['")
                .unwrap()
                .into(),
            SofiaGateway::new("", "]")
                .unwrap()
                .with_profile(" ")
                .unwrap()
                .into(),
        ];
        assert_eq!(
            BridgeDialString::new(vec![loopbacks(1), spanning]),
            Err(OriginateError::BracketSpansLegs { group: 1 })
        );
        assert_eq!(
            BridgeDialString::new(vec![loopbacks(MAX_PEERS + 1)]),
            Err(OriginateError::TooManyLegs {
                group: Some(0),
                max: MAX_PEERS
            })
        );
        assert_eq!(
            BridgeDialString::new(vec![loopbacks(1); MAX_PEERS + 1]),
            Err(OriginateError::TooManyLegs {
                group: None,
                max: MAX_PEERS
            })
        );
        assert!(BridgeDialString::new(vec![loopbacks(MAX_PEERS)]).is_ok());
    }

    // === Render ===

    /// What renders for a revision parses back at it.
    #[test]
    fn round_trips_at_a_named_revision() {
        let mut vars = Variables::new(VariablesType::Default);
        vars.insert("cid", "it's")
            .unwrap();
        let mut ep_vars = Variables::new(VariablesType::Channel);
        ep_vars
            .insert("path", r"C:\path")
            .unwrap();
        let bridge = BridgeDialString::new(vec![vec![SofiaGateway::new("gw", "1234")
            .unwrap()
            .with_variables(ep_vars)
            .into()]])
        .unwrap()
        .with_variables(vars);

        let rendered = bridge.render(BlockParse::PairSplitCleans);
        assert_eq!(
            BridgeDialString::parse_for(&rendered, BlockParse::PairSplitCleans)
                .unwrap_or_else(|e| panic!("{rendered} failed to parse: {e}")),
            bridge
        );
    }

    /// The block reaches a dialplan application whole, one tokenizer pass
    /// shallower than an `originate` argument, so a quoted value carries one
    /// backslash fewer here than the same value rendered on its own.
    #[test]
    fn bridge_renders_one_level_shallower_than_the_default() {
        let mut vars = Variables::new(VariablesType::Default);
        vars.insert("cid", "it's")
            .unwrap();
        let ep = SofiaGateway::new("gw", "18005551234").unwrap();
        let bridge = BridgeDialString::new(vec![vec![ep.into()]])
            .unwrap()
            .with_variables(vars.clone());

        assert_eq!(
            bridge.render(BlockParse::MEASURED),
            r"{cid=it\\\\\\'s}sofia/gateway/gw/18005551234"
        );
        assert_eq!(vars.render(DialStringCarrier::EslApi), r"{cid=it\\\\\\\'s}");
    }

    /// Both halves have to agree on the carrier. Rendering at dialplan depth
    /// and parsing back at the default would unescape one level too deep and
    /// silently return a different value than was put in.
    #[test]
    fn per_endpoint_escaped_values_round_trip() {
        let mut ep_vars = Variables::new(VariablesType::Channel);
        ep_vars
            .insert("path", r"C:\path")
            .unwrap();
        ep_vars
            .insert("other", "a,b")
            .unwrap();
        let bridge = BridgeDialString::new(vec![vec![SofiaGateway::new("gw", "1234")
            .unwrap()
            .with_variables(ep_vars)
            .into()]])
        .unwrap();

        let rendered = bridge.render(BlockParse::MEASURED);
        let back: BridgeDialString = BridgeDialString::parse_for(&rendered, BlockParse::MEASURED)
            .unwrap_or_else(|e| panic!("{rendered} failed to parse: {e}"));
        assert_eq!(back, bridge, "rendered {rendered}");
    }

    /// A quote in a `[]` block pairs across values before the switch parses the
    /// block, at any escaping depth, so the parser refuses one rather than hand
    /// back a value the wire would not deliver.
    #[test]
    fn per_endpoint_quoted_value_is_refused() {
        let err = BridgeDialString::parse_for(
            r"[cid=it\\\\\\'s]sofia/gateway/gw/1234",
            BlockParse::MEASURED,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("cid") && err.contains("channel scope"),
            "error does not name the variable and scope: {err}"
        );
    }

    #[test]
    fn display_single_endpoint() {
        let bridge =
            BridgeDialString::new(vec![vec![SofiaGateway::new("my_provider", "18005551234")
                .unwrap()
                .into()]])
            .unwrap();
        assert_eq!(
            bridge.render(BlockParse::MEASURED),
            "sofia/gateway/my_provider/18005551234"
        );
    }

    #[test]
    fn display_simultaneous_ring() {
        let bridge = BridgeDialString::new(vec![vec![
            SofiaGateway::new("primary", "18005551234")
                .unwrap()
                .into(),
            SofiaGateway::new("secondary", "18005551234")
                .unwrap()
                .into(),
        ]])
        .unwrap();
        assert_eq!(
            bridge.render(BlockParse::MEASURED),
            "sofia/gateway/primary/18005551234,sofia/gateway/secondary/18005551234"
        );
    }

    #[test]
    fn display_sequential_failover() {
        let bridge = BridgeDialString::new(vec![
            vec![SofiaGateway::new("primary", "18005551234")
                .unwrap()
                .into()],
            vec![SofiaGateway::new("backup", "18005551234")
                .unwrap()
                .into()],
        ])
        .unwrap();
        assert_eq!(
            bridge.render(BlockParse::MEASURED),
            "sofia/gateway/primary/18005551234|sofia/gateway/backup/18005551234"
        );
    }

    #[test]
    fn display_mixed_simultaneous_and_sequential() {
        let bridge = BridgeDialString::new(vec![
            vec![
                SofiaGateway::new("primary", "1234")
                    .unwrap()
                    .into(),
                SofiaGateway::new("secondary", "1234")
                    .unwrap()
                    .into(),
            ],
            vec![SofiaGateway::new("backup", "1234")
                .unwrap()
                .into()],
        ])
        .unwrap();
        assert_eq!(
            bridge.render(BlockParse::MEASURED),
            "sofia/gateway/primary/1234,sofia/gateway/secondary/1234|sofia/gateway/backup/1234"
        );
    }

    #[test]
    fn display_with_global_variables() {
        let mut vars = Variables::new(VariablesType::Default);
        vars.insert("hangup_after_bridge", "true")
            .unwrap();
        let bridge =
            BridgeDialString::new(vec![vec![SofiaEndpoint::new("internal", "1000@domain")
                .unwrap()
                .into()]])
            .unwrap()
            .with_variables(vars);
        assert_eq!(
            bridge.render(BlockParse::MEASURED),
            "{hangup_after_bridge=true}sofia/internal/1000@domain"
        );
    }

    #[test]
    fn display_with_per_endpoint_variables() {
        let mut ep_vars = Variables::new(VariablesType::Channel);
        ep_vars
            .insert("leg_timeout", "30")
            .unwrap();
        let bridge = BridgeDialString::new(vec![vec![
            SofiaGateway::new("gw1", "1234")
                .unwrap()
                .with_variables(ep_vars)
                .into(),
            SofiaGateway::new("gw2", "1234")
                .unwrap()
                .into(),
        ]])
        .unwrap();
        assert_eq!(
            bridge.render(BlockParse::MEASURED),
            "[leg_timeout=30]sofia/gateway/gw1/1234,sofia/gateway/gw2/1234"
        );
    }

    #[test]
    fn display_with_error_endpoint_failover() {
        let bridge = BridgeDialString::new(vec![
            vec![SofiaGateway::new("primary", "1234")
                .unwrap()
                .into()],
            vec![ErrorEndpoint::new(crate::channel::HangupCause::UserBusy).into()],
        ])
        .unwrap();
        assert_eq!(
            bridge.render(BlockParse::MEASURED),
            "sofia/gateway/primary/1234|error/USER_BUSY"
        );
    }

    #[test]
    fn display_with_loopback() {
        let bridge = BridgeDialString::new(vec![vec![LoopbackEndpoint::new("9199")
            .unwrap()
            .with_context("default")
            .unwrap()
            .into()]])
        .unwrap();
        assert_eq!(bridge.render(BlockParse::MEASURED), "loopback/9199/default");
    }

    // === FromStr ===

    #[test]
    fn from_str_single_endpoint() {
        let bridge: BridgeDialString = BridgeDialString::parse_for(
            "sofia/gateway/my_provider/18005551234",
            BlockParse::MEASURED,
        )
        .unwrap();
        assert_eq!(
            bridge
                .groups()
                .len(),
            1
        );
        assert_eq!(bridge.groups()[0].len(), 1);
        assert!(bridge
            .variables()
            .is_none());
    }

    #[test]
    fn from_str_simultaneous_ring() {
        let bridge: BridgeDialString = BridgeDialString::parse_for(
            "sofia/gateway/primary/1234,sofia/gateway/secondary/1234",
            BlockParse::MEASURED,
        )
        .unwrap();
        assert_eq!(
            bridge
                .groups()
                .len(),
            1
        );
        assert_eq!(bridge.groups()[0].len(), 2);
    }

    #[test]
    fn from_str_sequential_failover() {
        let bridge: BridgeDialString = BridgeDialString::parse_for(
            "sofia/gateway/primary/1234|sofia/gateway/backup/1234",
            BlockParse::MEASURED,
        )
        .unwrap();
        assert_eq!(
            bridge
                .groups()
                .len(),
            2
        );
        assert_eq!(bridge.groups()[0].len(), 1);
        assert_eq!(bridge.groups()[1].len(), 1);
    }

    #[test]
    fn from_str_mixed() {
        let bridge = BridgeDialString::parse_for(
            "sofia/gateway/primary/1234,sofia/gateway/secondary/1234|sofia/gateway/backup/1234",
            BlockParse::MEASURED,
        )
        .unwrap();
        assert_eq!(
            bridge
                .groups()
                .len(),
            2
        );
        assert_eq!(bridge.groups()[0].len(), 2);
        assert_eq!(bridge.groups()[1].len(), 1);
    }

    #[test]
    fn from_str_with_global_variables() {
        let bridge: BridgeDialString = BridgeDialString::parse_for(
            "{hangup_after_bridge=true}sofia/internal/1000@domain",
            BlockParse::MEASURED,
        )
        .unwrap();
        assert!(bridge
            .variables()
            .is_some());
        assert_eq!(
            bridge
                .variables()
                .unwrap()
                .get("hangup_after_bridge"),
            Some("true")
        );
        assert_eq!(
            bridge
                .groups()
                .len(),
            1
        );
        assert_eq!(bridge.groups()[0].len(), 1);
    }

    #[test]
    fn from_str_with_per_endpoint_variables() {
        let bridge = BridgeDialString::parse_for(
            "[leg_timeout=30]sofia/gateway/gw1/1234,sofia/gateway/gw2/1234",
            BlockParse::MEASURED,
        )
        .unwrap();
        assert_eq!(
            bridge
                .groups()
                .len(),
            1
        );
        assert_eq!(bridge.groups()[0].len(), 2);
        let ep = &bridge.groups()[0][0];
        if let Endpoint::SofiaGateway(gw) = ep {
            assert!(gw
                .variables()
                .is_some());
        } else {
            panic!("expected SofiaGateway");
        }
    }

    /// An enterprise block is global too. Read as part of the first endpoint it
    /// either fails the parse or lands on one leg of a forked dial.
    #[test]
    fn from_str_with_enterprise_global_variables() {
        let bridge: BridgeDialString = BridgeDialString::parse_for(
            "<originate_timeout=60>sofia/internal/1000@domain",
            BlockParse::MEASURED,
        )
        .unwrap();
        assert_eq!(
            bridge
                .variables()
                .expect("enterprise block is global")
                .get("originate_timeout"),
            Some("60")
        );
        assert_eq!(bridge.groups()[0].len(), 1);
    }

    /// A channel block binds to the endpoint that follows it, so the global
    /// slot must not take it.
    #[test]
    fn from_str_leaves_a_channel_block_on_its_endpoint() {
        let bridge: BridgeDialString = BridgeDialString::parse_for(
            "[leg_timeout=30]sofia/gateway/gw1/1234",
            BlockParse::MEASURED,
        )
        .unwrap();
        assert!(bridge
            .variables()
            .is_none());
        assert!(bridge.groups()[0][0]
            .variables()
            .is_some());
    }

    #[test]
    fn from_str_round_trip_single() {
        let input = "sofia/gateway/my_provider/18005551234";
        let bridge: BridgeDialString =
            BridgeDialString::parse_for(input, BlockParse::MEASURED).unwrap();
        assert_eq!(bridge.render(BlockParse::MEASURED), input);
    }

    #[test]
    fn from_str_round_trip_mixed() {
        let input =
            "sofia/gateway/primary/1234,sofia/gateway/secondary/1234|sofia/gateway/backup/1234";
        let bridge: BridgeDialString =
            BridgeDialString::parse_for(input, BlockParse::MEASURED).unwrap();
        assert_eq!(bridge.render(BlockParse::MEASURED), input);
    }

    #[test]
    fn from_str_round_trip_with_global_vars() {
        let input = "{hangup_after_bridge=true}sofia/internal/1000@domain";
        let bridge: BridgeDialString =
            BridgeDialString::parse_for(input, BlockParse::MEASURED).unwrap();
        assert_eq!(bridge.render(BlockParse::MEASURED), input);
    }

    // === Serde ===

    #[test]
    fn serde_round_trip_single() {
        let bridge =
            BridgeDialString::new(vec![vec![SofiaGateway::new("my_provider", "18005551234")
                .unwrap()
                .into()]])
            .unwrap();
        let json = serde_json::to_string(&bridge).unwrap();
        let parsed: BridgeDialString = serde_json::from_str(&json).unwrap();
        assert_eq!(bridge, parsed);
    }

    #[test]
    fn serde_round_trip_multi_group() {
        let mut vars = Variables::new(VariablesType::Default);
        vars.insert("hangup_after_bridge", "true")
            .unwrap();
        let bridge = BridgeDialString::new(vec![
            vec![
                SofiaGateway::new("primary", "1234")
                    .unwrap()
                    .into(),
                SofiaGateway::new("secondary", "1234")
                    .unwrap()
                    .into(),
            ],
            vec![ErrorEndpoint::new(crate::channel::HangupCause::UserBusy).into()],
        ])
        .unwrap()
        .with_variables(vars);
        let json = serde_json::to_string(&bridge).unwrap();
        let parsed: BridgeDialString = serde_json::from_str(&json).unwrap();
        assert_eq!(bridge, parsed);
    }

    // === Edge cases ===

    #[test]
    fn from_str_empty_string_rejected() {
        let result = BridgeDialString::parse_for("", BlockParse::MEASURED);
        assert!(result.is_err());
    }

    #[test]
    fn from_str_whitespace_only_rejected() {
        let result = BridgeDialString::parse_for("   ", BlockParse::MEASURED);
        assert!(result.is_err());
    }

    #[test]
    fn from_str_empty_groups_from_trailing_pipe() {
        // "ep1|" should parse as one group (empty trailing group is skipped)
        let bridge: BridgeDialString =
            BridgeDialString::parse_for("sofia/gateway/gw1/1234|", BlockParse::MEASURED).unwrap();
        assert_eq!(
            bridge
                .groups()
                .len(),
            1
        );
    }

    #[test]
    fn from_str_empty_variable_block() {
        let bridge: BridgeDialString =
            BridgeDialString::parse_for("{}sofia/gateway/gw1/1234", BlockParse::MEASURED).unwrap();
        assert!(bridge
            .variables()
            .is_none());
        assert_eq!(
            bridge
                .groups()
                .len(),
            1
        );
    }

    #[test]
    fn from_str_mismatched_bracket_rejected() {
        let result = BridgeDialString::parse_for(
            "{unclosed=true sofia/gateway/gw1/1234",
            BlockParse::MEASURED,
        );
        assert!(result.is_err());
    }

    /// A separator inside endpoint text is escaped for the leg split that would read it.
    #[test]
    fn a_separator_in_endpoint_text_round_trips() {
        let bridge = BridgeDialString::new(vec![
            vec![
                SofiaEndpoint::new("internal", "a,b")
                    .unwrap()
                    .into(),
                SofiaEndpoint::new("internal", "c|d")
                    .unwrap()
                    .into(),
            ],
            vec![LoopbackEndpoint::new("it's a")
                .unwrap()
                .into()],
        ])
        .unwrap();
        let rendered = bridge.render(BlockParse::MEASURED);
        let back: BridgeDialString = BridgeDialString::parse_for(&rendered, BlockParse::MEASURED)
            .unwrap_or_else(|e| panic!("{rendered} failed to parse: {e}"));
        assert_eq!(back, bridge, "rendered {rendered}");
    }

    /// The switch's comma scan protects commas from a `[` to its matching `]`, whichever leg
    /// holds either, so a range crossing a leg separator merges the legs.
    #[test]
    fn a_bracket_spanning_legs_is_refused_at_config_load() {
        let spanning = r#"{"groups":[[{"sofia":{"profile":"internal","destination":"x["}},{"sofia":{"profile":"internal","destination":"y]"}}]]}"#;
        assert!(serde_json::from_str::<BridgeDialString>(spanning).is_err());
        for fine in [
            r#"{"groups":[[{"sofia":{"profile":"internal","destination":"x[]"}},{"sofia":{"profile":"internal","destination":"y]"}}]]}"#,
            r#"{"groups":[[{"sofia":{"profile":"internal","destination":"x["}}],[{"sofia":{"profile":"internal","destination":"y]"}}]]}"#,
        ] {
            assert!(
                serde_json::from_str::<BridgeDialString>(fine).is_ok(),
                "{fine}"
            );
        }
    }

    /// A `[` in an earlier leg, its `]` in a later leg's endpoint, and a `^^` block between.
    fn bracket_around_block(sep: char, value: &str) -> Vec<Endpoint> {
        let mut vars = Variables::new(VariablesType::Channel)
            .with_separator(sep)
            .unwrap();
        vars.insert("v0", value)
            .unwrap();
        vec![
            Endpoint::PortAudio(
                AudioEndpoint::new()
                    .with_destination("[\\")
                    .unwrap(),
            ),
            SofiaGateway::new("]", "")
                .unwrap()
                .with_variables(vars)
                .into(),
        ]
    }

    /// The comma scan takes the `^^,` head inside the range for a plain comma and writes the
    /// block's own default, which the block then splits on alike.
    #[test]
    fn a_comma_head_the_scan_rewrites_round_trips() {
        let bridge = BridgeDialString::new(vec![bracket_around_block(',', "undef")]).unwrap();
        let rendered = bridge.render(BlockParse::MEASURED);
        let back: BridgeDialString = BridgeDialString::parse_for(&rendered, BlockParse::MEASURED)
            .unwrap_or_else(|e| panic!("{rendered} failed to parse: {e}"));
        assert_eq!(
            back.render(BlockParse::MEASURED),
            r"portaudio/[\\\\\\\\,[v0=undef]sofia/gateway/]/"
        );
        let json = serde_json::to_value(&bridge).unwrap();
        assert_eq!(
            serde_json::from_value::<BridgeDialString>(json).unwrap(),
            bridge
        );
    }

    /// The range opened in the first leg decides how the scan rewrites a comma in the second
    /// leg's `^^:` value, which then reaches the channel as the scan's placeholder.
    #[test]
    fn a_range_rewriting_a_later_legs_value_is_refused() {
        let group = bracket_around_block(':', "a,b");
        let rendered = group
            .iter()
            .map(|endpoint| endpoint.render(CARRIER))
            .collect::<Vec<_>>()
            .join(",");
        assert!(
            matches!(
                BridgeDialString::parse_for(&rendered, BlockParse::MEASURED),
                Err(OriginateError::BracketSpansLegs { group: 0 })
            ),
            "{rendered}: {:?}",
            BridgeDialString::parse_for(&rendered, BlockParse::MEASURED)
        );
        assert_eq!(
            BridgeDialString::new(vec![group.clone()]),
            Err(OriginateError::BracketSpansLegs { group: 0 })
        );
        let json = serde_json::json!({ "groups": [group] });
        assert!(serde_json::from_value::<BridgeDialString>(json).is_err());
    }

    fn loopbacks(count: usize) -> Vec<Endpoint> {
        (0..count)
            .map(|n| {
                LoopbackEndpoint::new(n.to_string())
                    .unwrap()
                    .into()
            })
            .collect()
    }

    fn joined(endpoints: &[Endpoint], separator: &str) -> String {
        endpoints
            .iter()
            .map(|endpoint| endpoint.render(DialStringCarrier::EslApi))
            .collect::<Vec<_>>()
            .join(separator)
    }

    /// Past `MAX_PEERS` the switch dials the rest of the text as the last leg's or group's
    /// endpoint, which is refused as the limit, never as a bracket.
    #[test]
    fn a_group_past_the_leg_limit_is_refused_as_the_limit() {
        let over = joined(&loopbacks(MAX_PEERS + 1), ",");
        let err = BridgeDialString::parse_for(&over, BlockParse::MEASURED).unwrap_err();
        assert_eq!(
            err,
            OriginateError::TooManyLegs {
                group: Some(0),
                max: MAX_PEERS
            }
        );
        assert!(
            err.to_string()
                .contains(&MAX_PEERS.to_string()),
            "{err}"
        );
        let at = joined(&loopbacks(MAX_PEERS), ",");
        let bridge: BridgeDialString =
            BridgeDialString::parse_for(&at, BlockParse::MEASURED).unwrap();
        assert_eq!(bridge.groups()[0].len(), MAX_PEERS);

        let second = format!("loopback/a|{over}");
        assert_eq!(
            BridgeDialString::parse_for(&second, BlockParse::MEASURED),
            Err(OriginateError::TooManyLegs {
                group: Some(1),
                max: MAX_PEERS
            })
        );
    }

    #[test]
    fn groups_past_the_limit_are_refused_as_the_limit() {
        let over = joined(&loopbacks(MAX_PEERS + 1), "|");
        let err = BridgeDialString::parse_for(&over, BlockParse::MEASURED).unwrap_err();
        assert_eq!(
            err,
            OriginateError::TooManyLegs {
                group: None,
                max: MAX_PEERS
            }
        );
        assert!(
            err.to_string()
                .contains(&MAX_PEERS.to_string()),
            "{err}"
        );
        let bridge: BridgeDialString =
            BridgeDialString::parse_for(&joined(&loopbacks(MAX_PEERS), "|"), BlockParse::MEASURED)
                .unwrap();
        assert_eq!(
            bridge
                .groups()
                .len(),
            MAX_PEERS
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn a_config_past_the_leg_limit_is_refused_as_the_limit() {
        let load = |groups: Vec<Vec<Endpoint>>| {
            let json = serde_json::json!({ "groups": groups });
            serde_json::from_value::<BridgeDialString>(json).map_err(|e| e.to_string())
        };
        for groups in [
            vec![loopbacks(MAX_PEERS + 1)],
            vec![loopbacks(1); MAX_PEERS + 1],
        ] {
            let err = load(groups).unwrap_err();
            assert!(err.contains(&MAX_PEERS.to_string()), "{err}");
            assert!(!err.contains("bracket"), "{err}");
        }
        assert!(load(vec![loopbacks(MAX_PEERS)]).is_ok());
        assert!(load(vec![loopbacks(1); MAX_PEERS]).is_ok());
    }

    /// The dialplan carrier's expansion, the leg splits over a `[]` block and both of a block's own
    /// splits each read `\\` as one backslash, and none reads `\b` or `\d`.
    #[test]
    fn parse_reads_blocks_as_the_switch_installs_them() {
        let bridge: BridgeDialString = BridgeDialString::parse_for(
            r"{k=a\\\\b}[j=c\\\\\\\\d]loopback/9199/test",
            BlockParse::MEASURED,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            bridge
                .variables()
                .and_then(|vars| vars.get("k")),
            Some(r"a\b")
        );
        assert_eq!(
            bridge.groups()[0][0]
                .variables()
                .and_then(|vars| vars.get("j")),
            Some(r"c\d")
        );
    }

    #[test]
    fn serde_to_display_wire_format() {
        let json = r#"{
            "groups": [[{
                "sofia_gateway": {
                    "gateway": "my_gw",
                    "destination": "18005551234"
                }
            }]]
        }"#;
        let bridge: BridgeDialString = serde_json::from_str(json).unwrap();
        assert_eq!(
            bridge.render(BlockParse::MEASURED),
            "sofia/gateway/my_gw/18005551234"
        );
    }
}
