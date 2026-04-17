use std::time::Duration;

use eoip_proto::gre::{self, EOIP_HEADER_LEN, EOIP_MAGIC};
use eoip_proto::etherip;
use eoip_proto::udp_shim;

use crate::deviation::{
    self, check_eoip_deviations, check_eoipv6_deviations, check_udp_shim_deviations, Deviation,
};
use crate::ethernet::{self, EthernetFrame};
use crate::ip::{self, IpHeader, PROTO_ETHERIP, PROTO_GRE, PROTO_UDP};
use crate::pcap_reader::RawPacket;
use crate::AnalyzerError;

/// A fully decoded EoIP packet with all layers parsed.
#[derive(Debug)]
pub struct DecodedPacket {
    pub packet_number: usize,
    pub timestamp: Duration,
    pub ip_header: IpHeader,
    pub variant: EoipVariant,
    pub tunnel_id: u16,
    pub is_keepalive: bool,
    pub inner_ethernet: Option<EthernetFrame>,
    pub deviations: Vec<Deviation>,
    pub raw_bytes: Vec<u8>,
}

#[derive(Debug)]
pub enum EoipVariant {
    /// EoIP over IPv4 (IP protocol 47, GRE-like)
    Eoip {
        magic: [u8; 4],
        payload_len: u16,
        tunnel_id: u16,
    },
    /// EoIPv6 over IPv6 (IP protocol 97, EtherIP)
    EoipV6 {
        version_nibble: u8,
        tunnel_id: u16,
    },
    /// UDP-encapsulated with EoIP-rs shim
    UdpEncap {
        udp_src_port: u16,
        udp_dst_port: u16,
        inner_type: u8,
        reserved_byte: u8,
        inner: Box<EoipVariant>,
    },
    /// Standard GRE (not EoIP) — proto 47 but wrong magic
    StandardGre {
        first_bytes: [u8; 4],
    },
    /// Non-EoIP UDP packet (no EO shim magic)
    NonEoipUdp {
        udp_src_port: u16,
        udp_dst_port: u16,
    },
    /// Unrecognized IP protocol
    Skipped {
        protocol: u8,
    },
}

/// Decode a single raw packet from pcap into a fully parsed structure.
pub fn decode_packet(
    packet_number: usize,
    raw: &RawPacket,
    udp_port: u16,
) -> Result<DecodedPacket, AnalyzerError> {
    let (ip_header, ip_payload) = ip::parse_ip_header(&raw.ip_data)?;
    let mut deviations = Vec::new();

    let protocol = ip_header.protocol();

    let (variant, tunnel_id, is_keepalive, inner_ethernet) = match protocol {
        PROTO_GRE => decode_eoip(ip_payload, &mut deviations)?,
        PROTO_ETHERIP => decode_eoipv6(ip_payload, &mut deviations)?,
        PROTO_UDP => decode_udp(ip_payload, udp_port, &mut deviations)?,
        other => (EoipVariant::Skipped { protocol: other }, 0, false, None),
    };

    Ok(DecodedPacket {
        packet_number,
        timestamp: raw.timestamp,
        ip_header,
        variant,
        tunnel_id,
        is_keepalive,
        inner_ethernet,
        deviations,
        raw_bytes: raw.full_data.clone(),
    })
}

fn decode_eoip(
    payload: &[u8],
    deviations: &mut Vec<Deviation>,
) -> Result<(EoipVariant, u16, bool, Option<EthernetFrame>), AnalyzerError> {
    if payload.len() < 4 {
        return Err(AnalyzerError::PacketTooShort(
            "GRE payload too short".into(),
        ));
    }

    // Check if this is actually EoIP (MikroTik magic) or standard GRE
    let mut magic = [0u8; 4];
    magic.copy_from_slice(&payload[0..4]);

    if magic != EOIP_MAGIC {
        deviations.push(deviation::flag_standard_gre(&payload[..4.min(payload.len())]));
        return Ok((
            EoipVariant::StandardGre {
                first_bytes: magic,
            },
            0,
            false,
            None,
        ));
    }

    if payload.len() < EOIP_HEADER_LEN {
        return Err(AnalyzerError::PacketTooShort(
            "EoIP header incomplete".into(),
        ));
    }

    let (tunnel_id, payload_len, _hdr_len) = gre::decode_eoip_header(payload)
        .map_err(|e| AnalyzerError::Decode(e.to_string()))?;

    let eth_data = &payload[EOIP_HEADER_LEN..];
    let is_keepalive = payload_len == 0;

    // Run deviation checks
    deviations.extend(check_eoip_deviations(&magic, payload_len, eth_data.len()));

    let inner_ethernet = if !is_keepalive && eth_data.len() >= 14 {
        ethernet::parse_ethernet_frame(eth_data).ok()
    } else {
        None
    };

    Ok((
        EoipVariant::Eoip {
            magic,
            payload_len,
            tunnel_id,
        },
        tunnel_id,
        is_keepalive,
        inner_ethernet,
    ))
}

fn decode_eoipv6(
    payload: &[u8],
    deviations: &mut Vec<Deviation>,
) -> Result<(EoipVariant, u16, bool, Option<EthernetFrame>), AnalyzerError> {
    if payload.len() < 2 {
        return Err(AnalyzerError::PacketTooShort(
            "EtherIP payload too short".into(),
        ));
    }

    let version_nibble = payload[0] & 0x0F;
    deviations.extend(check_eoipv6_deviations(version_nibble));

    let (tunnel_id, hdr_len) = etherip::decode_eoipv6_header(payload)
        .map_err(|e| AnalyzerError::Decode(e.to_string()))?;

    let eth_data = &payload[hdr_len..];
    let is_keepalive = eth_data.is_empty();

    let inner_ethernet = if !is_keepalive && eth_data.len() >= 14 {
        ethernet::parse_ethernet_frame(eth_data).ok()
    } else {
        None
    };

    Ok((
        EoipVariant::EoipV6 {
            version_nibble,
            tunnel_id,
        },
        tunnel_id,
        is_keepalive,
        inner_ethernet,
    ))
}

fn decode_udp(
    payload: &[u8],
    expected_port: u16,
    deviations: &mut Vec<Deviation>,
) -> Result<(EoipVariant, u16, bool, Option<EthernetFrame>), AnalyzerError> {
    if payload.len() < 8 {
        return Err(AnalyzerError::PacketTooShort(
            "UDP header too short".into(),
        ));
    }

    let src_port = u16::from_be_bytes([payload[0], payload[1]]);
    let dst_port = u16::from_be_bytes([payload[2], payload[3]]);
    let udp_payload = &payload[8..];

    // Only attempt EO shim decode if port matches
    if src_port != expected_port && dst_port != expected_port {
        return Ok((
            EoipVariant::NonEoipUdp {
                udp_src_port: src_port,
                udp_dst_port: dst_port,
            },
            0,
            false,
            None,
        ));
    }

    // Try to decode EO shim
    match udp_shim::decode_udp_shim(udp_payload) {
        Ok((inner_type, shim_len)) => {
            let reserved_byte = if udp_payload.len() >= 4 {
                udp_payload[3]
            } else {
                0
            };
            deviations.extend(check_udp_shim_deviations(reserved_byte));

            let inner_data = &udp_payload[shim_len..];

            let (inner_variant, tunnel_id, is_keepalive, inner_ethernet) = match inner_type {
                udp_shim::UDP_INNER_TYPE_EOIP => decode_eoip(inner_data, deviations)?,
                udp_shim::UDP_INNER_TYPE_EOIPV6 => decode_eoipv6(inner_data, deviations)?,
                _ => unreachable!("decode_udp_shim validates type"),
            };

            Ok((
                EoipVariant::UdpEncap {
                    udp_src_port: src_port,
                    udp_dst_port: dst_port,
                    inner_type,
                    reserved_byte,
                    inner: Box::new(inner_variant),
                },
                tunnel_id,
                is_keepalive,
                inner_ethernet,
            ))
        }
        Err(_) => {
            // Not an EoIP-rs UDP packet
            Ok((
                EoipVariant::NonEoipUdp {
                    udp_src_port: src_port,
                    udp_dst_port: dst_port,
                },
                0,
                false,
                None,
            ))
        }
    }
}
