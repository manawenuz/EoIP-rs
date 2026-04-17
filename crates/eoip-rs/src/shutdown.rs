//! Graceful shutdown coordination using `CancellationToken`.
//!
//! Handles SIGTERM and SIGINT, propagating cancellation to all
//! spawned tasks via a shared token.

use tokio_util::sync::CancellationToken;

/// Coordinates graceful shutdown across all daemon tasks.
#[derive(Clone)]
pub struct ShutdownCoordinator {
    token: CancellationToken,
}

impl ShutdownCoordinator {
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }

    /// Get a clone of the cancellation token for use in spawned tasks.
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Trigger shutdown (cancels all tokens).
    pub fn shutdown(&self) {
        tracing::warn!("shutdown initiated");
        self.token.cancel();
    }

    /// Returns true if shutdown has been initiated.
    pub fn is_shutting_down(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Spawn a tokio task that listens for OS signals and triggers shutdown.
    pub fn spawn_signal_handler(&self) {
        let coordinator = self.clone();
        tokio::spawn(async move {
            let ctrl_c = tokio::signal::ctrl_c();

            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                let mut sigterm =
                    signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");

                tokio::select! {
                    _ = ctrl_c => {
                        tracing::info!("received SIGINT");
                    }
                    _ = sigterm.recv() => {
                        tracing::info!("received SIGTERM");
                    }
                }
            }

            #[cfg(not(unix))]
            {
                ctrl_c.await.expect("failed to listen for ctrl_c");
                tracing::info!("received Ctrl+C");
            }

            coordinator.shutdown();
        });
    }
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self::new()
    }
}
