# Re-Exec Support for Zero-Downtime Binary Upgrades

## Overview

Long-running ESL daemons can upgrade their binary without dropping
the FreeSWITCH connection. The process receives a signal (e.g. SIGUSR2),
serializes application state, extracts the TCP socket file descriptor
from the library, and calls `exec()`. The new binary image restores
the connection using the preserved fd and any residual parser bytes.

## API

### Teardown (old process)

```rust
#[cfg(unix)]
let handoff = client.teardown_for_reexec().await?;
let (fd, residual) = (handoff.fd(), handoff.residual());
```

Returns a `ReexecHandoff` carrying the raw socket fd and any unparsed
bytes remaining in the parser buffer. After this call:

- The connection status is `Disconnected(ReexecTeardown)`.
- The caller must clear `CLOEXEC` on the fd before `exec()`.
- The caller must not drop the `EslClient` (or must `mem::forget` it)
  to keep the fd open through `exec()`.

### Adopt (new process)

```rust
let stream = unsafe { std::net::TcpStream::from_raw_fd(fd) };
stream.set_nonblocking(true)?;
let tokio_stream = tokio::net::TcpStream::from_std(stream)?;
let (client, events) = EslClient::adopt_stream(tokio_stream, &residual)?;
```

Creates a new `EslClient` from an already-authenticated TCP stream.
The parser is pre-seeded with any residual bytes from the previous
process.

## Drain Protocol

When `teardown_for_reexec()` is called, the reader loop must stop at
a clean message boundary. The ESL wire format is:

```
[headers]\n\n[body of Content-Length bytes]
```

Headers are consumed destructively by the parser. If we stopped while
the parser was mid-body (headers already consumed, waiting for body
bytes), the residual would be a partial body without its headers,
which is corrupt and unusable.

The drain logic handles this:

1. **Stop signal sent** to the reader loop via a oneshot channel.
2. **Reader checks parser state** after `parse_message()` returns `None`:
   - `WaitingForHeaders`: safe to stop. Residual bytes are a clean
     prefix of a new message. Send them back via the result channel.
   - `WaitingForBody`: keep reading from the socket (with a 5-second
     drain timeout) until the body completes. Dispatch the completed
     message normally, then stop at the next `WaitingForHeaders`.
3. **Drain timeout** (5s, `REEXEC_DRAIN_TIMEOUT_MS`, per read): if the
   body stops arriving, return `ReexecReaderLost` rather than hanging.
4. **Teardown deadline** (the drain timeout plus 1s, overall): a body
   that keeps trickling in is abandoned there. Teardown sets the status
   to `Disconnected(ProtocolError(..))` and returns `ReexecReaderLost`;
   the reader sees the caller gone and stops without finishing the
   drain.

## Which error says the connection survived

- `ReexecFailed` is a refusal before the stop signal: a command in
  flight, or a second call. The reader still runs and the connection
  stays usable.
- `ReexecReaderLost` is every failure after the stop signal, and a
  reader already gone when teardown starts. The connection is over:
  `is_connection_error()` is true, and the status already reads
  `Disconnected` when the error returns, naming why:
  `ConnectionClosed` when the switch closed mid-message,
  `ProtocolError` for a timeout or a framing fault, `IoError` for a
  read error.

A daemon carries on after the first and exits, or reconnects, after
the second.

Messages completed during drain are dispatched normally to the event
channel, so the caller receives all in-flight events.

## Preconditions

- **No command in-flight.** If a command holds the writer, or
  `pending_reply` is occupied (a command was sent but the reply hasn't
  arrived), teardown returns `ReexecFailed` at once. The caller must
  wait for commands to complete. A command whose future was dropped
  before its reply arrived still counts as in flight.
- **Owed replies cross the exec.** A command that timed out does not
  block teardown, but the switch still owes its reply. If it has not
  arrived as `LateReply` before teardown, the new process receives it
  as `UnsolicitedReply`, with no ticket to pair it with. A caller that
  needs that result waits for the `LateReply` before tearing down.
- **Single call.** Teardown takes the reexec channel from SharedState.
  Calling it twice returns `ReexecFailed`.

## Race Prevention

- **The writer is held** from the in-flight check to the end of
  teardown. A command sent meanwhile waits for it, then finds the
  connection ended and fails with `ConnectionClosed` without writing;
  written, its reply would reach the next process with nothing to pair
  it with.
- **Liveness timeout** is disabled (set to 0) before sending the stop
  signal. Otherwise, if no traffic arrives between the signal and the
  reader processing it, the reader could exit with `HeartbeatExpired`
  and the result channel would never be sent. A teardown refused at the
  signal puts the caller's timeout back.
- **Biased select** prioritizes the stop signal over socket reads, so
  the drain begins immediately when the signal fires.
- **A dispatch waiting on event-queue capacity** under
  `EventOverflow::BlockFor` is cut short by the same signal, and the
  drain itself never waits. The event the dispatch was holding is
  discarded and counted in `dropped_event_count()`; no `QueueFull`
  marker reaches the consumer, since that marker needs a later
  dispatch and the process is leaving.

## Event Subscriptions

FreeSWITCH maintains event subscriptions server-side on the TCP
connection. After `adopt_stream()`, events subscribed by the old
process arrive immediately without re-subscribing. The caller must
have their event loop ready before calling `adopt_stream()`, or
events buffered in the kernel's TCP receive buffer will queue up in
the library's event channel.

## Non-Goals

- **Preserving `EslEventStream` across exec.** The mpsc channel is
  process-local. The new process creates a fresh one via
  `adopt_stream()`. That call rebuilds with default options, so a
  daemon running a non-default queue size or overflow policy must use
  `adopt_stream_with_options()` to keep it.
- **Automatic reconnection.** The library detects disconnection; the
  caller controls reconnection strategy.
- **Cross-platform support.** `teardown_for_reexec()` is
  `#[cfg(unix)]` only. `adopt_stream()` works on all platforms (it
  just takes a `TcpStream`).

## Same-Process Testing Caveat

The `examples/reexec_demo.rs` simulates re-exec without actually
calling `exec()`. This introduces two issues that don't exist in
a real re-exec:

1. **Tokio reactor registration.** The old `EslClient` has the fd
   registered with the tokio epoll reactor. `from_raw_fd` on the
   same fd triggers Rust's I/O safety check (double ownership). In
   a real `exec()`, the epoll fd has `CLOEXEC` and is gone.

2. **TCP FIN on drop.** Dropping the `EslClient` drops the
   `OwnedWriteHalf`, which sends a TCP FIN. FreeSWITCH sees EOF
   and sends a disconnect notice. In a real `exec()`,
   `mem::forget(client)` prevents the Drop.

The demo works around both by calling `dup(fd)` to get a clean fd
unknown to the reactor, then `mem::forget(client)` to prevent the
TCP FIN. This leaks the old client's reactor registration (harmless
since the process exits shortly after).

## Usage Flow

```
SIGUSR2 received:
  // 1. Stop accepting new work
  // 2. Wait for pending commands to complete
  let handoff = client.teardown_for_reexec().await?;
  let fd = handoff.fd();
  // 3. Serialize app state + handoff.residual() to disk/env
  // 4. Clear CLOEXEC on fd
  nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::empty()))?;
  // 5. mem::forget(client) to keep fd open
  std::mem::forget(client);
  // 6. exec()

New process starts:
  // 1. Read fd and residual from disk/env
  let stream = unsafe { std::net::TcpStream::from_raw_fd(fd) };
  stream.set_nonblocking(true)?;
  let tokio_stream = tokio::net::TcpStream::from_std(stream)?;
  // 2. Adopt the stream
  let (client, mut events) = EslClient::adopt_stream(tokio_stream, &residual)?;
  // 3. Restore app state
  // 4. Enter event loop
```
