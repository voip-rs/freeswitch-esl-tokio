# Codec strings and SDP (`sdp` feature)

Off by default -- add `features = ["sdp"]` to the `freeswitch-esl-tokio` dependency line to reach the `sdp` module.

`CodecString` models the FreeSWITCH codec-string grammar in both directions, so a codec string can be read back, rewritten, or checked rather than assembled by concatenation. `SdpCodecs` turns a peer's SDP offer into a typed codec list. The crate supplies the grammar's own operations — append, deduplicate, filter — and leaves the policy to you: there is no merge, because intersecting an offer while *generating* one is not something FreeSWITCH does, and it silently narrows a list that some interfaces require to stay complete. The block below compiles only with the `sdp` feature enabled.

```rust,ignore
# use freeswitch_esl_tokio::EslClient;
use freeswitch_esl_tokio::sdp::{
    CodecImplementation, CodecString, CodecStringOptions, SdpCodecs,
};
use freeswitch_esl_tokio::{ChannelVariable, UuidSetVar};
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let (client, _events) = EslClient::connect("localhost", 8021, "ClueCon").await?;
# let uuid = "11111111-1111-1111-1111-111111111111";
# let remote_sdp = "v=0";

// The peer's offer, e.g. from the switch_r_sdp channel variable.
let offer = SdpCodecs::parse(remote_sdp)?;
let mut codecs = offer.audio_codec_string(&CodecStringOptions::audio(), None)?;

// Append what must always be offered. Nothing from the peer is removed or
// reordered; these land at the tail as a floor.
codecs.extend_from(&"PCMU,PCMA".parse::<CodecString>()?);

// FreeSWITCH's own dedup key, so a bare PCMU and PCMU@8000h@20i collapse.
// The first occurrence wins, which is why concatenation order is the policy.
codecs.dedup();

// What this switch has loaded. No ESL API exposes the real table, so this is
// yours to supply; entries matching nothing come back rather than vanishing.
let loaded = [CodecImplementation::new("PCMU"), CodecImplementation::new("PCMA")];
for dropped in codecs.retain_available(&loaded) {
    eprintln!("not offering {dropped}");
}

// UuidSetVar quotes the value for uuid_setvar's blank split, and AMR format
// parameters routinely contain "; ".
let set = UuidSetVar::new(uuid, ChannelVariable::AbsoluteCodecString.to_string(), codecs.to_string())?;
client.api(&set.to_string()).await?.api_result()?;
# Ok(())
# }
```

Ordering is whatever you concatenate, since `dedup()` keeps the first occurrence: offer-then-backup preserves the peer's preference and its qualifiers, backup-then-offer preserves yours.

## What the switch read, or what the peer wrote

`SdpCodecs::parse` reads an offer the way the switch's SIP stack does: a refusal or an early stop is a warning, not an error, and a port-0 section (a held or declined stream) names only the static payload types of its format list, because the switch reads none of its `a=` lines. That is the view to build a codec string from. A reader analyzing SDP taken from logs or traces, which wants the codecs the peer put on a held stream, opts in:

```rust,ignore
use freeswitch_esl_tokio::sdp::{SdpCodecs, SdpParseOptions};

let options = SdpParseOptions::default().with_port_zero_attributes(true);
let offer = SdpCodecs::parse_with_options(remote_sdp, &options)?;
for section in offer.sections().iter().filter(|s| s.port() == 0) {
    // codecs the peer named on the held stream; the switch ignored them
}
```

The option reaches only port-0 sections, so the codec strings and every live section come out the same either way.

See [codec-string-format.md](../codec-string-format.md) for the grammar, the characters a format-parameter value cannot carry, the G.722 rate/packetization trap, and the two places FreeSWITCH drops a codec-string entry without logging it.
