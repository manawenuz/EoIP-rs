use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::AnalyzerError;

/// IP protocol numbers we care about.
pub const PROTO_GRE: u8 = 47;
pub const PROTO_ETHERIP: u8 = 97;
pub const PROTO_UDP: u8 = 17;

#[derive(Debug, Clone)]
pub enum IpHeader {
    V4(Ipv4Header),
    V6(Ipv6Header),
}

impl IpHeader {
    pub fn src(&self) -> IpAddr {
        match self {
            IpHeader::V4(h) => IpAddr::V4(h.src),
            IpHeader::V6(h) => IpAddr::V6(h.src),
        }
    }

    pub fn dst(&self) -> IpAddr {
        match self {
            IpHeader::V4(h) => IpAddr::V4(h.dst),
            IpHeader::V6(h) => IpAddr::V6(h.dst),
        }
    }

    pub fn protocol(&self) -> u8 {
        match self {
            IpHeader::V4(h) => h.protocol,
            IpHeader::V6(h) => h.next_header,
        }
    }

    pub fn ttl(&self) -> u8 {
        match self {
            IpHeader::V4(h) => h.ttl,
            IpHeader::V6(h) => h.hop_limit,
        }
    }

    pub fn total_length(&self) -> u16 {
        match self {
            IpHeader::V4(h) => h.total_length,
            IpHeader::V6(h) => 40 + h.payload_length,
        }
    }

    #[allow(dead_code)]
    pub fn header_len(&self) -> usize {
        match self {
            IpHeader::V4(h) => h.header_len,
            IpHeader::V6(_) => 40,
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Ipv4Header {
    pub header_len: usize,
    pub total_length: u16,
    pub ttl: u8,
    pub protocol: u8,
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
}

#[derive(Debug, Clone)]
pub struct Ipv6Header {
    pub payload_length: u16,
    pub next_header: u8,
    pub hop_limit: u8,
    pub src: Ipv6Addr,
    pub dst: Ipv6Addr,
}

/// Parse an IP header from raw bytes. Returns the header and a slice to the payload.
pub fn parse_ip_header(data: &[u8]) -> Result<(IpHeader, &[u8]), AnalyzerError> {
    if data.is_empty() {
        return Err(AnalyzerError::PacketTooShort("IP header: empty".into()));
    }

    let version = data[0] >> 4;
    match version {
        4 => parse_ipv4(data),
        6 => parse_ipv6(data),
        _ => Err(AnalyzerError::UnsupportedProtocol(format!(
            "IP version {version}"
        ))),
    }
}

fn parse_ipv4(data: &[u8]) -> Result<(IpHeader, &[u8]), AnalyzerError> {
    if data.len() < 20 {
        return Err(AnalyzerError::PacketTooShort(format!(
            "IPv4: got {} bytes, need 20",
            data.len()
        )));
    }

    let ihl = (data[0] & 0x0F) as usize;
    let header_len = ihl * 4;
    if header_len < 20 || data.len() < header_len {
        return Err(AnalyzerError::PacketTooShort(format!(
            "IPv4: IHL={ihl} ({}B) but got {}B",
            header_len,
            data.len()
        )));
    }

    let total_length = u16::from_be_bytes([data[2], data[3]]);
    let ttl = data[8];
    let protocol = data[9];
    let src = Ipv4Addr::new(data[12], data[13], data[14], data[15]);
    let dst = Ipv4Addr::new(data[16], data[17], data[18], data[19]);

    let payload_end = (total_length as usize).min(data.len());
    let payload = &data[header_len..payload_end];

    Ok((
        IpHeader::V4(Ipv4Header {
            header_len,
            total_length,
            ttl,
            protocol,
            src,
            dst,
        }),
        payload,
    ))
}

fn parse_ipv6(data: &[u8]) -> Result<(IpHeader, &[u8]), AnalyzerError> {
    if data.len() < 40 {
        return Err(AnalyzerError::PacketTooShort(format!(
            "IPv6: got {} bytes, need 40",
            data.len()
        )));
    }

    let payload_length = u16::from_be_bytes([data[4], data[5]]);
    let next_header = data[6];
    let hop_limit = data[7];

    let mut src_bytes = [0u8; 16];
    src_bytes.copy_from_slice(&data[8..24]);
    let mut dst_bytes = [0u8; 16];
    dst_bytes.copy_from_slice(&data[24..40]);

    let payload_end = (40 + payload_length as usize).min(data.len());
    let payload = &data[40..payload_end];

    Ok((
        IpHeader::V6(Ipv6Header {
            payload_length,
            next_header,
            hop_limit,
            src: Ipv6Addr::from(src_bytes),
            dst: Ipv6Addr::from(dst_bytes),
        }),
        payload,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ipv4_basic() {
        // Minimal IPv4 header: version=4, IHL=5, total_length=28, proto=47 (GRE), src=10.0.0.1, dst=10.0.0.2
        let mut pkt = vec![0u8; 28];
        pkt[0] = 0x45; // version=4, IHL=5
        pkt[2..4].copy_from_slice(&28u16.to_be_bytes()); // total_length
        pkt[8] = 64; // TTL
        pkt[9] = 47; // protocol = GRE
        pkt[12..16].copy_from_slice(&[10, 0, 0, 1]); // src
        pkt[16..20].copy_from_slice(&[10, 0, 0, 2]); // dst

        let (hdr, payload) = parse_ip_header(&pkt).unwrap();
        assert_eq!(hdr.protocol(), 47);
        assert_eq!(hdr.src(), IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        assert_eq!(hdr.dst(), IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
        assert_eq!(hdr.ttl(), 64);
        assert_eq!(hdr.header_len(), 20);
        assert_eq!(payload.len(), 8); // 28 - 20
    }

    #[test]
    fn parse_ipv6_basic() {
        let mut pkt = vec![0u8; 48]; // 40 header + 8 payload
        pkt[0] = 0x60; // version=6
        pkt[4..6].copy_from_slice(&8u16.to_be_bytes()); // payload_length
        pkt[6] = 97; // next_header = EtherIP
        pkt[7] = 64; // hop_limit
        // src = ::1
        pkt[23] = 1;
        // dst = ::2
        pkt[39] = 2;

        let (hdr, payload) = parse_ip_header(&pkt).unwrap();
        assert_eq!(hdr.protocol(), 97);
        assert_eq!(hdr.ttl(), 64);
        assert_eq!(payload.len(), 8);
    }

    #[test]
    fn parse_too_short() {
        assert!(parse_ip_header(&[]).is_err());
        assert!(parse_ip_header(&[0x45; 10]).is_err());
    }
}
