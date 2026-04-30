use std::net::IpAddr;

/// All errors produced by the EoIP protocol library.
#[derive(Debug, thiserror::Error)]
pub enum EoipError {
    #[error("invalid GRE header: {0}")]
    InvalidGreHeader(String),

    #[error("invalid EtherIP header: {0}")]
    InvalidEtherIpHeader(String),

    #[error("unknown tunnel: id={tunnel_id} peer={peer_addr}")]
    UnknownTunnel { tunnel_id: u16, peer_addr: IpAddr },

    #[error("packet too short: got {got} bytes, need {need}")]
    PacketTooShort { got: usize, need: usize },

    #[error("tunnel ID {id} out of range (max {max})")]
    TunnelIdOutOfRange { id: u16, max: u16 },

    #[error("payload too large: {size} bytes exceeds limit of {limit}")]
    PayloadTooLarge { size: usize, limit: usize },

    #[error("payload too small: {got} bytes, minimum {min}")]
    PayloadTooSmall { got: usize, min: usize },

    #[error("TAP error on interface {iface}: {source}")]
    TapError {
        iface: String,
        #[source]
        source: std::io::Error,
    },

    #[error("raw socket error: {0}")]
    RawSocketError(#[from] std::io::Error),

    #[error("helper process disconnected")]
    HelperDisconnected,

    #[error("configuration error: {0}")]
    ConfigError(String),

    #[error("invalid magic bytes: expected {expected:02x?}, got {got:02x?}")]
    InvalidMagicBytes { expected: &'static [u8], got: [u8; 4] },

    #[error("invalid version: expected {expected}, got {got}")]
    InvalidVersion { expected: u8, got: u8 },

    #[error("wire protocol serialization error: {0}")]
    WireSerialize(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn display_packet_too_short() {
        let e = EoipError::PacketTooShort { got: 7, need: 8 };
        assert_eq!(e.to_string(), "packet too short: got 7 bytes, need 8");
    }

    #[test]
    fn display_unknown_tunnel() {
        let e = EoipError::UnknownTunnel {
            tunnel_id: 42,
            peer_addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        };
        assert_eq!(e.to_string(), "unknown tunnel: id=42 peer=10.0.0.1");
    }

    #[test]
    fn display_invalid_magic() {
        let e = EoipError::InvalidMagicBytes {
            expected: &[0x20, 0x01, 0x64, 0x00],
            got: [0x20, 0x01, 0x64, 0x01],
        };
        assert!(e.to_string().contains("[20, 01, 64, 01]"));
    }

    #[test]
    fn io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "no access");
        let e: EoipError = io_err.into();
        assert!(matches!(e, EoipError::RawSocketError(_)));
        assert!(e.to_string().contains("no access"));
    }

    #[test]
    fn error_source_chain() {
        use std::error::Error;
        let io_err = std::io::Error::new(std::io::ErrorKind::Other, "tap failed");
        let e = EoipError::TapError {
            iface: "eoip0".into(),
            source: io_err,
        };
        assert!(e.source().is_some());
    }
}
