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
    command::EslResponse,
    constants::{CONTENT_TYPE_LOG_DATA, HEADER_CONTENT_TYPE, READER_TICK_MS, SOCKET_BUF_SIZE},
    error::EslError,
    event::{EslEvent, EventFormat},
    protocol::{EslMessage, EslParser, MessageType},
    HeaderLookup,
};

use super::reexec::ReexecReader;
#[cfg(unix)]
use super::reexec::REEXEC_ABANDONED;
use super::{read_into_parser, DisconnectReason, EventOverflow, ReadStep, SharedState};

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
        .dropped_event_count
        .fetch_add(1, Ordering::Relaxed);
    // Once per run: the notice, once delivered, re-arms the next one.
    if !shared
        .queue_full_notice_armed
        .swap(true, Ordering::Relaxed)
    {
        warn!("Event queue full, dropping events until the consumer catches up; dropped_event_count() keeps the total");
    }
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
        Err(payload) => {
            let reason = describe_panic(payload.as_ref());
            tracing::error!("{reason}");
            DisconnectReason::IoError(reason)
        }
    };
    // `event_tx` is still alive here: a consumer that sees recv() -> None must
    // find the disconnect status already published, not a stale Connected.
    shared.publish_disconnect(reason);
    fail_pending_reply(&shared).await;
}

/// `panic!` carries a `&'static str` for a literal message and a `String` for a
/// formatted one; `panic_any` can carry anything else.
fn describe_panic(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        format!("reader task panicked: {message}")
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!("reader task panicked: {message}")
    } else {
        "reader task panicked with a non-string payload".to_string()
    }
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
    pending
        .stale
        .clear();
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
) -> ControlFlow<DisconnectReason> {
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
                    return ControlFlow::Break(DisconnectReason::EventStreamDropped);
                }
                return ControlFlow::Continue(());
            }
            None => EventFormat::Plain,
        }
    };

    let event_result = parser.parse_event(message, format);
    if !dispatch_event(sink, shared, event_result).await {
        debug!("Event channel closed, reader exiting");
        return ControlFlow::Break(DisconnectReason::EventStreamDropped);
    }
    ControlFlow::Continue(())
}

/// Hands a reply no command is awaiting to the caller, who alone can tell what
/// the switch answered. Called with the pending-reply lock released, since the
/// dispatch may wait for queue capacity.
async fn dispatch_reply_report(
    error: EslError,
    shared: &SharedState,
    sink: &mut EventSink<'_>,
) -> ControlFlow<DisconnectReason> {
    // Display names the content type and lengths, never the text.
    warn!("{error}; delivered on the event stream");
    if !dispatch_event(sink, shared, Err(error)).await {
        debug!("Event channel closed, reader exiting");
        return ControlFlow::Break(DisconnectReason::EventStreamDropped);
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
) -> ControlFlow<DisconnectReason> {
    match message.message_type {
        MessageType::Event => return dispatch_parsed_event(message, parser, shared, sink).await,
        MessageType::CommandReply | MessageType::ApiResponse => {
            let mut pending = shared
                .pending_reply
                .lock()
                .await;
            let late = |ticket, message| EslError::LateReply {
                response: Box::new(EslResponse::from_message(message)),
                ticket,
            };
            let report = if let Some(ticket) = pending
                .stale
                .pop_front()
            {
                late(ticket, message)
            } else if let Some(waiter) = pending
                .waiting
                .take()
            {
                // Fails only when the command's future was dropped mid-wait.
                match waiter
                    .tx
                    .send(message)
                {
                    Ok(()) => return ControlFlow::Continue(()),
                    Err(message) => late(waiter.ticket, message),
                }
            } else {
                EslError::UnsolicitedReply {
                    response: Box::new(EslResponse::from_message(message)),
                }
            };
            drop(pending);
            return dispatch_reply_report(report, shared, sink).await;
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
            ControlFlow::Break(DisconnectReason::ServerNotice {
                controlled_session_uuid,
                body: message.body,
            })
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
            ControlFlow::Break(DisconnectReason::AccessDenied(reason))
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
            ControlFlow::Break(DisconnectReason::ProtocolError(reason))
        }
    }
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
) -> ControlFlow<DisconnectReason> {
    use crate::constants::REEXEC_DRAIN_TIMEOUT_MS;

    if parser.is_waiting_for_headers() {
        let residual = parser
            .remaining_bytes()
            .to_vec();
        debug!("Re-exec drain complete, {} residual bytes", residual.len());
        let delivered = reexec
            .result_tx
            .take()
            .is_some_and(|tx| {
                tx.send(Ok(residual))
                    .is_ok()
            });
        if !delivered {
            warn!("Re-exec caller gone before the residual bytes were delivered");
            return ControlFlow::Break(DisconnectReason::ProtocolError(REEXEC_ABANDONED.into()));
        }
        return ControlFlow::Break(DisconnectReason::ReexecTeardown);
    }

    // WaitingForBody: more socket data is needed to finish the current message.
    let drain_timeout = Duration::from_millis(REEXEC_DRAIN_TIMEOUT_MS);
    let read = read_into_parser(reader, parser, read_buffer, drain_timeout);
    let step = tokio::select! {
        biased;
        () = caller_gone(reexec) => {
            warn!("{REEXEC_ABANDONED}");
            return ControlFlow::Break(DisconnectReason::ProtocolError(REEXEC_ABANDONED.into()));
        }
        step = read => step,
    };
    match step {
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
            ControlFlow::Break(DisconnectReason::ConnectionClosed)
        }
        Ok(ReadStep::Idle) => {
            let reason = "drain timeout waiting for message body";
            warn!("Re-exec {reason}");
            fail_reexec(
                reexec,
                EslError::ReexecFailed {
                    reason: reason.into(),
                    source: None,
                },
            );
            ControlFlow::Break(DisconnectReason::ProtocolError(reason.into()))
        }
        Err(e) => {
            warn!(
                "Re-exec drain failed while reading the message body: {}",
                e.display_chain()
            );
            let reason = read_failure_reason(&e);
            fail_reexec(reexec, e);
            ControlFlow::Break(reason)
        }
    }
}

/// Resolves once the teardown caller has stopped waiting for the drain.
#[cfg(unix)]
async fn caller_gone(reexec: &mut ReexecReader) {
    match reexec
        .result_tx
        .as_mut()
    {
        Some(tx) => {
            tx.closed()
                .await
        }
        None => std::future::pending().await,
    }
}

/// The reason a failed socket read or parser feed ends the reader with.
fn read_failure_reason(error: &EslError) -> DisconnectReason {
    match error {
        EslError::Io(e) => DisconnectReason::IoError(e.to_string()),
        e => DisconnectReason::ProtocolError(
            e.display_chain()
                .to_string(),
        ),
    }
}

#[cfg(test)]
fn run_induced_panic(shared: &SharedState) {
    let hook = shared
        .induced_panic
        .lock()
        .expect("test seam lock")
        .take();
    if let Some(hook) = hook {
        hook();
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

/// Returns the reason its caller publishes, a re-exec drain's failure
/// included: the teardown result channel reaches only the teardown caller.
async fn reader_loop_inner(
    mut reader: OwnedReadHalf,
    mut parser: EslParser,
    shared: Arc<SharedState>,
    event_tx: &mpsc::Sender<Result<EslEvent, EslError>>,
    overflow: EventOverflow,
    #[cfg_attr(not(unix), allow(unused_variables, unused_mut))] mut reexec: ReexecReader,
) -> DisconnectReason {
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

                #[cfg(test)]
                run_induced_panic(&shared);
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
                return DisconnectReason::ProtocolError(reason);
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
                return DisconnectReason::ConnectionClosed;
            }
            Ok(ReadStep::Idle) => {
                if liveness_expired(&shared, last_recv) {
                    return DisconnectReason::HeartbeatExpired;
                }
            }
            Err(e) => {
                warn!("Read error: {}", e.display_chain());
                return read_failure_reason(&e);
            }
        }
    }
}
