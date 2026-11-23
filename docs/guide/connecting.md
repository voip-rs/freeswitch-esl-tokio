# Connecting and running commands

## Architecture

```text
connect() -> (EslClient, EslEventStream)

EslClient (Clone + Send)         EslEventStream
|- send commands from any task    |- events via mpsc channel
|- writer half behind Arc<Mutex>  '- connection status via watch
'- replies via oneshot channel

Background reader task
|- owns the read half + parser
|- routes CommandReply/ApiResponse -> pending oneshot
|- routes Event -> mpsc channel
|- tracks liveness (any TCP traffic resets timer)
'- broadcasts ConnectionStatus on disconnect
```

See [design-rationale.md](../design-rationale.md) for the full story.

## Inbound connection

```rust,no_run
# use freeswitch_esl_tokio::{EslClient, EslError};
# #[tokio::main]
# async fn main() -> Result<(), EslError> {
let (client, mut events) = EslClient::connect("localhost", 8021, "ClueCon").await?;

let response = client.api("status").await?;
// api_result() is the whole check. A command the switch refuses is answered as
// a reply with no body; one that runs and fails reports in the body. Both come
// back as Err here. It also strips the +OK prefix action commands carry, and
// returns a query's body as-is.
println!("{}", response.api_result()?);
# let _ = &mut events;
# Ok(())
# }
```

Multi-tenant with per-user ACL:

```rust,no_run
# use freeswitch_esl_tokio::{AuthMethod, EslClient, EslConnectOptions, EslError};
# #[tokio::main]
# async fn main() -> Result<(), EslError> {
let (client, mut events) = EslClient::connect_with_auth(
    "localhost",
    8021,
    AuthMethod::user("admin@default", "ClueCon"),
    EslConnectOptions::default(),
)
.await?;
# let _ = (&client, &mut events);
# Ok(())
# }
```

## Event loop with liveness detection

`set_liveness_timeout` fires `Disconnected(HeartbeatExpired)` when no inbound traffic arrives for the threshold, catching a silently dead TCP connection. The library **never sends keepalives on its own** -- the timer is fed only by what the server pushes. On a busy connection ordinary event traffic feeds it; on an **idle** connection you supply the traffic, normally by subscribing to `HEARTBEAT` (FreeSWITCH emits one every ~20s).

Subscribe to `HEARTBEAT` on its own command, separate from your functional events: a permission-restricted user (`esl-allowed-events` without `HEARTBEAT`) is rejected with `-ERR permission denied`, and bundling would sink the whole subscription. That rejection is recoverable -- detect it with `EslError::is_permission_denied()`, keep the connection, and skip `set_liveness_timeout` for that user (nothing would feed the timer, so it would trip on a healthy idle socket).

See [reconnecting_client.rs](../../examples/reconnecting_client.rs) for the full gated pattern inside a reconnection loop.

## Background API calls

`api()` **blocks the entire ESL socket** until FreeSWITCH finishes the command -- no events are delivered and no other commands can be sent on the connection until it returns. Use `bgapi()` for anything that may take time (originate, conference operations, bulk queries). `bgapi()` returns immediately with a Job-UUID; the result arrives as a `BACKGROUND_JOB` event.

`BgJobTracker` handles the Job-UUID correlation so you don't have to maintain a pending-jobs HashMap yourself:

```rust,no_run
# use freeswitch_esl_tokio::{EslClient, EslError, EventFormat, EslEventType};
use freeswitch_esl_tokio::{BgJobTracker, EventSubscription};
# #[tokio::main]
# async fn main() -> Result<(), EslError> {
# let (client, mut events) = EslClient::connect("localhost", 8021, "ClueCon").await?;

client.apply_subscription(
    &EventSubscription::new(EventFormat::Plain)
        .with_event(EslEventType::BackgroundJob),
).await?;

let mut bg = BgJobTracker::new();
bg.send(&client, "sofia xmlstatus profile internal").await?;

while let Some(event) = events.try_next().await? {
    if let Some(((), result)) = bg.try_complete(&event) {
        match result.parse_body() {
            Ok(data) => println!("{}", data),
            Err(e) => eprintln!("command failed: {}", e.display_chain()),
        }
        break;
    }
}
# Ok(())
# }
```

Attach caller context to each job for dispatch without a separate map. The context is returned alongside the result:

```rust,no_run
# use freeswitch_esl_tokio::{EslClient, EslError, BgJobTracker};
# #[tokio::main]
# async fn main() -> Result<(), EslError> {
# let (client, mut events) = EslClient::connect("localhost", 8021, "ClueCon").await?;
# let channel_uuids: Vec<String> = Vec::new();
let mut bg: BgJobTracker<String> = BgJobTracker::new();

for uuid in &channel_uuids {
    bg.bgapi(&client, &format!("uuid_dump {uuid}"), uuid.clone()).await?;
}

while let Some(event) = events.try_next().await? {
    if let Some((channel_uuid, result)) = bg.try_complete(&event) {
        // parse_body(), not body(): a job that failed reports it in the body,
        // so the raw string reads as output.
        match result.parse_body() {
            Ok(dump) => println!("dump for {channel_uuid}: {dump}"),
            Err(e) => eprintln!("dump for {channel_uuid} failed: {}", e.display_chain()),
        }
    }
    // ... handle other events
}
# Ok(())
# }
```

## Outbound mode

FreeSWITCH connects to your application via the `socket` dialplan app. After accepting, send `connect` to establish the session:

```rust,no_run
use freeswitch_esl_tokio::{EslClient, AppCommand, EventFormat, HeaderLookup};
use tokio::net::TcpListener;
use freeswitch_esl_tokio::EslError;
# #[tokio::main]
# async fn main() -> Result<(), EslError> {
let listener = TcpListener::bind("[::]:8040").await.map_err(EslError::Io)?;
let (client, mut events) = EslClient::accept_outbound(&listener).await?;

// Must be the first command after accept, returns channel info as an EslResponse
let channel_data = client.connect_session().await?;
// Channel-Name is always present in connect response
println!("Channel: {}", channel_data.channel_name().unwrap());

// Subscribe, enable linger, resume dialplan
client.myevents(EventFormat::Plain).await?;
client.linger(None).await?;
client.resume().await?;

// Control the call
client.send_command(AppCommand::answer()).await?;
client.send_command(AppCommand::playback("ivr/ivr-welcome.wav")).await?;

while let Some(event) = events.try_next().await? {
    // handle events...
}
# Ok(())
# }
```

See [outbound-esl-quirks.md](../outbound-esl-quirks.md) for outbound mode gotchas (`connect_session` ordering, `async full` requirement, socket app quoting).
