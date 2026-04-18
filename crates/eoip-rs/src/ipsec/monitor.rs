//! IPsec SA monitoring task.
//!
//! Periodically checks SA status via VICI and re-initiates if down.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::ipsec::IpsecManager;

/// Spawn a background task that monitors IPsec SA health.
///
/// Every 60 seconds, checks all IPsec tunnels and re-initiates any that
/// have lost their SA (e.g., after peer reboot or network disruption).
pub fn spawn_ipsec_monitor(manager: Arc<IpsecManager>, cancel: CancellationToken) {
    tokio::spawn(async move {
        // Wait 60s before first check — give SAs time to establish after startup.
        // The initial `initiate` in setup_tunnel handles the first connection.
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_secs(60)) => {}
        }

        let mut interval = tokio::time::interval(Duration::from_secs(60));

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::debug!("IPsec monitor shutting down");
                    break;
                }
                _ = interval.tick() => {
                    check_and_reinitiate(&manager);
                }
            }
        }
    });
}

fn check_and_reinitiate(manager: &IpsecManager) {
    for tid in manager.tunnel_ids() {
        if !manager.is_sa_established(tid) {
            tracing::warn!(tunnel_id = tid, "IPsec SA not established, re-initiating");
            manager.reinitiate(tid);
        } else {
            tracing::debug!(tunnel_id = tid, "IPsec SA healthy");
        }
    }
}
