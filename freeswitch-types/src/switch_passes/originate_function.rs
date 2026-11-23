//! Port of `originate_function` in `mod_commands.c`, which reads the `originate` API's arguments
//! by position and runs a target opening `&` as an application.

use super::inline_hunt::{
    check_inline_delimiter, split_inline_actions, split_inline_prefix, DEFAULT_INLINE_DELIMITER,
};
use crate::commands::originate::{Application, DialplanType, OriginateError, OriginateTarget};

#[cfg(all(test, freeswitch_oracle))]
pub(crate) mod c_oracle;
#[cfg(test)]
mod tests;

/// FreeSWITCH keyword for omitted positional arguments.
///
/// `switch_separate_string` converts `"undef"` to NULL, making it the
/// canonical placeholder when a later positional arg forces earlier ones
/// to be present on the wire.
pub(crate) const UNDEF: &str = "undef";

/// Whether `originate_function` runs `target` as an application: `&` and at least one byte more.
pub(crate) fn reads_as_application(target: &str) -> bool {
    target
        .strip_prefix('&')
        .is_some_and(|rest| !rest.is_empty())
}

/// Reject a target `originate_function` reads as something else: it runs any target opening `&`
/// and more as an application, and ends that application's arguments at the first `)`.
pub(crate) fn check_target_readable(target: &OriginateTarget) -> Result<(), OriginateError> {
    match target {
        OriginateTarget::Extension(extension) if reads_as_application(extension) => {
            Err(OriginateError::ExtensionReadsAsApplication)
        }
        OriginateTarget::Application(app)
            if app
                .name()
                .contains(['(', ')'])
                || app
                    .args()
                    .is_some_and(|args| args.contains(')')) =>
        {
            Err(OriginateError::ParenthesisInApplication {
                application: app
                    .name()
                    .to_string(),
            })
        }
        _ => Ok(()),
    }
}

/// An originate's arguments after the endpoint, as the switch reads them.
pub(crate) struct Slots {
    pub(crate) target: String,
    pub(crate) dialplan: Option<DialplanType>,
    pub(crate) context: Option<String>,
    pub(crate) cid_name: Option<String>,
    pub(crate) cid_num: Option<String>,
    pub(crate) timeout: Option<String>,
}

impl Slots {
    /// The tokens after the endpoint, each already through its split's cleanup, read strictly
    /// by position as `originate_function` does.
    pub(crate) fn read(args: impl Iterator<Item = String>) -> Result<Self, OriginateError> {
        let mut args = args.map(undef_to_none);
        let target = match args.next() {
            None => {
                return Err(OriginateError::ParseError(
                    "missing target in originate".into(),
                ))
            }
            Some(None) => return Err(OriginateError::UndefPositional("target")),
            Some(Some(target)) => target,
        };
        let dialplan = args
            .next()
            .flatten()
            .map(DialplanType::from_name);
        let mut next = || {
            args.next()
                .flatten()
        };
        let (context, cid_name, cid_num, timeout) = (next(), next(), next(), next());
        if args
            .next()
            .is_some()
        {
            return Err(OriginateError::ParseError(
                "originate takes at most seven arguments".into(),
            ));
        }
        Ok(Self {
            target,
            dialplan,
            context,
            cid_name,
            cid_num,
            timeout,
        })
    }
}

/// FreeSWITCH's `undef` placeholder, in any case, read as the absent value it stands for.
fn undef_to_none(value: String) -> Option<String> {
    (!value.eq_ignore_ascii_case(UNDEF)).then_some(value)
}

/// Parse the target argument of an originate command in the order `originate_function` decides
/// it: an application whatever the dialplan, an inline action list under `inline`, else an
/// extension.
pub(crate) fn parse_originate_target(
    s: &str,
    dialplan: Option<&DialplanType>,
) -> Result<OriginateTarget, OriginateError> {
    if let Some(rest) = s
        .strip_prefix('&')
        .filter(|_| reads_as_application(s))
    {
        let rest = rest
            .strip_suffix(')')
            .ok_or_else(|| OriginateError::ParseError("missing closing paren".into()))?;
        let (name, args) = rest
            .split_once('(')
            .ok_or_else(|| OriginateError::ParseError("missing opening paren".into()))?;
        let args = if args.is_empty() { None } else { Some(args) };
        let target = OriginateTarget::Application(Application::new(name, args));
        check_target_readable(&target)?;
        Ok(target)
    } else if dialplan.is_some_and(DialplanType::is_inline) {
        let (delimiter, s) = split_inline_prefix(s);
        let delimiter = delimiter.unwrap_or(DEFAULT_INLINE_DELIMITER);
        check_inline_delimiter(delimiter)?;
        // The hunt's split takes a leading `^^` and the byte after it as its separator.
        if let Some(picked) = s
            .strip_prefix("^^")
            .and_then(|rest| {
                rest.chars()
                    .next()
            })
            .filter(|picked| !picked.is_ascii())
        {
            return Err(OriginateError::InvalidInlineDelimiter(picked));
        }
        let mut apps = Vec::new();
        for action in split_inline_actions(s, delimiter) {
            let part = action.trim_start_matches(' ');
            let (name, args) = match part.split_once(':') {
                Some((n, "")) => (n, None),
                Some((n, a)) => (n, Some(a)),
                None => (part, None),
            };
            apps.push(Application::new(name, args));
        }
        Ok(OriginateTarget::InlineApplications(apps))
    } else {
        Ok(OriginateTarget::Extension(s.to_string()))
    }
}
