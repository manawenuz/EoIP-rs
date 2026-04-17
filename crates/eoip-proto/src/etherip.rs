//! EoIPv6 / EtherIP header codec (IPv6, IP protocol 97).
//!
//! ## Wire format (2 bytes)
//!
//! MikroTik encodes a 12-bit tunnel ID into the RFC 3378 EtherIP header:
//!
//! ```text
//! Byte 0:  [ TID[11:8] (4 bits) | Version 0x3 (4 bits) ]
//! Byte 1:  [ TID[7:0]  (8 bits)                        ]
//! ```
//!
//! When `tunnel_id == 0`, this produces `[0x03, 0x00]` which is fully
//! RFC 3378-compliant (version=3, reserved=0).
//!
//! ## Worked examples
//!
//! - TID=300  (0x12C) → `[0x13, 0x2C]`
//! - TID=1000 (0x3E8) → `[0x33, 0xE8]`
//! - TID=4095 (0xFFF) → `[0xF3, 0xFF]`

use crate::error::EoipError;

/// EtherIP version field (low nibble of byte 0).
pub const ETHERIP_VERSION: u8 = 0x03;

/// EoIPv6 header length in bytes (fixed).
pub const ETHERIP_HEADER_LEN: usize = 2;

/// Maximum tunnel ID for EoIPv6 (12-bit field).
pub const MAX_EOIPV6_TUNNEL_ID: u16 = 4095;

/// Minimum Ethernet frame size: 6B dst + 6B src + 2B ethertype.
const MIN_ETHERNET_FRAME: usize = 14;

/// Encode a 2-byte EoIPv6/EtherIP header with nibble-packed 12-bit tunnel ID.
///
/// # Examples
///
/// ```
/// # use eoip_proto::etherip::*;
/// let mut buf = [0u8; 2];
/// encode_eoipv6_header(300, &mut buf).unwrap();
/// assert_eq!(buf, [0x13, 0x2C]);
/// ```
pub fn encode_eoipv6_header(tunnel_id: u16, buf: &mut [u8]) -> Result<(), EoipError> {
    if tunnel_id > MAX_EOIPV6_TUNNEL_ID {
        return Err(EoipError::TunnelIdOutOfRange {
            id: tunnel_id,
            max: MAX_EOIPV6_TUNNEL_ID,
        });
    }

    if buf.len() < ETHERIP_HEADER_LEN {
        return Err(EoipError::PacketTooShort {
            got: buf.len(),
            need: ETHERIP_HEADER_LEN,
        });
    }

    let tid = tunnel_id & 0x0FFF;
    let tid_hi = (tid >> 8) as u8;
    let tid_lo = (tid & 0xFF) as u8;

    // Pack TID[11:8] into high nibble, version 0x3 into low nibble
    buf[0] = (tid_hi << 4) | ETHERIP_VERSION;
    buf[1] = tid_lo;

    Ok(())
}

/// Decode a 2-byte EoIPv6/EtherIP header, extracting the 12-bit tunnel ID.
///
/// Returns `(tunnel_id, header_len)` where `header_len` is always 2.
///
/// # Examples
///
/// ```
/// # use eoip_proto::etherip::*;
/// let (tid, hlen) = decode_eoipv6_header(&[0x33, 0xE8]).unwrap();
/// assert_eq!(tid, 1000);
/// assert_eq!(hlen, 2);
/// ```
pub fn decode_eoipv6_header(buf: &[u8]) -> Result<(u16, usize), EoipError> {
    if buf.len() < ETHERIP_HEADER_LEN {
        return Err(EoipError::PacketTooShort {
            got: buf.len(),
            need: ETHERIP_HEADER_LEN,
        });
    }

    let version = buf[0] & 0x0F;
    if version != ETHERIP_VERSION {
        return Err(EoipError::InvalidVersion {
            expected: ETHERIP_VERSION,
            got: version,
        });
    }

    let tid_hi = (buf[0] >> 4) as u16;
    let tid_lo = buf[1] as u16;
    let tunnel_id = (tid_hi << 8) | tid_lo;

    Ok((tunnel_id, ETHERIP_HEADER_LEN))
}

/// Validate a complete EoIPv6 packet (header + Ethernet frame).
///
/// Checks:
/// 1. Buffer has at least 16 bytes (2B header + 14B minimum Ethernet frame)
/// 2. Version field is 0x3
/// 3. Buffer length matches IPv6 payload length
pub fn validate_eoipv6_packet(buf: &[u8], ipv6_payload_len: usize) -> Result<(), EoipError> {
    let min_len = ETHERIP_HEADER_LEN + MIN_ETHERNET_FRAME;
    if buf.len() < min_len {
        return Err(EoipError::PacketTooShort {
            got: buf.len(),
            need: min_len,
        });
    }

    let version = buf[0] & 0x0F;
    if version != ETHERIP_VERSION {
        return Err(EoipError::InvalidVersion {
            expected: ETHERIP_VERSION,
            got: version,
        });
    }

    if buf.len() != ipv6_payload_len {
        return Err(EoipError::InvalidEtherIpHeader(format!(
            "buffer length {} does not match IPv6 payload length {}",
            buf.len(),
            ipv6_payload_len
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Encode tests ──────────────────────────────────────────────

    #[test]
    fn encode_tid_0() {
        let mut buf = [0u8; 2];
        encode_eoipv6_header(0, &mut buf).unwrap();
        assert_eq!(buf, [0x03, 0x00]);
    }

    #[test]
    fn encode_tid_300() {
        let mut buf = [0u8; 2];
        encode_eoipv6_header(300, &mut buf).unwrap();
        assert_eq!(buf, [0x13, 0x2C]);
    }

    #[test]
    fn encode_tid_1000() {
        let mut buf = [0u8; 2];
        encode_eoipv6_header(1000, &mut buf).unwrap();
        assert_eq!(buf, [0x33, 0xE8]);
    }

    #[test]
    fn encode_tid_4095() {
        let mut buf = [0u8; 2];
        encode_eoipv6_header(4095, &mut buf).unwrap();
        assert_eq!(buf, [0xF3, 0xFF]);
    }

    #[test]
    fn encode_tid_4096_fails() {
        let mut buf = [0u8; 2];
        let err = encode_eoipv6_header(4096, &mut buf).unwrap_err();
        assert!(matches!(
            err,
            EoipError::TunnelIdOutOfRange { id: 4096, max: 4095 }
        ));
    }

    #[test]
    fn encode_short_buffer() {
        let mut buf = [0u8; 1];
        let err = encode_eoipv6_header(0, &mut buf).unwrap_err();
        assert!(matches!(
            err,
            EoipError::PacketTooShort { got: 1, need: 2 }
        ));
    }

    // ── Decode tests ──────────────────────────────────────────────

    #[test]
    fn decode_tid_0() {
        let (tid, hlen) = decode_eoipv6_header(&[0x03, 0x00]).unwrap();
        assert_eq!(tid, 0);
        assert_eq!(hlen, 2);
    }

    #[test]
    fn decode_tid_300() {
        let (tid, _) = decode_eoipv6_header(&[0x13, 0x2C]).unwrap();
        assert_eq!(tid, 300);
    }

    #[test]
    fn decode_tid_1000() {
        let (tid, _) = decode_eoipv6_header(&[0x33, 0xE8]).unwrap();
        assert_eq!(tid, 1000);
    }

    #[test]
    fn decode_tid_4095() {
        let (tid, _) = decode_eoipv6_header(&[0xF3, 0xFF]).unwrap();
        assert_eq!(tid, 4095);
    }

    #[test]
    fn decode_version_2_rejected() {
        let err = decode_eoipv6_header(&[0x02, 0x00]).unwrap_err();
        assert!(matches!(
            err,
            EoipError::InvalidVersion { expected: 3, got: 2 }
        ));
    }

    #[test]
    fn decode_version_4_rejected() {
        let err = decode_eoipv6_header(&[0x04, 0x00]).unwrap_err();
        assert!(matches!(
            err,
            EoipError::InvalidVersion { expected: 3, got: 4 }
        ));
    }

    #[test]
    fn decode_short_buffer() {
        let err = decode_eoipv6_header(&[0x03]).unwrap_err();
        assert!(matches!(
            err,
            EoipError::PacketTooShort { got: 1, need: 2 }
        ));
    }

    // ── Validate tests ────────────────────────────────────────────

    #[test]
    fn validate_good_packet() {
        let mut buf = vec![0u8; 2 + 60];
        encode_eoipv6_header(42, &mut buf).unwrap();
        assert!(validate_eoipv6_packet(&buf, 62).is_ok());
    }

    #[test]
    fn validate_minimum_ethernet() {
        let mut buf = vec![0u8; 2 + 14];
        encode_eoipv6_header(0, &mut buf).unwrap();
        assert!(validate_eoipv6_packet(&buf, 16).is_ok());
    }

    #[test]
    fn validate_too_short() {
        let buf = vec![0x03; 15];
        let err = validate_eoipv6_packet(&buf, 15).unwrap_err();
        assert!(matches!(
            err,
            EoipError::PacketTooShort { got: 15, need: 16 }
        ));
    }

    #[test]
    fn validate_wrong_version() {
        let mut buf = vec![0u8; 16];
        buf[0] = 0x02; // version 2
        let err = validate_eoipv6_packet(&buf, 16).unwrap_err();
        assert!(matches!(err, EoipError::InvalidVersion { .. }));
    }

    #[test]
    fn validate_length_mismatch() {
        let mut buf = vec![0u8; 20];
        encode_eoipv6_header(0, &mut buf).unwrap();
        let err = validate_eoipv6_packet(&buf, 16).unwrap_err();
        assert!(matches!(err, EoipError::InvalidEtherIpHeader(_)));
    }

    // ── Roundtrip / property tests ────────────────────────────────

    #[test]
    fn roundtrip_all_4096_tunnel_ids() {
        for tid in 0..=4095u16 {
            let mut buf = [0u8; 2];
            encode_eoipv6_header(tid, &mut buf).unwrap();
            let (decoded, hlen) = decode_eoipv6_header(&buf).unwrap();
            assert_eq!(decoded, tid, "roundtrip failed for TID={tid}");
            assert_eq!(hlen, 2);
        }
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn roundtrip_valid_tids(tid in 0u16..=4095u16) {
                let mut buf = [0u8; 2];
                encode_eoipv6_header(tid, &mut buf).unwrap();
                let (decoded, hlen) = decode_eoipv6_header(&buf).unwrap();
                prop_assert_eq!(decoded, tid);
                prop_assert_eq!(hlen, 2);
            }

            #[test]
            fn version_nibble_always_3(tid in 0u16..=4095u16) {
                let mut buf = [0u8; 2];
                encode_eoipv6_header(tid, &mut buf).unwrap();
                prop_assert_eq!(buf[0] & 0x0F, 0x03);
            }

            #[test]
            fn out_of_range_always_rejected(tid in 4096u16..=u16::MAX) {
                let mut buf = [0u8; 2];
                let result = encode_eoipv6_header(tid, &mut buf);
                prop_assert!(result.is_err());
            }
        }
    }
}
