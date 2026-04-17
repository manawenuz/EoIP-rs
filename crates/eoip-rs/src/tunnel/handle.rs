//! Tunnel handle — the per-tunnel runtime state held in the registry.

use std::sync::Arc;

use eoip_proto::TunnelStats;

use crate::config::TunnelConfig;
use crate::tunnel::lifecycle::AtomicTunnelState;

#[cfg(unix)]
mod unix_fields {
    pub use crossbeam::channel::{self, Receiver, Sender};
    pub use crate::packet::buffer::PacketBuf;
    pub const RX_CHANNEL_CAP: usize = 1024;
}

/// Runtime handle for a single tunnel, stored in the `TunnelRegistry`.
#[derive(Debug)]
pub struct TunnelHandle {
    pub config: TunnelConfig,
    pub state: AtomicTunnelState,
    pub stats: Arc<TunnelStats>,
    #[cfg(unix)]
    pub tap_fd: Option<std::os::fd::OwnedFd>,
    #[cfg(unix)]
    pub rx_channel: Option<unix_fields::Sender<unix_fields::PacketBuf>>,
    #[cfg(unix)]
    pub rx_receiver: Option<unix_fields::Receiver<unix_fields::PacketBuf>>,
}

impl TunnelHandle {
    pub fn new(config: TunnelConfig) -> Self {
        use crate::tunnel::lifecycle::TunnelState;

        #[cfg(unix)]
        let (tx, rx) = unix_fields::channel::bounded(unix_fields::RX_CHANNEL_CAP);

        Self {
            config,
            state: AtomicTunnelState::new(TunnelState::Initializing),
            stats: Arc::new(TunnelStats::new()),
            #[cfg(unix)]
            tap_fd: None,
            #[cfg(unix)]
            rx_channel: Some(tx),
            #[cfg(unix)]
            rx_receiver: Some(rx),
        }
    }
}
