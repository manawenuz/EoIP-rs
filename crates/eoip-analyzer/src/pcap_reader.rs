use std::io::Read;
use std::time::Duration;

use pcap_file::pcap::PcapReader;
use pcap_file::pcapng::PcapNgReader;
use pcap_file::DataLink;

use crate::AnalyzerError;

/// A raw packet extracted from a pcap file with link-layer header stripped.
#[derive(Debug)]
pub struct RawPacket {
    pub timestamp: Duration,
    /// Raw bytes starting at the IP header (link-layer stripped).
    pub ip_data: Vec<u8>,
    /// Full raw bytes including link-layer for hex dumps.
    pub full_data: Vec<u8>,
}

/// Abstraction over pcap and pcapng readers.
pub enum PcapSource {
    Pcap(PcapIter),
    PcapNg(PcapNgIter),
}

impl PcapSource {
    pub fn open(mut reader: impl Read + 'static) -> Result<Self, AnalyzerError> {
        // Peek at magic to determine format
        let mut magic = [0u8; 4];
        reader
            .read_exact(&mut magic)
            .map_err(|e| AnalyzerError::Io(e.to_string()))?;

        // Reconstruct reader with peeked bytes
        let chained = std::io::Cursor::new(magic.to_vec()).chain(reader);
        let boxed: Box<dyn Read> = Box::new(chained);

        match magic {
            // pcapng: Section Header Block magic
            [0x0A, 0x0D, 0x0D, 0x0A] => {
                let ng_reader = PcapNgReader::new(boxed)
                    .map_err(|e| AnalyzerError::PcapParse(e.to_string()))?;
                Ok(PcapSource::PcapNg(PcapNgIter {
                    reader: ng_reader,
                    link_type: DataLink::ETHERNET, // updated per-interface
                }))
            }
            // pcap: either big-endian or little-endian magic
            [0xD4, 0xC3, 0xB2, 0xA1] | [0xA1, 0xB2, 0xC3, 0xD4] => {
                let pcap_reader = PcapReader::new(boxed)
                    .map_err(|e| AnalyzerError::PcapParse(e.to_string()))?;
                let link_type = pcap_reader.header().datalink;
                Ok(PcapSource::Pcap(PcapIter {
                    reader: pcap_reader,
                    link_type,
                }))
            }
            _ => Err(AnalyzerError::PcapParse(format!(
                "unrecognized file magic: {:02x?}",
                magic
            ))),
        }
    }
}

impl Iterator for PcapSource {
    type Item = Result<RawPacket, AnalyzerError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            PcapSource::Pcap(iter) => iter.next(),
            PcapSource::PcapNg(iter) => iter.next(),
        }
    }
}

pub struct PcapIter {
    reader: PcapReader<Box<dyn Read>>,
    link_type: DataLink,
}

impl Iterator for PcapIter {
    type Item = Result<RawPacket, AnalyzerError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.reader.next_packet() {
            Some(Ok(pkt)) => {
                let ts = Duration::new(pkt.timestamp.as_secs(), pkt.timestamp.subsec_nanos());
                let full_data = pkt.data.to_vec();
                match strip_link_layer(self.link_type, &full_data) {
                    Ok(ip_data) => Some(Ok(RawPacket {
                        timestamp: ts,
                        ip_data: ip_data.to_vec(),
                        full_data,
                    })),
                    Err(e) => Some(Err(e)),
                }
            }
            Some(Err(e)) => Some(Err(AnalyzerError::PcapParse(e.to_string()))),
            None => None,
        }
    }
}

pub struct PcapNgIter {
    reader: PcapNgReader<Box<dyn Read>>,
    link_type: DataLink,
}

impl Iterator for PcapNgIter {
    type Item = Result<RawPacket, AnalyzerError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.reader.next_block() {
                Some(Ok(block)) => {
                    use pcap_file::pcapng::Block;
                    match block {
                        Block::InterfaceDescription(idb) => {
                            self.link_type = idb.linktype;
                            continue;
                        }
                        Block::EnhancedPacket(epb) => {
                            let ts = Duration::new(
                                epb.timestamp.as_secs(),
                                epb.timestamp.subsec_nanos(),
                            );
                            let full_data = epb.data.to_vec();
                            match strip_link_layer(self.link_type, &full_data) {
                                Ok(ip_data) => {
                                    return Some(Ok(RawPacket {
                                        timestamp: ts,
                                        ip_data: ip_data.to_vec(),
                                        full_data,
                                    }))
                                }
                                Err(e) => return Some(Err(e)),
                            }
                        }
                        Block::SimplePacket(spb) => {
                            let full_data = spb.data.to_vec();
                            match strip_link_layer(self.link_type, &full_data) {
                                Ok(ip_data) => {
                                    return Some(Ok(RawPacket {
                                        timestamp: Duration::ZERO,
                                        ip_data: ip_data.to_vec(),
                                        full_data,
                                    }))
                                }
                                Err(e) => return Some(Err(e)),
                            }
                        }
                        _ => continue, // Skip non-packet blocks
                    }
                }
                Some(Err(e)) => return Some(Err(AnalyzerError::PcapParse(e.to_string()))),
                None => return None,
            }
        }
    }
}

/// Strip the link-layer header and return a slice starting at the IP header.
fn strip_link_layer(link_type: DataLink, data: &[u8]) -> Result<&[u8], AnalyzerError> {
    match link_type {
        DataLink::ETHERNET => {
            // 14-byte Ethernet header: 6 dst + 6 src + 2 ethertype
            if data.len() < 14 {
                return Err(AnalyzerError::PacketTooShort(
                    "Ethernet frame too short".into(),
                ));
            }
            let ethertype = u16::from_be_bytes([data[12], data[13]]);
            let offset = match ethertype {
                0x8100 => 18, // 802.1Q VLAN tag
                _ => 14,
            };
            if data.len() < offset {
                return Err(AnalyzerError::PacketTooShort(
                    "Ethernet+VLAN frame too short".into(),
                ));
            }
            Ok(&data[offset..])
        }
        DataLink::RAW => {
            // Raw IP — no link-layer header
            Ok(data)
        }
        DataLink::LINUX_SLL => {
            // Linux cooked capture v1: 16-byte header
            if data.len() < 16 {
                return Err(AnalyzerError::PacketTooShort(
                    "Linux SLL header too short".into(),
                ));
            }
            Ok(&data[16..])
        }
        DataLink::LINUX_SLL2 => {
            // Linux cooked capture v2: 20-byte header
            if data.len() < 20 {
                return Err(AnalyzerError::PacketTooShort(
                    "Linux SLL2 header too short".into(),
                ));
            }
            Ok(&data[20..])
        }
        other => Err(AnalyzerError::UnsupportedProtocol(format!(
            "unsupported link type: {:?}",
            other
        ))),
    }
}
