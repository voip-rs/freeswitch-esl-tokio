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
use super::{ConnectionStatus, DisconnectReason, EslClient};

/// Result type for the reexec drain operation (residual bytes or error).
#[cfg(unix)]
pub(crate) type ReexecResult = Result<Vec<u8>, EslError>;

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
    /// - No ESL command may be in-flight (pending reply). Returns
    ///   [`EslError::ReexecFailed`] if a command is pending.
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
    /// - The caller **must not drop** the `EslClient` before `exec()` (or
    ///   must [`std::mem::forget`] it) to keep the fd open.
    /// - The caller is responsible for clearing `CLOEXEC` on the fd.
    pub async fn teardown_for_reexec(&self) -> EslResult<ReexecHandoff> {
        use crate::constants::REEXEC_DRAIN_TIMEOUT_MS;
        use std::os::unix::io::AsRawFd;

        // Reject if a command is in-flight
        {
            let pending = self
                .shared
                .pending_reply
                .lock()
                .await;
            if pending
                .waiting
                .is_some()
            {
                return Err(EslError::ReexecFailed {
                    reason: "command still in-flight".into(),
                    source: None,
                });
            }
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
        self.shared
            .liveness_timeout_ms
            .store(0, Ordering::Relaxed);

        // A dropped receiver means the reader is already gone; reporting it here
        // keeps it out of the drain timeout below, which would name the wrong cause.
        if reexec
            .stop_tx
            .send(())
            .is_err()
        {
            return Err(EslError::ReexecFailed {
                reason: "reader task is gone, stop signal not delivered".into(),
                source: None,
            });
        }

        // Wait for reader to drain and return residual bytes.
        // Extra 1s margin so the reader's own drain timeout fires first
        // with a descriptive error.
        let outer_timeout = Duration::from_millis(REEXEC_DRAIN_TIMEOUT_MS) + Duration::from_secs(1);
        let residual = timeout(outer_timeout, reexec.result_rx)
            .await
            .map_err(|e| EslError::ReexecFailed {
                reason: "timed out waiting for reader to stop".into(),
                source: Some(Box::new(e)),
            })?
            .map_err(|e| EslError::ReexecFailed {
                reason: "reader task exited without sending result".into(),
                source: Some(Box::new(e)),
            })??;

        // Get fd from writer
        let writer = self
            .writer
            .lock()
            .await;
        let fd = writer
            .as_ref()
            .as_raw_fd();

        // Mark connection as dead so other clones see it
        self.shared
            .status_tx
            .send_replace(ConnectionStatus::Disconnected(
                DisconnectReason::ReexecTeardown,
            ));

        info!(
            "Re-exec teardown complete, fd={}, {} residual bytes",
            fd,
            residual.len()
        );
        Ok(ReexecHandoff { fd, residual })
    }
}
