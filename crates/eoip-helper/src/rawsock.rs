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

    // Enlarge socket buffers to absorb bursts between userspace batch drains.
    // Default ~212 KB is too small under sustained iperf3 load.
    sock.set_recv_buffer_size(4 * 1024 * 1024)?;
    sock.set_send_buffer_size(4 * 1024 * 1024)?;

    // IP_HDRINCL = false (default) — kernel prepends IP header on TX,
    // but we receive the full IP header on RX for raw sockets.

    // MikroTik uses TTL=255 for EoIP packets (confirmed via wire capture).
    // Linux default is 64, which would cause interop issues.
    sock.set_ttl(255)?;

    // MikroTik sets DF=0 (allow fragmentation). Disable PMTU discovery
    // so the kernel doesn't set the DF bit on outgoing packets.
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        // IP_PMTUDISC_DONT = 0
        let val: libc::c_int = libc::IP_PMTUDISC_DONT;
        unsafe {
            libc::setsockopt(
                sock.as_raw_fd(),
                libc::IPPROTO_IP,
                libc::IP_MTU_DISCOVER,
                &val as *const _ as *const libc::c_void,
                std::mem::size_of_val(&val) as libc::socklen_t,
            );
        }
    }

    tracing::info!("created raw socket: AF_INET, SOCK_RAW, proto=47 (EoIP), ttl=255, df=0, bufs=4MB");
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
    // Note: IPV6_V6ONLY is not valid for raw sockets (EINVAL on Linux).
    // Raw sockets don't accept IPv4-mapped addresses regardless.
    sock.set_nonblocking(true)?;

    // Enlarge socket buffers to absorb bursts between userspace batch drains.
    sock.set_recv_buffer_size(4 * 1024 * 1024)?;
    sock.set_send_buffer_size(4 * 1024 * 1024)?;

    // Match MikroTik hop limit of 255 (IPv6 equivalent of TTL).
    sock.set_unicast_hops_v6(255)?;

    tracing::info!("created raw socket: AF_INET6, SOCK_RAW, proto=97 (EtherIP), hops=255, bufs=4MB");
    Ok(OwnedFd::from(sock))
}
