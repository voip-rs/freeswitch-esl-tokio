//! `originate_function` against the switch's own C on every built tree, and the originate specs
//! the builder properties render.

use std::time::Duration;

use freeswitch_c_oracle::{against_the_c, Action, Oracle};
use proptest::collection::vec;
use proptest::option;
use proptest::prelude::*;
use proptest::sample::select;

use super::reads_as_application;
use crate::commands::BlockParse;
use crate::commands::{Application, DialplanType, Endpoint, LoopbackEndpoint, Originate};
use crate::switch_passes::against_the_c_at_its_revision;
use crate::switch_passes::api_argument::STRIPPED_WHITESPACE;
use crate::switch_passes::separate::separate;
use crate::switch_passes::{trace, untrace};
use crate::test_text::{opens_with_a_non_ascii_head, text};

#[derive(Debug, Clone)]
pub(crate) enum TargetSpec {
    Extension(String),
    Application(&'static str, Option<String>),
    Inline(Vec<(&'static str, Option<String>)>, Option<char>),
}

#[derive(Debug, Clone)]
pub(crate) enum DialplanSpec {
    Unset,
    Typed(DialplanType),
    Raw(String),
}

pub(crate) const APP_NAMES: &[&str] = &["park", "set", "playback"];

pub(crate) fn target_spec() -> impl Strategy<Value = TargetSpec> {
    prop_oneof![
        text().prop_map(TargetSpec::Extension),
        (select(APP_NAMES), option::of(text()))
            .prop_map(|(name, args)| TargetSpec::Application(name, args)),
        (
            vec((select(APP_NAMES), option::of(text())), 1..3),
            option::of(select(&[';', '|', ' '][..]))
        )
            .prop_map(|(apps, delimiter)| TargetSpec::Inline(apps, delimiter)),
    ]
}

pub(crate) fn dialplan_spec() -> impl Strategy<Value = DialplanSpec> {
    prop_oneof![
        2 => Just(DialplanSpec::Unset),
        1 => select(&[DialplanType::XML, DialplanType::INLINE][..]).prop_map(DialplanSpec::Typed),
        1 => text().prop_map(DialplanSpec::Raw),
    ]
}

#[derive(Debug, Clone)]
pub(crate) struct OriginateSpec {
    target: TargetSpec,
    dialplan: DialplanSpec,
    context: Option<String>,
    cid_name: Option<String>,
    cid_num: Option<String>,
    timeout: Option<u64>,
    argv_separator: Option<char>,
}

pub(crate) fn originate_spec() -> impl Strategy<Value = OriginateSpec> {
    (
        target_spec(),
        dialplan_spec(),
        option::of(text()),
        option::of(text()),
        option::of(text()),
        option::of(0u64..100_000),
        option::of(select(&['~', '!'][..])),
    )
        .prop_map(
            |(target, dialplan, context, cid_name, cid_num, timeout, argv_separator)| {
                OriginateSpec {
                    target,
                    dialplan,
                    context,
                    cid_name,
                    cid_num,
                    timeout,
                    argv_separator,
                }
            },
        )
}

pub(crate) const ORIGINATE_ENDPOINT: &str = "loopback/9199/test";

pub(crate) fn build_originate(spec: &OriginateSpec) -> Option<Originate> {
    let endpoint = Endpoint::Loopback(
        LoopbackEndpoint::new("9199")
            .unwrap()
            .with_context("test")
            .unwrap(),
    );
    let apps = |apps: &[(&str, Option<String>)]| {
        apps.iter()
            .map(|(name, args)| Application::new(*name, args.clone()))
            .collect::<Vec<_>>()
    };
    let originate = match &spec.target {
        TargetSpec::Extension(extension) => Originate::extension(endpoint, extension).ok()?,
        TargetSpec::Application(name, args) => {
            Originate::application(endpoint, Application::new(*name, args.clone())).ok()?
        }
        TargetSpec::Inline(list, None) => Originate::inline(endpoint, apps(list)).ok()?,
        TargetSpec::Inline(list, Some(delimiter)) => {
            Originate::inline_with_delimiter(endpoint, apps(list), *delimiter).ok()?
        }
    };
    let mut originate = match &spec.dialplan {
        DialplanSpec::Unset => originate,
        DialplanSpec::Typed(dialplan) => originate
            .with_dialplan(dialplan.clone())
            .ok()?,
        DialplanSpec::Raw(name) => originate
            .with_dialplan(DialplanType::named(name.as_str()).ok()?)
            .ok()?,
    };
    if let Some(context) = &spec.context {
        originate = originate
            .with_context(context)
            .ok()?;
    }
    if let Some(cid_name) = &spec.cid_name {
        originate = originate
            .with_cid_name(cid_name)
            .ok()?;
    }
    if let Some(cid_num) = &spec.cid_num {
        originate = originate
            .with_cid_num(cid_num)
            .ok()?;
    }
    if let Some(timeout) = spec.timeout {
        originate = originate
            .with_timeout(Duration::from_secs(timeout))
            .ok()?;
    }
    match spec.argv_separator {
        Some(sep) => originate
            .with_argv_separator(sep)
            .ok(),
        None => Some(originate),
    }
}

/// What `originate_function` runs: an application from `&name(arg)`, or a transfer to the
/// extension, which the inline dialplan reads as its action list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SwitchTarget {
    Application { name: String, arg: String },
    Extension(String),
    Inline(Vec<(String, String)>),
}

/// `originate_function`'s `&` branch: the name up to the first `(`, the argument up to the
/// first `)`.
pub(crate) fn switch_application(exten: &str) -> Option<SwitchTarget> {
    let app = exten
        .strip_prefix('&')
        .filter(|_| reads_as_application(exten))?;
    let app = app
        .split_once(')')
        .map_or(app, |(before, _)| before);
    let (name, arg) = app
        .split_once('(')
        .unwrap_or((app, ""));
    Some(SwitchTarget::Application {
        name: name.to_owned(),
        arg: arg.to_owned(),
    })
}

/// `inline_dialplan_hunt`: an `m:<delim>:` prefix, `switch_separate_string` on the delimiter,
/// each action split at its first colon with leading spaces dropped from the name.
pub(crate) fn switch_inline(exten: &str) -> SwitchTarget {
    let bytes = exten.as_bytes();
    let (delim, list) = match bytes {
        [b'm', b':', delim, b':', ..] => (*delim as char, &exten[4..]),
        _ => (',', exten),
    };
    let actions = separate(&trace(list), delim, 128)
        .tokens
        .into_iter()
        .map(|token| {
            let action = untrace(&token.text);
            let (name, data) = action
                .split_once(':')
                .unwrap_or((&action, ""));
            (
                name.trim_start_matches(' ')
                    .to_owned(),
                data.to_owned(),
            )
        })
        .collect();
    SwitchTarget::Inline(actions)
}

/// `switch_strip_whitespace`, which `switch_api_execute` runs over an API command's argument line.
pub(crate) fn strip_whitespace(line: &str) -> &str {
    line.trim_matches(STRIPPED_WHITESPACE)
}

/// The arguments `originate_function` reads from a rendered command, `undef` in any case absent.
pub(crate) fn originate_argv(rendered: &str) -> Result<Vec<Option<String>>, String> {
    let arguments = rendered
        .strip_prefix("originate ")
        .ok_or("no originate prefix")?;
    let argv: Vec<Option<String>> = separate(&trace(strip_whitespace(arguments)), ' ', 10)
        .tokens
        .iter()
        .map(|token| untrace(&token.text))
        .map(|argument| (!argument.eq_ignore_ascii_case("undef")).then_some(argument))
        .collect();
    if !(2..=7).contains(&argv.len()) {
        return Err(format!("usage: {} arguments", argv.len()));
    }
    Ok(argv)
}

/// The positionals `originate_function` reads and the target it acts on, the target's slot
/// left out where the inline hunt reads it instead.
pub(crate) fn switch_view(rendered: &str) -> Result<(Vec<Option<String>>, SwitchTarget), String> {
    let mut argv = originate_argv(rendered)?;
    let target = switch_reads(
        &argv,
        argv.get(2)
            .cloned()
            .flatten()
            .as_deref(),
    )
    .ok_or("no target")?;
    if matches!(target, SwitchTarget::Inline(_)) {
        argv[1] = None;
    }
    Ok((argv, target))
}

/// The positionals and the target the switch acts on, from the struct.
pub(crate) fn intended(
    spec: &OriginateSpec,
    originate: &Originate,
) -> (Vec<Option<String>>, SwitchTarget) {
    let inline = matches!(spec.target, TargetSpec::Inline(..));
    let dialplan = originate
        .dialplan()
        .map(DialplanType::to_string)
        .or_else(|| inline.then(|| "inline".to_owned()));
    let mut slots = vec![
        Some(ORIGINATE_ENDPOINT.to_owned()),
        None,
        dialplan,
        spec.context
            .clone(),
        spec.cid_name
            .clone(),
        spec.cid_num
            .clone(),
        spec.timeout
            .map(|secs| secs.to_string()),
    ];
    let target = match &spec.target {
        TargetSpec::Extension(extension) => {
            slots[1] = Some(extension.clone());
            SwitchTarget::Extension(extension.clone())
        }
        TargetSpec::Application(name, args) => {
            let arg = args
                .clone()
                .unwrap_or_default();
            slots[1] = Some(format!("&{name}({arg})"));
            SwitchTarget::Application {
                name: (*name).to_owned(),
                arg,
            }
        }
        TargetSpec::Inline(apps, _) => SwitchTarget::Inline(
            apps.iter()
                .map(|(name, args)| {
                    (
                        (*name).to_owned(),
                        args.clone()
                            .unwrap_or_default(),
                    )
                })
                .collect(),
        ),
    };
    let present = slots
        .iter()
        .rposition(Option::is_some)
        .map_or(0, |last| last + 1)
        .max(2);
    slots.truncate(present);
    (slots, target)
}

/// The target and positionals `originate_function` reads, the target left as written for an
/// inline action list, which only the hunt reads.
pub(crate) fn switch_reads(
    argv: &[Option<String>],
    dialplan: Option<&str>,
) -> Option<SwitchTarget> {
    let exten = argv
        .get(1)?
        .as_deref()?;
    if let Some(application) = switch_application(exten) {
        return Some(application);
    }
    Some(match dialplan {
        Some("inline") => switch_inline(exten),
        _ => SwitchTarget::Extension(exten.to_owned()),
    })
}

/// What `originate_function` does with an argument line, the same shape whichever side read it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ApiReading {
    Usage,
    /// `switch_assert(exten)` fails on an `undef` target, which aborts the switch.
    TargetUnset,
    Originated {
        aleg: Option<Vec<u8>>,
        cid_name: Option<Vec<u8>>,
        cid_num: Option<Vec<u8>>,
        timeout: u32,
        action: ApiAction,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ApiAction {
    Application {
        name: Vec<u8>,
        arg: Vec<u8>,
    },
    Transfer {
        extension: Vec<u8>,
        dialplan: Vec<u8>,
        context: Vec<u8>,
    },
}

/// glibc's `atoi`: `strtol` clamped to `long`, truncated to `int`, stored in a `uint32_t`.
fn atoi_timeout(text: &str) -> u32 {
    let text = text.trim_start_matches([' ', '\t', '\n', '\u{b}', '\u{c}', '\r']);
    let (negative, digits) = match text
        .as_bytes()
        .first()
    {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let cap = i128::from(i64::MAX) + 1;
    let magnitude = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i128, |n, digit| {
            (n * 10 + i128::from(digit - b'0')).min(cap)
        });
    let value = if negative { -magnitude } else { magnitude };
    let long = value.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
    long as i32 as u32
}

/// The reading `originate_argv` and `switch_application` model, which the positional properties
/// above take for the switch's.
fn port_api_reading(line: &str) -> ApiReading {
    let Ok(argv) = originate_argv(line) else {
        return ApiReading::Usage;
    };
    let slot = |at: usize| {
        argv.get(at)
            .cloned()
            .flatten()
    };
    let Some(exten) = slot(1) else {
        return ApiReading::TargetUnset;
    };
    let action = match switch_application(&exten) {
        Some(SwitchTarget::Application { name, arg }) => ApiAction::Application {
            name: name.into_bytes(),
            arg: arg.into_bytes(),
        },
        _ => ApiAction::Transfer {
            extension: exten.into_bytes(),
            dialplan: slot(2)
                .unwrap_or_else(|| "XML".to_owned())
                .into_bytes(),
            context: slot(3)
                .unwrap_or_else(|| "default".to_owned())
                .into_bytes(),
        },
    };
    ApiReading::Originated {
        aleg: slot(0).map(String::into_bytes),
        cid_name: slot(4).map(String::into_bytes),
        cid_num: slot(5).map(String::into_bytes),
        timeout: slot(6).map_or(60, |timeout| atoi_timeout(&timeout)),
        action,
    }
}

fn switch_api_reading(c: Oracle, line: &str) -> ApiReading {
    let api = c.api_originate(
        line.strip_prefix("originate ")
            .unwrap_or(line)
            .as_bytes(),
    );
    if api
        .assertion
        .is_some()
    {
        return ApiReading::TargetUnset;
    }
    let Some(originated) = api.originated else {
        return ApiReading::Usage;
    };
    let action = match originated
        .action
        .expect("originate_function acts on every channel it originates")
    {
        Action::Application { name, arg } => ApiAction::Application {
            name,
            arg: arg.unwrap_or_default(),
        },
        Action::Transfer {
            extension,
            dialplan,
            context,
        } => ApiAction::Transfer {
            extension,
            dialplan,
            context,
        },
    };
    ApiReading::Originated {
        aleg: originated.aleg,
        cid_name: originated.cid_name,
        cid_num: originated.cid_num,
        timeout: originated.timeout,
        action,
    }
}

/// A rendered originate, or `None` where the builder refuses the spec, and lines of stray text.
fn api_lines() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        3 => originate_spec().prop_map(|spec| build_originate(&spec).map(|originate| originate.render(BlockParse::MEASURED))),
        1 => vec(text(), 0..9).prop_map(|tokens| Some(format!("originate {}", tokens.join(" ")))),
    ]
}

#[test]
fn originate_function_reads_lines_as_the_port_does() {
    against_the_c(
        file!(),
        "originate_function_reads_lines_as_the_port_does",
        api_lines(),
        |c, line| {
            let Some(line) = line.filter(|line| {
                !opens_with_a_non_ascii_head(strip_whitespace(
                    line.strip_prefix("originate ")
                        .unwrap_or(line),
                ))
            }) else {
                return Ok(());
            };
            prop_assert_eq!(
                switch_api_reading(c, &line),
                port_api_reading(&line),
                "{:?}",
                line
            );
            Ok(())
        },
    );
}

/// A line `Originate` parses renders back to one `originate_function` reads the same: the dial
/// string compared by what the switch's leg passes read of it, a dialplan name without regard to
/// case as the module lookup reads it, and an inline action list by the port's model of the hunt.
#[test]
fn a_parsed_originate_reads_back_the_same_through_the_switch() {
    against_the_c_at_its_revision(
        file!(),
        "a_parsed_originate_reads_back_the_same_through_the_switch",
        api_lines(),
        |c, block_parse, line| {
            let Some(line) = line else {
                return Ok(());
            };
            let Ok(parsed) = Originate::parse_for(&line, block_parse) else {
                return Ok(());
            };
            let rendered = parsed.render(block_parse);
            let read = |text: &str| {
                let mut reading = switch_api_reading(c, text);
                let (mut dial, mut hunt) = (None, None);
                if let ApiReading::Originated { aleg, action, .. } = &mut reading {
                    dial = aleg
                        .take()
                        .map(|aleg| c.dial(&aleg));
                    if let ApiAction::Transfer {
                        extension,
                        dialplan,
                        ..
                    } = action
                    {
                        dialplan.make_ascii_lowercase();
                        if *dialplan == *b"inline" {
                            let list = std::mem::take(extension);
                            hunt = Some(switch_inline(&String::from_utf8_lossy(&list)));
                        }
                    }
                }
                (dial, hunt, reading)
            };
            prop_assert_eq!(
                read(&rendered),
                read(&line),
                "{:?} renders {:?}",
                line,
                rendered
            );
            Ok(())
        },
    );
}
