//! FreeSWITCH's C that `freeswitch-types` ports, compiled from every tree `hooks/source-refs.yaml`
//! names, for differential tests of the port against each, and sofia-sip's SDP parser from the
//! release the index pins, which the switch links whatever its tree.
//!
//! Every call works on bytes, and the C reads its input up to the first NUL. Passes that install
//! channel variables or call into the core run against stubs that report each call and store
//! nothing: a variable lookup or API call answers nothing.

use std::ffi::{c_char, c_int, c_long, c_uint, c_void, CStr};

mod every_tree;
#[cfg(test)]
#[path = "../build/extract.rs"]
mod extract;
mod sdp;

pub use every_tree::{against_the_c, config, on_every_tree, oracles, trees_agree};
pub use sdp::{
    against_sofia, sofia_sdp, SdpAttribute, SdpMedia, SdpParse, SdpRtpmap, SdpSession, SofiaSdp,
};

/// One FreeSWITCH tree the build compiles: the pin, or an entry of `trees` in the index.
#[derive(Debug)]
pub struct Tree {
    name: &'static str,
    commit: &'static str,
    block_parse: &'static str,
    public: bool,
    abi: Result<&'static Abi, &'static str>,
}

impl Tree {
    /// `pin` for the commit the source references index, else the name the index gives.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The commit compiled.
    pub fn commit(&self) -> &'static str {
        self.commit
    }

    /// The block-parse revision the index names for the commit, in `BlockParse`'s string form.
    pub fn block_parse(&self) -> &'static str {
        self.block_parse
    }

    /// The index names a public remote for the commit, which CI fetches.
    pub fn is_public(&self) -> bool {
        self.public
    }

    /// The tree's C, or why the build found no source for it.
    pub fn oracle(&self) -> Result<Oracle, &'static str> {
        self.abi
            .map(|abi| Oracle { abi })
    }
}

/// Every tree the index names, the pin first, built or not.
pub fn trees() -> &'static [Tree] {
    TREES
}

/// The tree the index names `name`, built or not.
pub fn tree(name: &str) -> Option<&'static Tree> {
    TREES
        .iter()
        .find(|tree| tree.name == name)
}

/// The C of one built tree.
#[derive(Debug, Clone, Copy)]
pub struct Oracle {
    abi: &'static Abi,
}

type Emit = unsafe extern "C" fn(*mut c_void, c_int, *const c_char, *const c_char);
type Recorded = unsafe extern "C" fn(*const c_char, Emit, *mut c_void);

include!(concat!(env!("OUT_DIR"), "/trees.rs"));

/// A header an extracted pass installed: its name, then its value.
pub type Pair = (Vec<u8>, Vec<u8>);

/// What `switch_event_create_brackets` read of the block opening its data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Brackets {
    /// The headers the event holds, in the order they were last set.
    pub pairs: Vec<Pair>,
    /// The offset of the data after the block.
    pub rest: usize,
    /// The string at `rest` as the parse left it.
    pub following: Vec<u8>,
}

/// What `switch_ivr_originate` and `switch_ivr_enterprise_originate` read of a dial string, up to
/// each leg's endpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dial {
    /// `<>` headers ahead of an enterprise split.
    pub enterprise: Vec<Pair>,
    /// One per `:_:` thread, or the whole dial string when there is none.
    pub threads: Vec<Thread>,
    /// Why the enterprise originate stopped before its threads.
    pub failure: Option<Vec<u8>>,
}

/// One thread of a [`Dial`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Thread {
    /// `switch_stristr` found `origination_nested_vars=true` in the thread's text.
    pub nested_vars: bool,
    /// `<>` and `{}` headers.
    pub pairs: Vec<Pair>,
    /// The `|` groups, each its `,` legs.
    pub groups: Vec<Vec<Leg>>,
    /// Why the thread's originate stopped.
    pub failure: Option<Vec<u8>>,
}

/// One leg of a [`Thread`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Leg {
    /// `[]` headers.
    pub pairs: Vec<Pair>,
    /// The text after the leg's blocks, `None` where the originate stopped first.
    pub endpoint: Option<Vec<u8>>,
}

/// What `switch_channel_expand_variables_check` made of its input.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expansion {
    /// The output, every reference substituted with nothing.
    pub text: Vec<u8>,
    /// Every variable name looked up, in order.
    pub lookups: Vec<Vec<u8>>,
    /// Every API function called, with its argument.
    pub api_calls: Vec<(Vec<u8>, Option<Vec<u8>>)>,
}

/// What `originate_function` did with an API argument line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApiOriginate {
    /// Every line written to the API stream.
    pub output: Vec<Vec<u8>>,
    /// The call to `switch_ivr_originate`, absent when the arguments were refused.
    pub originated: Option<Originated>,
    /// The `switch_assert` expression that failed, which aborts the switch.
    pub assertion: Option<Vec<u8>>,
}

/// The arguments `originate_function` handed on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Originated {
    /// The dial string, `None` for `undef`.
    pub aleg: Option<Vec<u8>>,
    /// Seconds.
    pub timeout: u32,
    /// The caller id name, `None` when absent or `undef`.
    pub cid_name: Option<Vec<u8>>,
    /// The caller id number, `None` when absent or `undef`.
    pub cid_num: Option<Vec<u8>>,
    /// What the new channel runs.
    pub action: Option<Action>,
}

/// What the originated channel runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `&name(arg)`.
    Application {
        /// Up to the first `(`.
        name: Vec<u8>,
        /// Between the first `(` and the first `)`, `None` without a `(`.
        arg: Option<Vec<u8>>,
    },
    /// `switch_ivr_session_transfer`.
    Transfer {
        /// The extension argument.
        extension: Vec<u8>,
        /// The dialplan, `XML` when absent.
        dialplan: Vec<u8>,
        /// The context, `default` when absent.
        context: Vec<u8>,
    },
}

/// An application `inline_dialplan_hunt` added: its name, then its data, `None` without a `:`.
pub type InlineApplication = (Vec<u8>, Option<Vec<u8>>);

/// What `switch_channel_execute_on_value` handed on of an `execute_on_*` value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecuteOnValue {
    /// The application name.
    pub app: Vec<u8>,
    /// Its argument, `None` where the value carries none.
    pub arg: Option<Vec<u8>>,
    /// Queued on the session rather than run where the hook fires.
    pub queued: bool,
    /// Every variable name the argument's discarded expansion looked up, in order.
    pub lookups: Vec<Vec<u8>>,
    /// Every API function that expansion called, with its argument.
    pub api_calls: Vec<(Vec<u8>, Option<Vec<u8>>)>,
}

/// What `switch_core_session_exec` hands an application of the argument it was given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecArgument {
    /// The argument the application receives, `None` where it receives none.
    pub argument: Option<Vec<u8>>,
    /// The scope variables a `%[` block set, in the order they were last set.
    pub scope: Vec<Pair>,
    /// Every variable name looked up, `app_disable_expand_variables` first.
    pub lookups: Vec<Vec<u8>>,
    /// Every API function the expansion called, with its argument.
    pub api_calls: Vec<(Vec<u8>, Option<Vec<u8>>)>,
}

/// What `protect_dest_uri` did to a destination number.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProtectedDestination {
    /// The destination number the call left, whether it returned nonzero or not.
    pub destination: Vec<u8>,
    /// The call returned nonzero, having URL-encoded the user part.
    pub encoded: bool,
}

/// What the mod_sofia stubs answer: every lookup is reported, and only a listed name is found.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sofia<'a> {
    /// Profiles `sofia_glue_find_profile` finds, each with SIP IP `192.0.2.1` and no domain name.
    pub profiles: &'a [&'a [u8]],
    /// Gateways `sofia_reg_find_gateway` finds, each up over UDP with an empty destination prefix,
    /// proxy `sip:gateway.example.com`, contact `<sip:gw@192.0.2.1:5060>` and from
    /// `<sip:gw@gateway.example.com>`.
    pub gateways: &'a [&'a [u8]],
    /// Headers of the originate's variable event, name then value, matched in any case.
    pub headers: &'a [(&'a [u8], &'a [u8])],
}

/// What `sofia_outgoing_channel` made of a destination. A registration and a host resolution
/// answer nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SofiaOutgoing {
    /// The cause it failed with, `None` where it reached the attach.
    pub cause: Option<i32>,
    /// The destination number `protect_dest_uri` left.
    pub destination_number: Vec<u8>,
    /// Every variable event header read, in order.
    pub header_lookups: Vec<Vec<u8>>,
    /// Every channel variable read, in order.
    pub variable_lookups: Vec<Vec<u8>>,
    /// Every gateway name looked up.
    pub gateway_lookups: Vec<Vec<u8>>,
    /// Every profile name looked up.
    pub profile_lookups: Vec<Vec<u8>>,
    /// Every registration looked up, user then host.
    pub registration_lookups: Vec<(Vec<u8>, Vec<u8>)>,
    /// Every host name resolved.
    pub host_lookups: Vec<Vec<u8>>,
    /// Channel variables set, in order.
    pub variables: Vec<Pair>,
    /// The `sofia_transport_t` of the private object.
    pub transport: i32,
    /// `gateway_name` of the private object.
    pub gateway_name: Option<Vec<u8>>,
    /// `gateway_from_str` of the private object.
    pub gateway_from_str: Option<Vec<u8>>,
    /// `dest`, the request URI.
    pub dest: Option<Vec<u8>>,
    /// `e_dest`.
    pub e_dest: Option<Vec<u8>>,
    /// `dest_to`, the To URI.
    pub dest_to: Option<Vec<u8>>,
    /// `invite_contact`.
    pub invite_contact: Option<Vec<u8>>,
    /// `local_url`.
    pub local_url: Option<Vec<u8>>,
    /// `mparams.remote_ip`.
    pub remote_ip: Option<Vec<u8>>,
}

/// What `sofia_contact_function` did with its argument. The profile hash is empty and a
/// registration query finds no contact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SofiaContact {
    /// Every line written to the API stream.
    pub output: Vec<Vec<u8>>,
    /// Every profile name looked up.
    pub profile_lookups: Vec<Vec<u8>>,
    /// How often the core's default domain was asked for.
    pub default_domain_lookups: usize,
    /// Every registration query, in order.
    pub selects: Vec<ContactSelect>,
    /// The `switch_assert` expression that failed, which aborts the switch.
    pub assertion: Option<Vec<u8>>,
}

/// The arguments of one `select_from_profile` call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContactSelect {
    /// The profile's name.
    pub profile: Vec<u8>,
    /// Contacts seen on an earlier profile are skipped.
    pub dedup: bool,
    /// The user matched.
    pub user: Option<Vec<u8>>,
    /// The host matched.
    pub domain: Option<Vec<u8>>,
    /// Appended to each contact.
    pub concat: Option<Vec<u8>>,
    /// A contact containing it is skipped.
    pub exclude_contact: Option<Vec<u8>>,
    /// A user agent that does not contain it is skipped.
    pub match_user_agent: Option<Vec<u8>>,
}

/// The caller profile `channel_outgoing_channel` gives the new loopback channel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoopbackOutgoing {
    /// The channel name.
    pub name: Option<Vec<u8>>,
    /// The extension.
    pub destination_number: Vec<u8>,
    /// The context.
    pub context: Option<Vec<u8>>,
    /// The dialplan.
    pub dialplan: Option<Vec<u8>>,
    /// The channel runs `loopback_app` rather than a dialplan.
    pub app: bool,
    /// Channel variables set, in order.
    pub variables: Vec<Pair>,
}

/// The directory entry `user_outgoing_channel` looks up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserOutgoing {
    /// The user id.
    pub user: Vec<u8>,
    /// The domain.
    pub domain: Vec<u8>,
    /// The domain is the core's default, [`DEFAULT_DOMAIN`].
    pub default_domain: bool,
}

/// The group `group_call_function` looks up and how it joins its members.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupCall {
    /// The group name.
    pub group: Vec<u8>,
    /// The domain.
    pub domain: Option<Vec<u8>>,
    /// What separates members in the dial string.
    pub call_delim: Vec<u8>,
    /// The domain is the core's default, [`DEFAULT_DOMAIN`].
    pub default_domain: bool,
}

/// The fields a harness reported, taken by name.
#[derive(Default)]
struct Fields(Vec<(Vec<u8>, Option<Vec<u8>>)>);

impl Fields {
    fn push(&mut self, name: Option<Vec<u8>>, value: Option<Vec<u8>>) {
        self.0
            .push((text(name), value));
    }

    fn is_empty(&self) -> bool {
        self.0
            .is_empty()
    }

    fn take(&mut self, name: &str) -> Option<Vec<u8>> {
        let at = self
            .0
            .iter()
            .rposition(|(field, _)| field == name.as_bytes())
            .unwrap_or_else(|| panic!("the harness reported no field {name}"));
        self.0
            .remove(at)
            .1
    }
}

/// NUL-terminated copies of some strings behind a NULL-terminated pointer array.
struct CArray {
    _buffers: Vec<Vec<u8>>,
    pointers: Vec<*const c_char>,
}

impl CArray {
    fn new<'a>(strings: impl IntoIterator<Item = &'a [u8]>) -> Self {
        let buffers: Vec<Vec<u8>> = strings
            .into_iter()
            .map(buffer)
            .collect();
        let pointers = buffers
            .iter()
            .map(|buffer| {
                buffer
                    .as_ptr()
                    .cast()
            })
            .chain(std::iter::once(std::ptr::null()))
            .collect();
        Self {
            _buffers: buffers,
            pointers,
        }
    }

    fn as_ptr(&self) -> *const *const c_char {
        self.pointers
            .as_ptr()
    }
}

fn number(field: Option<Vec<u8>>) -> i32 {
    String::from_utf8(text(field))
        .ok()
        .and_then(|number| {
            number
                .parse()
                .ok()
        })
        .expect("the harness prints the number in decimal")
}

type Record = (c_int, Option<Vec<u8>>, Option<Vec<u8>>);

/// # Safety
///
/// `ctx` is the `Vec<Record>` [`recorded`] passed, and each non-null string is NUL-terminated.
unsafe extern "C" fn record(ctx: *mut c_void, tag: c_int, a: *const c_char, b: *const c_char) {
    let owned = |s: *const c_char| {
        // SAFETY: the caller guarantees a non-null `s` is NUL-terminated.
        (!s.is_null()).then(|| {
            unsafe { CStr::from_ptr(s) }
                .to_bytes()
                .to_vec()
        })
    };
    // SAFETY: the caller guarantees `ctx` is the records vector, borrowed for this call only.
    let records = unsafe { &mut *ctx.cast::<Vec<Record>>() };
    records.push((tag, owned(a), owned(b)));
}

/// What the harness reported while `call` ran it with [`record`].
fn recorded(call: impl FnOnce(Emit, *mut c_void)) -> Vec<Record> {
    let mut records: Vec<Record> = Vec::new();
    call(record, std::ptr::from_mut(&mut records).cast());
    records
}

/// `input` and its terminator. After a trailing backslash the C steps over the terminator and
/// reads the next byte, so a second NUL keeps that read inside the buffer and ends the split.
fn buffer(input: &[u8]) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(input.len() + 2);
    buffer.extend_from_slice(input);
    buffer.extend_from_slice(&[0, 0]);
    buffer
}

/// The strings `array` points at, each into a buffer still alive.
fn tokens(array: &[*mut c_char]) -> Vec<Vec<u8>> {
    array
        .iter()
        // SAFETY: the C set every counted slot to a NUL-terminated string inside the buffer.
        .map(|&token| {
            unsafe { CStr::from_ptr(token) }
                .to_bytes()
                .to_vec()
        })
        .collect()
}

/// Run a C split over a NUL-terminated copy of `input` with `limit` slots.
fn split(
    input: &[u8],
    limit: u32,
    call: impl FnOnce(*mut c_char, *mut *mut c_char, c_uint) -> c_uint,
) -> Vec<Vec<u8>> {
    let mut buffer = buffer(input);
    let mut array = vec![std::ptr::null_mut::<c_char>(); limit as usize];
    let count = call(
        buffer
            .as_mut_ptr()
            .cast(),
        array.as_mut_ptr(),
        limit,
    );
    tokens(&array[..count as usize])
}

/// Run a C encoder over a NUL-terminated copy of `url` into a zeroed buffer of `len` bytes.
fn encode_into(
    url: &[u8],
    len: usize,
    call: impl FnOnce(*const c_char, *mut c_char) -> *mut c_char,
) -> Vec<u8> {
    assert!(len > 0, "the C writes a terminator into the last byte");
    let input = buffer(url);
    let mut buf = vec![0u8; len];
    let written = call(
        input
            .as_ptr()
            .cast(),
        buf.as_mut_ptr()
            .cast(),
    );
    assert!(!written.is_null(), "the encoder was given a buffer");
    // SAFETY: the C terminated what it wrote inside `buf`.
    unsafe { CStr::from_ptr(written) }
        .to_bytes()
        .to_vec()
}

fn text(field: Option<Vec<u8>>) -> Vec<u8> {
    field.unwrap_or_default()
}

impl Oracle {
    /// `cleanup_separated_string`, `delim` 0 where the switch passes none.
    pub fn cleanup(self, input: &[u8], delim: u8) -> Vec<u8> {
        let mut buffer = buffer(input);
        // SAFETY: the buffer is NUL-terminated; the C rewrites it in place and returns a pointer
        // into it.
        let start = unsafe {
            (self
                .abi
                .cleanup)(
                buffer
                    .as_mut_ptr()
                    .cast(),
                delim as c_char,
            )
        };
        tokens(&[start]).remove(0)
    }

    /// `switch_separate_string`, `^^X` head included, keeping at most `limit` tokens.
    pub fn separate_string(self, input: &[u8], delim: u8, limit: u32) -> Vec<Vec<u8>> {
        split(input, limit, |buf, array, len| {
            // SAFETY: `buf` is NUL-terminated and `array` holds `len` slots.
            unsafe {
                (self
                    .abi
                    .separate_string)(buf, delim as c_char, array, len)
            }
        })
    }

    /// `separate_string_char_delim` with no `^^X` head read.
    pub fn char_delim(self, input: &[u8], delim: u8, limit: u32) -> Vec<Vec<u8>> {
        split(input, limit, |buf, array, len| {
            // SAFETY: `buf` is NUL-terminated and `array` holds `len` slots.
            unsafe {
                (self
                    .abi
                    .char_delim)(buf, delim as c_char, array, len)
            }
        })
    }

    /// `separate_string_blank_delim` with no `^^X` head read.
    pub fn blank_delim(self, input: &[u8], limit: u32) -> Vec<Vec<u8>> {
        split(input, limit, |buf, array, len| {
            // SAFETY: `buf` is NUL-terminated and `array` holds `len` slots.
            unsafe {
                (self
                    .abi
                    .blank_delim)(buf, array, len)
            }
        })
    }

    /// `switch_separate_string_string`, keeping at most `limit` tokens.
    pub fn separate_string_string(self, input: &[u8], delim: &[u8], limit: u32) -> Vec<Vec<u8>> {
        let mut delim = buffer(delim);
        split(input, limit, |buf, array, len| {
            // SAFETY: both strings are NUL-terminated and `array` holds `len` slots.
            unsafe {
                (self
                    .abi
                    .separate_string_string)(
                    buf,
                    delim
                        .as_mut_ptr()
                        .cast(),
                    array,
                    len,
                )
            }
        })
    }

    /// `switch_find_end_paren`: the byte offset of the close, or `None` when there is none.
    pub fn find_end_paren(self, input: &[u8], open: u8, close: u8) -> Option<usize> {
        let buffer = buffer(input);
        // SAFETY: the buffer is NUL-terminated and the C only reads it.
        let end = unsafe {
            (self
                .abi
                .find_end_paren)(
                buffer
                    .as_ptr()
                    .cast(),
                open as c_char,
                close as c_char,
            )
        };
        // SAFETY: a non-null result points into the same buffer.
        (!end.is_null()).then(|| unsafe {
            end.cast_const()
                .offset_from(
                    buffer
                        .as_ptr()
                        .cast(),
                )
        } as usize)
    }

    /// `switch_url_encode_opt` into a buffer of `len` bytes, terminator included.
    pub fn url_encode_opt(self, url: &[u8], len: usize, double_encode: bool) -> Vec<u8> {
        encode_into(url, len, |url, buf| {
            // SAFETY: `url` is NUL-terminated and `buf` holds `len` bytes, at least one.
            unsafe {
                (self
                    .abi
                    .url_encode_opt)(url, buf, len, c_int::from(double_encode))
            }
        })
    }

    /// `switch_url_encode` into a buffer of `len` bytes, terminator included.
    pub fn url_encode(self, url: &[u8], len: usize) -> Vec<u8> {
        encode_into(url, len, |url, buf| {
            // SAFETY: `url` is NUL-terminated and `buf` holds `len` bytes, at least one.
            unsafe {
                (self
                    .abi
                    .url_encode)(url, buf, len)
            }
        })
    }

    /// `switch_needs_url_encode`, the check `protect_dest_uri` in mod_sofia runs first.
    pub fn needs_url_encode(self, s: &[u8]) -> bool {
        let buffer = buffer(s);
        // SAFETY: the buffer is NUL-terminated and the C only reads it.
        let needs = unsafe {
            (self
                .abi
                .needs_url_encode)(
                buffer
                    .as_ptr()
                    .cast(),
            )
        };
        needs != 0
    }

    /// `switch_core_url_encode_opt`, which sizes its own buffer from a memory pool.
    pub fn core_url_encode_opt(self, url: &[u8], double_encode: bool) -> Vec<u8> {
        let input = buffer(url);
        let mut out = vec![0u8; url.len() * 3 + 1];
        // SAFETY: `input` is NUL-terminated and `out` holds the length passed.
        let len = unsafe {
            (self
                .abi
                .core_url_encode_opt)(
                input
                    .as_ptr()
                    .cast(),
                c_int::from(double_encode),
                out.as_mut_ptr()
                    .cast(),
                out.len(),
            )
        };
        assert!(
            len != usize::MAX,
            "an encoding longer than three bytes per input byte"
        );
        out.truncate(len);
        out
    }

    /// `SWITCH_URL_UNSAFE`.
    pub fn url_unsafe(self) -> &'static [u8] {
        // SAFETY: the C returns a string literal.
        unsafe {
            CStr::from_ptr((self
                .abi
                .url_unsafe)())
        }
        .to_bytes()
    }

    /// `switch_event_create_brackets` on the block opening `data` between `open` and `close`,
    /// splitting pairs on `comma` without a `^^` head, or `None` where the call fails.
    pub fn brackets(self, data: &[u8], open: u8, close: u8, comma: u8) -> Option<Brackets> {
        let mut buffer = buffer(data);
        let mut rest = -1;
        let records = recorded(|emit, ctx| {
            // SAFETY: the buffer is NUL-terminated, and `emit` and `ctx` outlive the call.
            rest = unsafe {
                (self
                    .abi
                    .brackets)(
                    buffer
                        .as_mut_ptr()
                        .cast(),
                    open as c_char,
                    close as c_char,
                    comma as c_char,
                    emit,
                    ctx,
                )
            };
        });
        let rest = usize::try_from(rest).ok()?;
        let following = buffer[rest..]
            .split(|&byte| byte == 0)
            .next()
            .unwrap_or_default()
            .to_vec();
        let pairs = records
            .into_iter()
            .map(|(tag, name, value)| {
                assert_eq!(tag, PAIR, "the bracket parse reports only pairs");
                (text(name), text(value))
            })
            .collect();
        Some(Brackets {
            pairs,
            rest,
            following,
        })
    }

    /// The passes of `switch_ivr_originate`, through its enterprise split, that read `bridgeto`
    /// up to each leg's endpoint.
    pub fn dial(self, bridgeto: &[u8]) -> Dial {
        let mut dial = Dial::default();
        for (tag, a, b) in self.run(
            self.abi
                .dial,
            bridgeto,
        ) {
            if tag == THREAD {
                dial.threads
                    .push(Thread::default());
                continue;
            }
            let thread = dial
                .threads
                .last_mut();
            match (tag, thread) {
                (PAIR, None) => dial
                    .enterprise
                    .push((text(a), text(b))),
                (FAILURE, None) => dial.failure = a,
                (PAIR, Some(thread)) => match thread
                    .groups
                    .last_mut()
                    .and_then(|group| group.last_mut())
                {
                    Some(leg) => leg
                        .pairs
                        .push((text(a), text(b))),
                    None => thread
                        .pairs
                        .push((text(a), text(b))),
                },
                (NESTED, Some(thread)) => thread.nested_vars = true,
                (GROUP, Some(thread)) => thread
                    .groups
                    .push(Vec::new()),
                (LEG, Some(thread)) => thread
                    .groups
                    .last_mut()
                    .expect("a leg follows its group")
                    .push(Leg::default()),
                (ENDPOINT, Some(thread)) => {
                    thread
                        .groups
                        .last_mut()
                        .and_then(|group| group.last_mut())
                        .expect("an endpoint follows its leg")
                        .endpoint = a;
                }
                (FAILURE, Some(thread)) => thread.failure = a,
                (tag, _) => panic!("the dial harness reported tag {tag}"),
            }
        }
        dial
    }

    /// `switch_channel_expand_variables_check` on `input`.
    pub fn expand(self, input: &[u8]) -> Expansion {
        let mut expansion = Expansion::default();
        for (tag, a, b) in self.run(
            self.abi
                .expand,
            input,
        ) {
            match tag {
                OUTPUT => expansion.text = text(a),
                LOOKUP => expansion
                    .lookups
                    .push(text(a)),
                API => expansion
                    .api_calls
                    .push((text(a), b)),
                tag => panic!("the expansion harness reported tag {tag}"),
            }
        }
        expansion
    }

    /// `originate_function` on an API argument line, stripped as `switch_api_execute` strips it.
    pub fn api_originate(self, arg: &[u8]) -> ApiOriginate {
        let mut api = ApiOriginate::default();
        for (tag, a, b) in self.run(
            self.abi
                .api_originate,
            arg,
        ) {
            if tag == ORIGINATE {
                api.originated = Some(Originated {
                    aleg: a,
                    timeout: String::from_utf8(text(b))
                        .ok()
                        .and_then(|timeout| {
                            timeout
                                .parse()
                                .ok()
                        })
                        .expect("the harness prints the timeout as a number"),
                    ..Originated::default()
                });
                continue;
            }
            let originated = api
                .originated
                .as_mut();
            match (tag, originated) {
                (OUTPUT, _) => api
                    .output
                    .push(text(a)),
                (FAILURE, _) => api.assertion = a,
                (CALLER_ID, Some(originated)) => {
                    originated.cid_name = a;
                    originated.cid_num = b;
                }
                (APPLICATION, Some(originated)) => {
                    originated.action = Some(Action::Application {
                        name: text(a),
                        arg: b,
                    });
                }
                (TRANSFER, Some(originated)) => {
                    originated.action = Some(Action::Transfer {
                        extension: text(a),
                        dialplan: text(b),
                        context: Vec::new(),
                    });
                }
                (CONTEXT, Some(originated)) => {
                    if let Some(Action::Transfer { context, .. }) = &mut originated.action {
                        *context = text(a);
                    }
                }
                (tag, _) => panic!("the originate harness reported tag {tag}"),
            }
        }
        api
    }

    /// `switch_true`.
    pub fn switch_true(self, expr: &[u8]) -> bool {
        let buffer = buffer(expr);
        // SAFETY: the buffer is NUL-terminated and the C only reads it.
        let truth = unsafe {
            (self
                .abi
                .switch_true)(
                buffer
                    .as_ptr()
                    .cast(),
            )
        };
        truth != 0
    }

    /// `inline_dialplan_hunt` on `target`, an `m:<delim>:` head included, over an empty
    /// destination number: the applications it adds, `None` where it returns no extension.
    pub fn inline_dialplan_hunt(self, target: &[u8]) -> Option<Vec<InlineApplication>> {
        let mut applications = Vec::new();
        let mut extension = false;
        for (tag, a, b) in self.run(
            self.abi
                .inline_dialplan_hunt,
            target,
        ) {
            match tag {
                APPLICATION => applications.push((text(a), b)),
                EXTENSION => extension = true,
                tag => panic!("the inline hunt harness reported tag {tag}"),
            }
        }
        extension.then_some(applications)
    }

    /// `switch_channel_execute_on_value` on an `execute_on_*` value, over a channel whose variable
    /// and API lookups answer nothing.
    pub fn execute_on_value(self, value: &[u8]) -> ExecuteOnValue {
        let mut hook = ExecuteOnValue::default();
        let mut fields = Fields::default();
        for (tag, a, b) in self.run(
            self.abi
                .execute_on_value,
            value,
        ) {
            match tag {
                APPLICATION => {
                    hook.app = text(a);
                    hook.arg = b;
                }
                LOOKUP => hook
                    .lookups
                    .push(text(a)),
                API => hook
                    .api_calls
                    .push((text(a), b)),
                FIELD => fields.push(a, b),
                tag => panic!("the execute_on harness reported tag {tag}"),
            }
        }
        hook.queued = fields
            .take("queued")
            .as_deref()
            == Some(b"true");
        hook
    }

    /// `switch_core_session_exec` on an application argument, up to the call into the application.
    pub fn exec_argument(self, arg: &[u8]) -> ExecArgument {
        let mut exec = ExecArgument::default();
        for (tag, a, b) in self.run(
            self.abi
                .exec_argument,
            arg,
        ) {
            match tag {
                APPLICATION => exec.argument = b,
                PAIR => exec
                    .scope
                    .push((text(a), text(b))),
                LOOKUP => exec
                    .lookups
                    .push(text(a)),
                API => exec
                    .api_calls
                    .push((text(a), b)),
                tag => panic!("the exec harness reported tag {tag}"),
            }
        }
        exec
    }

    /// `switch_channel_str2cause`.
    pub fn str2cause(self, text: &[u8]) -> i32 {
        let buffer = buffer(text);
        // SAFETY: the buffer is NUL-terminated and the C only reads it.
        unsafe {
            (self
                .abi
                .str2cause)(
                buffer
                    .as_ptr()
                    .cast(),
            )
        }
    }

    /// Every named entry of `CAUSE_CHART`, in its order.
    pub fn cause_chart(self) -> Vec<(Vec<u8>, i32)> {
        recorded(|emit, ctx| {
            // SAFETY: `emit` and `ctx` outlive the call.
            unsafe {
                (self
                    .abi
                    .cause_chart)(emit, ctx);
            }
        })
        .into_iter()
        .map(|(tag, name, cause)| {
            assert_eq!(tag, CAUSE, "the cause chart reports only causes");
            (text(name), number(cause))
        })
        .collect()
    }

    /// `protect_dest_uri` in mod_sofia on a destination number.
    pub fn protect_dest_uri(self, destination: &[u8]) -> ProtectedDestination {
        let mut protected = ProtectedDestination::default();
        for (tag, a, _) in self.run(
            self.abi
                .protect_dest_uri,
            destination,
        ) {
            match tag {
                OUTPUT => protected.destination = text(a),
                RESULT => protected.encoded = number(a) != 0,
                tag => panic!("the protect_dest_uri harness reported tag {tag}"),
            }
        }
        protected
    }

    /// `sofia_outgoing_channel` in mod_sofia on a destination, the `sofia/` endpoint text after its
    /// prefix, up to attaching its private object to the profile.
    pub fn sofia_outgoing_channel(self, destination: &[u8], sofia: &Sofia<'_>) -> SofiaOutgoing {
        let input = buffer(destination);
        let headers = CArray::new(
            sofia
                .headers
                .iter()
                .flat_map(|&(name, value)| [name, value]),
        );
        let profiles = CArray::new(
            sofia
                .profiles
                .iter()
                .copied(),
        );
        let gateways = CArray::new(
            sofia
                .gateways
                .iter()
                .copied(),
        );
        let records = recorded(|emit, ctx| {
            // SAFETY: every string is NUL-terminated, every array NULL-terminated, and all of them,
            // `emit` and `ctx` outlive the call.
            unsafe {
                (self
                    .abi
                    .sofia_outgoing_channel)(
                    input
                        .as_ptr()
                        .cast(),
                    headers.as_ptr(),
                    profiles.as_ptr(),
                    gateways.as_ptr(),
                    emit,
                    ctx,
                );
            }
        });
        let mut outgoing = SofiaOutgoing::default();
        let mut fields = Fields::default();
        for (tag, a, b) in records {
            match tag {
                HEADER => outgoing
                    .header_lookups
                    .push(text(a)),
                LOOKUP => outgoing
                    .variable_lookups
                    .push(text(a)),
                VARIABLE => outgoing
                    .variables
                    .push((text(a), text(b))),
                GATEWAY => outgoing
                    .gateway_lookups
                    .push(text(a)),
                PROFILE => outgoing
                    .profile_lookups
                    .push(text(a)),
                REGISTRATION => outgoing
                    .registration_lookups
                    .push((text(a), text(b))),
                HOST => outgoing
                    .host_lookups
                    .push(text(a)),
                CAUSE => outgoing.cause = Some(number(a)),
                FIELD => fields.push(a, b),
                tag => panic!("the sofia outgoing harness reported tag {tag}"),
            }
        }
        outgoing.destination_number = text(fields.take("destination_number"));
        outgoing.transport = number(fields.take("transport"));
        outgoing.gateway_name = fields.take("gateway_name");
        outgoing.gateway_from_str = fields.take("gateway_from_str");
        outgoing.dest = fields.take("dest");
        outgoing.e_dest = fields.take("e_dest");
        outgoing.dest_to = fields.take("dest_to");
        outgoing.invite_contact = fields.take("invite_contact");
        outgoing.local_url = fields.take("local_url");
        outgoing.remote_ip = fields.take("remote_ip");
        outgoing
    }

    /// `sofia_contact_function` in mod_sofia on an API argument, with no session.
    pub fn sofia_contact(self, arg: &[u8], profiles: &[&[u8]]) -> SofiaContact {
        let input = buffer(arg);
        let profiles = CArray::new(
            profiles
                .iter()
                .copied(),
        );
        let records = recorded(|emit, ctx| {
            // SAFETY: the argument is NUL-terminated, the array NULL-terminated, and both, `emit`
            // and `ctx` outlive the call.
            unsafe {
                (self
                    .abi
                    .sofia_contact)(
                    input
                        .as_ptr()
                        .cast(),
                    profiles.as_ptr(),
                    emit,
                    ctx,
                );
            }
        });
        let mut contact = SofiaContact::default();
        let mut selects: Vec<(Vec<u8>, bool, Fields)> = Vec::new();
        for (tag, a, b) in records {
            match tag {
                OUTPUT => contact
                    .output
                    .push(text(a)),
                PROFILE => contact
                    .profile_lookups
                    .push(text(a)),
                DOMAIN => contact.default_domain_lookups += 1,
                FAILURE => contact.assertion = a,
                SELECT => selects.push((text(a), b.as_deref() == Some(b"true"), Fields::default())),
                FIELD => selects
                    .last_mut()
                    .expect("a select reports its fields after it")
                    .2
                    .push(a, b),
                tag => panic!("the sofia contact harness reported tag {tag}"),
            }
        }
        contact.selects = selects
            .into_iter()
            .map(|(profile, dedup, mut fields)| ContactSelect {
                profile,
                dedup,
                user: fields.take("user"),
                domain: fields.take("domain"),
                concat: fields.take("concat"),
                exclude_contact: fields.take("exclude_contact"),
                match_user_agent: fields.take("match_user_agent"),
            })
            .collect();
        contact
    }

    /// `channel_outgoing_channel` in mod_loopback on a destination, the `loopback/` endpoint text
    /// after its prefix, over an outbound profile with no context or dialplan.
    pub fn loopback_outgoing_channel(self, destination: &[u8]) -> LoopbackOutgoing {
        let mut loopback = LoopbackOutgoing::default();
        let mut fields = Fields::default();
        for (tag, a, b) in self.run(
            self.abi
                .loopback_outgoing_channel,
            destination,
        ) {
            match tag {
                VARIABLE => loopback
                    .variables
                    .push((text(a), text(b))),
                FIELD => fields.push(a, b),
                tag => panic!("the loopback harness reported tag {tag}"),
            }
        }
        loopback.name = fields.take("name");
        loopback.destination_number = text(fields.take("destination_number"));
        loopback.context = fields.take("context");
        loopback.dialplan = fields.take("dialplan");
        loopback.app = fields
            .take("app")
            .as_deref()
            == Some(b"true");
        loopback
    }

    /// `user_outgoing_channel` in mod_dptools on a destination, the `user/` endpoint text after its
    /// prefix, up to the directory lookup: `None` where it stops first.
    pub fn user_outgoing_channel(self, destination: &[u8]) -> Option<UserOutgoing> {
        let mut default_domain = false;
        let mut fields = Fields::default();
        for (tag, a, b) in self.run(
            self.abi
                .user_outgoing_channel,
            destination,
        ) {
            match tag {
                DOMAIN => default_domain = true,
                FIELD => fields.push(a, b),
                tag => panic!("the user harness reported tag {tag}"),
            }
        }
        (!fields.is_empty()).then(|| UserOutgoing {
            user: text(fields.take("user")),
            domain: text(fields.take("domain")),
            default_domain,
        })
    }

    /// `group_call_function` in mod_commands on an API argument, up to the directory lookup:
    /// `None` where it stops first.
    pub fn group_call(self, arg: &[u8]) -> Option<GroupCall> {
        let mut default_domain = false;
        let mut fields = Fields::default();
        for (tag, a, b) in self.run(
            self.abi
                .group_call,
            arg,
        ) {
            match tag {
                DOMAIN => default_domain = true,
                FIELD => fields.push(a, b),
                tag => panic!("the group call harness reported tag {tag}"),
            }
        }
        (!fields.is_empty()).then(|| GroupCall {
            group: text(fields.take("group")),
            domain: fields.take("domain"),
            call_delim: text(fields.take("call_delim")),
            default_domain,
        })
    }

    /// Run a recording harness function over a NUL-terminated copy of `input`.
    fn run(self, call: Recorded, input: &[u8]) -> Vec<Record> {
        let input = buffer(input);
        recorded(|emit, ctx| {
            // SAFETY: the input is NUL-terminated, and `emit` and `ctx` outlive the call.
            unsafe {
                call(
                    input
                        .as_ptr()
                        .cast(),
                    emit,
                    ctx,
                );
            }
        })
    }
}
