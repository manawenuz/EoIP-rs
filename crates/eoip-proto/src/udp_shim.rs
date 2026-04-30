//! UDP encapsulation shim header codec (EoIP-rs extension, NOT MikroTik-compatible).
//!
//! Wraps EoIP or EoIPv6 packets in UDP datagrams for NAT traversal.
//! Default port: 26969 (configurable).
//!
//! ## Wire format (4 bytes)
//!
//! ```text
//! Offset  Size  Field
//! 0       2B    Magic: [0x45, 0x4F] (ASCII "EO")
//! 2       1B    Inner type: 0x04 = EoIP, 0x06 = EoIPv6
//! 3       1B    Reserved: 0x00
//! ```
//!
//! After the 4-byte shim, the inner EoIP (8B) or EoIPv6 (2B) header follows,
//! then the Ethernet frame payload.

use crate::error::EoipError;

/// Magic bytes identifying an EoIP-rs UDP-encapsulated packet (ASCII "EO").
pub const UDP_SHIM_MAGIC: [u8; 2] = [0x45, 0x4F];

/// UDP shim header length in bytes (fixed).
pub const UDP_SHIM_HEADER_LEN: usize = 4;

/// Inner type code for EoIP (IPv4 GRE-style, 8-byte inner header).
pub const UDP_INNER_TYPE_EOIP: u8 = 0x04;

/// Inner type code for EoIPv6 (EtherIP-style, 2-byte inner header).
pub const UDP_INNER_TYPE_EOIPV6: u8 = 0x06;

/// Encode a 4-byte UDP shim header.
///
/// # Examples
///
/// ```
/// # use eoip_proto::udp_shim::*;
/// let mut buf = [0u8; 4];
/// encode_udp_shim(UDP_INNER_TYPE_EOIP, &mut buf).unwrap();
/// assert_eq!(buf, [0x45, 0x4F, 0x04, 0x00]);
/// ```
pub fn encode_udp_shim(inner_type: u8, buf: &mut [u8]) -> Result<(), EoipError> {
    if inner_type != UDP_INNER_TYPE_EOIP && inner_type != UDP_INNER_TYPE_EOIPV6 {
        return Err(EoipError::InvalidVersion {
            expected: UDP_INNER_TYPE_EOIP, // "0x04 or 0x06"
            got: inner_type,
        });
    }

    if buf.len() < UDP_SHIM_HEADER_LEN {
        return Err(EoipError::PacketTooShort {
            got: buf.len(),
            need: UDP_SHIM_HEADER_LEN,
        });
    }

    buf[0..2].copy_from_slice(&UDP_SHIM_MAGIC);
    buf[2] = inner_type;
    // Reserved byte must be 0x00 on transmission
    buf[3] = 0x00;

    Ok(())
}

/// Decode a 4-byte UDP shim header.
///
/// Returns `(inner_type, shim_len)` where `shim_len` is always 4.
/// The inner packet (EoIP or EoIPv6) starts at `buf[shim_len..]`.
///
/// # Examples
///
/// ```
/// # use eoip_proto::udp_shim::*;
/// let (inner_type, offset) = decode_udp_shim(&[0x45, 0x4F, 0x06, 0x00]).unwrap();
/// assert_eq!(inner_type, UDP_INNER_TYPE_EOIPV6);
/// assert_eq!(offset, 4);
/// ```
pub fn decode_udp_shim(buf: &[u8]) -> Result<(u8, usize), EoipError> {
    if buf.len() < UDP_SHIM_HEADER_LEN {
        return Err(EoipError::PacketTooShort {
            got: buf.len(),
            need: UDP_SHIM_HEADER_LEN,
        });
    }

    if buf[0..2] != UDP_SHIM_MAGIC {
        return Err(EoipError::InvalidMagicBytes {
            expected: &UDP_SHIM_MAGIC,
            got: [buf[0], buf[1], 0, 0],
        });
    }

    let inner_type = buf[2];
    if inner_type != UDP_INNER_TYPE_EOIP && inner_type != UDP_INNER_TYPE_EOIPV6 {
        return Err(EoipError::InvalidVersion {
            expected: UDP_INNER_TYPE_EOIP,
            got: inner_type,
        });
    }

    // Reserved byte at buf[3] is ignored for forward compatibility
    Ok((inner_type, UDP_SHIM_HEADER_LEN))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Encode tests ──────────────────────────────────────────────

    #[test]
    fn encode_eoip_type() {
        let mut buf = [0u8; 4];
        encode_udp_shim(UDP_INNER_TYPE_EOIP, &mut buf).unwrap();
        assert_eq!(buf, [0x45, 0x4F, 0x04, 0x00]);
    }

    #[test]
    fn encode_eoipv6_type() {
        let mut buf = [0u8; 4];
        encode_udp_shim(UDP_INNER_TYPE_EOIPV6, &mut buf).unwrap();
        assert_eq!(buf, [0x45, 0x4F, 0x06, 0x00]);
    }

    #[test]
    fn encode_invalid_type() {
        let mut buf = [0u8; 4];
        assert!(encode_udp_shim(0x99, &mut buf).is_err());
    }

    #[test]
    fn encode_short_buffer() {
        let mut buf = [0u8; 3];
        let err = encode_udp_shim(UDP_INNER_TYPE_EOIP, &mut buf).unwrap_err();
        assert!(matches!(
            err,
            EoipError::PacketTooShort { got: 3, need: 4 }
        ));
    }

    #[test]
    fn encode_reserved_byte_always_zero() {
        let mut buf = [0xFF; 4];
        encode_udp_shim(UDP_INNER_TYPE_EOIP, &mut buf).unwrap();
        assert_eq!(buf[3], 0x00);
    }

    // ── Decode tests ──────────────────────────────────────────────

    #[test]
    fn decode_eoip_type() {
        let (t, len) = decode_udp_shim(&[0x45, 0x4F, 0x04, 0x00]).unwrap();
        assert_eq!(t, UDP_INNER_TYPE_EOIP);
        assert_eq!(len, 4);
    }

    #[test]
    fn decode_eoipv6_type() {
        let (t, len) = decode_udp_shim(&[0x45, 0x4F, 0x06, 0x00]).unwrap();
        assert_eq!(t, UDP_INNER_TYPE_EOIPV6);
        assert_eq!(len, 4);
    }

    #[test]
    fn decode_wrong_magic() {
        let err = decode_udp_shim(&[0x45, 0x4E, 0x04, 0x00]).unwrap_err();
        assert!(matches!(err, EoipError::InvalidMagicBytes { .. }));
    }

    #[test]
    fn decode_invalid_type() {
        let err = decode_udp_shim(&[0x45, 0x4F, 0x99, 0x00]).unwrap_err();
        assert!(matches!(err, EoipError::InvalidVersion { .. }));
    }

    #[test]
    fn decode_short_buffer() {
        let err = decode_udp_shim(&[0x45, 0x4F, 0x04]).unwrap_err();
        assert!(matches!(
            err,
            EoipError::PacketTooShort { got: 3, need: 4 }
        ));
    }

    #[test]
    fn decode_reserved_byte_ignored() {
        let (t, _) = decode_udp_shim(&[0x45, 0x4F, 0x04, 0xFF]).unwrap();
        assert_eq!(t, UDP_INNER_TYPE_EOIP);
    }

    // ── Roundtrip tests ───────────────────────────────────────────

    #[test]
    fn roundtrip_both_types() {
        for inner_type in [UDP_INNER_TYPE_EOIP, UDP_INNER_TYPE_EOIPV6] {
            let mut buf = [0u8; 4];
            encode_udp_shim(inner_type, &mut buf).unwrap();
            let (decoded_type, len) = decode_udp_shim(&buf).unwrap();
            assert_eq!(decoded_type, inner_type);
            assert_eq!(len, 4);
        }
    }
}
