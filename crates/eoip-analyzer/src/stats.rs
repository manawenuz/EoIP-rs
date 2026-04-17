use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::Duration;

use crate::decode::{DecodedPacket, EoipVariant};
use crate::AnalyzerError;

#[derive(Debug)]
pub struct SessionStats {
    pub total_packets: usize,
    pub eoip_packets: usize,
    pub eoipv6_packets: usize,
    pub udp_encap_packets: usize,
    pub standard_gre_packets: usize,
    pub skipped_packets: usize,
    pub error_packets: usize,
    pub keepalive_packets: usize,
    pub deviation_count: usize,
    pub total_bytes: usize,
    pub first_timestamp: Option<Duration>,
    pub last_timestamp: Option<Duration>,
    pub tunnels: HashMap<u16, TunnelSessionStats>,
}

#[derive(Debug)]
pub struct TunnelSessionStats {
    pub packet_count: usize,
    pub keepalive_count: usize,
    pub byte_count: usize,
    pub peers: HashSet<IpAddr>,
    pub first_seen: Duration,
    pub last_seen: Duration,
    pub inner_ethertypes: HashMap<u16, usize>,
}

impl SessionStats {
    pub fn new() -> Self {
        Self {
            total_packets: 0,
            eoip_packets: 0,
            eoipv6_packets: 0,
            udp_encap_packets: 0,
            standard_gre_packets: 0,
            skipped_packets: 0,
            error_packets: 0,
            keepalive_packets: 0,
            deviation_count: 0,
            total_bytes: 0,
            first_timestamp: None,
            last_timestamp: None,
            tunnels: HashMap::new(),
        }
    }

    pub fn record(&mut self, result: &Result<DecodedPacket, AnalyzerError>) {
        self.total_packets += 1;

        let pkt = match result {
            Ok(p) => p,
            Err(_) => {
                self.error_packets += 1;
                return;
            }
        };

        self.total_bytes += pkt.raw_bytes.len();
        self.deviation_count += pkt.deviations.len();

        // Update timestamps
        if self.first_timestamp.is_none() {
            self.first_timestamp = Some(pkt.timestamp);
        }
        self.last_timestamp = Some(pkt.timestamp);

        // Classify variant
        match &pkt.variant {
            EoipVariant::Eoip { .. } => self.eoip_packets += 1,
            EoipVariant::EoipV6 { .. } => self.eoipv6_packets += 1,
            EoipVariant::UdpEncap { .. } => self.udp_encap_packets += 1,
            EoipVariant::StandardGre { .. } => {
                self.standard_gre_packets += 1;
                return;
            }
            EoipVariant::NonEoipUdp { .. } | EoipVariant::Skipped { .. } => {
                self.skipped_packets += 1;
                return;
            }
        }

        if pkt.is_keepalive {
            self.keepalive_packets += 1;
        }

        // Per-tunnel stats
        if pkt.tunnel_id > 0 || matches!(&pkt.variant, EoipVariant::Eoip { .. } | EoipVariant::EoipV6 { .. } | EoipVariant::UdpEncap { .. })
        {
            let tunnel = self
                .tunnels
                .entry(pkt.tunnel_id)
                .or_insert_with(|| TunnelSessionStats {
                    packet_count: 0,
                    keepalive_count: 0,
                    byte_count: 0,
                    peers: HashSet::new(),
                    first_seen: pkt.timestamp,
                    last_seen: pkt.timestamp,
                    inner_ethertypes: HashMap::new(),
                });

            tunnel.packet_count += 1;
            tunnel.byte_count += pkt.raw_bytes.len();
            tunnel.last_seen = pkt.timestamp;
            tunnel.peers.insert(pkt.ip_header.src());
            tunnel.peers.insert(pkt.ip_header.dst());

            if pkt.is_keepalive {
                tunnel.keepalive_count += 1;
            }

            if let Some(ref eth) = pkt.inner_ethernet {
                *tunnel.inner_ethertypes.entry(eth.ethertype).or_insert(0) += 1;
            }
        }
    }

    pub fn record_skipped(&mut self) {
        self.total_packets += 1;
        self.skipped_packets += 1;
    }

    pub fn record_error(&mut self) {
        self.total_packets += 1;
        self.error_packets += 1;
    }

    pub fn duration(&self) -> Option<Duration> {
        match (self.first_timestamp, self.last_timestamp) {
            (Some(first), Some(last)) if last > first => Some(last - first),
            _ => None,
        }
    }
}
