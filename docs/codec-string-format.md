# FreeSWITCH Codec String Format

Reference for the codec-string grammar, how FreeSWITCH parses and consumes it,
and how an SDP offer maps onto it. Verified against upstream FreeSWITCH `v1.11.1`
(commit `c2c59645f6911a76589e5008c4d73349ded44b65`) across
`switch_loadable_module.c`, `switch_core_media.c`, `switch_utils.c`,
`switch_core.c`, and the codec modules; behaviour confirmed on that tag.
Line numbers drift between releases; treat them as navigation hints.

## Grammar

A codec string is a comma-separated list. Each entry
(`switch_parse_codec_buf`, `switch_loadable_module.c:2746`):

```
[modname.]name[~fmtp][@<n>h|@<n>k][@<n>i][@<n>b][@<n>c]
```

| Part | Meaning |
|---|---|
| `modname.` | pins which module serves the name |
| `name` | the codec's iananame (`PCMU`, `G722`, `AMR-WB`, `opus`) |
| `~fmtp` | explicit format parameters |
| `@<n>h` / `@<n>k` | sample rate |
| `@<n>i` | interval, i.e. ptime in ms |
| `@<n>b` | bitrate in bits/s |
| `@<n>c` | channel count |

Examples:

```
PCMU@8000h@20i@64000b
G722@8000h@20i@64000b
opus@48000h@20i@2c
mod_opus.opus@48000h@20i
AMR-WB~octet-align=1@16000h@20i
```

### The qualifiers are order-free

The parser splits the entry on `@` and classifies each part by scanning it for a
qualifier letter (`:2759-2776`). `PCMU@20i@8000h` and `PCMU@8000h@20i` are
equivalent. A part with no `h`/`k`/`i`/`b`/`c` logs
`Bad syntax for codec string. Missing qualifier` and is ignored — so every
numeric part must carry its letter.

Note that the classification is a substring scan, not a suffix check. A part is
matched against `i`, then `k`/`h`, then `b`, then `c`, in that order, and the
number is taken with `atoi`. This is why an unexpected character inside an entry
can be silently absorbed as a qualifier rather than rejected — see the fmtp
hazards below.

### Parse order within an entry

This order is the source of the hazards further down:

1. split on `@` — everything before the first `@` becomes the name segment
2. classify the remaining parts as qualifiers
3. split the name segment on the first `.` — prefix becomes `modname`
4. split what remains on the first `~` — suffix becomes `fmtp`

So `fmtp` must sit **before** the first `@`, and the `.` split happens
**before** the `~` split.

## How the string is consumed

`absolute_codec_string` takes precedence over `codec_string`
(`switch_core_media.c:2251-2267`); a `codec_string` beginning with `=` is
treated as absolute. The list is split with `switch_separate_string(…, ',', …)`
and capped at `SWITCH_MAX_CODECS` (50, `switch_types.h:595`), then resolved by
`switch_loadable_module_get_codecs_sorted` (`switch_loadable_module.c:2796`).
The effective string is echoed back on the channel as `rtp_use_codec_string`.

Codec name lookup is **case-insensitive**: the codec hash is built with
`switch_core_hash_init_nocase` (`:2120`), the module-name comparison is
`strcasecmp` (`:2557`), and `switch_default_ptime`'s table is also nocase
(`switch_core.c:1903`). `OPUS` and `opus` resolve identically.

### Two-pass implementation matching

For each entry, `get_codecs_sorted` walks the codec's implementations twice.

The first pass (`switch_loadable_module.c:2851-2882`) requires the ptime to equal either the requested
interval or the codec's default, and compares a requested rate against
`imp->actual_samples_per_second`.

The second pass (`:2885-2909`) drops the default-ptime requirement and compares
a requested rate against `crate`, which is `samples_per_second` for G.722 and
`actual_samples_per_second` for everything else (`:2887`).

If neither pass matches, the entry is dropped **with no log at all** — the
enclosing block ends at `:2919` with no else branch. A codec silently missing
from `rtp_use_codec_string` is the only symptom.

This is severe in practice, not theoretical. `mod_amr.c:690-712` registers both
AMR implementations at `20000` µs, so 20 ms is the only packetization AMR has. A
peer offering `a=ptime:40` yields `AMR@8000h@40i`, which matches neither pass and
vanishes — and a check that only verifies the *name* is loadable reports the list
as complete.

### The other silent drop: the interface lookup

Before either pass runs, `switch_loadable_module_get_codec_interface(name, modname)`
must return non-NULL (`switch_loadable_module.c:2849`). It too has no else branch and no log. It returns
NULL when the name is absent from the codec hash — an unloaded module, a typo, or
the interface *display* name (`G.711 ulaw`) used where the iananame (`PCMU`)
belongs — or when a `modname.` prefix matches no module. Note `:2555-2561`
compares the prefix against `node->interface_name`, which is set to the **module**
name at `:237`, not the codec interface's description.

So there are two unlogged drops with different causes: the codec is not loaded at
all, or it is loaded but not at the requested rate, packetization, bitrate or
channel count. Only the first is detectable from a list of codec names.

### The 50-entry cap and its edges

`switch_separate_string` stops at `SWITCH_MAX_CODECS` tokens, and the edges are
sharper than "the tail is truncated":

- **Empty tokens consume slots.** `separate_string_char_delim` assigns
  `array[count++]` per token including empty ones (`switch_utils.c:2782-2783`), so
  `",,PCMU"` uses three of the fifty.
- **Entries 1-49 are safe; entry 50 depends on its shape.** The 50th token holds
  the entire unsplit remainder. `switch_parse_codec_buf` runs its `@` loop over the
  whole buffer *before* the `.` and `~` splits
  (`switch_loadable_module.c:2754-2777`), so a *qualified*
  entry 50 still parses correctly out of that remainder, an *unqualified* one takes
  the remainder as its name and fails the hash lookup, and one carrying `~fmtp`
  folds the remainder into its fmtp.
- **Entries 51+ are always lost**, with no return-value signal and no log.

### Per-entry length cap

Each preference is copied into a 256-byte stack buffer before parsing
(`switch_loadable_module.c:2805`). A longer entry is truncated mid-value and then parsed as if complete.
A long format-parameter string is the realistic way to hit this.

### Duplicate suppression

Entries are de-duplicated against earlier entries on name, interval, rate,
channels and fmtp, all case-insensitively, with unspecified interval and rate
resolved to their defaults first (`switch_loadable_module.c:2811-2847`). Note
this key includes fmtp, so
two entries differing only in format parameters both survive.

Two omissions from that key are worth stating outright, because they are the
opposite of what the grammar suggests. **Bitrate and the `modname.` prefix are
parsed and then never compared** (`:2843-2846`), so `PCMU@64000b` collides with
`PCMU`, and `mod_a.PCMU` collides with `mod_b.PCMU`. And because unspecified
qualifiers are resolved to their defaults *before* comparison, a bare `PCMU` and
a fully-qualified `PCMU@8000h@20i` are the **same entry**. The earlier one wins;
the later is skipped via `goto next_x`.

Consequence for anyone building a list by concatenation: append a fallback list
after the peer's codecs and the duplicates collapse on their own, with the peer's
ordering and qualifiers surviving because they came first.

#### The loop compares against dropped entries too

The inner loop walks entry `x` against every earlier entry `0..x` of the
*original* list, including ones already dropped as duplicates, and skips `x` via
`goto next_x` on the first match. That coincides with comparing against survivors
only, because the key is a pure function of the entry: if `x` duplicates `j` and
`j` was dropped as a duplicate of `i`, then `x` matches `i` directly. The shape is
worth keeping rather than replacing with a set, because it is the shape a future
edge case will follow.

One thing the key cannot see is a deployment's own default. `switch.conf`'s
`<default-ptimes>` overrides the per-codec default ptime
(`switch_core.c:2061-2085`), and nothing exposes it over ESL. A bare `G729` and an
explicit `G729@20i` therefore normalize to the same key here, while on a switch
configured for 40 ms they mean different things. What collapses is always a
qualified variant of a name, never the last entry bearing that name — which is
why a bare name always survives to take whatever the switch's default turns out
to be.

### Stripping qualifiers that equal a default

Removing a qualifier whose value already equals the default is safe **only on an
entry that already matched a loaded implementation**, because the default table
is keyed by name and an implementation registers its own rate.
`AMR-WB@8000h` matches neither pass — both compare the explicit 8000 against
AMR-WB's actual 16000 — and the switch drops it unlogged. But
`switch_default_rate` answers 8000 for that name, so stripping the rate leaves a
bare `AMR-WB`, whose unconstrained rate falls through to whichever implementation
comes first. The entry starts matching, having been dropped a moment earlier.
Every codec whose real rate differs from what the default table returns for its
name has this shape.

Channels `1` is always strippable: it is the C's own initial value.
Bitrate has no default function and nothing to compare against.

### Zero and unset are the same value, under three different rules

`0` is the C's "not set", so `@0h` and an absent `@Nh` mostly mean the same
thing — but the three places that read a qualifier resolve it differently, and
conflating them changes which codec the switch picks.

| Rule | Where | Unset and `0` become |
|---|---|---|
| dedup key | `switch_loadable_module.c:2816-2826` | the per-name default (rate, ptime) or `1` (channels) |
| match constraint | `:2851-2909` | unconstrained — every comparison is guarded by `if (rate && …)` |
| default-stripping | this crate, on an already-matched entry | absent, so the switch resolves them at match time |

Channels is the exception in the middle row: the preference-parsing variable is
seeded at `channels = 1`, not `0` like rate, interval and bitrate
(`switch_loadable_module.c:2806`), so an *absent* `@Nc` still constrains to mono
and only an explicit `@0c` disables the check. A dedup key that folds absent and
`0` together is right for dedup and wrong here.

## G.722 has two sampling rates

G.722 is advertised in SDP at 8000 Hz (an RFC 3551 quirk) but runs at 16 kHz.
FreeSWITCH registers it with `samples_per_second = 8000` and
`actual_samples_per_second = 16000` (`mod_spandsp_codecs.c:842-858`).

Consequences for `G722@8000h`:

- the first pass compares 8000 against `actual_samples_per_second` (16000) and
  can never match
- the second pass compares against `crate` (8000) and matches

The second pass still honours an explicit interval, but with no `@<n>i` it takes
whichever implementation is first in the list rather than the 20 ms default. So:

```
G722@8000h@20i     correct — 20 ms implementation
G722@8000h         matches, but the packetization is whatever is first
G722              matches at the default rate and ptime
G722@16000h@20i    also matches, via the first pass
```

**Always emit a ptime alongside a rate for G.722.**

Two consequences follow for anything reasoning about G.722 entries. Modelling a
loaded implementation with a single rate field cannot express which pass would
fire, so a rate comparison against a G.722 implementation answers nothing and is
better skipped. And default-stripping is unsafe here even on a matched entry:
strip `@8000h@20i` and the second pass, which applies no default-ptime
preference, takes whichever implementation is first in registration order rather
than the 20 ms one.

## Format parameters and delimiter collisions

`~fmtp` values legitimately contain characters the surrounding grammar uses.
The separator layer runs first (`separate_string_char_delim`,
`switch_utils.c:2768`, then `cleanup_separated_string`, `:2702`), and
`switch_parse_codec_buf` runs on the already-cleaned token.

### `,` — escape it

`\,` survives: the split honours `\` as an escape (`:2789`) and cleanup
unescapes it (`:2720-2722`). Required in practice — AMR `mode-set=0,1,2` is
common in carrier SDP.

```
AMR-WB~mode-set=0\,2\,4@16000h@20i
```

`^^` at the start of the whole string re-delimits it (`:2877-2884`), which is a
caller-level alternative:

```
^^:AMR-WB~mode-set=0,2,4:PCMU@8000h@20i
```

### `\` and `'` — escape them

A lone `\` swallows the next character's delimiter-ness, and `\n`/`\s`/`\t`/`\r`
become control characters or a space at cleanup (`:2632-2653`, `:2720`). Emit
`\\`.

A lone `'` toggles quote state, and the lookahead for a matching quote scans
across entry boundaries (`:2728`, `:2791`), so it can swallow the commas
between entries. Emit `\'`.

### `@` — unrepresentable

The `@` split precedes the `~` split, so an `@` inside fmtp is read as a
qualifier. There is no escape: cleanup has already consumed backslashes by then.
The tail is not merely truncated — it is scanned for `i`/`k`/`h`/`b`/`c` and
`atoi`'d into a qualifier, usually with **no log**
(`switch_loadable_module.c:2759-2776`).

### `.` — unrepresentable without a modname

The `.` split also precedes the `~` split, so the first dot in an unprefixed
entry becomes the modname boundary. The mangled name misses the hash and the
codec is dropped with no log. With an explicit `modname.` prefix the first dot
is the module's and later dots survive.

This is not hypothetical: EVS advertises `br=13.2-24.4;bw=nb-swb;…`. Any codec
with a dotted format parameter hits it.

### Delivery-path escaping

Escaping correctly for this grammar is not enough — the layer that sets the
channel variable may consume the escape first.

| Path | Behaviour |
|---|---|
| `uuid_setvar` | splits on space, honours `'` quotes and `\` escapes (`switch_utils.c:2841-2845`); `\,` is not an escape under a space delimiter so it passes through (`:2720`). **Single-quote the value** — unquoted it truncates at the first space, and most AMR fmtp contains `; `. |
| inline originate `{var=…}` | `switch_event_create_brackets` (`switch_event.c:1665`) splits the variable list through the same helper, on the delimiter its caller passes — `,` for originate (`:1732`) — spending the escape when the variable is stored. Needs `\\,`. |

## Mapping an SDP offer onto the string

FreeSWITCH does this itself in `switch_core_media_set_r_sdp_codec_string`
(`switch_core_media.c:13543`), formatting each entry with `add_audio_codec`
(`:13483`). The output goes to the `ep_codec_string` channel variable.

Emitted form is `modname.encoding` plus rate, ptime and bitrate/channels
(`:13538`). The **audio** path emits no `~fmtp`; only the video path does
(`:13767`, `:13808`).

### Walk

- session-level `a=ptime` is read first as a default (`:13594-13603`), then
  overridden per media section (`:13617-13619`)
- m-lines with port 0 contribute nothing (`:13608`, `:13650`). They are still
  parsed here and reachable through `SdpCodecs::sections()` — a held stream is
  what a reader most needs
- any `m=image` with a nonzero port contributes the literal `t38`, regardless of
  proto or fmt (`:13650-13651`)
- the walk reads `m->m_rtpmaps`, which libsofia fills only for the protos
  `sdp_media_transport` maps to an RTP transport (`sdp_media_has_rtp`), so a
  section on any other proto arrives empty whatever its media type
- an inbound leg, or one with `ep_codec_prefer_sdp` set, walks the SDP's rtpmaps
  in the outer loop so SDP order wins; an outbound leg walks the local
  preference list outer so local order wins (`:13674-13731`)
- `already_did[]` is *checked* in the audio branches but only *set* in the video
  ones (`:13677`, `:13711` vs `:13772`, `:13813`), so audio duplicate
  suppression is a no-op here

### ptime and bitrate resolution

This is a sequential overwrite, not a first-match-wins chain
(`:13483-13520`) — later steps beat earlier ones:

1. `codec_ms` = the resolved `a=ptime` (media level, else session level)
2. if unset, the per-codec default from `switch_default_ptime`
   (`switch_core.c:2022`) — 30 for `ilbc`/`isac`/`G723`, else 20
3. `G723` with no `a=ptime` ⇒ 30 (`switch_core_media.c:13499`)
4. bitrate = `switch_known_bitrate(pt)` (`switch_utils.h:478-492`) — static
   payload types only
5. **no fmtp** and `ilbc` ⇒ ptime 30, bitrate 13330; `isac` ⇒ ptime 30, bitrate
   32000 (`switch_core_media.c:13503-13510`) — these override an explicit
   `a=ptime`
6. **fmtp present** ⇒ `switch_core_codec_parse_fmtp`
   (`switch_core_codec.c:607`); anything it yields overrides the above
   (`switch_core_media.c:13512-13518`)

So `a=ptime:20` plus opus `fmtp …;ptime=40` produces `@40i`, not `@20i`.

Stock modules that feed those two fields:

| Module | Parameter | Sets |
|---|---|---|
| mod_opus | `ptime=` | ptime (`mod_opus.c:287`) |
| mod_ilbc | `mode=` | ptime, default 30 when fmtp present but no `mode=` (`mod_ilbc.c:43-62`) |
| mod_siren | `bitrate=` | bitrate (`mod_siren.c:78`) |

mod_silk registers a parser but its bitrate assignment is commented out
(`mod_silk.c:152`), so it is a no-op. Neither AMR module registers one.

### Bitrate and channels collide

`add_audio_codec` formats bitrate and channels into the **same buffer**, so
`@<n>c` overwrites `@<n>b` when channels > 1
(`switch_core_media.c:13530-13536`). Upstream
therefore never emits both. The consuming grammar accepts both fine.

### Static payload types

`m=audio 5004 RTP/AVP 0 8 18` with no `a=rtpmap` is common. sofia-sip
auto-populates rtpmaps for well-known static payload types, and FreeSWITCH
relies on that, so a converter needs sofia's table, `sdp_rtpmap_well_known` in
`sdp_parse.c`, to name them. It is RFC 3551's PT 0–34 plus three numbers RFC 3551
reserves: 1 (`1016`), 2 (`G721`) and 19 (`CN`, comfort noise like 13).

### telephone-event and CN are not codecs

DTMF and comfort noise are negotiated outside the codec string, through
`smh->mparams->te` and `cng_pt` (`switch_core_media.c:9924-9960` in
`generate_m()` at `:9730`).
No loadable codec is named `telephone-event` or `CN`, so neither belongs in a
codec string. They are excluded from it and retained as data instead.

Reading an offer, the switch keeps only a payload type and a clock rate for each,
picking the entry whose rate matches the negotiated codec's advertised rate
(`switch_core_media.c:5805`, `:5816`) and forcing the retained rate to 8000 Hz
when it does not
match (`:5829-5834`). It never reads their `a=fmtp`: both leave the rtpmap walk
at `:5447`/`:5456`, ahead of `switch_core_codec_parse_fmtp` at `:5493`, and a
generated offer synthesizes the DTMF digit range from `NDLB_line_flash_16`
instead (`:10653-10659`).

## What the codec string cannot do

### `~fmtp` does not reach a generated audio offer

`~fmtp` is parsed out (`switch_loadable_module.c:2786-2791`), copied into the
fmtp array (`:2876-2878`), and passed along as `smh->fmtp[]`
(`switch_core_media.c:2285`). That array is read in only two places:

- `:11140-11143` — the **video** SDP branch, which checks it first. Video fmtp
  pinning works.
- `:10406` — as a **match key only**, looking a codec up in another session's
  payload map. On success the value stored is `orig_fmtp`, the *partner's* fmtp
  (`:10412-10414`), and the whole branch is guarded by `if (orig_session …)`.

The audio `a=fmtp` comes from a different array, `smh->fmtps[]` (`:9902-9904`,
emitted at `:9918-9920`), which is populated only in that partner-dependent
branch. Absent a match, the fmtp is the implementation's default. The one other
override hook is dead code: `map` is declared NULL at `:10238` and its only
populating call is commented out at `:10499-10501`.

**A leg with no bridged partner at INVITE time — an originate, for instance —
therefore always offers the codec module's default audio fmtp**, whatever the
codec string said.

To force it, use the `rtp_force_audio_fmtp` channel variable (`:10237`, applied
at `:10459-10460`, emitted at `:10624-10625`). It applies to the chosen primary
audio payload map only — one fmtp per offer, not per codec.

### Two implementations under one name are not selectable

`mod_amrwb.c:615-634` registers `AMR-WB / Octet Aligned` (PT 100) and
`AMR-WB / Bandwidth Efficient` (PT 110) as two interfaces sharing the iananame
`AMR-WB`, differing in payload type and default fmtp. `mod_amr.c:685-719` is the
same shape. The `show codec` strings are interface descriptions, not
codec-string names.

Neither is individually selectable:

- codec-hash nodes are **prepended** (`switch_loadable_module.c:238-242`) and a
  bare name takes the head (`:2562-2564`), which is the last-registered
  interface — Bandwidth Efficient
- `modname.` cannot disambiguate: both nodes carry
  `interface_name = "mod_amrwb"` (`:237`), and the lookup returns the first name
  match (`:2556-2561`)
- fmtp is not a selection key — it is only copied and used for de-duplication

So `AMR-WB~octet-align=1` selects the BE implementation and offers the BE
default fmtp.

Because those two interfaces differ only in fmtp, an SDP offer listing both
yields two entries that are byte-identical once fmtp emission is off — and the
duplicate suppression above then collapses them. Turn fmtp emission on and they
stay distinct, FreeSWITCH assigns two dynamic payload types
(`switch_core_media.c:10416`), and both `a=rtpmap` lines carry the module default
anyway on a partnerless leg, for the reason given above. Suppressing audio fmtp
is what makes the pair collapse cleanly.

### `t38` is not a codec

`t38` is the literal FreeSWITCH writes into `ep_codec_string` for a T.38 m-line
(`switch_core_media.c:13651`), but no codec interface by that name exists. It
therefore fails the interface lookup like any unloaded codec. A list filtered
against loaded codec names will strip it unless `t38` is deliberately allowed
through.

## What the switch will not tell you

Nothing exposes the loaded implementation table over ESL, so the two silent drops
above cannot be predicted from the switch itself. Verified against a running
switch, not only from source:

- `show codec` is a SQL select over the `interfaces` table
  (`mod_commands.c:5780`) returning `type,name,ikey`. Its `name` is the interface
  **description** (`G.711 ulaw`, `AMR / Octet Aligned`, `OPUS (STANDARD)`), which
  has no mechanical mapping to the iananame a codec string uses. No rate, ptime,
  bitrate or channel column exists in that table. `show interfaces` has the same
  three columns.
- The only codec APIs are `uuid_codec_debug`, `uuid_codec_param` and
  `uuid_media_reneg`, all per-call.
- `show file` can list loaded audio iananames indirectly, via
  `mod_native_file.c:182-191`, but only if that module is loaded; it snapshots at
  its own load time and omits VBR and video codecs.
- A `null`/loopback channel reveals nothing: `absolute_codec_string` reads back
  byte-identical, keeping both a nonexistent codec and an unloadable qualifier
  combination, while `read_codec` stays `L16` and `ep_codec_string` is `_undef_`.
  mod_loopback never resolves the codec string.
- A generated local SDP carries one `a=ptime` per m-line, so it cannot express
  per-codec packetization even where it can be captured.

The complete per-implementation tuple is emitted only as a `NOTICE` at module
load (`switch_loadable_module.c:222-232`, `Adding Codec %s %d %s %dhz %dms %dch
%dbps`), never on a queryable interface. Deciding what a switch can load is
therefore the operator's knowledge to supply, not something a client can derive.

## Related channel variables

| Variable | Role |
|---|---|
| `absolute_codec_string` | pins the list, overriding everything |
| `codec_string` | preference list; leading `=` makes it absolute |
| `ep_codec_string` | what FreeSWITCH derived from the peer's SDP |
| `rtp_use_codec_string` | the string actually in effect |
| `rtp_force_audio_fmtp` | forces the audio fmtp for the primary payload |
| `switch_r_sdp` / `switch_l_sdp` | remote and local SDP (`switch_types.h:197-198`) |
| `ep_codec_prefer_sdp` | make an outbound leg honour SDP order |
