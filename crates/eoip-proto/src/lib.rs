//! Pure Rust implementation of MikroTik EoIP/EoIPv6 protocol codecs.
//!
//! This crate provides encode/decode for GRE-based EoIP (IPv4, protocol 47),
//! RFC 3378 EtherIP (IPv6, protocol 97), and a UDP encapsulation mode.
//!
//! No async runtime or I/O — this is a pure protocol library.
//!
//! # Examples
//!
//! ```
//! use eoip_proto::{DemuxKey, TunnelId, EoipError};
//! use std::net::{IpAddr, Ipv4Addr};
//!
//! let key = DemuxKey {
//!     tunnel_id: 100,
//!     peer_addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
//! };
//!
//! // EoIPv6 tunnel IDs are limited to 12 bits
//! let tid = TunnelId::new_v6(4095).unwrap();
//! assert!(TunnelId::new_v6(4096).is_err());
//! ```
//!
//! # Validation Pipelines
//!
//! For RX-path fast validation before demux, use the convenience functions:
//! - [`validate_and_parse_eoip`] — raw IP protocol 47 packets
//! - [`validate_and_parse_eoipv6`] — IPv6 protocol 97 packets
//! - [`validate_and_parse_udp_encap`] — UDP-encapsulated packets (dispatches to inner)

pub mod error;
pub mod etherip;
pub mod gre;
pub mod types;
pub mod udp_shim;
pub mod wire;

pub use error::EoipError;
pub use types::{DemuxKey, TunnelConfig, TunnelId, TunnelStats, TunnelStatsSnapshot};
pub use wire::{DaemonMsg, HelperMsg};

/// Validate and parse a raw EoIP packet (IP protocol 47).
///
/// Returns `(tunnel_id, payload_offset)` where payload_offset is the byte
/// offset to the start of the encapsulated Ethernet frame.
pub fn validate_and_parse_eoip(buf: &[u8]) -> Result<(u16, usize), EoipError> {
    gre::validate_eoip_packet(buf, buf.len())?;
    let (tunnel_id, _payload_len, hdr_len) = gre::decode_eoip_header(buf)?;
    Ok((tunnel_id, hdr_len))
}

/// Validate and parse a raw EoIPv6 packet (IPv6 protocol 97 / EtherIP).
///
/// Returns `(tunnel_id, payload_offset)` where payload_offset is the byte
/// offset to the start of the encapsulated Ethernet frame.
pub fn validate_and_parse_eoipv6(buf: &[u8]) -> Result<(u16, usize), EoipError> {
    etherip::validate_eoipv6_packet(buf, buf.len())?;
    let (tunnel_id, hdr_len) = etherip::decode_eoipv6_header(buf)?;
    Ok((tunnel_id, hdr_len))
}

/// Validate and parse a UDP-encapsulated EoIP/EoIPv6 packet.
///
/// Strips the 4-byte UDP shim, then dispatches to the appropriate inner
/// protocol validator. Returns `(tunnel_id, payload_offset)` where
/// payload_offset is the total offset from the start of `buf` to the
/// encapsulated Ethernet frame.
pub fn validate_and_parse_udp_encap(buf: &[u8]) -> Result<(u16, usize), EoipError> {
    let (inner_type, shim_len) = udp_shim::decode_udp_shim(buf)?;
    let inner = &buf[shim_len..];

    match inner_type {
        udp_shim::UDP_INNER_TYPE_EOIP => {
            let (tunnel_id, inner_offset) = validate_and_parse_eoip(inner)?;
            Ok((tunnel_id, shim_len + inner_offset))
        }
        udp_shim::UDP_INNER_TYPE_EOIPV6 => {
            let (tunnel_id, inner_offset) = validate_and_parse_eoipv6(inner)?;
            Ok((tunnel_id, shim_len + inner_offset))
        }
        _ => unreachable!("decode_udp_shim already validates inner_type"),
    }
}

#[cfg(test)]
mod pipeline_tests {
    use super::*;

    #[test]
    fn parse_eoip_packet() {
        let mut buf = vec![0u8; 8 + 60]; // header + ethernet frame
        gre::encode_eoip_header(42, 60, &mut buf).unwrap();
        let (tid, offset) = validate_and_parse_eoip(&buf).unwrap();
        assert_eq!(tid, 42);
        assert_eq!(offset, 8);
    }

    #[test]
    fn parse_eoipv6_packet() {
        let mut buf = vec![0u8; 2 + 60]; // header + ethernet frame
        etherip::encode_eoipv6_header(300, &mut buf).unwrap();
        let (tid, offset) = validate_and_parse_eoipv6(&buf).unwrap();
        assert_eq!(tid, 300);
        assert_eq!(offset, 2);
    }

    #[test]
    fn parse_udp_encap_eoip() {
        // 4B shim + 8B EoIP header + 60B ethernet
        let mut buf = vec![0u8; 4 + 8 + 60];
        udp_shim::encode_udp_shim(udp_shim::UDP_INNER_TYPE_EOIP, &mut buf).unwrap();
        gre::encode_eoip_header(100, 60, &mut buf[4..]).unwrap();
        let (tid, offset) = validate_and_parse_udp_encap(&buf).unwrap();
        assert_eq!(tid, 100);
        assert_eq!(offset, 12); // 4 + 8
    }

    #[test]
    fn parse_udp_encap_eoipv6() {
        // 4B shim + 2B EtherIP header + 60B ethernet
        let mut buf = vec![0u8; 4 + 2 + 60];
        udp_shim::encode_udp_shim(udp_shim::UDP_INNER_TYPE_EOIPV6, &mut buf).unwrap();
        etherip::encode_eoipv6_header(500, &mut buf[4..]).unwrap();
        let (tid, offset) = validate_and_parse_udp_encap(&buf).unwrap();
        assert_eq!(tid, 500);
        assert_eq!(offset, 6); // 4 + 2
    }

    #[test]
    fn parse_eoip_malformed_magic() {
        let buf = vec![0xFF; 68];
        assert!(validate_and_parse_eoip(&buf).is_err());
    }

    #[test]
    fn parse_eoipv6_truncated() {
        let buf = vec![0x03; 10]; // too short for header + min ethernet
        assert!(validate_and_parse_eoipv6(&buf).is_err());
    }

    #[test]
    fn parse_udp_encap_wrong_shim_magic() {
        let buf = vec![0x00; 72];
        assert!(validate_and_parse_udp_encap(&buf).is_err());
    }
}
