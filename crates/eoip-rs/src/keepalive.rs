//! Keepalive state machine: send zero-payload EoIP packets at regular intervals,
//! detect stale tunnels when no packets are received within the timeout.
//!
//! - **Active**: Sending keepalives, receiving packets normally.
//! - **Stale**: Keepalive timeout expired. TX suspended, RX still active for recovery.
//! - Back to **Active** when RX resumes.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::packet::tx;
use crate::tunnel::handle::TunnelHandle;
use crate::tunnel::lifecycle::TunnelState;

/// Spawn a keepalive task for a single tunnel.
///
/// Periodically sends zero-payload EoIP packets and monitors `last_rx_timestamp`
/// to detect stale tunnels.
pub fn spawn_keepalive_task(
    handle: Arc<TunnelHandle>,
    raw_fd: std::os::fd::RawFd,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let interval = Duration::from_secs(handle.config.keepalive_interval_secs);
    let timeout = Duration::from_secs(handle.config.keepalive_timeout_secs);
    let tunnel_id = handle.config.tunnel_id;

    tokio::spawn(async move {
        tracing::debug!(tunnel_id, ?interval, ?timeout, "keepalive task started");
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await; // first tick is immediate

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = ticker.tick() => {
                    // Send keepalive
                    if let Err(e) = tx::send_keepalive(raw_fd, &handle).await {
                        tracing::warn!(tunnel_id, %e, "failed to send keepalive");
                    }

                    // Check if tunnel has gone stale
                    let last_rx_ms = handle.stats.last_rx_timestamp.load(Ordering::Relaxed);
                    if last_rx_ms > 0 {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);

                        let elapsed = Duration::from_millis((now_ms - last_rx_ms).max(0) as u64);

                        let current_state = handle.state.load();

                        if elapsed > timeout && current_state == TunnelState::Active {
                            if handle.state.transition(TunnelState::Active, TunnelState::Stale).is_ok() {
                                tracing::warn!(
                                    tunnel_id,
                                    elapsed_secs = elapsed.as_secs(),
                                    "tunnel went stale (keepalive timeout)"
                                );
                            }
                        } else if elapsed <= timeout && current_state == TunnelState::Stale {
                            // Recovery: RX resumed
                            if handle.state.transition(TunnelState::Stale, TunnelState::Active).is_ok() {
                                tracing::info!(tunnel_id, "tunnel recovered from stale");
                            }
                        }
                    }
                }
            }
        }

        tracing::debug!(tunnel_id, "keepalive task stopped");
    })
}
