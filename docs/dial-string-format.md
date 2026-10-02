# FreeSWITCH Dial String Format

Reference for endpoint strings, variable scoping, and bridge semantics as they
appear on the ESL wire and in FreeSWITCH configuration. Based on the FreeSWITCH
source at `v1.11.1` (commit `c2c59645f6911a76589e5008c4d73349ded44b65`), the
commit hooks/source-refs.yaml pins, chiefly switch_ivr_originate.c, mod_sofia.c,
mod_loopback.c and mod_dptools.c; line numbers index that commit. Behaviour
described as measured was measured on FreeSWITCH 1.10.13-dev (git 8bb2a39).

## Endpoint types

FreeSWITCH endpoints are **module-specific**. The core splits the dial string
on the first `/`, looks up the module name in an endpoint hash table
(`switch_loadable_module_get_endpoint_interface()`), and delegates all
remaining parsing to the module's `outgoing_channel` callback. There is no
universal endpoint grammar -- each module defines its own format.

### sofia (SIP)

Direct profile routing:

```
sofia/{profile}/{destination}
sofia/internal/1000@pbx.example.com
sofia/external/18005551234@carrier.example.com
```

Gateway routing (uses pre-configured gateway credentials and transport):

```
sofia/gateway/{gateway_name}/{destination}
sofia/gateway/my_provider/18005551234
```

Gateway with explicit profile qualifier:

```
sofia/gateway/{profile}::{gateway_name}/{destination}
sofia/gateway/internal::my_provider/18005551234
```

Source: `mod_sofia.c` -- checks if remainder starts with `gateway/`, then
parses as 3-part `gateway/name/dest`; otherwise parses as 2-part
`profile/dest` and formats the destination as a SIP URI.

### loopback

Re-enters the dialplan on a new channel pair. Useful for applying dialplan
logic to an originated call or for codec renegotiation.

```
loopback/{extension}[/{context}[/{dialplan}]]
loopback/9199
loopback/9199/default
loopback/9199/default/xml
loopback/app=lua:script.lua
```

Context defaults to `"default"`, dialplan defaults to `"xml"`.

Source: `mod_loopback.c` -- checks for `app=` prefix (direct application
execution), otherwise splits on `/` for extension/context/dialplan.

### user

Directory-based endpoint. Resolves via the user's `dial-string` parameter
in the FreeSWITCH directory XML. Typically expands to a `sofia_contact()`
expression.

```
user/{name}[@{domain}]
user/1000
user/bob@pbx.example.com
```

Common directory `dial-string` configuration:

```xml
<param name="dial-string"
  value="{^^:sip_invite_domain=${dialed_domain}:presence_id=${dialed_user}@${dialed_domain}}${sofia_contact(*/${dialed_user}@${dialed_domain})}"/>
```

### error

Pseudo-endpoint that terminates the call with a specific hangup cause.
Used in bridge dial strings for explicit failure routing.

```
error/{hangup_cause}
error/user_busy
error/user_not_registered
error/no_route_destination
```

### group

Directory-based group endpoint. Resolves to the `group_call()` API function
result.

```
group/{group_name}@{domain}
group/support@pbx.example.com
```

Equivalent to `${group_call(support@pbx.example.com)}`.

### portaudio / pulseaudio / alsa

Audio device endpoints for local sound hardware. The destination is optional
and typically `auto_answer` or a device identifier.

```
portaudio
portaudio/auto_answer
pulseaudio
pulseaudio/auto_answer
alsa
alsa/auto_answer
```

All three share the same `AudioEndpoint` struct in the library and differ
only in the module prefix.

### Endpoint text

The text after a leg's blocks, which the endpoint module parses, meets the
carrier's pass and both leg splits first: `switch_ivr_originate` cuts groups on
`|` and legs on `,` with `switch_separate_string`, each token through
`cleanup_separated_string`. An endpoint renders that text escaped once for each
pass, as a `{}` value is, with `\,` and `\|` for the separators: a backslash
needs eight on either carrier, a quote seven backslashes over `originate` and six
through a dialplan application, a space quotes the text on the blank split, and
an edge space rides as `\s`. A `${…}` reference is left to the switch as in a
block value ([A value naming a variable](#a-value-naming-a-variable)): through a
dialplan application the expansion substitutes it before the leg splits, so
`sofia/gateway/gw/${destination_number}` dials the executing channel's number. A
text holding `$$` and no reference has every `$` written `\$` behind a leading
`\'`, as a value does, so the module receives `$$` whole. `sofia_contact` and
`group_call` expressions are written as they stand. Only a dialplan application
or the `expand` API expands them: `originate` over the API dials the text as
an unknown endpoint and answers `CHAN_NOT_IMPLEMENTED` (measured), so parse at
the API carrier and `Originate` config load refuse them
(`OriginateError::UnexpandedExpression`).

What no escaping delivers is refused on parse and config load, and built
unchecked:

- `:_:` in any field.
- sofia: a profile carrying `/`, `^` or `@`, or reading `gateway` in any case.
  `sofia_outgoing_channel` cuts its text at the first `^` for the To override and
  the profile at the first `/`, and takes a text opening `gateway/` in any case
  for the gateway path.
- sofia gateway: a gateway or profile carrying `/`, `^` or `@`, a profile carrying
  `::` or ending in `:`, and a gateway carrying `::` without a profile. The
  gateway is looked up by the whole text between `gateway/` and the next `/`, in
  a hash holding each gateway under both `name` and `profile::name`, so the key
  does not say where a profile ends. A profile ending `:_` or a gateway opening
  `_:` forms `:_:` in the join, which splits the dial string into enterprise
  threads.
- loopback: an extension or context carrying `/`, and an empty context or
  dialplan, which `channel_outgoing_channel` replaces with `default` and `xml`.
  An extension opening `app=` in any case runs that application: a `/` may
  follow the first `:` but not precede it, and a context or dialplan after it
  is read into the argument.
- sofia and sofia gateway: `@` in a profile or gateway puts one in the text
  `protect_dest_uri` reads, which then cuts a destination holding a
  `SWITCH_URL_UNSAFE` byte and no `@` at the last `/` (measured: a profile `a@b`
  with destination `x y` fails `INVALID_URL`).
- user: a name carrying `@`, where `user_outgoing_channel` starts the domain,
  and an empty name with no domain, on which it stops before its lookup.
- audio: an empty destination, which the module reads as none.
- sofia_contact: a user carrying `~` or `@`, or `/` without a profile; a domain
  or profile carrying `~` or `/`, or empty. `sofia_contact_function` cuts its
  argument at the first `~`, then `/`, then `@`, then a `/` after the domain,
  replaces an empty domain with the switch's default and reads an empty profile
  as none.
- group_call: a group carrying `+` or `@`, and a domain carrying `+`.
  `group_call_function` takes the order at the first `+` before it looks for
  `@`.
- sofia_contact and group_call, any field: a space, `,`, `|`, a quote, a
  backslash, or a brace or parenthesis left unbalanced. The expression is
  written as it stands, so the argument split cuts it at a space or quote, the
  leg splits at `,` or `|`, and the reference parse ends it at the first
  unbalanced `)` or `}`. A `${…}` reference in a field is left to the switch.

A sofia destination is mod_sofia's own grammar and arrives whole. Unless
`sofia_suppress_url_encoding` is true, `protect_dest_uri` URL-encodes the user
part of a text holding `@`, and truncates the text at its last `/` when what
follows carries a `SWITCH_URL_UNSAFE` character and no `@`, as a profile
holding `@` or a destination with a `/` after its `@` do: `sofia/internal/x@y/z w`
leaves `internal/x@y`. The crate holds such a destination whole and does
not refuse it, since whether it is cut rests on a channel variable the dial
string need not carry.

On every tree `switch_needs_url_encode` reads only `SWITCH_URL_UNSAFE`, so a
user part holding no byte of that set outside a valid uppercase `%XX` is sent
as it stands, controls other than CR and LF, DEL and non-ASCII included; one
holding such a byte has those encoded too.

URL encoding is where the trees `hooks/source-refs.yaml` names differ. Upstream
`switch_url_encode_opt` copies a `%` opening a valid uppercase `%XX` through
when `double_encode` is false, so `a%41 b` becomes `a%41%20b`, measured on
master at `b3ba603f49`; a lowercase `%e9` is encoded. The 1.10.13 fork
(`8bb2a39`) encodes every `%`, giving `a%2541%20b`. Its
`switch_core_url_encode_opt` sizes the output by the input's length, so a value
holding a byte to encode arrives cut to that length, an encoded byte that does
not fit dropped with everything after it: the caller-id number mod_sofia
writes into an outbound INVITE's From, and the To user it rebuilds `sip_to_uri`
from on an inbound one. Upstream sizes that buffer to fit the encoding. Either
way sofia-sip canonicalises an escaped unreserved character when it sends the
request, so upstream's Request-URI reads `aA%20b`. `sip_destination_url` holds
the encoded form; read it from a JSON event or `uuid_getvar`, since a plain
event's value passes through the same `switch_url_encode`
([events guide](guide/events.md)).

In a bridge, the comma scan ahead of the leg split protects commas from a `[`
to its matching `]` whichever leg holds either, so an endpoint whose `[` closes
in a later leg of its group merges the legs between. Where the separator
survives, because a backslash precedes it, the commas of a later leg's `[]`
block are rewritten by the first bracket's state instead of their own: a `^^,`
head becomes the block's default split and reads alike, but a comma in a `^^:`
value reaches the channel as the scan's `\x02` placeholder. `BridgeDialString`
refuses a group on parse and config load when the switch reads its legs
otherwise than each alone.

## Variable scoping

Channel variables can be set on the B-leg (destination) of an originate or
bridge via bracket notation in the dial string. Three bracket types exist
with different scopes; [Combined example](#combined-example) says which wins
when they name the same variable.

### `[k=v]` -- channel (local) scope

Applies only to the **immediately following endpoint**.

```
[origination_caller_id_number=1234]sofia/internal/1000@domain
```

Multiple blocks accumulate:

```
[var1=a][var2=b]sofia/internal/1000@domain
```

Unlike `{}` and `<>`, a `[]` block is parsed after the dial string is split
into legs on `|` and then on `,`, and both splits run the same backslash-consuming
cleanup as the block parse. Three consequences, all measured:

- A literal backslash needs **thirty-two** backslashes here, not eight. At
  eight, `a\nb` arrives carrying a newline.
- A `|` in a value is read by the leg split, and the block becomes a leg with
  no endpoint (`CHAN_NOT_IMPLEMENTED`). `\|` carries it. The same goes for `|`
  as a `^^` separator, which `Variables` refuses in this scope. In `{}` and
  `<>`, parsed and removed before that split, a `|` is ordinary text.
- A value cannot carry a single quote. The scan that protects commas inside
  quotes during the leg split toggles on every `'` it meets, escaped or not, so
  two values each carrying one quote pair with each other and the first
  swallows the second: `[p1=it's,p2=don't,p3=x]` arrives as `p1=its,p2=dont`
  with no `p2` at all, at every escaping depth. `Variables` refuses such a
  value in channel scope.
- A comma cannot follow a backslash unguarded. The scan that protects commas
  inside the block tests only the byte before each comma, so the separator
  after a value ending in a backslash, or a literal comma after one in a `^^`
  block, is left to the leg split, which cuts the block open. `Variables` puts
  an empty `''` between them, escaped to reach that scan bare and stripped by
  the leg split.

### `{k=v}` -- default (global) scope

Applies to **all endpoints** in the current originate/bridge set. Multiple
blocks accumulate.

```
{hangup_after_bridge=true}sofia/gateway/gw/1234
{ignore_early_media=true}{call_timeout=30}sofia/gateway/gw/1234
```

### `<k=v>` -- enterprise (ultra-global) scope

Applies across **all threads** in an enterprise originate (`:_:` separated
sections).

```
<originate_timeout=60>{thread1_var=a}endpoint1:_:{thread2_var=b}endpoint2
```

### Combined example

```
<ultra_global=1>{thread_global=2}[per_endpoint=3]sofia/internal/1000@domain
```

Effective variables on the channel: `ultra_global=1`, `thread_global=2`,
`per_endpoint=3`.

When two scopes name the same variable, the wider one wins.
`switch_ivr_originate` installs a leg's `[]` variables first and the
originate-wide `<>` and `{}` variables after them, so `{k=g}[k=l]` and
`<k=e>[k=l]` both deliver the wider value. `local_var_clobber=true` among the
originate-wide variables reverses the order, and the leg's value wins. `<>` and
`{}` share one event, parsed in that order, so `{}` beats `<>`. Within one scope
the block parsed last wins. All measured over `originate`.

## Variable value escaping

How much escaping a value needs depends on **which command carries the block**,
because the switch escape-processes it a different number of times per carrier.
See [Parse depth](#parse-depth) below before relying on any of the forms here;
the rules in this section are what the `freeswitch-esl-tokio` crate emits, and
were measured on the build named at the top rather than derived from the source.

### Backslash escaping

A comma is escaped with a backslash, on either carrier:

```
{sip_h_Call-Info=<url>;meta=123\,<uri>}endpoint
```

A literal backslash needs **eight**, on either carrier. Fewer and the switch
reads the sequence as an escape and substitutes the character it names, so
`a\nb` arrives carrying a newline:

```
{path=C:\\\\\\\\Users}endpoint
```

A single quote is the one rule that differs by carrier — six backslashes
through a dialplan application, seven through the `originate` API:

```
{greeting=it\\\\\\'s_me}endpoint      <- dialplan: bridge, sendmsg execute
{greeting=it\\\\\\\'s_me}endpoint     <- api originate, bgapi originate
```

Each count leaves `\'` entering the carrier's last pass, which is what makes it
right: the parity differs because the dialplan carrier's first pass deletes a
`\'` outright while the API carrier's first pass keeps it. No count satisfies
both. Two and three also measure correctly on a block whose values carry one
quote each, and that is the trap — they deliver the quote bare to the last pass,
whose cleanup keeps a lone quote only while no other quote follows it in the
same field, so a value carrying two loses both. Test with two quoted values in
the block and two quotes in one value.

Values containing spaces are wrapped in single quotes. Those wrapping quotes are
balanced, so unlike a quote *inside* a value they behave identically on both
carriers:

```
{sip_h_X-Info='value with spaces'}endpoint
```

Those quotes do not keep a space at the value's edge. An earlier pass consumes
them, and the `=` split's `cleanup_separated_string` then skips leading spaces
and cuts trailing ones outside quotes. An edge space rides as `\s`, which that
cleanup reads after trimming, with its backslash doubled for every earlier pass
— four in `{}` and `<>` on either carrier, sixteen in `[]`. Measured in all
three scopes over `originate`, `bridge` and an `originate ^^~` line:

```
{greeting='\\\\slead and trail\\\\s'}endpoint
```

### Values that cannot be expressed at all

- **An empty value.** `{k=}` never reaches the channel: the switch splits the
  pair on `=`, requires exactly two fields, and `k=` yields one. The only
  `switch_log_printf` in that loop is inside the successful branch, so nothing
  is logged at any level. Quoting does not help — `k=''`, `k=\'\'` and
  `k=\\'\\'`, written raw into the block, were all measured discarded on both
  carriers.
- **A value closing a bracket it never opened.** `switch_find_end_paren` counts
  depth and honours no escape while doing so, so a lone `}`, `]` or `>` ends the
  block early and the remainder becomes dial-string text. A balanced pair such
  as `${var}` is fine and ordinary.
- **A value containing `:_:`.** Any `:_:` in the dial string sends
  `switch_ivr_originate` down the enterprise path, whose split honours no quote
  or escape, so no quoting delivers it. Inside `<>` the value itself arrives,
  but the originate is still split into threads. `Variables` refuses such a
  value wherever it is parsed or loaded.

### Variable names

A key meets every pass its value meets: the carrier's, the leg splits in `[]`,
the separator split, and then the `=` split, which keeps its first field whole
as the key and the rest as the value. `Variables` escapes a key as it escapes a
value in the same block, and adds two rules for the `=` split:

- `=` is written `\=`. The split skips the byte after a backslash, and its
  cleanup reads `\=` as `=`; every earlier pass leaves `\=` alone.
- A key opening `^^` follows an empty `''`, escaped to reach the `=` split bare.
  `switch_separate_string` reads a token opening `^^` and a byte as naming its
  own separator, so such a key would pick the `=` split's delimiter, and as the
  block's first key the block's separator.

What no escaping delivers is refused on parse and config load: an empty key,
which `switch_channel_set_variable_var_check` installs nowhere; `:_:`; a single
quote in channel scope; a bracket of the block's own kind left unbalanced; in a
`^^` block, the separator; `[`, which `switch_event_base_add_header` reads as an
array index, installing the value under the text before it; and a key differing
only in ASCII case from an earlier one in the block, since the block's event
carries `EF_UNIQ_HEADERS` and replaces a header by `strcasecmp`, so
`{k=1,K=2}` installs `K=2` alone. A refusal names the variable name as the
field, never its text.

The channel's variables replace by `strcasecmp` too, so the same fold happens
between scopes at install and is not refused: it is the wider-scope rule of
[Combined example](#combined-example), with the spelling of whichever block
installs last. `{k=g}[K=l]` leaves `k=g`, and with `local_var_clobber=true`
leaves `K=l`. A key `_body` also sets the block event's body, which nothing
in the originate reads, and arrives as a variable. All measured on both trees
named in [sofia](#sofia-sip).

### A value naming a variable

`Variables` writes a `${…}` reference as it stands and leaves it to the switch.
The dialplan carrier substitutes it before the block is parsed. What reaches
install is dropped: `switch_ivr_originate` sets each pair through
`switch_channel_set_variable_var_check`, which refuses a value holding `${` with
a CRIT log line unless `origination_nested_vars` is true on the list, the
originating channel or the core, or the dial string opts in per enterprise
thread: `origination_nested_vars=true`, in any case, within that thread's own
text, or a `<>` block ahead of the `:_:` split setting it true for every
thread. A `{}` block in one thread lets no `${` through on another.

### The inline action list is a third carrier

Everything above concerns a `{k=v}` block. An inline action list —
`app:args,app:args` with the `inline` dialplan — is parsed by
`inline_dialplan_hunt` rather than by the block tokenizer, and its rules are its
own. All of the following were measured on a live switch.

**The separator is escaped, not chosen.** A bare comma inside an argument ends
the action, so the switch builds and runs applications nobody wrote and logs
nothing. One backslash reaching the action split is enough, because
`cleanup_separated_string` unescapes a character only when it is the delimiter
of the split being cleaned up after: the originate line is split on spaces
first, where `\,` is left alone, then the action list is split on its own
separator, where the same `\,` becomes a comma. The same cleanup reads `\\`,
`\'`, `\"`, `\n`, `\r`, `\t` and `\s` and trims a space at an action's end, so
`Originate::inline` escapes each action once for that split, as an argument is
escaped under a `^^X` separator, then quotes the whole list through
`originate_quote`, which doubles every backslash for the space split to consume.

An `m:<delim>:` prefix immediately before the first action changes the separator
for the list, the way `^^` does for a block. It is consumed by the hunt, so
nothing of it survives into the extension — a masquerade onto another channel
carries the actions, never the prefix. `Originate::inline_with_delimiter` emits
it, and escapes the named separator the same way. The hunt's split is the one
a `^^X` argument separator names, so the constructor and parse refuse what
`with_argv_separator` refuses for the switch's reasons, and `:`, where each
action splits into application and data: under `m:s:` an edge space written
`\s` arrives as `s`.

**A quote is escaped for both splits.** Written into the line with one
backslash, a quote reaches the hunt's split bare. With the list wrapped in
quotes — which happens whenever any argument contains a space — a bare quote
loses the value entirely, and a second one in the same value is read as closing
a quoted region, so both are stripped:

```
set:v=a\'b with space     -> a'b with space
set:v=x\'a\'y with space  -> xay with space
```

This is what breaks a hand-written `${cond('${x}' != '' ? a : b)}`: `cond`
receives no operands and returns `-ERR` into the channel variable. Escaped once
for the hunt and again for the line, `\\\'` on the blank split, every quote
arrives: `x'a'y z` and a `cond` over quoted operands were measured intact, and
that is what `Originate::inline` writes.

### `^^X` block separator

Placed **immediately after the opening bracket**, `^^` and a replacement
character change the separator between pairs for the whole block, so values may
contain commas with no escaping:

```
{^^:sip_invite_domain=example.com:presence_id=bob@example.com}endpoint
{^^:codecs=PCMA,PCMU,G729:tenant=acme}endpoint
```

This is the only mechanism available when values arrive by `${...}` expansion,
because substitution happens *before* the block is parsed and no escaping can be
inserted into the result. It works identically on both carriers.

`Variables` refuses a separator that breaks the pair split or its cleanup, the
same set `with_argv_separator` refuses for the switch's reasons: space and
controls, non-ASCII, `\` (the split skips the byte after it, so it never
splits), `'` (it pairs with the next one as quotes) and lowercase `n r t s`
(the cleanup reads `\s` as the separator, not a space). It also refuses either
of the block's own brackets, `=`, `^`, `|` in a `[]` block, and a separator
that a key or value already contains. `$` and `{` are refused in every scope:
through a dialplan application, expansion reads a value ending in `$` before a
`{` separator, or a `$` separator before a key opening `{`, as a variable
reference, and substitutes it before the block is parsed.

Only the comma changes. The block reaches the same tokenizer the same number of
times either way, so a literal backslash still needs eight and a single quote
still needs its per-carrier count: written raw in a `^^` block, `a\nb` arrives
carrying a newline, and a quote still turns off space splitting for the rest of
an `originate` line.

It does not help with a quoted value: the quote pairing suppresses splitting on
whichever separator is in use, so two quoted values still merge.

`switch_event_create_brackets` splits the pairs in place, and some blocks the
switch accepts reach past their close. `FlattenedDialString` reads them as it
does:

- A block of `^^` alone splits on its terminator and reads its pairs from the
  text after the close, rewriting that text; a backslash before the close
  escapes the terminator and reads on the same way. The leg carries
  `LegWarning::BlockRewritesFollowingText`, a list or thread block raises
  `ListWarning::BlockRewritesFollowingText`, and what follows is read from the
  rewritten text.
- A pair opening a `^^` head that names a non-ASCII separator is split on that
  separator's first byte, and what that installs, if anything, no string
  carries: the pair is read as installing nothing and carries
  `LegWarning::PairUnreadable`.
- A group or leg opening such a head, a block with a non-ASCII separator whose
  content ends in a backslash or that a second block follows, and a pair opening
  such a head whose text ends in a backslash, send a split on the first byte into
  text no string carries: `FlattenedDialStringError::SplitSeparatorUnreadable`.

**A `^^X` prefix on an individual value is not a general mechanism.** Writing
`{k=^^:a:b}` sets `k` to the literal `^^:a:b` — measured on both carriers. Only
consumers that specifically decode it, such as the codec-string parser reading
`absolute_codec_string`, interpret the form; the bracket parser stores it
verbatim.

### Parse depth

`switch_event_create_brackets` tokenizes a block **twice on its own** — once
splitting the pairs on the separator, once splitting each pair on `=` — and both
calls run the full quote-stripping, backslash-consuming cleanup
(`cleanup_separated_string`) over their results. Each carrier adds one pass of
its own before those. Through the `originate` API it is `mod_commands` splitting
its argument list with `separate_string_blank_delim`, whose quote handling has
no lookahead: a quote opens a quoted region regardless of any delimiter. Through
a dialplan application it is variable expansion of the application's argument
(`switch_channel_expand_variables_check`), which consumes `\\` and deletes a `\'`
outright — both characters — and is skipped when `app_disable_expand_variables`
is true on the channel, which then leaves that carrier at the API's depth.

| Carrier | Passes |
|---|---|
| `bridge` and other dialplan applications, incl. `sendmsg execute` | expansion + 2 |
| `api originate`, `bgapi originate` | argv split + 2 |
| `originate` after `^^X` | argv split on `X` + 2 |

Consequences worth knowing before hand-writing a block:

- An unescaped quote reaching the `originate` argv pass turns off space
  splitting for the rest of the line, so the command fails with a usage error
  rather than corrupting a value. That is the loud case.
- Two quotes that survive to the block parse pair with each other across the
  separator, so the pair between them is not split: the *first* value absorbs
  the second and the second is never set. The variable that goes missing is not
  the one that contained the quote.
- Two quotes that reach the last pass bare pair with each other inside the
  value, and both are stripped. The value stays otherwise intact, so a document
  that loses every apostrophe is still well-formed and nothing fails.
- Expansion, whenever it runs over the argument, drops the first `$` of a `$$`
  that opens no reference, and `\$` keeps it only while expansion runs. Through
  a dialplan application a value holding `$$` has every `$` written `\$` behind
  a leading `\'`, which makes expansion run and is deleted by it.
- A log line is not evidence either way. `mod_logfile` splits its own output
  with the same tokenizer, so a value is mangled in the log whether or not it
  was mangled on the wire. Read values back with `uuid_getvar` or `uuid_dump`.

### Parser revisions

Every count above belongs to one revision of the block parser,
`BlockParse::PairSplitCleans`, in which both of the block's own splits run
`cleanup_separated_string`. Nothing on the wire says which revision a switch
runs, so the application names the FreeSWITCH version it targets and
`BlockParse::for_version` answers:

- Releases 1.10.0 through 1.11.3 map to `PairSplitCleans`. The block parse,
  both carrier passes and the leg splits are unchanged in meaning across that
  source — from 1.10.11 on, `switch_event_create_brackets`, the tokenizer it
  splits with, `switch_channel_expand_variables_check` and the originate leg
  loops are byte-identical at every release tag — and the live escaping suite
  passes on a 1.10 and a 1.11 build.
- Every `-dev` build is refused: it reports the same version before and after
  an upstream commit.

A refusal names the vouched range, and the application then passes a revision
explicitly. After a switch upgrade, run
`cargo test --test live_channel -- --ignored escaping` with
`FREESWITCH_BLOCK_PARSE` naming the revision under test; a switch that fails it
parses blocks in a way this crate has no revision for yet.

A revision covers `{}`, `<>` and `[]` escaping only. The inline action list has
its own parser, and the quote pre-scan that makes a quote undeliverable in a
`[]` value runs before any block parse, so neither changes with it.

## Keeping a value out of the tokenizer entirely

A large or free-text value — a PIDF-LO document for `sip_multipart`, say — has
no business crossing the block tokenizer at all: every apostrophe, comma,
backslash and space in it is a trap, and the block is line-delimited on the ESL
wire besides (`read_packet` in `mod_event_socket.c` takes the first line of the
packet as the command, so a newline never rides any `api` or `bgapi` command).
The switch offers one place where a value can be set on the new channel before
its INVITE is built, and it takes no value on the dial string.

`execute_on_originate` is a channel variable `switch_ivr_originate` reads off
the *new* channel after the bracket blocks have been installed on it and before
it launches that channel's session thread. `switch_channel_execute_on_value`
runs the named application synchronously on the originating thread (a `::`
between application and argument queues it instead — not what is wanted here).
mod_sofia sends the INVITE from `sofia_on_init`, on the session thread, so a
variable the hook sets is present when `sofia_glue_do_invite` reads
`sip_multipart`. Measured on a `sofia/` leg: the INVITE went out as
`multipart/mixed` carrying the document byte for byte, apostrophes and commas
intact, and the hook's `set` was logged before *sending invite*.

The dial string then carries paths and nothing else:

```
{execute_on_originate=lua /run/app/load_pidf.lua /run/app/<uuid>.xml}sofia/<profile>/<destination>
```

with the script reading the file and calling
`session:setVariable("sip_multipart", "application/pidf+xml:" .. body)`.
`CoreSession::setVariable` sets without the `${` check, so a document
containing that sequence is not refused. `process_mp` in `sofia_media.c` splits
the value at its first colon into content type and body, and
`sofia_media_get_multipart` wraps every `sip_multipart` value (the variable may
be stacked) and the SDP into one `multipart/mixed` body. The same is what an
inbound INVITE's parts look like on the far side, which
[`MultipartBody`](../freeswitch-types/src/variables/sip_multipart.rs) reads.

Things that bit while measuring it, each of which leaves the INVITE going out
*without* the part and one `ERR` line from mod_lua as the only trace:

- The application name is split from its argument at the first space or single
  colon. Keep the name bare and pass paths, never content: the argument is
  variable-expanded by `switch_core_session_exec` before the application sees
  it.
- The application must be flagged `SAF_SUPPORT_NOMEDIA`, or the media gate in
  `switch_core_session_execute_application_get_flags` refuses it on an outbound
  channel that has no media yet. `lua`, `set` and `export` are.
- The file is opened by FreeSWITCH, in FreeSWITCH's mount namespace, under
  FreeSWITCH's uid. A path that exists on the host and not in the service's
  namespace fails with *No such file or directory*, so a check that only
  inspects the host side proves nothing. Run the loader through the switch:
  the `lua` API runs a script on the calling thread and returns what it
  writes, whereas `luarun` spawns a thread and answers `+OK` unconditionally,
  so it cannot report a failure. That needs `lua` in the ESL user's
  `esl-allowed-api`.
- mod_lua's `io` read takes `"*a"`; `read("a")` is an invalid option there.
- Not on a loopback leg. Measured with both `set` and `lua`: the originate
  never returns and `loopback/…-a` sits in `CS_INIT` with the hook as its
  running application until something hangs it up. The hook runs before the
  channel has a session thread, and mod_loopback's B leg, which does have one,
  waits on the A leg reaching a state it cannot reach from inside the hook. A
  `sofia/` leg, whose thread nothing waits on, is fine.

Carriers that looked like alternatives and are not:

- `sendmsg` with a `text/plain` body is genuinely length-delimited — the body
  becomes the application argument untouched (`switch_ivr_parse_event`) — but it
  addresses an existing session, and the value has to be on the channel before
  its INVITE exists.
- `global_setvar` splits its argument on `=` into three fields
  (`switch_separate_string`), so any value with an `=` in it is misread.
- A `user/` endpoint applies the directory user's `<variables>` to the new
  channel only after `switch_ivr_originate` has returned, which is after the
  INVITE; only `dial-var-*` params reach the variable event first. A directory
  served per call by mod_xml_curl is therefore a carrier, but a heavy one next
  to the hook.
- `\s` is a real escape (`unescape_char` maps `n`, `r`, `t` and `s`), and would
  spare a value the wrapping quotes, but the quote strip happens at the `=` pass
  regardless of spaces, so it fixes nothing on its own.

## Bridge separators

Bridge and originate dial strings support multiple endpoints with different
failure/concurrency semantics.

### `,` -- simultaneous ring (forked dialing)

All endpoints in a comma-separated group ring at the same time. The first
endpoint to **provide media** (answer or early media) wins; others stop
ringing.

```
sofia/internal/100@domain,sofia/internal/101@domain
```

Use `ignore_early_media=true` on the A-leg to prevent early media (ringback,
music) from prematurely selecting a winner -- common with cell phones.

### `|` -- sequential failover

Endpoints separated by pipe are tried **one at a time**, in order. The next
endpoint is tried only after the previous one fails.

```
sofia/gateway/primary/1234|sofia/gateway/secondary/1234|sofia/gateway/backup/1234
```

### `:_:` -- enterprise originate

Each `:_:`-separated section is originated in a **separate thread**.
Enterprise-scope `<>` variables apply across all threads. Each thread can
have its own `{}` global variables.

```
<originate_timeout=30>{thread1_cid=100}sofia/gw/a/1234:_:{thread2_cid=200}sofia/gw/b/1234
```

Constant: `SWITCH_ENT_ORIGINATE_DELIM = ":_:"` in `switch_types.h`.

### At most 128 threads, groups or legs

`switch_ivr_enterprise_originate` splits threads, and `switch_ivr_originate`
groups and legs, into arrays of `MAX_PEERS` (128, `switch_ivr_originate.c`).
`switch_separate_string_string` and `separate_string_char_delim`
(`switch_utils.c`) stop once the array is full, so the 128th thread, group or
leg keeps the rest of the text, separators included:

```
null/0,null/1,...,null/126,null/127,null/128
```

is 128 legs, the last dialling the endpoint `null/127,null/128`. Nothing past
the limit is dialled on its own.

`BridgeDialString` refuses such a group, or more than 128 groups, with
`OriginateError::TooManyLegs`, at parse and at config load. `FlattenedDialString`
reads the list as the switch does and reports `ListWarning::LegsPastLimit`,
naming the split and how many separators its last token keeps.

### Combined example

```
{hangup_after_bridge=true}[t=10]sofia/gw/a/1234,[t=10]sofia/gw/b/1234|sofia/gw/backup/1234
```

Ring gateways `a` and `b` simultaneously (10s timeout each). If both fail,
try `backup` sequentially.

## Runtime expressions

FreeSWITCH supports `${}` variable expansion in dial strings. Some
expressions resolve to endpoint strings at call time.

### `sofia_contact`

Resolves the current registered SIP contact URI for a directory user. Returns
`error/user_not_registered` if no active registration.

```
${sofia_contact(user@domain)}
${sofia_contact(profile/user@domain)}
${sofia_contact(*/user@domain)}
```

The `*` searches all profiles. An optional `~user_agent` suffix filters by
User-Agent header.

Source: `sofia_contact_function` and `contact_callback` in `mod_sofia.c`. Each
registration becomes one leg, `sofia/<profile>/sip:<contact>`, comma-joined:

- A text after a `/` following the domain is pasted in front of every leg.
- The contact's scheme is cut at its first colon, so a `sips:` contact reads
  `sip:`.
- The `*` search drops a contact that is a substring of one already written.
- Row order is not stable between calls.

Over ESL, `eval` has no session, so `sip_exclude_contact` and
`sip_match_user_agent` are not applied.

Contacts over TCP, behind NAT detection (`fs_nat=yes`) and with a bracketed IPv6
host have been captured from a registrar; a contact carrying `fs_path` and one
registered as `sips:` have not.

### `group_call`

Resolves directory group members to a multi-endpoint dial string.

```
${group_call(group@domain)}
${group_call(group@domain+A)}
${group_call(group@domain+E)}
${group_call(group@domain+F)}
```

The flag picks the separator written between members: `A` a comma
(simultaneous, also the default), `E` `:_:` (enterprise), `F` a `|` (failover).
The last flag letter wins. The separator goes only between members: a member's
own dial string keeps whatever separators it holds, so `+F` over a member with
two registrations still yields a comma pair inside the failover list.

What `group_call_function` and `output_flattened_dial_string` in
`mod_commands.c` do to each member, all measured:

- **No dial string.** A member without a `dial-string` or `group-dial-string`
  becomes `user/<id>@<domain>`. A `group-dial-string` wins over a `dial-string`
  at the same level.
- **Expansion.** The member's dial string is expanded once, and only when it
  holds a variable reference or escaped data (`\\`, `\n`, `\s`, `\t`, `\'`).
- **Flattening.** Every `{}` and `<>` block is rewritten to `[]` and repeated
  ahead of each leg, so the output is channel scope throughout. Block order on a
  leg follows `local_var_clobber`, found by a case-sensitive substring match:

  | Member has | `local_var_clobber` | Emitted order |
  |---|---|---|
  | own `[]` block | none | `[ent][all][leg]` |
  | own `[]` block | in `{}` | `[ent][leg][all]` |
  | own `[]` block | in `<>` | `[all][leg][ent]` |
  | own `[]` block | in both | `[leg][all][ent]` |
  | no `[]` block | any | `[all][ent]` |

  The last block on a leg wins, so in the last row `<>` beats `{}`.
- **Values that break once flattened.** A `]` inside a `{}` value closes the
  rewritten block early. A `|` in a value is read by the leg split.
- **Trailing separator.** The strip cuts at the first separator found in the
  member's last three bytes, not only at a trailing one.
- **Empty expansion.** A member that expands to nothing still gets a separator,
  giving `x,,y`. A group whose members all expand empty returns `,`.
- **Empty or missing group.** Output is `error/NO_ROUTE_DESTINATION`, uppercase.
- **Pointers.** A pointer resolves to the first `<user>` whose id matches, and
  groups are searched before the domain's `<users>`, so a pointer placed before
  the real user resolves to itself and yields `user/<id>@<domain>`.

A seat therefore owns as many legs as it has registrations, repeated under one
`presence_id`.

### `eval` prefix (API evaluation)

Some applications evaluate an `eval` prefix by calling the FreeSWITCH API
first, then using the result as the dial string:

```
eval ${group_call(calltakers@${domain_name}+A)}
```

This is an application-level convention, not a core FreeSWITCH feature.

## Special bridge features

### `^` -- SIP To: header override

Appended after `@host` to override the To: header in the outbound SIP INVITE.
Useful for number portability routing where the Request-URI needs extra
parameters but the To: header should contain the clean number.

```
sip:12135551212;rn=12135550000;npdi=yes@1.2.3.4:5060^12135551212
```

### Bridge control variables

Common variables that affect bridge behavior (set on the A-leg before bridge):

| Variable | Effect |
|----------|--------|
| `call_timeout` | Seconds to wait for answer |
| `originate_timeout` | Per-endpoint timeout in originate |
| `hangup_after_bridge` | Hang up A-leg after B-leg disconnects |
| `bypass_media` | SDP passthrough (RTP flows directly between endpoints) |
| `ignore_early_media` | Don't select winner on early media (183/180+SDP) |
| `ringback` | Play tone/file to A-leg during ringing |
| `transfer_ringback` | Play during attended transfer |
| `fail_on_single_reject` | Fail entire bridge if any endpoint rejects |
| `hangup_on_single_reject` | Hang up if any endpoint rejects |
| `continue_on_fail` | Continue dialplan after bridge failure |
| `bridge_early_media` | Bridge early media to A-leg |

## Wire format in ESL

When constructing dial strings via ESL (`api originate`, `bgapi originate`,
`sendmsg execute bridge`), the complete format is:

```
originate <[vars]endpoint> <app> [dialplan] [context] [cid_name] [cid_num] [timeout]
```

For bridge (via sendmsg):

```
execute bridge <[vars]endpoint[,endpoint][|endpoint]>
```

Application arguments containing spaces must be single-quoted in originate:

```
originate loopback/9199/test '&socket(127.0.0.1:8040 async full)'
```

The space split's cleanup also strips quotes and reads `\\`, `\'`, `\"`, `\n`,
`\r`, `\t` and `\s` in every token, so a value carrying a quote or backslash
needs the same treatment. `switch_api_execute` strips tab, vertical tab, CR,
newline and space from both edges of the argument line before the split, so a
last argument ending in one loses it unless quoted. `originate_quote()` wraps
any token that is empty or carries a space, quote, backslash or one of those,
escaping `'` and `\` inside; `originate_unquote()` runs the cleanup and reads it
back.

### `^^X` argument separator

A line whose arguments open with `^^` and a byte, with at least one more byte
after it, splits `originate`'s arguments on that byte instead of on blanks
(`switch_separate_string`), and the prefix is dropped:

```
originate ^^~{k=v}loopback/9199/test~&park()
```

All of the following were measured over `originate`:

- **One byte.** The switch reads the separator as a byte, so `^^é` splits on
  the first byte of its UTF-8 encoding.
- **Spaces are text.** `^^~ error/USER_BUSY &park()` is a single argument and
  answers with the usage line. `^^ ` names the blank split itself.
- **Trailing and doubled separators.** A trailing separator adds no argument:
  `^^~error/USER_BUSY~` is one argument. Two in a row, `~~`, add an empty one.
- **Quotes still pair.** The split keeps its quote handling, so a `'` separator
  splits once and pairs with the next `'` as quotes after that.

Each argument is cleaned up once with the separator as the delimiter, so an
argument written for this split escapes, once:

- `\`, `'` and the separator itself with a backslash;
- newline, CR and tab as `\n`, `\r`, `\t`;
- a space at either edge of the argument as `\s`, since the edges are trimmed
  and `\s` keeps them;
- a vertical tab at either edge beside an empty `''`, since the API strips one
  from the edges of its line and no escape names it.

The same cleanup also turns `\"` into `"`, under every separator and under the
blank split alike, and leaves an unknown escape with its backslash. It is the
same single pass the blank split makes, so the block escape counts under
[Variable value escaping](#variable-value-escaping) stand: the argument escape
wraps what the API carrier already writes.

A space needs no quotes under a separator: `q=a b` arrives as `a b` in `{}`,
`<>`, `[]`, a `|` failover and a `:_:` list. A quote that protects a delimiter
inside a value has to survive to the pass splitting on that delimiter, and the
argument cleanup consumes one quote level:

| Delimiter in the value | Block | Written under `^^~` |
|---|---|---|
| `,` | `{}`, `<>` | `q=\'a,b\'` |
| `,` | `[]` | `q=\\\'a,b\\\'`, the leg split consuming a level |
| `\|` | `[]` | `q=\'x\|y\'` |
| `:_:` | any | none: the enterprise split ignores quotes at every depth |

`originate_split` honours the override, for an ASCII separator only; a line
opening with a non-ASCII one splits on `split_at` whole.
`DialStringTarget::with_argv_separator` names the split. `Variables`, `Endpoint`
and `FlattenedDialString` rendered at such a target are escaped once at the edge
of the argument. Parsed at any `DialStringCarrier::EslApi` target, with a
separator or on blanks, they run that argument split and its cleanup first, and
refuse text that splits into a second argument, leaves a quote open, or hides a
separator inside quotes.

`escape_argument` escapes caller text as one argument of either split. The blank
split skips the character after a backslash and its cleanup reads `\s` as a space,
so on blanks every space is written `\s`, with `\`, `'`, newline, CR and tab
escaped as under a separator. Each character carries its own escape, so a leg
sliced out of the escaped text by `retain` is still one argument. An empty text
is written `''`, since neither split keeps an empty argument at the end of a
line nor the blank split anywhere. On blanks a text opening `^^` follows a
leading `''`, or opening the line it would name the split's separator.
`escape_argument` is `None` at `DialStringCarrier::Dialplan`, which takes no
separator either, because an application's argument is never split.

`with_argv_separator` refuses separators that break the split or its escapes:

- space, which is the blank split;
- `\`, because the cleanup reads an escape before it tests for the delimiter,
  so `^^\` never splits;
- `'`, which pairs with the next separator as quotes;
- non-ASCII, which the switch splits on its first byte;
- ASCII controls: newline and CR end the ESL command, and the rest are
  unprintable or whitespace the switch trims at an argument's edge;
- lowercase `n`, `r`, `t`, `s`: under `^^n`, `\n` is an escaped separator and
  no spelling carries a newline.

It refuses others as policy, though the switch splits on them:

- `^`, `"`, `,`, `|`, `[`, `]`, `{`, `}`, `<`, `>`, `=`, `:`, which a reader
  takes for the dial-string grammar, a block's `^^` separator or quoting;
- uppercase `N`, `R`, `T`, `S`, which under `^^N` still leave `\n` a newline
  but read like the escapes;
- every other letter and digit, which a reader cannot tell from the words it
  separates.

These two lines are the two carriers of [Parse depth](#parse-depth), and the
typed API picks the right escaping for each without being told: `Originate`
renders for the API carrier, `BridgeDialString` for the dialplan one. `Display`
on a bare `Variables` or `Endpoint` means the API carrier, because that is what
this crate mostly drives — so a block rendered on its own and spliced into a
dialplan string by hand is the one case that needs
`display_for(DialStringCarrier::Dialplan)` and will otherwise be escaped one
level too deep. All of these assume the default
[parser revision](#parser-revisions): `Originate::display_with` and
`BridgeDialString::display_with` take another, and a `DialStringTarget` carries
carrier and revision together wherever `display_for` accepts a carrier.
