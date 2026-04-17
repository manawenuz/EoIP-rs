use crate::AnalyzerError;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct EthernetFrame {
    pub dst_mac: [u8; 6],
    pub src_mac: [u8; 6],
    pub vlan: Option<VlanTag>,
    pub ethertype: u16,
    pub payload_offset: usize,
}

#[derive(Debug, Clone)]
pub struct VlanTag {
    pub pcp: u8,
    pub dei: bool,
    pub vid: u16,
}

/// Parse an Ethernet frame header (optionally with 802.1Q VLAN tag).
pub fn parse_ethernet_frame(data: &[u8]) -> Result<EthernetFrame, AnalyzerError> {
    if data.len() < 14 {
        return Err(AnalyzerError::PacketTooShort(format!(
            "Ethernet: got {} bytes, need 14",
            data.len()
        )));
    }

    let mut dst_mac = [0u8; 6];
    let mut src_mac = [0u8; 6];
    dst_mac.copy_from_slice(&data[0..6]);
    src_mac.copy_from_slice(&data[6..12]);

    let ethertype = u16::from_be_bytes([data[12], data[13]]);

    // Check for 802.1Q VLAN tag
    if ethertype == 0x8100 {
        if data.len() < 18 {
            return Err(AnalyzerError::PacketTooShort(format!(
                "Ethernet+VLAN: got {} bytes, need 18",
                data.len()
            )));
        }

        let tci = u16::from_be_bytes([data[14], data[15]]);
        let vlan = VlanTag {
            pcp: (tci >> 13) as u8,
            dei: (tci >> 12) & 1 == 1,
            vid: tci & 0x0FFF,
        };
        let real_ethertype = u16::from_be_bytes([data[16], data[17]]);

        Ok(EthernetFrame {
            dst_mac,
            src_mac,
            vlan: Some(vlan),
            ethertype: real_ethertype,
            payload_offset: 18,
        })
    } else {
        Ok(EthernetFrame {
            dst_mac,
            src_mac,
            vlan: None,
            ethertype,
            payload_offset: 14,
        })
    }
}

/// Format a MAC address as colon-separated hex.
pub fn format_mac(mac: &[u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

/// Human-readable name for common EtherTypes.
pub fn ethertype_name(et: u16) -> &'static str {
    match et {
        0x0800 => "IPv4",
        0x0806 => "ARP",
        0x8035 => "RARP",
        0x86DD => "IPv6",
        0x8100 => "802.1Q",
        0x88A8 => "802.1ad",
        0x8847 => "MPLS",
        0x8848 => "MPLS-MC",
        0x88CC => "LLDP",
        0x88F7 => "PTP",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_frame() {
        let mut data = vec![0u8; 60];
        // dst MAC
        data[0..6].copy_from_slice(&[0xd8, 0x50, 0xe6, 0xaa, 0xbb, 0xcc]);
        // src MAC
        data[6..12].copy_from_slice(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        // EtherType = IPv4
        data[12..14].copy_from_slice(&[0x08, 0x00]);

        let frame = parse_ethernet_frame(&data).unwrap();
        assert_eq!(frame.dst_mac, [0xd8, 0x50, 0xe6, 0xaa, 0xbb, 0xcc]);
        assert_eq!(frame.src_mac, [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        assert_eq!(frame.ethertype, 0x0800);
        assert!(frame.vlan.is_none());
        assert_eq!(frame.payload_offset, 14);
    }

    #[test]
    fn parse_vlan_frame() {
        let mut data = vec![0u8; 64];
        data[0..6].copy_from_slice(&[0xff; 6]); // broadcast dst
        data[6..12].copy_from_slice(&[0x00; 6]); // src
        data[12..14].copy_from_slice(&[0x81, 0x00]); // 802.1Q
        // TCI: PCP=5, DEI=0, VID=100
        let tci: u16 = (5 << 13) | 100;
        data[14..16].copy_from_slice(&tci.to_be_bytes());
        data[16..18].copy_from_slice(&[0x08, 0x06]); // ARP

        let frame = parse_ethernet_frame(&data).unwrap();
        assert_eq!(frame.ethertype, 0x0806);
        let vlan = frame.vlan.unwrap();
        assert_eq!(vlan.pcp, 5);
        assert!(!vlan.dei);
        assert_eq!(vlan.vid, 100);
        assert_eq!(frame.payload_offset, 18);
    }

    #[test]
    fn parse_too_short() {
        assert!(parse_ethernet_frame(&[0; 13]).is_err());
    }

    #[test]
    fn format_mac_address() {
        assert_eq!(
            format_mac(&[0xd8, 0x50, 0xe6, 0xaa, 0xbb, 0xcc]),
            "d8:50:e6:aa:bb:cc"
        );
    }

    #[test]
    fn ethertype_names() {
        assert_eq!(ethertype_name(0x0800), "IPv4");
        assert_eq!(ethertype_name(0x0806), "ARP");
        assert_eq!(ethertype_name(0x86DD), "IPv6");
        assert_eq!(ethertype_name(0x1234), "Unknown");
    }
}
