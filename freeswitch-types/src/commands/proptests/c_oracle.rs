//! Renders read back through the switch's own C for the passes an API target applies.

use super::*;
use crate::switch_passes::against_the_c_at_its_revision;

/// The pairs and endpoint text the switch's C reads of `dial` at `target`: `originate_function`
/// on the API line, the dialplan carrier's expansion or nothing, then `switch_ivr_originate`'s
/// passes.
pub(crate) fn c_reads_the_leg(
    c: Oracle,
    dial: &str,
    target: DialStringTarget,
) -> Result<(Vec<(String, String)>, String), String> {
    let utf8 = |bytes: &[u8]| String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string());
    let aleg = match target.carrier() {
        DialStringCarrier::Dialplan => {
            c.expand(dial.as_bytes())
                .text
        }
        DialStringCarrier::EslApi => c
            .api_originate(originate_line(dial, target).as_bytes())
            .originated
            .and_then(|originated| originated.aleg)
            .ok_or("originate_function dials nothing")?,
        DialStringCarrier::Direct => dial
            .as_bytes()
            .to_vec(),
    };
    let read = c.dial(&aleg);
    let [thread] = &read.threads[..] else {
        return Err(format!(
            "{:?} in {} threads",
            read.failure,
            read.threads
                .len()
        ));
    };
    if let Some(failure) = &thread.failure {
        return Err(utf8(failure)?);
    }
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
    let installed = read
        .enterprise
        .iter()
        .chain(&thread.pairs)
        .chain(&leg.pairs)
        .map(|(key, value)| Ok((utf8(key)?, utf8(value)?)))
        .collect::<Result<Vec<_>, String>>()?;
    let endpoint = leg
        .endpoint
        .as_deref()
        .ok_or("the leg has no endpoint")?;
    Ok((installed, utf8(endpoint)?))
}

/// The line `originate` reads for `dial` at `target`, the dial string its first argument.
fn originate_line(dial: &str, target: DialStringTarget) -> String {
    match target.argv_separator() {
        Some(argv) => format!("^^{argv}{dial}{argv}&park()"),
        None => format!("{dial} &park()"),
    }
}

/// A block of every scope at every target, read by the switch's C rather than the port.
#[test]
fn variables_arrive_through_the_c_passes() {
    against_the_c_at_its_revision(
        file!(),
        "variables_arrive_through_the_c_passes",
        (scope(), block_separator(), entries(1..4)),
        |c, block_parse, (scope, sep, values)| {
            let Some(vars) = build_vars(scope, "v", &values, sep) else {
                return Ok(());
            };
            if left_to_the_switch(&vars) || config_refuses_vars(&vars) {
                return Ok(());
            }
            let want = Ok((pairs(&vars), "null/drift".to_owned()));
            for target in targets_at(block_parse) {
                let dial = format!("{}null/drift", vars.display_for(target));
                prop_assert_eq!(
                    &c_reads_the_leg(c, &dial, target),
                    &want,
                    "{:?} at {:?}",
                    dial,
                    target
                );
            }
            Ok(())
        },
    );
}

/// Endpoints under a block of any scope or none at every target, read by the switch's C.
#[test]
fn endpoints_arrive_through_the_c_passes() {
    against_the_c_at_its_revision(
        file!(),
        "endpoints_arrive_through_the_c_passes",
        (
            bare_endpoint(),
            option::of((scope(), block_separator(), entries(1..3))),
        ),
        |c, block_parse, (bare, vars)| {
            if fields_name_a_variable(&bare) {
                return Ok(());
            }
            let mut endpoint = bare.clone();
            if let Some((scope, sep, values)) = vars {
                let Some(vars) = build_vars(scope, "v", &values, sep) else {
                    return Ok(());
                };
                if left_to_the_switch(&vars) || config_refuses_vars(&vars) {
                    return Ok(());
                }
                endpoint.set_variables(Some(vars));
            }
            let refused = serde_json::to_value(&endpoint)
                .and_then(serde_json::from_value::<Endpoint>)
                .is_err();
            if refused {
                return Ok(());
            }
            let installed = endpoint
                .variables()
                .map(pairs)
                .unwrap_or_default();
            let want = Ok((installed, bare.module_text()));
            let expression = matches!(bare, Endpoint::SofiaContact(_) | Endpoint::GroupCall(_));
            for target in targets_at(block_parse) {
                // The expansion substitutes the expression, which the stub answers with nothing.
                if expression && target.carrier() == DialStringCarrier::Dialplan {
                    continue;
                }
                let dial = endpoint
                    .display_for(target)
                    .to_string();
                prop_assert_eq!(
                    &c_reads_the_leg(c, &dial, target),
                    &want,
                    "{:?} at {:?}",
                    dial,
                    target
                );
            }
            Ok(())
        },
    );
}

/// A bridge dial string read by the switch's C: the dialplan expansion of the application's
/// argument, then the originate passes, one leg per endpoint.
#[test]
fn bridges_dial_through_the_c_passes() {
    type LegRead = (Vec<freeswitch_c_oracle::Pair>, Option<Vec<u8>>);
    let bytes = |vars: Option<&Variables>| -> Vec<freeswitch_c_oracle::Pair> {
        vars.map(pairs)
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| (key.into_bytes(), value.into_bytes()))
            .collect()
    };
    against_the_c_at_its_revision(
        file!(),
        "bridges_dial_through_the_c_passes",
        bridge_spec(),
        |c, block_parse, spec| {
            let Some(bridge) = build_bridge(&spec) else {
                return Ok(());
            };
            // The expansion substitutes an expression, which the stub answers with nothing.
            let expression = bridge
                .groups()
                .iter()
                .flatten()
                .any(|endpoint| {
                    matches!(endpoint, Endpoint::SofiaContact(_) | Endpoint::GroupCall(_))
                });
            if expression || config_refuses_bridge(&bridge) {
                return Ok(());
            }
            let rendered = bridge
                .display_with(block_parse)
                .to_string();
            let dial = c.dial(
                &c.expand(rendered.as_bytes())
                    .text,
            );
            let want: Vec<Vec<LegRead>> = bridge
                .groups()
                .iter()
                .map(|group| {
                    group
                        .iter()
                        .map(|endpoint| {
                            (
                                bytes(endpoint.variables()),
                                Some(
                                    endpoint
                                        .module_text()
                                        .into_bytes(),
                                ),
                            )
                        })
                        .collect()
                })
                .collect();
            let [thread] = &dial.threads[..] else {
                return Err(TestCaseError::fail(format!("{rendered:?} dials {dial:?}")));
            };
            let got: Vec<Vec<LegRead>> = thread
                .groups
                .iter()
                .map(|group| {
                    group
                        .iter()
                        .map(|leg| {
                            (
                                leg.pairs
                                    .clone(),
                                leg.endpoint
                                    .clone(),
                            )
                        })
                        .collect()
                })
                .collect();
            prop_assert_eq!(
                (&thread.failure, &thread.pairs, &got),
                (&None, &bytes(bridge.variables()), &want),
                "{:?}",
                rendered
            );
            Ok(())
        },
    );
}
