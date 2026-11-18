# Typed ESL Wrapper Crate Design

This document outlines the architecture for a future typed wrapper crate that
builds on `freeswitch-esl-tokio` to provide compile-checked ESL usage.

## Goal

Replace stringly-typed ESL interactions with fully typed Rust APIs:

```rust
// Current (transport-layer only):
client.api(&UuidKill::with_cause(id, "NORMAL_CLEARING").to_string()).await?;

// Future wrapper:
client.channel(&uuid).kill(HangupCause::NormalClearing).await?;
```

## Architectural Boundary

`freeswitch-esl-tokio` (this crate) is **transport only**: wire format, framing,
event delivery, and raw `api()`/`bgapi()`. It does not model a command's output
schema.

The line is not "no per-command reply handling", which the crate has never
managed to hold. A reply whose shape misleads a caller who reads it generically
is a protocol fact, and this crate is where it is known: `parse_channel_dump`
reads a `uuid_dump` body as the `CHANNEL_DATA` event it is rather than a second
line format, and `getvar` collapses the three spellings of an unset variable
that `getvar_raw` hands back as values. Both exist because the generic
accessor returns a plausible wrong answer, not because the command deserved a
typed API.

What stays out is the schema behind a reply that is already unambiguous —
`status`, `sofia status`, `show` rows. Those are the wrapper's, and
[design-rationale.md](design-rationale.md) records why `show` in particular is
not modelled anywhere.

The wrapper crate depends on this crate and adds:

- Typed command methods that send commands and parse responses
- Value enums (`HangupCause`, `ApplicationName`) for command parameters
- Response parsers (`StatusResponse`, `SofiaProfile`, `ShowChannels`)
- Handle objects (`ChannelHandle`, `ConferenceHandle`) with scoped methods

## How This Crate Enables the Wrapper

### VariableName trait extensibility

The wrapper can define its own variable enums for module-specific variables:

```rust
// In wrapper crate:
define_header_enum! {
    error_type: ParseConferenceVariableError,
    pub enum ConferenceVariable {
        ConferenceName => "conference_name",
        ConferenceMemberFlags => "conference_member_flags",
        // ...
    }
}

impl VariableName for ConferenceVariable {
    fn as_str(&self) -> &str { ConferenceVariable::as_str(self) }
}

// Works with EslEvent::variable() from freeswitch-esl-tokio:
event.variable(ConferenceVariable::ConferenceName)
```

### Command builders use Display

`UuidKill`, `Originate`, `AppCommand`, etc. all produce strings via `Display`.
The wrapper wraps them, not replaces them. A typed `HangupCause` enum converts
to the string the builder needs:

```rust
impl fmt::Display for HangupCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HangupCause::NormalClearing => write!(f, "NORMAL_CLEARING"),
            // ...
        }
    }
}
```

### No circular dependency

`commands/` doesn't import `connection.rs`. The wrapper depends on both the
command builders and the transport layer independently.

## Planned Typed Enums

`HangupCause` was the first of these and landed in `freeswitch-types`
(`channel.rs`), which settled the question the rest of this section asks: a
value enum belongs here when a builder already takes the string, and only the
parser that reads a reply's shape belongs to the wrapper.

### ApplicationName (dptools)

For type-safe `AppCommand` construction:

```rust
pub enum ApplicationName {
    Answer,
    Hangup,
    Bridge,
    Playback,
    Set,
    Park,
    Transfer,
    // ...
}
```

### Typed Response Parsers

These belong in the wrapper crate since they depend on parsing strategy
(XML output, regex, etc.) and may pull in heavier dependencies:

- `StatusResponse` -- from `api status`
- `SofiaProfile` -- from `api sofia status`
- `OriginateResult` -- parsed originate response with UUID extraction

`show` rows are the exception that is not merely deferred: they are refused
here and in a wrapper alike, for the reason
[design-rationale.md](design-rationale.md) gives. A caller wanting them parses
them.

## Wrapper API Sketch

```rust
pub struct TypedClient {
    inner: EslClient,
}

impl TypedClient {
    pub fn channel(&self, uuid: &str) -> ChannelHandle<'_> {
        ChannelHandle { client: &self.inner, uuid: uuid.to_string() }
    }

    pub async fn status(&self) -> Result<StatusResponse, EslError> {
        let resp = self.inner.api("status").await?;
        StatusResponse::parse(resp.body().unwrap_or(""))
    }

    pub async fn originate(&self, cmd: &Originate) -> Result<OriginateResult, EslError> {
        let resp = self.inner.api(&cmd.to_string()).await?;
        OriginateResult::parse(&resp)
    }
}

pub struct ChannelHandle<'a> {
    client: &'a EslClient,
    uuid: String,
}

impl ChannelHandle<'_> {
    pub async fn kill(&self, cause: HangupCause) -> Result<EslResponse, EslError> {
        let cmd = UuidKill::with_cause(self.uuid.clone(), cause.to_string());
        self.client.api(&cmd.to_string()).await
    }

    pub async fn set_var(&self, name: impl VariableName, value: &str) -> Result<EslResponse, EslError> {
        let cmd = UuidSetVar::new(self.uuid.clone(), name.as_str(), value)?;
        self.client.api(&cmd.to_string()).await
    }

    pub async fn bridge(&self, other: &str) -> Result<EslResponse, EslError> {
        let cmd = UuidBridge { uuid: self.uuid.clone(), other: other.into() };
        self.client.api(&cmd.to_string()).await
    }
}
```

## String-Typed Fields to Migrate

Current command builders that accept raw strings where typed enums would help:

| Field | Current Type | Future Type |
|-------|-------------|-------------|
| `UuidKill::cause` | `Option<String>` | `Option<HangupCause>` |
| `AppCommand::hangup(cause)` | `Option<&str>` | `Option<HangupCause>` |
| `EslCommand::Execute { app }` | `String` | `ApplicationName` or `impl Display` |
| `ErrorEndpoint::cause` | `String` | `HangupCause` |

These can be migrated with `impl Display` or `impl Into<String>` generics to
maintain backward compatibility.
