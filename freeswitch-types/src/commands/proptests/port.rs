//! Every render read back through the port of the switch's own passes.

use super::*;

proptest! {
    #![proptest_config(config())]

    #[test]
    fn variables_arrive_as_built_or_are_refused(
        scope in scope(),
        sep in block_separator(),
        values in entries(1..4),
    ) {
        let Some(vars) = build_vars(scope, "v", &values, sep) else {
            return Ok(());
        };
        if left_to_the_switch(&vars) {
            return Ok(());
        }
        let refused = config_refuses_vars(&vars);
        let want = pairs(&vars);
        for target in targets() {
            let block = vars.display_for(target).to_string();
            let dial = format!("{block}null/drift");
            let got = read_single_leg(&dial, target);
            let delivered = got.as_ref().is_ok_and(|(installed, endpoint)| {
                *installed == want && endpoint == "null/drift"
            });
            prop_assert!(
                delivered || refused,
                "{scope:?} ^^{sep:?} at {target:?}: rendered {dial:?}, port read {got:?}, want {want:?}"
            );
            if delivered && !refused {
                let parsed = Variables::parse_for(&block, target).map(|vars| pairs(&vars));
                prop_assert_eq!(parsed, Ok(want.clone()), "parse of {:?} at {:?}", block, target);
            }
        }
    }

    #[test]
    fn endpoints_arrive_as_built_or_are_refused(
        bare in bare_endpoint(),
        vars in option::of((scope(), block_separator(), entries(1..3))),
    ) {
        if fields_name_a_variable(&bare) {
            return Ok(());
        }
        let mut endpoint = bare.clone();
        if let Some((scope, sep, values)) = &vars {
            let Some(vars) = build_vars(*scope, "v", values, *sep) else {
                return Ok(());
            };
            if left_to_the_switch(&vars) {
                return Ok(());
            }
            endpoint.set_variables(Some(vars));
        }
        let want = endpoint
            .variables()
            .map(pairs)
            .unwrap_or_default();
        let refused = serde_json::to_value(&endpoint)
            .and_then(serde_json::from_value::<Endpoint>)
            .is_err();
        for target in targets() {
            let rendered = endpoint.display_for(target).to_string();
            let got = read_single_leg(&rendered, target);
            let delivered = got.as_ref().is_ok_and(|(installed, text)| {
                *installed == want
                    && *text == bare.module_text()
                    && Endpoint::parse_bare(text).as_ref() == Ok(&canonical(&bare))
            });
            let parsed = Endpoint::parse_for(&rendered, target);
            prop_assert!(
                delivered || refused,
                "{endpoint:?} at {target:?}: rendered {rendered:?}, port read {got:?}"
            );
            let unexpanded = endpoint.check_expanded(target.carrier()).is_err();
            prop_assert!(
                !unexpanded || refused || matches!(parsed, Err(crate::commands::originate::OriginateError::UnexpandedExpression { .. })),
                "{endpoint:?} at {target:?}: rendered {rendered:?}, parse {parsed:?}"
            );
            prop_assert!(
                parsed.is_ok() || refused || unexpanded,
                "{endpoint:?} at {target:?}: rendered {rendered:?} loads from config, parse {parsed:?}"
            );
            if delivered && !refused && !unexpanded {
                let parsed = parsed.map(|mut parsed| {
                    let vars = parsed.variables().map(|vars| (vars.scope(), pairs(vars)));
                    parsed.set_variables(None);
                    (parsed, vars)
                });
                let built = endpoint.variables().map(|vars| (vars.scope(), pairs(vars)));
                prop_assert_eq!(parsed, Ok((canonical(&bare), built)), "parse of {:?} at {:?}", rendered, target);
            }
        }
    }

    #[test]
    fn originate_positionals_arrive_as_built_or_are_refused(spec in originate_spec()) {
        let Some(originate) = build_originate(&spec) else {
            return Ok(());
        };
        let rendered = originate.to_string();
        let want = intended(&spec, &originate);
        let got = switch_view(&rendered);
        let delivered = got.as_ref() == Ok(&want);
        let parsed = Originate::from_str(&rendered);
        let refused = serde_json::to_value(&originate)
            .and_then(serde_json::from_value::<Originate>)
            .is_err()
            || parsed.is_err();
        prop_assert!(
            delivered || refused,
            "{spec:?}: rendered {rendered:?}, switch reads {got:?}, want {want:?}"
        );
        if delivered {
            let rerendered = parsed.map(|parsed| parsed.to_string());
            prop_assert_eq!(
                rerendered.as_deref().map(switch_view),
                Ok(Ok(want)),
                "parse of {:?} renders {:?}", rendered, rerendered
            );
        }
    }

    #[test]
    fn an_escaped_argument_is_one_argument(text in text()) {
        for carrier in [DialStringCarrier::Dialplan, DialStringCarrier::Direct] {
            prop_assert_eq!(DialStringTarget::new(carrier).escape_argument(&text), None);
        }
        for target in api_targets() {
            let escaped = target
                .escape_argument(&text)
                .expect("an API target escapes");
            let (prefix, sep) = match target.argv_separator() {
                Some(sep) => (format!("^^{sep}"), sep),
                None => (String::new(), ' '),
            };
            let x = "x".to_owned();
            let y = "y".to_owned();
            for (line, want) in [
                (format!("{prefix}{escaped}{sep}y"), vec![text.clone(), y.clone()]),
                (format!("{prefix}x{sep}{escaped}{sep}y"), vec![x.clone(), text.clone(), y.clone()]),
                (format!("{prefix}x{sep}{escaped}"), vec![x.clone(), text.clone()]),
            ] {
                let split = separate(&trace(strip_whitespace(&line)), ' ', usize::MAX);
                let tokens: Vec<String> = split
                    .tokens
                    .iter()
                    .map(|token| untrace(&token.text))
                    .collect();
                prop_assert_eq!(tokens, want, "{:?} at {:?}", line, target);
                prop_assert!(!split.open_quote, "{line:?}");
            }
        }
    }

    #[test]
    fn a_flattened_list_reads_back_after_retain_and_rerender(spec in list_spec()) {
        for target in targets() {
            let Some(rendered) = render_list(&spec, target) else {
                return Ok(());
            };
            let mut list = FlattenedDialString::parse_for(&rendered.text, target)
                .map_err(|e| TestCaseError::fail(format!("{:?} at {target:?}: {e:?}", rendered.text)))?;
            prop_assert_eq!(list.display_raw().to_string(), rendered.text.clone());
            prop_assert_eq!(views(&list, &rendered.legs), rendered.legs.clone(), "{:?} at {:?}", rendered.text, target);
            prop_assert!(list.warnings().is_empty(), "{:?}: {:?}", rendered.text, list.warnings());

            let canonical = list.display_for(target).to_string();
            let reread = FlattenedDialString::parse_for(&canonical, target)
                .map_err(|e| TestCaseError::fail(format!("{canonical:?} at {target:?}: {e:?}")))?;
            prop_assert_eq!(views(&reread, &rendered.legs), rendered.legs.clone(), "display_for {:?} at {:?}", canonical, target);

            let raws: Vec<String> = list.legs().map(|leg| leg.raw().to_owned()).collect();
            let mut at = 0;
            list.retain(|_| {
                let keep = spec.keep[at];
                at += 1;
                keep
            });
            let kept: Vec<usize> = (0..spec.legs.len()).filter(|&i| spec.keep[i]).collect();
            if kept.is_empty() {
                prop_assert!(list.is_empty());
                continue;
            }
            let forwarded = list.display_raw().to_string();
            let back = FlattenedDialString::parse_for(&forwarded, target)
                .map_err(|e| TestCaseError::fail(format!("{forwarded:?} at {target:?}: {e:?}")))?;
            let want: Vec<LegView> = kept.iter().map(|&i| rendered.legs[i].clone()).collect();
            let want_raws: Vec<String> = kept.iter().map(|&i| raws[i].clone()).collect();
            prop_assert_eq!(back.legs().map(|leg| leg.raw().to_owned()).collect::<Vec<_>>(), want_raws, "{:?}", forwarded);
            prop_assert_eq!(views(&back, &want), want, "retained {:?} at {:?}", forwarded, target);
        }
    }

    #[test]
    fn a_quoted_token_is_one_argument_of_the_blank_split(value in text()) {
        let quoted = originate_quote(&value);
        prop_assert_eq!(originate_unquote(&quoted), value.clone());
        let line = format!("x {quoted} y");
        let split = separate(&trace(&line), ' ', usize::MAX);
        let tokens: Vec<String> = split
            .tokens
            .iter()
            .map(|token| untrace(&token.text))
            .collect();
        prop_assert_eq!(tokens, vec!["x".to_owned(), value, "y".to_owned()], "{:?}", line);
        prop_assert!(!split.open_quote, "{line:?}");
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn a_bridge_parses_back_as_built_or_is_refused(spec in bridge_spec()) {
        let Some(bridge) = build_bridge(&spec) else {
            return Ok(());
        };
        if config_refuses_bridge(&bridge) {
            return Ok(());
        }
        let rendered = bridge.to_string();
        let parsed = rendered.parse::<BridgeDialString>();
        prop_assert_eq!(
            parsed.as_ref().map(bridge_view),
            Ok(bridge_view(&bridge)),
            "rendered {:?}",
            rendered
        );
    }
}
