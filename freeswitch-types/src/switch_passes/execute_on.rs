//! Port of `switch_channel_execute_on_value`, which splits an `execute_on_*` value into an
//! application and its argument, and of what `switch_core_session_exec` leaves of that argument.

use super::expansion::expand_escapes;
use super::{trace, untrace};

#[cfg(all(test, freeswitch_oracle))]
mod c_oracle;

/// What a hook's value hands the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hook<'a> {
    pub(crate) app: &'a str,
    pub(crate) arg: Option<&'a str>,
    /// Queued on the session rather than run where the hook fires.
    pub(crate) queued: bool,
}

/// The name a value opening it is queued under, whatever its case.
const QUEUED_PREFIX: &[u8] = b"perl";

/// What queues a value whole and splits the name the queue reads.
const QUEUE_MARKER: &str = "::";

/// `switch_channel_execute_on_value`'s split: the first space or lone colon ends the name, a `::`
/// ends the scan and queues the value, and so does a name opening `perl`. A queued name carrying
/// `::` splits there instead.
pub(crate) fn split(value: &str) -> Hook<'_> {
    let value = value
        .split('\0')
        .next()
        .unwrap_or_default();
    let bytes = value.as_bytes();
    let mut queued = false;
    let mut cut = None;
    for (at, &byte) in bytes
        .iter()
        .enumerate()
    {
        let queues = byte == b':' && bytes.get(at + 1) == Some(&b':');
        if byte == b' ' || (byte == b':' && !queues) {
            cut = Some(at);
            break;
        }
        if queues {
            queued = true;
            break;
        }
    }
    let (app, arg) = match cut {
        Some(at) => (&value[..at], Some(&value[at + 1..])),
        None => (value, None),
    };
    queued |= app
        .as_bytes()
        .get(..QUEUED_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(QUEUED_PREFIX));
    match (arg, app.split_once(QUEUE_MARKER)) {
        (None, Some((app, queued_arg))) => Hook {
            app,
            arg: Some(queued_arg),
            queued: true,
        },
        _ => Hook { app, arg, queued },
    }
}

/// The value a hook carrying `app` and `arg` is written as.
pub(crate) fn render(app: &str, arg: Option<&str>) -> String {
    match arg {
        Some(arg) => format!("{app} {arg}"),
        None => app.to_owned(),
    }
}

/// Whether the expansion `switch_core_session_exec` runs ahead of the application leaves `arg`
/// other than as written: it reads `\\`, `\'`, `\$` and substitutes a reference.
pub(crate) fn expansion_rewrites(arg: &str) -> bool {
    let text = trace(arg);
    let (expanded, references) = expand_escapes(&text);
    !references.is_empty() || untrace(&expanded) != arg
}

/// The head `switch_core_session_exec` reads as a block of scope variables, which never reaches the
/// application. A lone `%` is one: the byte the switch tests lies past the terminator.
pub(crate) fn opens_scope_variables(arg: &str) -> bool {
    let bytes = arg.as_bytes();
    bytes.first() == Some(&b'%')
        && (bytes.len() == 1 || bytes.get(1) == Some(&b'[') || bytes.get(2) == Some(&b'['))
}
