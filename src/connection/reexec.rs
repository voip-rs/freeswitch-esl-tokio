#[cfg(unix)]
use std::sync::atomic::Ordering;
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use tokio::sync::oneshot;
#[cfg(unix)]
use tokio::time::timeout;
#[cfg(unix)]
use tracing::info;

#[cfg(unix)]
use crate::error::{EslError, EslResult};

#[cfg(unix)]
use super::{DisconnectReason, EslClient};

/// Result type for the reexec drain operation (residual bytes or error).
#[cfg(unix)]
pub(crate) type ReexecResult = Result<Vec<u8>, EslError>;

/// Why a drain whose teardown caller stopped waiting ends the connection.
#[cfg(unix)]
pub(crate) const REEXEC_ABANDONED: &str = "re-exec teardown gave up before the drain finished";

/// What [`EslClient::teardown_for_reexec`] hands over for the next process image.
///
/// The fd stays open only while the old [`EslClient`] is alive, and its
/// `CLOEXEC` flag is still set: the caller clears it before `exec()`.
#[cfg(unix)]
#[must_use = "the fd and residual bytes are what the next process adopts"]
#[derive(Debug)]
pub struct ReexecHandoff {
    fd: std::os::unix::io::RawFd,
    residual: Vec<u8>,
}

#[cfg(unix)]
impl ReexecHandoff {
    /// The connected, authenticated socket.
    pub fn fd(&self) -> std::os::unix::io::RawFd {
        self.fd
    }

    /// Bytes the reader had read past the last complete message, for
    /// [`EslClient::adopt_stream`] to seed its parser with.
    pub fn residual(&self) -> &[u8] {
        &self.residual
    }
}

/// Caller-side half of the re-exec channel (stored in `SharedState`).
///
/// Taken by [`EslClient::teardown_for_reexec()`] to signal the reader and
/// receive residual bytes.
#[cfg(unix)]
pub(super) struct ReexecCaller {
    pub(super) stop_tx: oneshot::Sender<()>,
    pub(super) result_rx: oneshot::Receiver<ReexecResult>,
}

/// Reader-side half of the re-exec channel.
///
/// Owned by the reader loop task. The stop receiver fires when teardown is
/// requested; the result sender delivers residual bytes back to the caller.
/// On non-unix platforms this is a zero-size struct (re-exec is unix-only).
pub(crate) struct ReexecReader {
    #[cfg(unix)]
    pub(crate) stop_rx: oneshot::Receiver<()>,
    #[cfg(unix)]
    pub(crate) result_tx: Option<oneshot::Sender<ReexecResult>>,
}

#[cfg(unix)]
impl EslClient {
    /// Gracefully stop the reader loop and hand over the raw socket fd and any
    /// unparsed bytes remaining in the parser buffer.
    ///
    /// Used for zero-downtime binary upgrades via `exec()`. The caller
    /// serializes application state, clears `CLOEXEC` on the returned fd,
    /// and calls `exec()`. The new process reconstructs the connection with
    /// [`adopt_stream()`](Self::adopt_stream).
    ///
    /// # Preconditions
    ///
    /// - No ESL command may be in-flight (pending reply), nor hold the writer
    ///   while it writes. Returns [`EslError::ReexecFailed`] at once if one
    ///   does; a command sent while teardown runs waits for it and then fails
    ///   with a connection error, never reaching the wire.
    /// - A timed-out command's reply not yet received as
    ///   [`EslError::LateReply`] does not block teardown; the new process
    ///   receives it as [`EslError::UnsolicitedReply`].
    /// - May only be called once. Returns an error on subsequent calls.
    ///
    /// # After this call
    ///
    /// - The connection status is set to
    ///   [`DisconnectReason::ReexecTeardown`], so all clones see the
    ///   connection as dead.
    /// - [`ReexecFailed`](EslError::ReexecFailed) is a refusal before the
    ///   reader was signalled and leaves the connection usable.
    ///   [`ReexecReaderLost`](EslError::ReexecReaderLost) means the drain did
    ///   not finish, or the reader was already gone: the connection is over,
    ///   and the status already names why when the error returns. Past
    ///   teardown's own deadline the drain is abandoned and the status reads
    ///   [`DisconnectReason::ProtocolError`].
    /// - The caller **must not drop** the `EslClient` before `exec()` (or
    ///   must [`std::mem::forget`] it) to keep the fd open.
    /// - The caller is responsible for clearing `CLOEXEC` on the fd.
    pub async fn teardown_for_reexec(&self) -> EslResult<ReexecHandoff> {
        use crate::constants::REEXEC_DRAIN_TIMEOUT_MS;
        use std::os::unix::io::AsRawFd;

        let in_flight = || EslError::ReexecFailed {
            reason: "command still in-flight".into(),
            source: None,
        };
        // Held to the end: a command written after the in-flight check would
        // leave its reply to the next process image.
        let writer = self
            .writer
            .try_lock()
            .map_err(|_| in_flight())?;
        if self
            .shared
            .pending_reply
            .lock()
            .await
            .waiting
            .is_some()
        {
            return Err(in_flight());
        }

        // Take the reexec channel (one-shot: fails on second call)
        let reexec = {
            let mut guard = self
                .shared
                .reexec
                .lock()
                .await;
            guard
                .take()
                .ok_or_else(|| EslError::ReexecFailed {
                    reason: "teardown already called".into(),
                    source: None,
                })?
        };

        // Disable liveness to prevent HeartbeatExpired race
        let liveness_ms = self
            .shared
            .liveness_timeout_ms
            .swap(0, Ordering::Relaxed);

        // A dropped receiver means the reader is already gone; reporting it here
        // keeps it out of the drain timeout below, which would name the wrong cause.
        if reexec
            .stop_tx
            .send(())
            .is_err()
        {
            self.shared
                .liveness_timeout_ms
                .store(liveness_ms, Ordering::Relaxed);
            return Err(EslError::ReexecReaderLost {
                reason: "reader task is gone, stop signal not delivered".into(),
                source: None,
            });
        }

        // Wait for reader to drain and return residual bytes.
        // Extra 1s margin so the reader's own drain timeout fires first
        // with a descriptive error.
        let outer_timeout = Duration::from_millis(REEXEC_DRAIN_TIMEOUT_MS) + Duration::from_secs(1);
        let residual = match timeout(outer_timeout, reexec.result_rx).await {
            // The reader publishes its status before it drops the channel.
            Ok(result) => result.map_err(|e| EslError::ReexecReaderLost {
                reason: "reader task exited without sending result".into(),
                source: Some(Box::new(e)),
            })??,
            // Dropping the result channel stops the drain; the status is set
            // first so it already reads as ended when the caller sees the error.
            Err(e) => {
                self.shared
                    .publish_disconnect(DisconnectReason::ProtocolError(REEXEC_ABANDONED.into()));
                return Err(EslError::ReexecReaderLost {
                    reason: "timed out waiting for reader to stop".into(),
                    source: Some(Box::new(e)),
                });
            }
        };

        let fd = writer
            .as_ref()
            .as_raw_fd();

        // Mark connection as dead so other clones see it
        self.shared
            .publish_disconnect(DisconnectReason::ReexecTeardown);

        info!(
            "Re-exec teardown complete, fd={}, {} residual bytes",
            fd,
            residual.len()
        );
        Ok(ReexecHandoff { fd, residual })
    }
}
