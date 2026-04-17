//! Tunnel handle — the per-tunnel runtime state held in the registry.

use std::sync::Arc;

use crossbeam::channel::{self, Receiver, Sender};

use eoip_proto::TunnelStats;

use crate::config::TunnelConfig;
use crate::packet::buffer::PacketBuf;
use crate::tunnel::lifecycle::AtomicTunnelState;

/// Channel capacity for RX packets per tunnel.
const RX_CHANNEL_CAP: usize = 1024;

/// Runtime handle for a single tunnel, stored in the `TunnelRegistry`.
///
/// All fields are `Arc`-wrapped or atomic for safe sharing between
/// the TX task, RX demux path, keepalive timer, and gRPC API.
#[derive(Debug)]
pub struct TunnelHandle {
    /// Static configuration for this tunnel.
    pub config: TunnelConfig,
    /// Lifecycle state (lock-free atomic).
    pub state: AtomicTunnelState,
    /// Packet/byte counters (lock-free atomics).
    pub stats: Arc<TunnelStats>,
    /// TAP file descriptor (set after helper provides it).
    pub tap_fd: Option<std::os::fd::OwnedFd>,
    /// Crossbeam sender: RX worker → TAP writer task.
    pub rx_channel: Option<Sender<PacketBuf>>,
    /// Crossbeam receiver: consumed by TAP writer task.
    pub rx_receiver: Option<Receiver<PacketBuf>>,
}

impl TunnelHandle {
    pub fn new(config: TunnelConfig) -> Self {
        use crate::tunnel::lifecycle::TunnelState;
        let (tx, rx) = channel::bounded(RX_CHANNEL_CAP);
        Self {
            config,
            state: AtomicTunnelState::new(TunnelState::Initializing),
            stats: Arc::new(TunnelStats::new()),
            tap_fd: None,
            rx_channel: Some(tx),
            rx_receiver: Some(rx),
        }
    }
}
