//! Helper ↔ Daemon IPC wire protocol.
//!
//! File descriptors (TAP fds, raw socket fds) are passed separately via
//! `SCM_RIGHTS` ancillary data on the Unix domain socket — they are NOT
//! included in the serialized message payload.

use serde::{Deserialize, Serialize};

use crate::error::EoipError;

/// Messages sent from the privileged helper to the unprivileged daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HelperMsg {
    /// A TAP interface has been created. The fd is passed via SCM_RIGHTS.
    TapCreated {
        iface_name: String,
        tunnel_id: u16,
    },
    /// A raw socket has been created. The fd is passed via SCM_RIGHTS.
    /// `address_family`: `2` = AF_INET, `10` = AF_INET6.
    RawSocket {
        address_family: u16,
    },
    /// An error occurred in the helper.
    Error {
        msg: String,
    },
    /// Helper is initialized and ready to accept commands.
    HelperReady,
}

/// Messages sent from the unprivileged daemon to the privileged helper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonMsg {
    /// Request creation of a TAP interface for the given tunnel.
    CreateTunnel {
        iface_name: String,
        tunnel_id: u16,
        /// Overlay MTU to set on the TAP interface (0 = skip).
        mtu: u16,
        /// Whether to add an iptables TCP MSS clamping rule for this interface.
        clamp_tcp_mss: bool,
    },
    /// Request destruction of a TAP interface.
    DestroyTunnel {
        iface_name: String,
    },
    /// Request the helper to shut down gracefully.
    Shutdown,
}

/// Serialize a wire protocol message to a compact binary representation.
pub fn serialize_msg<T: Serialize>(msg: &T) -> Result<Vec<u8>, EoipError> {
    postcard::to_allocvec(msg).map_err(|e| EoipError::WireSerialize(e.to_string()))
}

/// Deserialize a wire protocol message from bytes.
pub fn deserialize_msg<T: for<'de> Deserialize<'de>>(buf: &[u8]) -> Result<T, EoipError> {
    postcard::from_bytes(buf).map_err(|e| EoipError::WireSerialize(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_msg_roundtrip_tap_created() {
        let msg = HelperMsg::TapCreated {
            iface_name: "eoip0".into(),
            tunnel_id: 100,
        };
        let bytes = serialize_msg(&msg).unwrap();
        let decoded: HelperMsg = deserialize_msg(&bytes).unwrap();
        match decoded {
            HelperMsg::TapCreated {
                iface_name,
                tunnel_id,
            } => {
                assert_eq!(iface_name, "eoip0");
                assert_eq!(tunnel_id, 100);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn helper_msg_roundtrip_raw_socket() {
        let msg = HelperMsg::RawSocket {
            address_family: 2, // AF_INET
        };
        let bytes = serialize_msg(&msg).unwrap();
        let decoded: HelperMsg = deserialize_msg(&bytes).unwrap();
        assert!(matches!(
            decoded,
            HelperMsg::RawSocket {
                address_family: 2
            }
        ));
    }

    #[test]
    fn helper_msg_roundtrip_error() {
        let msg = HelperMsg::Error {
            msg: "permission denied".into(),
        };
        let bytes = serialize_msg(&msg).unwrap();
        let decoded: HelperMsg = deserialize_msg(&bytes).unwrap();
        match decoded {
            HelperMsg::Error { msg } => assert_eq!(msg, "permission denied"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn helper_msg_roundtrip_ready() {
        let msg = HelperMsg::HelperReady;
        let bytes = serialize_msg(&msg).unwrap();
        let decoded: HelperMsg = deserialize_msg(&bytes).unwrap();
        assert!(matches!(decoded, HelperMsg::HelperReady));
    }

    #[test]
    fn daemon_msg_roundtrip_create() {
        let msg = DaemonMsg::CreateTunnel {
            iface_name: "eoip-dc1".into(),
            tunnel_id: 42,
            mtu: 1458,
            clamp_tcp_mss: true,
        };
        let bytes = serialize_msg(&msg).unwrap();
        let decoded: DaemonMsg = deserialize_msg(&bytes).unwrap();
        match decoded {
            DaemonMsg::CreateTunnel {
                iface_name,
                tunnel_id,
                mtu,
                clamp_tcp_mss,
            } => {
                assert_eq!(iface_name, "eoip-dc1");
                assert_eq!(tunnel_id, 42);
                assert_eq!(mtu, 1458);
                assert!(clamp_tcp_mss);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn daemon_msg_roundtrip_destroy() {
        let msg = DaemonMsg::DestroyTunnel {
            iface_name: "eoip0".into(),
        };
        let bytes = serialize_msg(&msg).unwrap();
        let decoded: DaemonMsg = deserialize_msg(&bytes).unwrap();
        match decoded {
            DaemonMsg::DestroyTunnel { iface_name } => assert_eq!(iface_name, "eoip0"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn daemon_msg_roundtrip_shutdown() {
        let msg = DaemonMsg::Shutdown;
        let bytes = serialize_msg(&msg).unwrap();
        let decoded: DaemonMsg = deserialize_msg(&bytes).unwrap();
        assert!(matches!(decoded, DaemonMsg::Shutdown));
    }

    #[test]
    fn serialized_size_is_compact() {
        let msg = DaemonMsg::CreateTunnel {
            iface_name: "eoip-dc1".into(),
            tunnel_id: 100,
            mtu: 1458,
            clamp_tcp_mss: true,
        };
        let bytes = serialize_msg(&msg).unwrap();
        assert!(bytes.len() < 100, "serialized size: {}", bytes.len());
    }

    #[test]
    fn malformed_input_returns_error() {
        let result: Result<HelperMsg, _> = deserialize_msg(&[0xFF, 0xFF, 0xFF]);
        assert!(result.is_err());
    }
}
