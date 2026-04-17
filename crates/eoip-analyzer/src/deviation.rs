use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Deviation {
    pub severity: Severity,
    pub field: String,
    pub message: String,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Severity {
    Warn,
    Error,
}

/// Check GRE/EoIP header conformance against MikroTik expectations.
pub fn check_eoip_deviations(
    magic: &[u8; 4],
    payload_len: u16,
    actual_remaining: usize,
) -> Vec<Deviation> {
    let mut devs = Vec::new();

    if *magic != [0x20, 0x01, 0x64, 0x00] {
        devs.push(Deviation {
            severity: Severity::Error,
            field: "gre.magic".into(),
            message: "GRE magic bytes do not match MikroTik EoIP".into(),
            expected: "20016400".into(),
            actual: format!("{:02x}{:02x}{:02x}{:02x}", magic[0], magic[1], magic[2], magic[3]),
        });
    }

    let plen = payload_len as usize;
    if plen > 0 && plen != actual_remaining {
        devs.push(Deviation {
            severity: if plen > actual_remaining {
                Severity::Error
            } else {
                Severity::Warn
            },
            field: "gre.payload_len".into(),
            message: "Payload length does not match actual data".into(),
            expected: format!("{plen}"),
            actual: format!("{actual_remaining}"),
        });
    }

    if plen > 0 && plen < 14 {
        devs.push(Deviation {
            severity: Severity::Error,
            field: "gre.payload_len".into(),
            message: "Payload too small for Ethernet frame (min 14)".into(),
            expected: ">=14".into(),
            actual: format!("{plen}"),
        });
    }

    devs
}

/// Check EoIPv6/EtherIP header conformance.
pub fn check_eoipv6_deviations(version_nibble: u8) -> Vec<Deviation> {
    let mut devs = Vec::new();

    if version_nibble != 0x03 {
        devs.push(Deviation {
            severity: Severity::Error,
            field: "etherip.version".into(),
            message: "EtherIP version is not 0x3".into(),
            expected: "3".into(),
            actual: format!("{version_nibble}"),
        });
    }

    devs
}

/// Check UDP shim header conformance.
pub fn check_udp_shim_deviations(reserved_byte: u8) -> Vec<Deviation> {
    let mut devs = Vec::new();

    if reserved_byte != 0x00 {
        devs.push(Deviation {
            severity: Severity::Warn,
            field: "udp_shim.reserved".into(),
            message: "UDP shim reserved byte is non-zero".into(),
            expected: "00".into(),
            actual: format!("{:02x}", reserved_byte),
        });
    }

    devs
}

/// Flag a packet as standard GRE (proto 47 but not EoIP).
pub fn flag_standard_gre(first_4_bytes: &[u8]) -> Deviation {
    Deviation {
        severity: Severity::Warn,
        field: "ip.protocol".into(),
        message: "IP protocol 47 packet is standard GRE, not MikroTik EoIP".into(),
        expected: "20016400".into(),
        actual: format!(
            "{:02x}{:02x}{:02x}{:02x}",
            first_4_bytes.first().copied().unwrap_or(0),
            first_4_bytes.get(1).copied().unwrap_or(0),
            first_4_bytes.get(2).copied().unwrap_or(0),
            first_4_bytes.get(3).copied().unwrap_or(0),
        ),
    }
}
