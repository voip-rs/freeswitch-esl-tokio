use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::tcp::OwnedReadHalf;
use tokio::sync::mpsc;
#[cfg(unix)]
use tokio::sync::oneshot;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::{
    constants::{CONTENT_TYPE_LOG_DATA, HEADER_CONTENT_TYPE, READER_TICK_MS, SOCKET_BUF_SIZE},
    error::EslError,
    event::{EslEvent, EventFormat},
    protocol::{EslMessage, EslParser, MessageType},
    HeaderLookup,
};

use super::reexec::ReexecReader;
use super::{
    read_into_parser, ConnectionStatus, DisconnectReason, EventOverflow, ReadStep, SharedState,
};

/// Per-iteration view of the event channel: the overflow policy, the re-exec
/// stop signal the wait races against, and what the wait cost.
struct EventSink<'a> {
    tx: &'a mpsc::Sender<Result<EslEvent, EslError>>,
    overflow: EventOverflow,
    #[cfg(unix)]
    stop: Option<&'a mut oneshot::Receiver<()>>,
    stalled: Duration,
    #[cfg(unix)]
    stop_fired: bool,
}

/// Hands one event (or error) to the application, returning `false` once the
/// receiver is gone.
///
/// Under [`EventOverflow::BlockFor`] a full channel parks here until capacity,
/// the budget, or the re-exec stop signal; every other outcome drops the item
/// and arms a `QueueFull` notice for the next dispatch to deliver.
#[must_use]
async fn dispatch_event(
    sink: &mut EventSink<'_>,
    shared: &SharedState,
    item: Result<EslEvent, EslError>,
) -> bool {
    if !flush_queue_full_notice(sink.tx, shared) {
        return false;
    }

    let item = match sink
        .tx
        .try_send(item)
    {
        Ok(()) => return true,
        Err(mpsc::error::TrySendError::Closed(_)) => return false,
        Err(mpsc::error::TrySendError::Full(item)) => item,
    };

    let EventOverflow::BlockFor(budget) = sink.overflow else {
        drop_event(shared);
        return true;
    };

    // Counted before the wait, so a consumer can see it is stalled right now
    // rather than only once the reader is moving again.
    shared
        .event_stall_count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let started = Instant::now();
    let waited = wait_for_capacity(sink, budget, item).await;
    let elapsed = started.elapsed();
    sink.stalled += elapsed;
    record_stall(shared, elapsed);
    match waited {
        WaitOutcome::Sent => true,
        WaitOutcome::Closed => false,
        WaitOutcome::GaveUp => {
            drop_event(shared);
            true
        }
    }
}

enum WaitOutcome {
    Sent,
    Closed,
    GaveUp,
}

/// Races queue capacity against the budget and the re-exec stop signal.
///
/// The stop signal must win: teardown preserves the socket across an upgrade
/// on a budget of its own, which a reader parked here would spend instead.
async fn wait_for_capacity(
    sink: &mut EventSink<'_>,
    budget: Duration,
    item: Result<EslEvent, EslError>,
) -> WaitOutcome {
    #[cfg(unix)]
    let EventSink {
        tx,
        stop,
        stop_fired,
        ..
    } = sink;
    #[cfg(not(unix))]
    let EventSink { tx, .. } = sink;

    let reserve = tx.reserve();
    tokio::pin!(reserve);

    #[cfg(unix)]
    if let Some(stop) = stop.as_mut() {
        tokio::select! {
            biased;
            _ = &mut **stop => {}
            permit = tokio::time::timeout(budget, &mut reserve) => return settle(permit, item),
        }
        *stop_fired = true;
        debug!("Re-exec stop signal received while waiting for event queue capacity");
        return finish_without_permit(tx, item);
    }

    settle(tokio::time::timeout(budget, &mut reserve).await, item)
}

type ReservedPermit<'a> = Result<
    Result<mpsc::Permit<'a, Result<EslEvent, EslError>>, mpsc::error::SendError<()>>,
    tokio::time::error::Elapsed,
>;

fn settle(permit: ReservedPermit<'_>, item: Result<EslEvent, EslError>) -> WaitOutcome {
    match permit {
        Ok(Ok(permit)) => {
            permit.send(item);
            WaitOutcome::Sent
        }
        Ok(Err(_)) => WaitOutcome::Closed,
        Err(_) => WaitOutcome::GaveUp,
    }
}

/// One last free attempt before the stop signal takes the reader down.
#[cfg(unix)]
fn finish_without_permit(
    event_tx: &mpsc::Sender<Result<EslEvent, EslError>>,
    item: Result<EslEvent, EslError>,
) -> WaitOutcome {
    match event_tx.try_send(item) {
        Ok(()) => WaitOutcome::Sent,
        Err(mpsc::error::TrySendError::Closed(_)) => WaitOutcome::Closed,
        Err(mpsc::error::TrySendError::Full(_)) => WaitOutcome::GaveUp,
    }
}

/// Deliver an armed `QueueFull` notice, returning `false` once the receiver is
/// gone. Never waits: the notice is a marker, not a loss if it lands late.
#[must_use]
fn flush_queue_full_notice(
    event_tx: &mpsc::Sender<Result<EslEvent, EslError>>,
    shared: &SharedState,
) -> bool {
    use std::sync::atomic::Ordering;

    if !shared
        .queue_full_notice_armed
        .load(Ordering::Relaxed)
    {
        return true;
    }
    match event_tx.try_send(Err(EslError::QueueFull)) {
        Ok(()) => {
            shared
                .queue_full_notice_armed
                .store(false, Ordering::Relaxed);
            true
        }
        Err(mpsc::error::TrySendError::Closed(_)) => false,
        Err(mpsc::error::TrySendError::Full(_)) => true,
    }
}

fn drop_event(shared: &SharedState) {
    use std::sync::atomic::Ordering;

    shared
        .queue_full_notice_armed
        .store(true, Ordering::Relaxed);
    shared
        .dropped_event_count
        .fetch_add(1, Ordering::Relaxed);
    warn!("Event queue full, dropping event");
}

fn record_stall(shared: &SharedState, elapsed: Duration) {
    use std::sync::atomic::Ordering;

    shared
        .event_stall_nanos
        .fetch_add(
            elapsed
                .as_nanos()
                .min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
}

/// Hands one error to the application without ever waiting for capacity.
///
/// The reader's last deliveries go out here: `reader_loop` publishes the
/// disconnect status only once the loop has returned, so waiting would leave
/// the connection reading as live with nothing left to revive it.
fn dispatch_notice(
    event_tx: &mpsc::Sender<Result<EslEvent, EslError>>,
    shared: &SharedState,
    item: Result<EslEvent, EslError>,
) -> bool {
    if !flush_queue_full_notice(event_tx, shared) {
        return false;
    }
    match event_tx.try_send(item) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Closed(_)) => false,
        Err(mpsc::error::TrySendError::Full(_)) => {
            drop_event(shared);
            true
        }
    }
}

/// Background reader loop
pub(super) async fn reader_loop(
    reader: OwnedReadHalf,
    parser: EslParser,
    shared: Arc<SharedState>,
    event_tx: mpsc::Sender<Result<EslEvent, EslError>>,
    overflow: EventOverflow,
    reexec: ReexecReader,
) {
    let result = std::panic::AssertUnwindSafe(reader_loop_inner(
        reader,
        parser,
        shared.clone(),
        &event_tx,
        overflow,
        reexec,
    ));
    let reason = match futures_util::FutureExt::catch_unwind(result).await {
        Ok(reason) => reason,
        Err(_) => {
            tracing::error!("reader task panicked");
            Some(DisconnectReason::IoError(
                "reader task panicked".to_string(),
            ))
        }
    };
    // `event_tx` is still alive here: a consumer that sees recv() -> None must
    // find the disconnect status already published, not a stale Connected.
    if let Some(reason) = reason {
        if shared
            .status_tx
            .send(ConnectionStatus::Disconnected(reason))
            .is_err()
        {
            debug!("No status receiver left to observe the disconnect");
        }
    }
    fail_pending_reply(&shared).await;
}

/// Fail the in-flight waiter: dropping its `oneshot::Sender` resolves
/// `send_command` to `ConnectionClosed` rather than its full command timeout.
///
/// `reader_dead` is set under the same lock so a later install fails fast
/// instead of waiting on a task that is gone.
async fn fail_pending_reply(shared: &SharedState) {
    let mut pending = shared
        .pending_reply
        .lock()
        .await;
    pending.reader_dead = true;
    if pending
        .waiting
        .take()
        .is_some()
    {
        debug!("Failing in-flight command waiter: reader loop exited");
    }
    pending.stale_replies = 0;
}

/// Pick the body format from `Content-Type`, parse the event, hand it over.
///
/// `Break` ends the reader loop, which here only happens once the consumer has
/// dropped the stream.
async fn dispatch_parsed_event(
    message: EslMessage,
    parser: &EslParser,
    shared: &SharedState,
    sink: &mut EventSink<'_>,
) -> ControlFlow<Option<DisconnectReason>> {
    let ct = message
        .headers
        .header_str(HEADER_CONTENT_TYPE);

    // log/data uses single-level framing handled inside parse_event.
    let format = if ct == Some(CONTENT_TYPE_LOG_DATA) {
        EventFormat::Plain
    } else {
        match ct.map(EventFormat::from_content_type) {
            Some(Ok(f)) => f,
            Some(Err(e)) => {
                warn!("Unknown event content type: {}", e);
                if !dispatch_notice(
                    sink.tx,
                    shared,
                    Err(EslError::InvalidEventFormat {
                        format: e
                            .0
                            .clone(),
                    }),
                ) {
                    debug!("Event channel closed, reader exiting");
                    return ControlFlow::Break(None);
                }
                return ControlFlow::Continue(());
            }
            None => EventFormat::Plain,
        }
    };

    let event_result = parser.parse_event(message, format);
    if !dispatch_event(sink, shared, event_result).await {
        debug!("Event channel closed, reader exiting");
        return ControlFlow::Break(None);
    }
    ControlFlow::Continue(())
}

/// Routes one parsed message to the event channel or to the waiting command.
///
/// `Break` ends the reader loop, carrying the same reason its exits return.
async fn dispatch_message(
    message: EslMessage,
    parser: &EslParser,
    shared: &SharedState,
    sink: &mut EventSink<'_>,
) -> ControlFlow<Option<DisconnectReason>> {
    match message.message_type {
        MessageType::Event => return dispatch_parsed_event(message, parser, shared, sink).await,
        MessageType::CommandReply | MessageType::ApiResponse => {
            let mut pending = shared
                .pending_reply
                .lock()
                .await;
            if pending.stale_replies > 0 {
                // A previous command timed out and its server reply
                // arrived late. Discard to preserve correlation.
                pending.stale_replies -= 1;
                let reply_text = message
                    .headers
                    .header_str("Reply-Text")
                    .unwrap_or("<none>");
                warn!(
                    "Discarded stale {:?} reply (Reply-Text: {}) to preserve \
                     command-reply correlation; {} stale replies remaining",
                    message.message_type, reply_text, pending.stale_replies,
                );
            } else if let Some(tx) = pending
                .waiting
                .take()
            {
                if tx
                    .send(message)
                    .is_err()
                {
                    debug!("Reply channel closed before delivery (timeout race); reply discarded");
                }
            } else {
                warn!(
                    "Received unsolicited {:?} with no pending command",
                    message.message_type,
                );
            }
        }
        MessageType::Disconnect => {
            let disposition = message
                .headers
                .header_str("Content-Disposition");
            if disposition == Some("linger") {
                debug!("Received disconnect notice with linger disposition, ignoring");
                return ControlFlow::Continue(());
            }
            let controlled_session_uuid = message
                .headers
                .header_str("Controlled-Session-UUID")
                .map(str::to_string);
            info!("Received disconnect notice from server");
            return ControlFlow::Break(Some(DisconnectReason::ServerNotice {
                controlled_session_uuid,
                body: message.body,
            }));
        }
        MessageType::RudeRejection => {
            let reason = message
                .body
                .unwrap_or_else(|| "rude-rejection without body".to_string());
            warn!("Rude rejection from server: {}", reason);
            if !dispatch_notice(
                sink.tx,
                shared,
                Err(EslError::AccessDenied {
                    reason: reason.clone(),
                }),
            ) {
                debug!("Event channel closed before the rude rejection was delivered");
            }
            return ControlFlow::Break(Some(DisconnectReason::AccessDenied(reason)));
        }
        MessageType::AuthRequest => {
            // Post-authentication it means FreeSWITCH and the client are out of
            // sync, so the session cannot be trusted to continue.
            let reason = "unsolicited auth/request received after authentication".to_string();
            warn!("{reason}");
            if !dispatch_notice(
                sink.tx,
                shared,
                Err(EslError::protocol_error(reason.clone())),
            ) {
                debug!("Event channel closed before the desync error was delivered");
            }
            return ControlFlow::Break(Some(DisconnectReason::ProtocolError(reason)));
        }
    }
    ControlFlow::Continue(())
}

/// The teardown caller awaits this channel, so a drain that ends without
/// sending leaves it blocked until its own timeout.
#[cfg(unix)]
fn fail_reexec(reexec: &mut ReexecReader, error: EslError) {
    if let Some(tx) = reexec
        .result_tx
        .take()
    {
        if tx
            .send(Err(error))
            .is_err()
        {
            debug!("Re-exec caller gone before the drain failure was delivered");
        }
    }
}

/// One drain step once the re-exec stop signal has fired.
///
/// Stops only at a clean message boundary: mid-body, the residual would be a
/// partial body without its headers, which the new process cannot parse.
#[cfg(unix)]
async fn drain_for_reexec(
    reader: &mut OwnedReadHalf,
    parser: &mut EslParser,
    read_buffer: &mut [u8],
    reexec: &mut ReexecReader,
) -> ControlFlow<Option<DisconnectReason>> {
    use crate::constants::REEXEC_DRAIN_TIMEOUT_MS;

    if parser.is_waiting_for_headers() {
        let residual = parser
            .remaining_bytes()
            .to_vec();
        debug!("Re-exec drain complete, {} residual bytes", residual.len());
        if let Some(tx) = reexec
            .result_tx
            .take()
        {
            if tx
                .send(Ok(residual))
                .is_err()
            {
                warn!("Re-exec caller gone before the residual bytes were delivered");
            }
        }
        return ControlFlow::Break(Some(DisconnectReason::ReexecTeardown));
    }

    // WaitingForBody: more socket data is needed to finish the current message.
    let drain_timeout = Duration::from_millis(REEXEC_DRAIN_TIMEOUT_MS);
    match read_into_parser(reader, parser, read_buffer, drain_timeout).await {
        Ok(ReadStep::Fed) => ControlFlow::Continue(()),
        Ok(ReadStep::Eof) => {
            warn!("Connection closed during re-exec drain");
            fail_reexec(
                reexec,
                EslError::ReexecFailed {
                    reason: "connection closed during drain".into(),
                    source: None,
                },
            );
            ControlFlow::Break(None)
        }
        Ok(ReadStep::Idle) => {
            warn!("Re-exec drain timeout waiting for message body");
            fail_reexec(
                reexec,
                EslError::ReexecFailed {
                    reason: "drain timeout waiting for message body".into(),
                    source: None,
                },
            );
            ControlFlow::Break(None)
        }
        Err(e) => {
            warn!(
                "Re-exec drain failed while reading the message body: {}",
                e.display_chain()
            );
            fail_reexec(reexec, e);
            ControlFlow::Break(None)
        }
    }
}

/// Traffic-idle check for the read-timeout tick; a zero threshold disables it.
fn liveness_expired(shared: &SharedState, last_recv: Instant) -> bool {
    use std::sync::atomic::Ordering;

    let threshold_ms = shared
        .liveness_timeout_ms
        .load(Ordering::Relaxed);
    if threshold_ms == 0 {
        return false;
    }
    let elapsed = last_recv.elapsed();
    if elapsed <= Duration::from_millis(threshold_ms) {
        return false;
    }
    warn!(
        "Liveness timeout: {}ms without traffic (threshold {}ms)",
        elapsed.as_millis(),
        threshold_ms
    );
    true
}

/// Returns the reason its caller broadcasts, or `None` for the exits that
/// publish none: a closed event channel, or a re-exec failure already delivered
/// on the teardown result channel.
async fn reader_loop_inner(
    mut reader: OwnedReadHalf,
    mut parser: EslParser,
    shared: Arc<SharedState>,
    event_tx: &mpsc::Sender<Result<EslEvent, EslError>>,
    overflow: EventOverflow,
    #[cfg_attr(not(unix), allow(unused_variables, unused_mut))] mut reexec: ReexecReader,
) -> Option<DisconnectReason> {
    let mut read_buffer = [0u8; SOCKET_BUF_SIZE];
    let mut last_recv = Instant::now();
    #[cfg(unix)]
    let mut draining = false;

    loop {
        // Try to parse a complete message from buffered data first
        match parser.parse_message() {
            Ok(Some(message)) => {
                // Draining never waits for capacity: teardown has a budget of
                // its own and a parked dispatch would spend it.
                #[cfg(unix)]
                let mut sink = EventSink {
                    tx: event_tx,
                    overflow: if draining {
                        EventOverflow::DropIncoming
                    } else {
                        overflow
                    },
                    stop: if draining {
                        None
                    } else {
                        Some(&mut reexec.stop_rx)
                    },
                    stalled: Duration::ZERO,
                    stop_fired: false,
                };
                #[cfg(not(unix))]
                let mut sink = EventSink {
                    tx: event_tx,
                    overflow,
                    stalled: Duration::ZERO,
                };

                let flow = dispatch_message(message, &parser, &shared, &mut sink).await;
                // A stall reads nothing, but the peer never stopped sending.
                last_recv += sink.stalled;
                #[cfg(unix)]
                if sink.stop_fired {
                    draining = true;
                }
                match flow {
                    ControlFlow::Continue(()) => continue,
                    ControlFlow::Break(reason) => {
                        #[cfg(unix)]
                        if draining {
                            fail_reexec(
                                &mut reexec,
                                EslError::ReexecFailed {
                                    reason: "reader stopped before the drain completed".into(),
                                    source: None,
                                },
                            );
                        }
                        return reason;
                    }
                }
            }
            Ok(None) => {
                // Need more data from socket
            }
            Err(e) => {
                let reason = e
                    .display_chain()
                    .to_string();
                warn!("Parser error: {}", reason);
                return Some(DisconnectReason::ProtocolError(reason));
            }
        }

        // Only reached with no complete message buffered, which is the state
        // the drain inspects for a clean stop.
        #[cfg(unix)]
        if draining {
            match drain_for_reexec(&mut reader, &mut parser, &mut read_buffer, &mut reexec).await {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(reason) => return reason,
            }
        }

        // Normal read path with optional reexec stop signal
        let tick = Duration::from_millis(READER_TICK_MS);
        #[cfg(unix)]
        let read_result = tokio::select! {
            biased;
            _ = &mut reexec.stop_rx, if !draining => {
                debug!("Re-exec stop signal received, draining parser");
                draining = true;
                continue;
            }
            result = read_into_parser(&mut reader, &mut parser, &mut read_buffer, tick) => result,
        };

        #[cfg(not(unix))]
        let read_result = read_into_parser(&mut reader, &mut parser, &mut read_buffer, tick).await;

        match read_result {
            Ok(ReadStep::Fed) => last_recv = Instant::now(),
            Ok(ReadStep::Eof) => {
                info!("Connection closed (EOF)");
                return Some(DisconnectReason::ConnectionClosed);
            }
            Ok(ReadStep::Idle) => {
                if liveness_expired(&shared, last_recv) {
                    return Some(DisconnectReason::HeartbeatExpired);
                }
            }
            Err(EslError::Io(e)) => {
                warn!("Read error: {}", e);
                return Some(DisconnectReason::IoError(e.to_string()));
            }
            Err(e) => {
                let reason = e
                    .display_chain()
                    .to_string();
                warn!("Buffer error: {}", reason);
                return Some(DisconnectReason::ProtocolError(reason));
            }
        }
    }
}
