//! Raw socket creation for EoIP (IPv4, protocol 47) and EoIPv6 (IPv6, protocol 97).
//!
//! Requires `CAP_NET_RAW` or root. Sockets are created in non-blocking mode
//! for async runtime compatibility.

use std::os::fd::OwnedFd;

use socket2::{Domain, Protocol, Socket, Type};

use eoip_proto::EoipError;

/// IP protocol number for GRE (used by MikroTik EoIP over IPv4).
const PROTO_GRE: i32 = 47;

/// IP protocol number for EtherIP (used by MikroTik EoIPv6 over IPv6).
const PROTO_ETHERIP: i32 = 97;

/// Create a raw IPv4 socket for EoIP (IP protocol 47 / GRE).
///
/// The socket is non-blocking and has `CLOEXEC` set. The kernel will
/// deliver all inbound GRE packets to this socket; userspace demux
/// filters by source IP and tunnel ID.
pub fn create_raw_socket_v4() -> Result<OwnedFd, EoipError> {
    let sock = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::from(PROTO_GRE)))?;
    sock.set_nonblocking(true)?;

    // IP_HDRINCL = false (default) — kernel prepends IP header on TX,
    // but we receive the full IP header on RX for raw sockets.

    tracing::info!("created raw socket: AF_INET, SOCK_RAW, proto=47 (EoIP)");
    Ok(OwnedFd::from(sock))
}

/// Create a raw IPv6 socket for EoIPv6 (IP protocol 97 / EtherIP).
///
/// The socket has `IPV6_V6ONLY` set to prevent IPv4-mapped addresses,
/// and is non-blocking with `CLOEXEC`.
pub fn create_raw_socket_v6() -> Result<OwnedFd, EoipError> {
    let sock = Socket::new(
        Domain::IPV6,
        Type::RAW,
        Some(Protocol::from(PROTO_ETHERIP)),
    )?;
    sock.set_only_v6(true)?;
    sock.set_nonblocking(true)?;

    tracing::info!("created raw socket: AF_INET6, SOCK_RAW, proto=97 (EtherIP)");
    Ok(OwnedFd::from(sock))
}
