//! Re-exec teardown and adopt demo
//!
//! Demonstrates the full re-exec cycle without actually calling exec():
//! 1. Connect to FreeSWITCH, subscribe to events
//! 2. Teardown: stop the reader, get the fd and residual bytes
//! 3. Adopt: reconstruct a new EslClient from the same fd
//! 4. Verify events still flow on the adopted connection
//!
//! Requires FreeSWITCH ESL. Configure via ESL_HOST, ESL_PORT, ESL_PASSWORD env vars.
//!
//! Usage: cargo run --example reexec_demo

#[cfg(unix)]
mod common;

#[cfg(unix)]
mod demo {
    use crate::common;
    use freeswitch_esl_tokio::{EslClient, EslEventStream, EslEventType, EventFormat, EventHeader};
    use std::os::unix::io::BorrowedFd;
    use std::time::Duration;
    use tracing::info;

    /// FreeSWITCH fires HEARTBEAT about every 20s, so one that has not arrived
    /// by now is not late.
    const HEARTBEAT_WAIT: Duration = Duration::from_secs(25);

    /// Wait for one heartbeat and report what it carried.
    ///
    /// Failing here has to be an error: the demo's only claim is that events
    /// flow, and a version that logged and carried on printed "complete" and
    /// exited 0 having proved nothing.
    async fn expect_heartbeat(
        events: &mut EslEventStream,
        stage: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match tokio::time::timeout(HEARTBEAT_WAIT, events.recv()).await {
            Ok(Some(Ok(event))) => {
                info!(
                    "{stage}: heartbeat, {}",
                    event
                        .header(EventHeader::EventInfo)
                        .unwrap_or("(no info)")
                );
                Ok(())
            }
            Ok(Some(Err(e))) => Err(format!("{stage}: event error: {e}").into()),
            Ok(None) => Err(format!("{stage}: event stream closed").into()),
            Err(_) => Err(format!("{stage}: no heartbeat within {HEARTBEAT_WAIT:?}").into()),
        }
    }

    pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
        tracing_subscriber::fmt::init();

        // Phase 1: connect and subscribe
        let (client, mut events) = common::connect_from_env().await?;

        client
            .subscribe_events(EventFormat::Plain, &[EslEventType::Heartbeat])
            .await?;

        expect_heartbeat(&mut events, "before teardown").await?;
        drop(events);

        // Phase 2: teardown
        info!("Tearing down for re-exec...");
        let handoff = client
            .teardown_for_reexec()
            .await?;
        info!(
            "Teardown complete: fd={}, {} residual bytes",
            handoff.fd(),
            handoff
                .residual()
                .len()
        );

        // A real re-exec clears CLOEXEC and execs here; this one stays in the
        // process, so it dup()s past the reactor registration the old client
        // still holds. Both steps are in docs/reexec.md.
        let dup_fd = nix::unistd::dup(unsafe { BorrowedFd::borrow_raw(handoff.fd()) })?;
        std::mem::forget(client);

        // Phase 3: adopt the stream (simulating new process)
        info!("Adopting stream (simulating new process)...");

        // Reconstruct a TcpStream from the dup'd fd.
        // OwnedFd transfers ownership to TcpStream (no close-on-drop race).
        let std_stream = std::net::TcpStream::from(dup_fd);
        std_stream.set_nonblocking(true)?;
        let tokio_stream = tokio::net::TcpStream::from_std(std_stream)?;

        let (new_client, mut new_events) =
            EslClient::adopt_stream(tokio_stream, handoff.residual())?;

        // Exercise the write half too. Reading alone would leave the demo
        // claiming a working connection on the evidence of one direction.
        let response = new_client
            .api("status")
            .await?;
        let status = response.api_result()?;
        info!(
            "adopted connection answers commands: {}",
            status
                .lines()
                .next()
                .unwrap_or("(empty)")
        );

        // The old subscription survives on the TCP connection, so
        // heartbeats arrive without re-subscribing.
        expect_heartbeat(&mut new_events, "after adopt").await?;

        info!("re-exec demo complete");
        new_client
            .disconnect()
            .await?;
        Ok(())
    }
}

#[cfg(unix)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    demo::run().await
}

#[cfg(not(unix))]
fn main() {
    eprintln!("reexec_demo requires unix (raw fd support)");
    std::process::exit(1);
}
