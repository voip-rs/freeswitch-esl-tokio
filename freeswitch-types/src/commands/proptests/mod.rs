//! Every render read back through the port of the switch's own passes, over text weighted
//! toward what those passes treat specially. A render either arrives as built, or config
//! load or parse refuses what was built.

use std::str::FromStr;

use freeswitch_c_oracle::{config, Oracle};
use proptest::collection::vec;
use proptest::option;
use proptest::prelude::*;
use proptest::sample::select;

use super::bridge::BridgeDialString;
use super::{
    originate_quote, originate_unquote, AudioEndpoint, BlockParse, DialString, DialStringCarrier,
    DialStringTarget, Endpoint, ErrorEndpoint, FlattenedDialString, FlattenedLeg, GroupCall,
    GroupCallOrder, LegTarget, LoopbackEndpoint, Originate, SofiaContact, SofiaEndpoint,
    SofiaGateway, UserEndpoint, Variables, VariablesType,
};
use crate::channel::HangupCause;
use crate::switch_passes::brackets::PairEffect;
use crate::switch_passes::expansion::names_a_variable;
use crate::switch_passes::originate_function::c_oracle::{
    build_originate, intended, originate_spec, strip_whitespace, switch_view,
};
use crate::switch_passes::pipeline;
use crate::switch_passes::separate::separate;
use crate::switch_passes::{trace, untrace};
use crate::test_text::text;
use crate::variables::VariableName;

mod c_oracle;
pub(crate) use c_oracle::c_reads_the_leg;
mod port;

fn scope() -> impl Strategy<Value = VariablesType> {
    prop_oneof![
        Just(VariablesType::Enterprise),
        Just(VariablesType::Default),
        Just(VariablesType::Channel),
    ]
}

/// Any ASCII byte and a non-ASCII char, weighted toward another pass's delimiter; `build_vars`
/// drops the separators `with_separator` refuses.
fn block_separator() -> impl Strategy<Value = Option<char>> {
    prop_oneof![
        2 => Just(None),
        2 => prop_oneof![
            3 => select(&[',', '|', '=', '\'', '\\', ' ', '{', '}', '[', ']', '<', '>'][..]),
            3 => (0u8..0x80).prop_map(char::from),
            1 => select(&['é', '😀'][..]),
        ]
        .prop_map(Some),
    ]
}

fn block_parses() -> Vec<BlockParse> {
    match BlockParse::default() {
        BlockParse::PairSplitCleans => vec![BlockParse::PairSplitCleans],
    }
}

fn api_targets() -> Vec<DialStringTarget> {
    let api = DialStringTarget::new(DialStringCarrier::EslApi);
    let separated = ['~', '!'].map(|sep| {
        api.with_argv_separator(sep)
            .expect("a usable argv separator")
    });
    let mut targets = vec![api];
    targets.extend(separated);
    targets
}

pub(crate) fn targets() -> Vec<DialStringTarget> {
    block_parses()
        .into_iter()
        .flat_map(targets_at)
        .collect()
}

/// Every carrier and argument split, at `block_parse`.
pub(crate) fn targets_at(block_parse: BlockParse) -> Vec<DialStringTarget> {
    let mut carriers = api_targets();
    carriers.push(DialStringTarget::new(DialStringCarrier::Dialplan));
    carriers.push(DialStringTarget::new(DialStringCarrier::Direct));
    carriers
        .into_iter()
        .map(|target| target.with_block_parse(block_parse))
        .collect()
}

/// A key the switch reads as text, or `None` for one named by its position.
type Entry = (Option<String>, String);

fn entries(count: std::ops::Range<usize>) -> impl Strategy<Value = Vec<Entry>> {
    vec((option::weighted(0.3, text()), text()), count)
}

fn plain(values: &[String]) -> Vec<Entry> {
    values
        .iter()
        .map(|value| (None, value.clone()))
        .collect()
}

fn build_vars(
    scope: VariablesType,
    prefix: &str,
    entries: &[Entry],
    sep: Option<char>,
) -> Option<Variables> {
    let vars = Variables::with_vars(
        scope,
        entries
            .iter()
            .enumerate()
            .map(|(i, (key, value))| {
                (
                    key.clone()
                        .unwrap_or_else(|| format!("{prefix}{i}")),
                    value.clone(),
                )
            }),
    );
    match sep {
        Some(sep) => vars
            .with_separator(sep)
            .ok(),
        None => Some(vars),
    }
}

fn pairs(vars: &Variables) -> Vec<(String, String)> {
    vars.iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn config_refuses_vars(vars: &Variables) -> bool {
    let json = serde_json::to_value(vars).expect("Variables serializes");
    serde_json::from_value::<Variables>(json).is_err()
}

/// Left to the switch, as the `Variables` rustdoc and dial-string-format.md's *A value naming a
/// variable* say, so no escaping delivers it verbatim.
fn left_to_the_switch(vars: &Variables) -> bool {
    vars.iter()
        .any(|(key, value)| names_a_variable(key) || names_a_variable(value))
}

/// The one leg a single-endpoint dial string reads to: every pair installed, and its endpoint.
fn read_single_leg(
    dial: &str,
    target: DialStringTarget,
) -> Result<(Vec<(String, String)>, String), String> {
    let list = pipeline::read(dial, target).map_err(|e| format!("{e:?}"))?;
    let [thread] = &list.threads[..] else {
        return Err(format!(
            "{} threads",
            list.threads
                .len()
        ));
    };
    let [group] = &thread.groups[..] else {
        return Err(format!(
            "{} groups",
            thread
                .groups
                .len()
        ));
    };
    let [leg] = &group[..] else {
        return Err(format!("{} legs", group.len()));
    };
    let installed = list
        .blocks
        .iter()
        .chain(&thread.blocks)
        .chain(&leg.blocks)
        .flat_map(|block| &block.pairs)
        .map(|pair| match &pair.effect {
            PairEffect::Set(value) => Ok((
                pair.key
                    .clone(),
                value.clone(),
            )),
            effect => Err(format!("{} {effect:?}", pair.key)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((
        installed,
        leg.endpoint
            .clone(),
    ))
}

pub(crate) fn bare_endpoint() -> impl Strategy<Value = Endpoint> {
    prop_oneof![
        (text(), option::of(text()), option::of(text())).prop_map(
            |(extension, context, dialplan)| {
                let mut ep = LoopbackEndpoint::new(extension);
                ep.context = context;
                ep.dialplan = dialplan;
                ep.into()
            }
        ),
        (text(), text())
            .prop_map(|(profile, destination)| { SofiaEndpoint::new(profile, destination).into() }),
        (text(), text(), option::of(text())).prop_map(|(gateway, destination, profile)| {
            let mut ep = SofiaGateway::new(gateway, destination);
            ep.profile = profile;
            ep.into()
        }),
        (text(), option::of(text())).prop_map(|(name, domain)| {
            let mut ep = UserEndpoint::new(name);
            ep.domain = domain;
            ep.into()
        }),
        select(
            &[
                HangupCause::UserBusy,
                HangupCause::NoRouteDestination,
                HangupCause::NormalClearing,
            ][..]
        )
        .prop_map(|cause| ErrorEndpoint::new(cause).into()),
        (text(), text(), option::of(text())).prop_map(|(user, domain, profile)| {
            let mut ep = SofiaContact::new(user, domain);
            ep.profile = profile;
            ep.into()
        }),
        (
            text(),
            text(),
            option::of(select(
                &[
                    GroupCallOrder::All,
                    GroupCallOrder::Enterprise,
                    GroupCallOrder::First,
                ][..]
            ))
        )
            .prop_map(|(group, domain, order)| {
                let mut ep = GroupCall::new(group, domain);
                ep.order = order;
                ep.into()
            }),
        (
            select(&[Endpoint::PortAudio, Endpoint::PulseAudio, Endpoint::Alsa][..]),
            option::of(text())
        )
            .prop_map(|(variant, destination)| {
                let mut ep = AudioEndpoint::new();
                ep.destination = destination;
                variant(ep)
            }),
    ]
}

/// Left to the switch like a block value naming a variable; an expression's own `${` is not.
pub(crate) fn fields_name_a_variable(bare: &Endpoint) -> bool {
    match bare {
        Endpoint::SofiaContact(ep) => [
            Some(&ep.user),
            Some(&ep.domain),
            ep.profile
                .as_ref(),
        ]
        .into_iter()
        .flatten()
        .any(|field| names_a_variable(field)),
        Endpoint::GroupCall(ep) => names_a_variable(&ep.group) || names_a_variable(&ep.domain),
        other => names_a_variable(&other.module_text()),
    }
}

/// A loopback dialplan without a context renders the switch's default context in the gap.
fn canonical(endpoint: &Endpoint) -> Endpoint {
    let mut endpoint = endpoint.clone();
    if let Endpoint::Loopback(loopback) = &mut endpoint {
        if loopback
            .dialplan
            .is_some()
            && loopback
                .context
                .is_none()
        {
            loopback.context = Some("default".to_owned());
        }
    }
    endpoint
}

#[derive(Debug, Clone)]
struct LegSpec {
    values: Vec<String>,
    endpoint: &'static str,
}

#[derive(Debug, Clone)]
struct ListSpec {
    enterprise: Option<Vec<String>>,
    default: Option<Vec<String>>,
    legs: Vec<LegSpec>,
    separators: Vec<&'static str>,
    keep: Vec<bool>,
}

const LEG_ENDPOINTS: &[&str] = &["loopback/9199/test", "error/USER_BUSY", "null/a"];

fn list_spec() -> impl Strategy<Value = ListSpec> {
    let leg = (vec(text(), 0..3), select(LEG_ENDPOINTS))
        .prop_map(|(values, endpoint)| LegSpec { values, endpoint });
    (
        option::of(vec(text(), 1..3)),
        option::of(vec(text(), 1..3)),
        vec(leg, 1..5),
        vec(select(&[",", "|"][..]), 4),
        vec(any::<bool>(), 5),
    )
        .prop_map(|(enterprise, default, legs, separators, keep)| ListSpec {
            enterprise,
            default,
            legs,
            separators,
            keep,
        })
}

/// A leg as a caller reads it: what it dials, and each key named in the spec with its value.
type LegView = (String, Vec<(String, Option<String>)>);

struct RenderedList {
    text: String,
    legs: Vec<LegView>,
}

/// The list the spec describes, every block rendered inside the argument and the whole list
/// escaped as one, or `None` when a value is one config load refuses or the switch expands.
fn render_list(spec: &ListSpec, target: DialStringTarget) -> Option<RenderedList> {
    let inner = target.inner();
    let mut text = String::new();
    let mut inherited = Vec::new();
    let block = |scope, prefix: &str, values: &[String], text: &mut String| {
        let vars = build_vars(scope, prefix, &plain(values), None)?;
        if vars.is_empty() {
            return Some(Vec::new());
        }
        if config_refuses_vars(&vars) || left_to_the_switch(&vars) {
            return None;
        }
        text.push_str(
            &vars
                .display_for(inner)
                .to_string(),
        );
        Some(pairs(&vars))
    };
    for (scope, prefix, values) in [
        (VariablesType::Enterprise, "e", &spec.enterprise),
        (VariablesType::Default, "g", &spec.default),
    ] {
        if let Some(values) = values {
            inherited.extend(block(scope, prefix, values, &mut text)?);
        }
    }
    let mut legs = Vec::new();
    for (i, leg) in spec
        .legs
        .iter()
        .enumerate()
    {
        if i > 0 {
            text.push_str(spec.separators[i - 1]);
        }
        let own = block(
            VariablesType::Channel,
            &format!("l{i}_"),
            &leg.values,
            &mut text,
        )?;
        text.push_str(leg.endpoint);
        let vars = own
            .into_iter()
            .chain(
                inherited
                    .iter()
                    .cloned(),
            )
            .map(|(key, value)| (key, Some(value)))
            .collect();
        legs.push((
            leg.endpoint
                .to_owned(),
            vars,
        ));
    }
    let text = match target.escape_argument(&text) {
        Some(escaped) => escaped.into_owned(),
        None => text,
    };
    Some(RenderedList { text, legs })
}

fn endpoint_text(target: &LegTarget) -> String {
    match target {
        LegTarget::Error(error) => format!("error/{}", error.as_written()),
        LegTarget::Endpoint(endpoint) => endpoint.to_string(),
        LegTarget::Unparsed(unparsed) => unparsed
            .endpoint()
            .to_owned(),
    }
}

struct Key<'a>(&'a str);

impl VariableName for Key<'_> {
    fn as_str(&self) -> &str {
        self.0
    }
}

fn view(leg: &FlattenedLeg, keys: &LegView) -> LegView {
    (
        endpoint_text(leg.target()),
        keys.1
            .iter()
            .map(|(key, _)| {
                (
                    key.clone(),
                    leg.variable(Key(key))
                        .map(str::to_owned),
                )
            })
            .collect(),
    )
}

fn views(list: &FlattenedDialString, keys: &[LegView]) -> Vec<LegView> {
    list.legs()
        .zip(keys)
        .map(|(leg, keys)| view(leg, keys))
        .collect()
}

/// A block's separator and entries, before `build_vars` names its scope.
type BlockSpec = (Option<char>, Vec<Entry>);

#[derive(Debug, Clone)]
struct BridgeSpec {
    global: Option<BlockSpec>,
    groups: Vec<Vec<(Endpoint, Option<BlockSpec>)>>,
}

fn bridge_spec() -> impl Strategy<Value = BridgeSpec> {
    let block = || (block_separator(), entries(1..3));
    let endpoint = (bare_endpoint(), option::of(block()));
    (option::of(block()), vec(vec(endpoint, 1..3), 1..3))
        .prop_map(|(global, groups)| BridgeSpec { global, groups })
}

/// The bridge the spec describes, or `None` where the builder refuses a separator or a field is
/// left to the switch's expansion.
fn build_bridge(spec: &BridgeSpec) -> Option<BridgeDialString> {
    let block = |scope, (sep, values): &BlockSpec| {
        build_vars(scope, "v", values, *sep).filter(|vars| !left_to_the_switch(vars))
    };
    let mut groups = Vec::new();
    for group in &spec.groups {
        let mut endpoints = Vec::new();
        for (bare, vars) in group {
            if fields_name_a_variable(bare) {
                return None;
            }
            let mut endpoint = bare.clone();
            if let Some(vars) = vars {
                endpoint.set_variables(Some(block(VariablesType::Channel, vars)?));
            }
            endpoints.push(endpoint);
        }
        groups.push(endpoints);
    }
    let bridge = BridgeDialString::new(groups);
    Some(match &spec.global {
        Some(vars) => bridge.with_variables(block(VariablesType::Default, vars)?),
        None => bridge,
    })
}

fn config_refuses_bridge(bridge: &BridgeDialString) -> bool {
    serde_json::to_value(bridge)
        .and_then(serde_json::from_value::<BridgeDialString>)
        .is_err()
}

type ScopedPairs = Option<(VariablesType, Vec<(String, String)>)>;

/// What a bridge dials: its own block's pairs, and each endpoint with the pairs of its block.
fn bridge_view(bridge: &BridgeDialString) -> (ScopedPairs, Vec<Vec<(Endpoint, ScopedPairs)>>) {
    let scoped = |vars: Option<&Variables>| {
        vars.filter(|vars| !vars.is_empty())
            .map(|vars| (vars.scope(), pairs(vars)))
    };
    let groups = bridge
        .groups()
        .iter()
        .map(|group| {
            group
                .iter()
                .map(|endpoint| {
                    let mut bare = canonical(endpoint);
                    bare.set_variables(None);
                    (bare, scoped(endpoint.variables()))
                })
                .collect()
        })
        .collect();
    (scoped(bridge.variables()), groups)
}
