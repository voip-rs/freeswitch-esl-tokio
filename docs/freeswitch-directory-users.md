# FreeSWITCH Directory User Configuration for ESL

This document describes how to configure FreeSWITCH directory users for ESL (Event Socket Layer) authentication using the `userauth` command.

## Overview

FreeSWITCH supports per-user ESL authentication through the directory XML configuration. This allows fine-grained control over which events and API commands each user can access.

## User Authentication Format

When connecting with userauth, the format is:

```
userauth user@domain:password
```

An alternative format is also supported:

```
userauth user:password@domain
```

For example:

```
userauth admin@default:MySecretPassword
userauth admin:MySecretPassword@default
```

The domain must match the domain name in your FreeSWITCH directory configuration.

## ESL Parameters

The following parameters can be set in the `<params>` section of a user, group, or domain configuration:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `esl-password` | string | (required) | Password for ESL authentication |
| `esl-allowed-events` | string | `all` | Comma/space-separated list of allowed event types |
| `esl-allowed-api` | string | `all` | Comma/space-separated list of allowed API commands |
| `esl-allowed-log` | boolean | `true` | Whether the user can receive log output |
| `esl-disable-command-logging` | boolean | `false` | Disable logging of commands from this user |

### Parameter Details

#### esl-password

The password required for authentication. Must match exactly what is provided in the `userauth` command.

#### esl-allowed-events

Controls which events the user can subscribe to.

Values:

- `all` - Allow all events including custom events (default)
- Space-separated or comma-separated list of event names (not both)

The delimiter is automatically detected: if the value contains a space, space is used as delimiter; otherwise comma is used.

Standard FreeSWITCH event types: `CHANNEL_CREATE`, `CHANNEL_DESTROY`, `CHANNEL_STATE`, `CHANNEL_CALLSTATE`, `CHANNEL_ANSWER`, `CHANNEL_HANGUP`, `CHANNEL_HANGUP_COMPLETE`, `CHANNEL_EXECUTE`, `CHANNEL_EXECUTE_COMPLETE`, `CHANNEL_BRIDGE`, `CHANNEL_UNBRIDGE`, `CHANNEL_PROGRESS`, `CHANNEL_PROGRESS_MEDIA`, `CHANNEL_OUTGOING`, `CHANNEL_PARK`, `CHANNEL_UNPARK`, `CHANNEL_APPLICATION`, `CHANNEL_ORIGINATE`, `CHANNEL_UUID`, `API`, `LOG`, `INBOUND_CHAN`, `OUTBOUND_CHAN`, `STARTUP`, `SHUTDOWN`, `PUBLISH`, `UNPUBLISH`, `TALK`, `NOTALK`, `SESSION_HEARTBEAT`, `CLIENT_DISCONNECTED`, `SERVER_DISCONNECTED`, `SEND_INFO`, `RECV_INFO`, `RECV_RTCP_MESSAGE`, `SEND_MESSAGE`, `RECV_MESSAGE`, `REQUEST_PARAMS`, `CHANNEL_DATA`, `GENERAL`, `COMMAND`, `SESSION_DATA`, `MESSAGE`, `PRESENCE_IN`, `PRESENCE_OUT`, `PRESENCE_PROBE`, `MESSAGE_WAITING`, `MESSAGE_QUERY`, `ROSTER`, `CODEC`, `BACKGROUND_JOB`, `DETECTED_SPEECH`, `DETECTED_TONE`, `PRIVATE_COMMAND`, `HEARTBEAT`, `TRAP`, `ADD_SCHEDULE`, `DEL_SCHEDULE`, `EXE_SCHEDULE`, `RE_SCHEDULE`, `RELOADXML`, `NOTIFY`, `PHONE_FEATURE`, `PHONE_FEATURE_SUBSCRIBE`, `MODULE_LOAD`, `MODULE_UNLOAD`, `DTMF`, `SESSION_CRASH`, `TEXT`, `CUSTOM`, `ALL`.

Custom event subclasses from modules are also supported (see section below).

**Long allow-lists corrupt the session.** mod_event_socket builds the
`Allowed-Events` userauth response header in a fixed 512-byte buffer
(`char event_reply[512]`). An allow-list whose names exceed that — roughly 25+
event types, or fewer once CUSTOM subclasses like `sofia::gateway_state` are
added — produces a truncated header that breaks the client's auth-response
parsing and corrupts the stream. There is no per-list length check in
FreeSWITCH. If a client needs that many events, omit `esl-allowed-events`
entirely (the `all` default sends a short fixed header) and rely on the client's
own `event ...` subscription to scope what it actually receives.

#### esl-allowed-api

Controls which API commands the user can execute.

Values:

- `all` - Allow all API commands (default)
- Empty string `""` - Disable all API access
- Space-separated or comma-separated list of command names (not both)

The delimiter is automatically detected: if the value contains a space, space is used as delimiter; otherwise comma is used.

**Important:** Only the command name (first word) is checked. Subcommands cannot be filtered individually. For example, allowing `sofia` permits all sofia subcommands (`sofia status`, `sofia profile`, `sofia xmlstatus`, etc.).

Example restricted list: `show sofia status version uptime`

#### esl-allowed-log

Boolean (`true`/`false`) controlling whether the user can receive log output. Default: `true`.

#### esl-disable-command-logging

Boolean (`true`/`false`) to suppress logging of commands executed by this user. Default: `false`.

## Configuration Hierarchy

Parameters can be set at three levels (in order of precedence, later overrides earlier):

1. Domain level (`<domain>` → `<params>`)
2. Group level (`<group>` → `<params>`)
3. User level (`<user>` → `<params>`)

## A dedicated domain for ESL service users

An ESL client authenticates as a directory user but never registers a SIP
endpoint and never places calls through a domain's `dial-string`. Putting such a
user in a real SIP domain couples it to things it does not use: its auth domain
becomes a runtime variable (`$${domain}`), it inherits `dial-string` and presence
params, and in multi-tenant setups its file sits among tenant user files.

A standalone `system.local` domain avoids all of that. It has no `dial-string`,
no groups, and no domain variable, so the service user has a stable auth domain
(`user@system.local`) independent of whichever SIP domains the switch serves, and
its files live in their own tree with their own permissions.

`directory/system-local.xml` declares the domain and includes its users:

```xml
<include>
    <domain name="system.local">
        <users>
            <X-PRE-PROCESS cmd="include" data="system.local/*.xml"/>
        </users>
    </domain>
</include>
```

One file per user under `directory/system.local/`, e.g. `monitor.xml`:

```xml
<include>
  <user id="monitor">
    <params>
      <param name="esl-password" value="..."/>
      <param name="esl-allowed-api" value="show status uptime"/>
      <param name="esl-allowed-events" value="CHANNEL_CREATE CHANNEL_ANSWER CHANNEL_HANGUP"/>
      <param name="esl-allowed-log" value="false"/>
    </params>
  </user>
</include>
```

## Example Configurations

### Basic User with Full Access

```xml
<user id="admin">
  <params>
    <param name="esl-password" value="SuperSecretPassword"/>
  </params>
</user>
```

### Restricted Monitoring User

```xml
<user id="monitor">
  <params>
    <param name="esl-password" value="MonitorPass123"/>
    <param name="esl-allowed-events" value="CHANNEL_CREATE CHANNEL_ANSWER CHANNEL_HANGUP"/>
    <param name="esl-allowed-api" value="show status version uptime"/>
    <param name="esl-allowed-log" value="false"/>
  </params>
</user>
```

### Event-Only User (No API Access)

```xml
<user id="events">
  <params>
    <param name="esl-password" value="EventsOnly"/>
    <param name="esl-allowed-events" value="all"/>
    <param name="esl-allowed-api" value=""/>
    <param name="esl-allowed-log" value="true"/>
  </params>
</user>
```

### Domain-Wide Defaults with User Override

```xml
<domain name="default">
  <params>
    <param name="esl-allowed-log" value="false"/>
    <param name="esl-allowed-api" value="show status version"/>
  </params>

  <groups>
    <group name="admins">
      <params>
        <param name="esl-allowed-log" value="true"/>
      </params>
      <users>
        <user id="superadmin">
          <params>
            <param name="esl-password" value="SuperAdmin123"/>
            <param name="esl-allowed-api" value="all"/>
          </params>
        </user>
      </users>
    </group>

    <group name="operators">
      <users>
        <user id="operator1">
          <params>
            <param name="esl-password" value="Operator1Pass"/>
          </params>
        </user>
      </users>
    </group>
  </groups>
</domain>
```

## Authentication Response

Upon successful authentication, FreeSWITCH returns the granted permissions:

```
Content-Type: command/reply
Reply-Text: +OK accepted
Allowed-Events: all
Allowed-API: show sofia status version uptime
Allowed-LOG: true
```

## Connecting with freeswitch-esl-tokio

```rust
use freeswitch_esl_tokio::{AuthMethod, EslClient, EslConnectOptions};

// Connect with userauth — returns (EslClient, EslEventStream)
let (client, mut events) = EslClient::connect_with_auth(
    "localhost",
    8021,
    AuthMethod::user("admin@default", "SuperSecretPassword")?, // user@domain required
    EslConnectOptions::default(),
).await?;
```

Or using the event_filter example:

```bash
cargo run --example event_filter -- -u admin@default -p SuperSecretPassword -e ALL
```

## Custom Event Subclasses

Custom events use the `CUSTOM` event type with a subclass name. These can be used in `esl-allowed-events`.

### mod_sofia Events

`sofia::register`, `sofia::pre_register`, `sofia::register_attempt`, `sofia::register_failure`, `sofia::unregister`, `sofia::expire`, `sofia::gateway_state`, `sofia::gateway_add`, `sofia::gateway_delete`, `sofia::gateway_invalid_digest_req`, `sofia::sip_user_state`, `sofia::notify_refer`, `sofia::reinvite`, `sofia::recovery_recv`, `sofia::recovery_send`, `sofia::recovery_recovered`, `sofia::error`, `sofia::profile_start`, `sofia::notify_watched_header`, `sofia::wrong_call_state`, `sofia::transferor`, `sofia::transferee`, `sofia::replaced`, `sofia::intercepted`, `sofia::bye_response`.

Registration lifecycle events:

| Subclass | When | `Event-Subclass` header |
|---|---|---|
| `sofia::pre_register` | FS issues a 401/407 challenge (no credentials yet) | `sofia::pre_register` |
| `sofia::register_attempt` | Credentials received, any outcome | `sofia::register_attempt` |
| `sofia::register` | Successful registration or refresh (`update-reg: true` on refresh) | `sofia::register` |
| `sofia::unregister` | Explicit `expires: 0` from UA | `sofia::unregister` |
| `sofia::expire` | Silent timeout, fired by the registrar's SQL expiry timer | `sofia::expire` |
| `sofia::register_failure` | Hard 403 Forbidden (wrong password, disabled user) | `sofia::register_failure` |

All are `SWITCH_EVENT_CUSTOM` in the C core — the ESL subscription token is `CUSTOM` followed by the subclass name(s). Subscribing to `CUSTOM sofia::gateway_state` receives **only** that subclass; other CUSTOM subclasses are filtered out by mod_event_socket before delivery.

#### Subscribing to registration events with this crate

```rust
use freeswitch_esl_tokio::{EslClient, EventFormat, EventSubscription};

let (client, mut events) = EslClient::connect(&host, port, &password).await?;

// Subscribe only to the subclasses needed to track registrations.
// custom_subclass() returns Err if the string contains spaces or newlines.
let subscription = EventSubscription::new(EventFormat::Plain)
    .with_custom_subclass("sofia::register").unwrap()
    .with_custom_subclass("sofia::unregister").unwrap()
    .with_custom_subclass("sofia::expire").unwrap();

// Sends: event plain CUSTOM sofia::register sofia::unregister sofia::expire
client.apply_subscription(&subscription).await?;

while let Some(event) = events.try_next().await? {
    let subclass = event.header_str("Event-Subclass").unwrap_or("");
    let user = event.header_str("from-user").unwrap_or("?");
    let host = event.header_str("from-host").unwrap_or("?");
    match subclass {
        "sofia::register"   => println!("registered:   {}@{}", user, host),
        "sofia::unregister" => println!("unregistered: {}@{}", user, host),
        "sofia::expire"     => println!("expired:      {}@{}", user, host),
        _                   => {}
    }
}
```

To restrict a directory ESL user to only these events, list the subclass names in `esl-allowed-events`. The `CUSTOM` token must appear before any subclass names:

```xml
<param name="esl-allowed-events" value="CUSTOM sofia::register sofia::unregister sofia::expire"/>
```

Standard event types and CUSTOM subclasses can be combined on the same line. **Standard event names must come before `CUSTOM`** — the parser uses a sticky flag: once `CUSTOM` is seen, every subsequent token is treated as a subclass name, not an event type.

```xml
<!-- Correct: standard events first, CUSTOM and subclasses at the end -->
<param name="esl-allowed-events" value="CHANNEL_CREATE CHANNEL_DESTROY CUSTOM sofia::register sofia::unregister sofia::expire"/>

<!-- Wrong: CHANNEL_CREATE after CUSTOM is inserted as a subclass string, not as the event type -->
<param name="esl-allowed-events" value="CUSTOM sofia::register CHANNEL_CREATE"/>
```

### mod_conference Events

`conference::maintenance`, `conference::cdr`.

## File Location

User configurations are typically stored in:

- `/etc/freeswitch/directory/default/*.xml` (Debian/Ubuntu)
- `/usr/local/freeswitch/conf/directory/default/*.xml` (source install)

The domain name in the directory XML must match the domain portion of the `user@domain` authentication string.
