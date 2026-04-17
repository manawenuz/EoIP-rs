//! MikroTik EoIP GRE header codec (IPv4, IP protocol 47).
//!
//! ## Wire format (8 bytes)
//!
//! ```text
//! Offset  Size  Endian  Field
//! 0       4B    —       Magic: [0x20, 0x01, 0x64, 0x00]
//! 4       2B    BE      Payload length (Ethernet frame size)
//! 6       2B    LE      Tunnel ID (0–65535)
//! ```
//!
//! **CRITICAL**: The mixed endianness is intentional — MikroTik uses big-endian
//! for the payload length but **little-endian** for the tunnel ID. This is a
//! real-world protocol quirk, not a bug.

use crate::error::EoipError;

/// MikroTik EoIP magic bytes identifying valid EoIP GRE packets.
pub const EOIP_MAGIC: [u8; 4] = [0x20, 0x01, 0x64, 0x00];

/// EoIP GRE header length in bytes (fixed).
pub const EOIP_HEADER_LEN: usize = 8;

/// Minimum Ethernet frame size: 6B dst + 6B src + 2B ethertype.
const MIN_ETHERNET_FRAME: usize = 14;

/// Encode an EoIP GRE header into `buf`.
///
/// Writes the 8-byte header: magic(4B) + payload_len(2B BE) + tunnel_id(2B LE).
///
/// # Example
///
/// ```
/// # use eoip_proto::gre::*;
/// let mut buf = [0u8; 8];
/// encode_eoip_header(100, 60, &mut buf).unwrap();
/// assert_eq!(buf, [0x20, 0x01, 0x64, 0x00, 0x00, 0x3C, 0x64, 0x00]);
/// ```
pub fn encode_eoip_header(tunnel_id: u16, payload_len: u16, buf: &mut [u8]) -> Result<(), EoipError> {
    if buf.len() < EOIP_HEADER_LEN {
        return Err(EoipError::PacketTooShort {
            got: buf.len(),
            need: EOIP_HEADER_LEN,
        });
    }

    buf[0..4].copy_from_slice(&EOIP_MAGIC);
    buf[4..6].copy_from_slice(&payload_len.to_be_bytes());
    // CRITICAL: tunnel_id is little-endian per MikroTik spec, NOT network byte order
    buf[6..8].copy_from_slice(&tunnel_id.to_le_bytes());

    Ok(())
}

/// Decode an EoIP GRE header from `buf`.
///
/// Returns `(tunnel_id, payload_len, header_len)` where `header_len` is always 8.
///
/// # Example
///
/// ```
/// # use eoip_proto::gre::*;
/// let buf = [0x20, 0x01, 0x64, 0x00, 0x00, 0x40, 0x2A, 0x00];
/// let (tid, plen, hlen) = decode_eoip_header(&buf).unwrap();
/// assert_eq!(tid, 42);
/// assert_eq!(plen, 64);
/// assert_eq!(hlen, 8);
/// ```
pub fn decode_eoip_header(buf: &[u8]) -> Result<(u16, u16, usize), EoipError> {
    if buf.len() < EOIP_HEADER_LEN {
        return Err(EoipError::PacketTooShort {
            got: buf.len(),
            need: EOIP_HEADER_LEN,
        });
    }

    if buf[0..4] != EOIP_MAGIC {
        return Err(EoipError::InvalidMagicBytes {
            expected: EOIP_MAGIC.to_vec(),
            got: buf[0..4].to_vec(),
        });
    }

    let payload_len = u16::from_be_bytes([buf[4], buf[5]]);
    // CRITICAL: Read tunnel_id as little-endian, NOT big-endian
    let tunnel_id = u16::from_le_bytes([buf[6], buf[7]]);

    Ok((tunnel_id, payload_len, EOIP_HEADER_LEN))
}

/// Validate a complete EoIP packet (header + payload) against IP-level constraints.
///
/// Checks:
/// 1. Buffer is large enough for the 8-byte header
/// 2. Magic bytes are correct
/// 3. Payload length >= 14 (minimum Ethernet frame)
/// 4. Payload length fits within the IP payload
/// 5. Buffer contains the full payload
pub fn validate_eoip_packet(buf: &[u8], ip_payload_len: usize) -> Result<(), EoipError> {
    let (_tunnel_id, payload_len, _hdr_len) = decode_eoip_header(buf)?;

    let payload_len = payload_len as usize;

    // Keepalive packets have payload_len == 0 — they are valid
    if payload_len != 0 && payload_len < MIN_ETHERNET_FRAME {
        return Err(EoipError::PayloadTooSmall {
            got: payload_len,
            min: MIN_ETHERNET_FRAME,
        });
    }

    if ip_payload_len < EOIP_HEADER_LEN {
        return Err(EoipError::PacketTooShort {
            got: ip_payload_len,
            need: EOIP_HEADER_LEN,
        });
    }

    let max_payload = ip_payload_len - EOIP_HEADER_LEN;
    if payload_len > max_payload {
        return Err(EoipError::PayloadTooLarge {
            size: payload_len,
            limit: max_payload,
        });
    }

    let total_needed = EOIP_HEADER_LEN + payload_len;
    if buf.len() < total_needed {
        return Err(EoipError::PacketTooShort {
            got: buf.len(),
            need: total_needed,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Encode tests ──────────────────────────────────────────────

    #[test]
    fn encode_tid_42_payload_64() {
        let mut buf = [0u8; 8];
        encode_eoip_header(42, 64, &mut buf).unwrap();
        assert_eq!(buf, [0x20, 0x01, 0x64, 0x00, 0x00, 0x40, 0x2A, 0x00]);
    }

    #[test]
    fn encode_tid_0x1234_payload_0x5678() {
        let mut buf = [0u8; 8];
        encode_eoip_header(0x1234, 0x5678, &mut buf).unwrap();
        // payload_len=0x5678 in BE: [0x56, 0x78]
        // tunnel_id=0x1234 in LE: [0x34, 0x12]
        assert_eq!(buf[4], 0x56);
        assert_eq!(buf[5], 0x78);
        assert_eq!(buf[6], 0x34);
        assert_eq!(buf[7], 0x12);
    }

    #[test]
    fn encode_tid_100_payload_60() {
        let mut buf = [0u8; 8];
        encode_eoip_header(100, 60, &mut buf).unwrap();
        assert_eq!(buf, [0x20, 0x01, 0x64, 0x00, 0x00, 0x3C, 0x64, 0x00]);
    }

    #[test]
    fn encode_short_buffer() {
        let mut buf = [0u8; 7];
        let err = encode_eoip_header(0, 0, &mut buf).unwrap_err();
        assert!(matches!(
            err,
            EoipError::PacketTooShort { got: 7, need: 8 }
        ));
    }

    #[test]
    fn encode_exact_buffer() {
        let mut buf = [0u8; 8];
        assert!(encode_eoip_header(0, 0, &mut buf).is_ok());
    }

    #[test]
    fn encode_larger_buffer_only_writes_first_8() {
        let mut buf = [0xFFu8; 16];
        encode_eoip_header(1, 14, &mut buf).unwrap();
        // First 8 bytes are the header
        assert_eq!(&buf[0..4], &EOIP_MAGIC);
        // Remaining bytes untouched
        assert_eq!(buf[8], 0xFF);
    }

    // ── Decode tests ──────────────────────────────────────────────

    #[test]
    fn decode_tid_42_payload_64() {
        let buf = [0x20, 0x01, 0x64, 0x00, 0x00, 0x40, 0x2A, 0x00];
        let (tid, plen, hlen) = decode_eoip_header(&buf).unwrap();
        assert_eq!(tid, 42);
        assert_eq!(plen, 64);
        assert_eq!(hlen, 8);
    }

    #[test]
    fn decode_mixed_endian_worked_example() {
        // tunnel_id=0xCDAB (little-endian bytes [0xAB, 0xCD])
        // payload_len=0x1234 (big-endian bytes [0x12, 0x34])
        let buf = [0x20, 0x01, 0x64, 0x00, 0x12, 0x34, 0xAB, 0xCD];
        let (tid, plen, _) = decode_eoip_header(&buf).unwrap();
        assert_eq!(tid, 0xCDAB);
        assert_eq!(plen, 0x1234);
    }

    #[test]
    fn decode_short_buffer() {
        let buf = [0x20, 0x01, 0x64, 0x00, 0x00, 0x40, 0x2A];
        let err = decode_eoip_header(&buf).unwrap_err();
        assert!(matches!(
            err,
            EoipError::PacketTooShort { got: 7, need: 8 }
        ));
    }

    #[test]
    fn decode_wrong_magic() {
        let buf = [0x20, 0x01, 0x64, 0x01, 0x00, 0x40, 0x2A, 0x00];
        let err = decode_eoip_header(&buf).unwrap_err();
        assert!(matches!(err, EoipError::InvalidMagicBytes { .. }));
    }

    // ── Validate tests ────────────────────────────────────────────

    #[test]
    fn validate_good_packet() {
        let mut buf = vec![0u8; 8 + 60];
        encode_eoip_header(1, 60, &mut buf).unwrap();
        assert!(validate_eoip_packet(&buf, 68).is_ok());
    }

    #[test]
    fn validate_minimum_ethernet_frame() {
        let mut buf = vec![0u8; 8 + 14];
        encode_eoip_header(1, 14, &mut buf).unwrap();
        assert!(validate_eoip_packet(&buf, 22).is_ok());
    }

    #[test]
    fn validate_keepalive_zero_payload() {
        let mut buf = vec![0u8; 8];
        encode_eoip_header(1, 0, &mut buf).unwrap();
        assert!(validate_eoip_packet(&buf, 8).is_ok());
    }

    #[test]
    fn validate_payload_too_small() {
        let mut buf = vec![0u8; 8 + 13];
        encode_eoip_header(1, 13, &mut buf).unwrap();
        let err = validate_eoip_packet(&buf, 21).unwrap_err();
        assert!(matches!(
            err,
            EoipError::PayloadTooSmall { got: 13, min: 14 }
        ));
    }

    #[test]
    fn validate_payload_exceeds_ip_payload() {
        let mut buf = vec![0u8; 8 + 100];
        encode_eoip_header(1, 100, &mut buf).unwrap();
        // ip_payload_len=50 means only 42 bytes available after header
        let err = validate_eoip_packet(&buf, 50).unwrap_err();
        assert!(matches!(
            err,
            EoipError::PayloadTooLarge {
                size: 100,
                limit: 42
            }
        ));
    }

    #[test]
    fn validate_truncated_buffer() {
        // Header says 60 bytes payload but buffer only has 10 bytes after header
        let mut buf = vec![0u8; 8 + 10];
        encode_eoip_header(1, 60, &mut buf).unwrap();
        let err = validate_eoip_packet(&buf, 68).unwrap_err();
        assert!(matches!(err, EoipError::PacketTooShort { got: 18, need: 68 }));
    }

    // ── Roundtrip tests ───────────────────────────────────────────

    #[test]
    fn encode_decode_roundtrip_boundary_values() {
        for (tid, plen) in [(0, 0), (0, 14), (65535, 1500), (1, u16::MAX)] {
            let mut buf = [0u8; 8];
            encode_eoip_header(tid, plen, &mut buf).unwrap();
            let (decoded_tid, decoded_plen, hlen) = decode_eoip_header(&buf).unwrap();
            assert_eq!(decoded_tid, tid, "tunnel_id mismatch for ({tid}, {plen})");
            assert_eq!(decoded_plen, plen, "payload_len mismatch for ({tid}, {plen})");
            assert_eq!(hlen, 8);
        }
    }

    // ── Property tests ────────────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn roundtrip_all_values(tid in 0u16..=65535u16, plen in 0u16..=65535u16) {
                let mut buf = [0u8; 8];
                encode_eoip_header(tid, plen, &mut buf).unwrap();
                let (decoded_tid, decoded_plen, hdr_len) = decode_eoip_header(&buf).unwrap();
                prop_assert_eq!(decoded_tid, tid);
                prop_assert_eq!(decoded_plen, plen);
                prop_assert_eq!(hdr_len, 8);
            }

            #[test]
            fn magic_bytes_always_present(tid in 0u16..=65535u16, plen in 0u16..=65535u16) {
                let mut buf = [0u8; 8];
                encode_eoip_header(tid, plen, &mut buf).unwrap();
                prop_assert_eq!(&buf[0..4], &EOIP_MAGIC);
            }

            #[test]
            fn payload_len_is_big_endian(plen in 0u16..=65535u16) {
                let mut buf = [0u8; 8];
                encode_eoip_header(0, plen, &mut buf).unwrap();
                let be_bytes = plen.to_be_bytes();
                prop_assert_eq!(buf[4], be_bytes[0]);
                prop_assert_eq!(buf[5], be_bytes[1]);
            }

            #[test]
            fn tunnel_id_is_little_endian(tid in 0u16..=65535u16) {
                let mut buf = [0u8; 8];
                encode_eoip_header(tid, 0, &mut buf).unwrap();
                let le_bytes = tid.to_le_bytes();
                prop_assert_eq!(buf[6], le_bytes[0]);
                prop_assert_eq!(buf[7], le_bytes[1]);
            }
        }
    }
}
