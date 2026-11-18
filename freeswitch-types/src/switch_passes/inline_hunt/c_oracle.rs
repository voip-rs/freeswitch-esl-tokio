//! `inline_dialplan_hunt`'s split against the switch's own C on every built tree.

use freeswitch_c_oracle::{against_the_c, Action, InlineApplication, Oracle};
use proptest::collection::vec;
use proptest::option;
use proptest::prelude::*;
use proptest::sample::select;

use crate::commands::originate::{DialplanType, OriginateError, OriginateTarget};
use crate::commands::{Application, Endpoint, LoopbackEndpoint, Originate};
use crate::switch_passes::originate_function::c_oracle::APP_NAMES;
use crate::switch_passes::originate_function::parse_originate_target;
use crate::test_text::text;

/// An inline target parses to the applications the switch's hunt adds, an empty data read as none,
/// or is refused for a separator the hunt's split breaks on.
#[test]
fn inline_targets_parse_as_the_c_hunt_reads_them() {
    let piece = prop_oneof![
        3 => text(),
        1 => select(&["m:|:", "m:;:", "m:,:", "m:::", "m: :", "m:\\:", "m:'", ":", ",", "set:", "park"][..])
            .prop_map(str::to_owned),
    ];
    against_the_c(
        file!(),
        "inline_targets_parse_as_the_c_hunt_reads_them",
        vec(piece, 0..5).prop_map(|pieces| pieces.concat()),
        |c, target| {
            if target.starts_with('&') {
                return Ok(());
            }
            match parse_originate_target(&target, Some(&DialplanType::INLINE)) {
                Ok(OriginateTarget::InlineApplications(apps)) => {
                    let parsed: Vec<InlineApplication> = apps
                        .iter()
                        .map(|app| {
                            (
                                app.name()
                                    .as_bytes()
                                    .to_vec(),
                                app.args()
                                    .map(|args| {
                                        args.as_bytes()
                                            .to_vec()
                                    }),
                            )
                        })
                        .collect();
                    let switch: Vec<InlineApplication> = c_hunt(c, target.as_bytes())
                        .into_iter()
                        .map(|(name, data)| (name, data.filter(|data| !data.is_empty())))
                        .collect();
                    prop_assert_eq!(parsed, switch, "{:?}", target);
                }
                Err(OriginateError::InvalidInlineDelimiter(_)) => {}
                other => {
                    return Err(TestCaseError::fail(format!(
                        "{target:?} parses as {other:?}"
                    )));
                }
            }
            Ok(())
        },
    );
}

/// An action list's applications as the switch's hunt adds them, each data `None` without a `:`,
/// empty where it returns no extension.
fn c_hunt(c: Oracle, list: &[u8]) -> Vec<InlineApplication> {
    c.inline_dialplan_hunt(list)
        .unwrap_or_default()
}

/// Every separator `inline_with_delimiter` accepts delivers each action as built through
/// `originate_function` and the inline hunt, on blanks and after `^^~`.
#[test]
fn inline_actions_arrive_through_the_c_hunt() {
    let argument = prop_oneof![
        3 => text(),
        1 => select(&["a\nb", "x\ry", "n\tt"][..]).prop_map(str::to_owned),
    ];
    against_the_c(
        file!(),
        "inline_actions_arrive_through_the_c_hunt",
        (
            (0u8..0x80).prop_map(char::from),
            vec((select(APP_NAMES), option::of(argument)), 1..3),
            any::<bool>(),
        ),
        |c, (delimiter, apps, separated)| {
            let endpoint = Endpoint::Loopback(LoopbackEndpoint::new("9199").with_context("test"));
            let built = apps
                .iter()
                .map(|(name, args)| Application::new(*name, args.clone()));
            let Ok(mut originate) = Originate::inline_with_delimiter(endpoint, built, delimiter)
            else {
                return Ok(());
            };
            if separated {
                originate = originate
                    .with_argv_separator('~')
                    .expect("a usable argv separator");
            }
            let line = originate.to_string();
            let api = c.api_originate(
                line.strip_prefix("originate ")
                    .unwrap_or(&line)
                    .as_bytes(),
            );
            let action = api
                .originated
                .and_then(|originated| originated.action);
            let Some(Action::Transfer {
                extension,
                dialplan,
                ..
            }) = action
            else {
                return Err(TestCaseError::fail(format!(
                    "{line:?} transfers nothing: {action:?}"
                )));
            };
            prop_assert_eq!(&dialplan[..], b"inline", "{:?}", line);
            let want: Vec<InlineApplication> = apps
                .iter()
                .map(|(name, args)| {
                    (
                        name.as_bytes()
                            .to_vec(),
                        args.clone()
                            .map(String::into_bytes),
                    )
                })
                .collect();
            prop_assert_eq!(c_hunt(c, &extension), want, "{:?}", line);
            Ok(())
        },
    );
}
