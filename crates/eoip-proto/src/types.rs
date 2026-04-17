use std::net::IpAddr;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use crate::error::EoipError;

/// Demultiplexing key for routing received packets to the correct tunnel.
///
/// Packets are uniquely identified by their tunnel ID and source IP address.
/// This key is used in the lock-free `DashMap` for O(1) packet demux on the RX path.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub struct DemuxKey {
    pub tunnel_id: u16,
    pub peer_addr: IpAddr,
}

/// Tunnel identifier newtype.
///
/// EoIP (IPv4) supports tunnel IDs 0–65535 (full u16 range).
/// EoIPv6 (EtherIP) supports only 0–4095 (12 bits).
/// Use [`TunnelId::new_v6`] when the ID must fit in 12 bits.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub struct TunnelId(pub u16);

impl TunnelId {
    /// Maximum tunnel ID for EoIPv6 (12-bit field).
    pub const MAX_V6: u16 = 4095;

    /// Create a tunnel ID for EoIP (IPv4). Any u16 value is valid.
    pub fn new(id: u16) -> Self {
        Self(id)
    }

    /// Create a tunnel ID for EoIPv6. Returns an error if `id > 4095`.
    pub fn new_v6(id: u16) -> Result<Self, EoipError> {
        if id > Self::MAX_V6 {
            return Err(EoipError::TunnelIdOutOfRange {
                id,
                max: Self::MAX_V6,
            });
        }
        Ok(Self(id))
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

/// Static configuration for a single tunnel.
#[derive(Debug, Clone)]
pub struct TunnelConfig {
    pub tunnel_id: TunnelId,
    pub local_addr: IpAddr,
    pub remote_addr: IpAddr,
    pub iface_name: String,
    pub mtu: u16,
    pub enabled: bool,
    pub keepalive_interval: Duration,
    pub keepalive_timeout: Duration,
}

/// Per-tunnel packet/byte counters. All fields are atomic for lock-free updates
/// from concurrent TX/RX paths.
pub struct TunnelStats {
    pub tx_packets: AtomicU64,
    pub tx_bytes: AtomicU64,
    pub rx_packets: AtomicU64,
    pub rx_bytes: AtomicU64,
    pub tx_errors: AtomicU64,
    pub rx_errors: AtomicU64,
    /// Unix timestamp in milliseconds of the last received packet.
    pub last_rx_timestamp: AtomicI64,
    /// Unix timestamp in milliseconds of the last transmitted packet.
    pub last_tx_timestamp: AtomicI64,
}

impl TunnelStats {
    pub fn new() -> Self {
        Self {
            tx_packets: AtomicU64::new(0),
            tx_bytes: AtomicU64::new(0),
            rx_packets: AtomicU64::new(0),
            rx_bytes: AtomicU64::new(0),
            tx_errors: AtomicU64::new(0),
            rx_errors: AtomicU64::new(0),
            last_rx_timestamp: AtomicI64::new(0),
            last_tx_timestamp: AtomicI64::new(0),
        }
    }

    /// Take a consistent snapshot of all counters (Relaxed ordering is fine for stats).
    pub fn snapshot(&self) -> TunnelStatsSnapshot {
        TunnelStatsSnapshot {
            tx_packets: self.tx_packets.load(Ordering::Relaxed),
            tx_bytes: self.tx_bytes.load(Ordering::Relaxed),
            rx_packets: self.rx_packets.load(Ordering::Relaxed),
            rx_bytes: self.rx_bytes.load(Ordering::Relaxed),
            tx_errors: self.tx_errors.load(Ordering::Relaxed),
            rx_errors: self.rx_errors.load(Ordering::Relaxed),
            last_rx_timestamp: self.last_rx_timestamp.load(Ordering::Relaxed),
            last_tx_timestamp: self.last_tx_timestamp.load(Ordering::Relaxed),
        }
    }
}

impl Default for TunnelStats {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for TunnelStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let snap = self.snapshot();
        f.debug_struct("TunnelStats")
            .field("tx_packets", &snap.tx_packets)
            .field("tx_bytes", &snap.tx_bytes)
            .field("rx_packets", &snap.rx_packets)
            .field("rx_bytes", &snap.rx_bytes)
            .field("tx_errors", &snap.tx_errors)
            .field("rx_errors", &snap.rx_errors)
            .finish()
    }
}

/// Non-atomic snapshot of tunnel statistics for reporting.
#[derive(Debug, Clone, Copy)]
pub struct TunnelStatsSnapshot {
    pub tx_packets: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub rx_bytes: u64,
    pub tx_errors: u64,
    pub rx_errors: u64,
    pub last_rx_timestamp: i64,
    pub last_tx_timestamp: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn demux_key_equality_ipv4() {
        let a = DemuxKey {
            tunnel_id: 100,
            peer_addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        };
        let b = DemuxKey {
            tunnel_id: 100,
            peer_addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn demux_key_differs_by_tunnel_id() {
        let a = DemuxKey {
            tunnel_id: 100,
            peer_addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        };
        let b = DemuxKey {
            tunnel_id: 200,
            peer_addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        };
        assert_ne!(a, b);
    }

    #[test]
    fn demux_key_differs_by_addr() {
        let a = DemuxKey {
            tunnel_id: 100,
            peer_addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        };
        let b = DemuxKey {
            tunnel_id: 100,
            peer_addr: IpAddr::V6(Ipv6Addr::LOCALHOST),
        };
        assert_ne!(a, b);
    }

    #[test]
    fn demux_key_hashing() {
        let mut set = HashSet::new();
        for tid in 0..1000u16 {
            set.insert(DemuxKey {
                tunnel_id: tid,
                peer_addr: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            });
        }
        assert_eq!(set.len(), 1000);
    }

    #[test]
    fn tunnel_id_v6_valid() {
        assert!(TunnelId::new_v6(0).is_ok());
        assert!(TunnelId::new_v6(4095).is_ok());
    }

    #[test]
    fn tunnel_id_v6_out_of_range() {
        assert!(TunnelId::new_v6(4096).is_err());
        assert!(TunnelId::new_v6(u16::MAX).is_err());
    }

    #[test]
    fn tunnel_stats_atomic_operations() {
        let stats = TunnelStats::new();
        stats.tx_packets.fetch_add(10, Ordering::Relaxed);
        stats.rx_bytes.fetch_add(1500, Ordering::Relaxed);
        let snap = stats.snapshot();
        assert_eq!(snap.tx_packets, 10);
        assert_eq!(snap.rx_bytes, 1500);
        assert_eq!(snap.rx_packets, 0);
    }
}
