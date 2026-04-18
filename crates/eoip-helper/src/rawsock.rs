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

/// Create an AF_PACKET socket for zero-copy RX via PACKET_MMAP (TPACKET_V3).
///
/// Uses `SOCK_RAW` + `ETH_P_IP` to receive full Ethernet frames. A BPF filter
/// restricts to inbound GRE packets only (IP protocol 47 at offset 23).
/// `PACKET_IGNORE_OUTGOING` prevents own TX from flooding the ring buffer.
///
/// Note: offset 23 assumes standard 14-byte Ethernet (no VLAN tags).
#[cfg(target_os = "linux")]
pub fn create_af_packet_socket_v4() -> Result<OwnedFd, EoipError> {
    use std::os::fd::FromRawFd;

    let fd = unsafe {
        libc::socket(
            libc::AF_PACKET,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            (libc::ETH_P_IP as u16).to_be() as i32,
        )
    };
    if fd < 0 {
        return Err(EoipError::RawSocketError(std::io::Error::last_os_error()));
    }

    let sock_fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let raw_fd = {
        use std::os::fd::AsRawFd;
        sock_fd.as_raw_fd()
    };

    // Drop outgoing packets (PACKET_IGNORE_OUTGOING, Linux 4.20+).
    // Without this, our own GRE TX floods the ring buffer under load.
    const SOL_PACKET: libc::c_int = 263;
    const PACKET_IGNORE_OUTGOING: libc::c_int = 23;
    let ignore_out: libc::c_int = 1;
    let ret = unsafe {
        libc::setsockopt(
            raw_fd,
            SOL_PACKET,
            PACKET_IGNORE_OUTGOING,
            &ignore_out as *const _ as *const libc::c_void,
            std::mem::size_of_val(&ignore_out) as libc::socklen_t,
        )
    };
    if ret < 0 {
        tracing::warn!("PACKET_IGNORE_OUTGOING not available, outgoing packets may flood ring");
    }

    // BPF filter: accept GRE (proto 47) and IP fragments of GRE packets.
    // With SOCK_RAW on AF_PACKET, BPF sees full L2 frame.
    // Offset 23 = 14 (ETH) + 9 (IP protocol field).
    // Offset 20 = 14 (ETH) + 6 (IP frag offset field, 2 bytes).
    //
    // GRE-encapsulated 1500-byte frames exceed MTU → IP fragmentation.
    // Only the first fragment has the GRE header; subsequent fragments
    // have fragment offset > 0 and no GRE header. Must accept both.
    //
    //   0: LDB  [23]             ; load IP protocol byte
    //   1: JEQ  #47, 4, 0        ; if GRE → accept
    //   2: LDH  [20]             ; load IP flags+frag_offset (2 bytes)
    //   3: JSET #0x1FFF, 0, 1    ; if frag_offset != 0 → accept (continuation fragment)
    //   4: RET  #0               ; drop (not GRE, not a fragment)
    //   5: RET  #0xFFFF          ; accept
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct SockFilter {
        code: u16,
        jt: u8,
        jf: u8,
        k: u32,
    }

    let filter: [SockFilter; 6] = [
        SockFilter { code: 0x30, jt: 0, jf: 0, k: 23 },        // LDB [23]
        SockFilter { code: 0x15, jt: 3, jf: 0, k: 47 },        // JEQ #47 → accept (skip 3)
        SockFilter { code: 0x28, jt: 0, jf: 0, k: 20 },        // LDH [20] (flags+frag_offset)
        SockFilter { code: 0x45, jt: 1, jf: 0, k: 0x1FFF },    // JSET #0x1FFF → accept (frag)
        SockFilter { code: 0x06, jt: 0, jf: 0, k: 0 },         // RET drop
        SockFilter { code: 0x06, jt: 0, jf: 0, k: 0xFFFF },    // RET accept
    ];

    #[repr(C)]
    struct SockFprog {
        len: u16,
        filter: *const SockFilter,
    }

    let prog = SockFprog {
        len: filter.len() as u16,
        filter: filter.as_ptr(),
    };

    let ret = unsafe {
        libc::setsockopt(
            raw_fd,
            libc::SOL_SOCKET,
            libc::SO_ATTACH_FILTER,
            &prog as *const _ as *const libc::c_void,
            std::mem::size_of::<SockFprog>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        return Err(EoipError::RawSocketError(std::io::Error::last_os_error()));
    }

    tracing::info!("created AF_PACKET socket: SOCK_RAW, ETH_P_IP, BPF=GRE-only, IGNORE_OUTGOING");
    Ok(sock_fd)
}
