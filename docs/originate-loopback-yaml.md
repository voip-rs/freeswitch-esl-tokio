# Originate to a loopback endpoint from YAML

`Originate` implements `Serialize`/`Deserialize`, so a complete originate
command can live in a config file. This walks through two of them: one that
exercises every field the serde representation offers, and one that drives a
loopback pair through a *bowout*.

Both YAML files are real, checked-in, and shared by the examples and the
tests, so the wire strings quoted below cannot drift from what the code emits:

| File | Runner | Purpose |
| --- | --- | --- |
| [`examples/originate_loopback.yaml`](../examples/originate_loopback.yaml) | [`examples/originate_loopback_yaml.rs`](../examples/originate_loopback_yaml.rs) | Every serde field; reads variables back off both legs |
| [`examples/originate_loopback_bowout.yaml`](../examples/originate_loopback_bowout.yaml) | [`examples/originate_loopback_bowout.rs`](../examples/originate_loopback_bowout.rs) | Bowout |
| [`examples/originate_loopback_scoped_vars.yaml`](../examples/originate_loopback_scoped_vars.yaml) | `live_freeswitch.rs` | Per-leg variables |

```sh
ESL_PORT=8022 FREESWITCH_VERSION=1.10.12 cargo run --example originate_loopback_yaml
ESL_PORT=8022 FREESWITCH_VERSION=1.10.12 cargo run --example originate_loopback_bowout
cargo test --test live_freeswitch -- --ignored
```

`FREESWITCH_VERSION` is the release the switch runs, which picks the parser
revision the originate is escaped for; a development build names one with
`ESL_BLOCK_PARSE=pair_split_cleans` instead.

The live tests need `mod_loopback`, `mod_dptools`, and extension `9199` in
context `test` (`answer` then `echo`).

## The YAML shape

```yaml
endpoint: !loopback
  extension: "9199"
  context: test
  variables:
    scope: channel
    vars:
      origination_caller_id_name: "Sales Desk"
      origination_caller_id_number: "5550100"
      ignore_early_media: "true"
      customer_id: "CUST-42"
      tenant: "acme"
      sip_h_X-Ticket: "T-1001, urgent"

application:
  name: park

dialplan: xml
context: test
cid_name: "Fallback CID"
cid_num: "5550199"
timeout_secs: 30
```

which deserializes and prints as:

```
originate [origination_caller_id_name='Sales Desk',origination_caller_id_number=5550100,ignore_early_media=true,customer_id=CUST-42,tenant=acme,sip_h_X-Ticket='T-1001\, urgent']loopback/9199/test &park() XML test 'Fallback CID' 5550199 30
```

Three things about the encoding are easy to get wrong.

**`endpoint` needs a YAML tag.** `Endpoint` is an externally tagged enum, and
`yaml_serde` spells those `!variant`, not as a nested mapping. `endpoint:
!loopback` works; `endpoint:` followed by an indented `loopback:` key is
rejected with `invalid type: map, expected a YAML tag starting with '!'`.

**The target is flattened.** Exactly one of `extension`, `application`, or
`inline_applications` sits at the top level, as a sibling of `endpoint`, not
under a `target:` key.

**`variables` takes two forms.** A bare mapping is `default` scope; the
`scope`/`vars` pair is required only for the other two scopes.

```yaml
variables:                    variables:
  customer_id: "CUST-42"        scope: channel
  tenant: "acme"                vars:
                                  customer_id: "CUST-42"
# -> {customer_id=CUST-42,...}    tenant: "acme"
                              # -> [customer_id=CUST-42,...]
```

The `scope`/`vars` form also takes an optional `separator`, which renders the
block in the `^^X` form described in
[dial-string-format.md](dial-string-format.md#x-block-separator) — the only way
a value can carry a comma when it arrives by `${...}` expansion. It is refused
at load if it cannot delimit that scope's block or if a value already contains
it. Absent, the block separates on a comma and no key is emitted.

```yaml
variables:
  scope: default
  vars:
    codecs: "PCMA,PCMU,G729"
  separator: "|"
# -> {^^|codecs=PCMA,PCMU,G729}
```

## Fields

`endpoint` and one target key are required; everything else is optional and
omitted from the wire string when absent.

| Key | Wire position | Notes |
| --- | --- | --- |
| `endpoint` | 1 | `!loopback` takes `extension`, optional `context`, optional `variables` |
| `extension` | 2 | Routes through the dialplan engine |
| `application` | 2 | `{name, args}` becomes `&name(args)` |
| `inline_applications` | 2 | List of `{name, args}`, becomes `name:args,name:args` |
| `dialplan` | 3 | `xml` or `inline` |
| `context` | 4 | Dialplan context for the originated channel |
| `cid_name` | 5 | |
| `cid_num` | 6 | |
| `timeout_secs` | 7 | Whole seconds |

`extension` with `dialplan: inline` is rejected during deserialization, as is
an empty `inline_applications` list.

Because FreeSWITCH parses arguments 3 through 7 by position, setting a late
one forces the earlier ones to be emitted. `timeout_secs` on its own yields
`... &park() undef undef undef undef 30`: `undef` is the FreeSWITCH keyword for
an omitted positional argument, which the switch reads as `XML`, `default` and
no caller id.

## The two legs

`loopback/9199/test` is not one channel but two:

```
loopback/9199-a  (A leg)  <-- the UUID `originate` returns; runs &park()
loopback/9199-b  (B leg)  <-- routed to extension 9199 in context `test`
```

They are cross-linked by `other_loopback_leg_uuid`, and each carries
`loopback_leg` set to `A` or `B`. The example uses the former to find the B
leg from the UUID the originate handed back.

### Which leg gets a variable

`switch_ivr_originate` builds **one `var_event` per outgoing channel**, merging
the global `{}` block with that endpoint's `[]` block (`[]` wins on conflict).
It hands that merged event to the endpoint module and then applies it to the
new channel. mod_loopback keeps a copy of it as `__loopback_vars__` and replays
the whole thing onto the B leg when it links the pair.

The consequence: **no bracket block can address one loopback leg**. Both legs
see everything in the originate's variable block, whether it was written as
`{}` or `[]`. That is deliberate, since the point of the B leg is to run a
dialplan that reads what the originator set.

`loopback_export` names A-leg variables to copy onto the B leg at link time,
but every route a caller has for putting a variable on the A leg goes through
`var_event`, which already crosses: the bracket blocks, `export_vars`, and the
`nolocal:` prefix all merge into it (`switch_channel_process_export`). What is
left for `loopback_export` to carry is the handful of variables mod_loopback
itself sets on the A leg, such as `loopback_from_uuid`. For anything you wrote
in the block, listing it changes nothing.

Caller ID is the one asymmetry worth knowing: the **B leg** is what your
dialplan sees, and that is where the caller ID lands. Both the positional
`cid_name`/`cid_num` and the `origination_caller_id_name`/`_number` variables
target it, and the variables win. The example YAML sets them to different
values on purpose, and the live test asserts the B leg reports `Sales Desk` /
`5550100` rather than `Fallback CID` / `5550199`. The A leg keeps a synthetic
outbound caller profile; do not read caller ID off it.

`loopback_initial_codec` has no effect when set in an originate variable
block. mod_loopback reads it while initialising the codec, which happens
before any of the block's variables have been applied to the channel, so the
pair always negotiates its default `L16`.

### Keeping a variable off a leg

Since the bracket block cannot split the loopback pair, per-leg variables come
from *nesting*: each dial string carries its own block, and each `originate` or
`bridge` builds its own `var_event`. Channel variables do not cross a bridge on
their own.

[`examples/originate_loopback_scoped_vars.yaml`](../examples/originate_loopback_scoped_vars.yaml):

```yaml
endpoint: !loopback
  extension: "9199"
  context: test
  variables:
    leg_a_only: "outer"
application:
  name: bridge
  args: "{leg_b_only=inner}null/far"
```

```
originate {leg_a_only=outer}loopback/9199/test &bridge({leg_b_only=inner}null/far)
```

| | `loopback/9199-a` | `loopback/9199-b` | `null/far` |
| --- | --- | --- | --- |
| `leg_a_only` | yes | yes | no |
| `leg_b_only` | no | no | yes |

So to keep a variable away from the far end, set it in the originate block; to
send one only to the far end, put it in the bridge's dial string. This is the
answer to "I do not want the provider to see this variable".

The second isolation mechanism is `[]` across **sibling endpoints** in one dial
string. `{}` merges into every channel's `var_event`; `[]` merges only into the
endpoint it prefixes:

```
originate {common=1}[secret=a]sofia/gateway/gw_a/1234,[secret=b]sofia/gateway/gw_b/1234 &park()
```

`gw_a`'s channel sees `secret=a`, `gw_b`'s sees `secret=b`, and both see
`common=1`. That is the only situation in which the choice of scope changes
behaviour.

`Originate` holds a single `Endpoint`, so it cannot express a multi-endpoint
dial string, and `scope: channel` versus a bare mapping makes no difference to
what FreeSWITCH does with these YAML files. Use
[`BridgeDialString`](dial-string-format.md) for the multi-endpoint forms.

### Escaping

`Variables` escapes values on the way out and FreeSWITCH unescapes them on the
way in, so a value round-trips verbatim:

| In the value | On the wire |
| --- | --- |
| `,` | `\,` |
| `'` | `\\\\\\\'` through `originate`, `\\\\\\'` through a dialplan application |
| `\` | eight backslashes; thirty-two in `[]` scope |
| `\|` | `\\|` in `[]` scope, plain elsewhere |
| any space | whole value wrapped in `'...'` |

A `'` cannot be carried in `[]` scope at all and is refused at load.

`sip_h_X-Ticket: "T-1001, urgent"` is a comma and a space at once. It becomes
`sip_h_X-Ticket='T-1001\, urgent'` on the wire, and the channel variable on
both legs reads back as `T-1001, urgent`. The quote and backslash counts are
explained in [dial-string-format.md](dial-string-format.md#parse-depth).

### Scopes

`channel` emits `[]`, `default` emits `{}`, `enterprise` emits `<>`. For a
single-endpoint originate all three behave identically, as described above.
The example uses `channel` to show the `scope`/`vars` encoding, not because it
changes anything on the wire's receiving end.

## Bowout

This section covers the frame-count path, which is what the example below exercises.
For the other resignation path, what each one leaves behind on the surviving channel,
and which fields lie afterwards, see
[loopback-bowout.md](loopback-bowout.md) — read it before writing any consumer that
acts on a resignation.

A loopback pair that finds itself in the middle of a call can splice its two
neighbours together and get out of the audio path. mod_loopback calls
`switch_ivr_uuid_bridge()` on the two real channels and both loopback legs
hang up:

```
before:  null/nearend = loopback-a : loopback-b = null/farend
after:   null/nearend = null/farend
```

It happens only when all of these hold:

- both loopback legs are bridged to a non-loopback channel
- both loopback legs are answered
- neither leg has `loopback_bowout` set to `false` (unset means yes)

The YAML arranges the two bridges without needing a dialplan:

```yaml
endpoint: !loopback
  extension: "app=bridge:null/farend"
  variables:
    loopback_bowout: "true"

application:
  name: bridge
  args: "null/nearend"
```

```
originate {loopback_bowout=true}loopback/app=bridge:null/farend &bridge(null/nearend)
```

`app=<application>[:<args>]` is a mod_loopback destination form: the B leg runs
that application directly instead of doing a dialplan lookup, which gives it
its bridge. The `&bridge(null/nearend)` target gives the A leg its own. `null/`
is a separate endpoint interface, so both count as non-loopback.

### Observing it

Just before it bridges the survivors, mod_loopback stamps both loopback legs
with `loopback_hangup_cause` and `loopback_bowout_other_uuid`, then lets them
hang up. So the signal is two `CHANNEL_HANGUP_COMPLETE` events carrying those
variables, followed by a `CHANNEL_BRIDGE` with a real channel on both sides:

```
resigned loopback/bridge-a    -> real channel fe091dbc-...
resigned loopback/bridge-b    -> real channel e6455adc-...
bridged  null/nearend = null/farend
```

`HeaderLookup::loopback_resignation()` reads both variables, and the channel's
own name says whether they describe it:

```rust,ignore
let is_loopback = evt
    .channel_name()
    .and_then(LoopbackChannelName::parse)
    .is_some();

match evt.loopback_resignation() {
    // The leg is gone but its call is not. Track other_uuid instead.
    Some(r) if is_loopback => reanchor(r.other_uuid()),
    // A real channel carrying the marker inherited it from the leg that
    // resigned onto it. It is up, and tearing it down ends a live call.
    Some(_) => {}
    None => teardown(),
}
```

**Match on presence, never on the cause value.** mod_loopback writes a
different token depending on which path resigned -- the two triggers below
do not agree -- and both mean the leg is gone while the call continues.
Testing for one of them reports every call that took the other path as a
teardown: a live conversation torn down while the parties are still talking,
or a dead one tracked until whatever timeout eventually reaps it. That is
what the accessor exists to prevent; it returns `Some` for any token, and
`r.cause()` answers which path separately, for logging.

Do not key on the `loopback::direct` CUSTOM event. mod_loopback only fires it
when `fire-bowout-on-bridge` is enabled in `loopback.conf.xml`, which is off by
default, and an absent `loopback.conf.xml` leaves it off.

`loopback_bowout_on_execute` is the other trigger. Set on a loopback leg, it
bows out as soon as that leg executes an application, masquerading the caller
extension onto the real channel behind its partner instead of waiting for
audio to flow. That path fires a `loopback::bowout` CUSTOM event
unconditionally and hangs the leg up with the cause from `bowout-hangup-cause`.
