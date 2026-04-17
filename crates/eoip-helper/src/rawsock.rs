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

    tracing::info!("created raw socket: AF_INET, SOCK_RAW, proto=47 (EoIP), ttl=255, df=0");
    Ok(OwnedFd::from(sock))
}

/// Create an AF_PACKET socket for zero-copy RX via PACKET_MMAP (TPACKET_V3).
///
/// Uses `SOCK_DGRAM` + `ETH_P_IP` so the kernel strips the L2 header and
/// delivers IP packets directly — matching the existing `process_v4_packet`
/// input format. A BPF filter restricts the ring buffer to inbound GRE
/// packets only (drops outgoing and non-GRE traffic).
#[cfg(target_os = "linux")]
pub fn create_af_packet_socket_v4() -> Result<OwnedFd, EoipError> {
    use std::os::fd::FromRawFd;

    let fd = unsafe {
        libc::socket(
            libc::AF_PACKET,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            (libc::ETH_P_IP as u16).to_be() as i32,
        )
    };
    if fd < 0 {
        return Err(EoipError::RawSocketError(std::io::Error::last_os_error()));
    }

    // Safety: fd is valid, we take ownership immediately
    let sock_fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let raw_fd = {
        use std::os::fd::AsRawFd;
        sock_fd.as_raw_fd()
    };

    // ── BPF filter: accept inbound GRE only ──────────────────────────
    //
    // BPF sees the raw L2 frame even with SOCK_DGRAM. Offsets:
    //   Ethernet header = 14 bytes, IP protocol field = byte 9 of IP header
    //   → absolute offset 23 for IP protocol
    //
    // Ancillary `SKF_AD_PKTTYPE` (via negative offset 0xFFFFF004) gives
    // the packet direction. PACKET_OUTGOING = 4.
    //
    // Program:
    //   0: LDW  #pkttype           ; load ancillary packet type
    //   1: JEQ  #4, drop(→5)      ; if PACKET_OUTGOING, drop
    //   2: LDB  [23]               ; load IP protocol byte
    //   3: JEQ  #47, accept(→4)   ; if GRE, accept
    //   4: RET  #0xFFFF            ; accept
    //   5: RET  #0                 ; drop

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct SockFilter {
        code: u16,
        jt: u8,
        jf: u8,
        k: u32,
    }

    #[repr(C)]
    struct SockFprog {
        len: u16,
        filter: *const SockFilter,
    }

    let filter = [
        // 0: BPF_LD|BPF_W|BPF_ABS — load pkttype via SKF_AD_OFF + SKF_AD_PKTTYPE
        SockFilter { code: 0x20, jt: 0, jf: 0, k: 0xFFFF_F004 },
        // 1: BPF_JMP|BPF_JEQ|BPF_K — if PACKET_OUTGOING (4), jt→+3(=5 drop), jf→+0(=2 continue)
        SockFilter { code: 0x15, jt: 3, jf: 0, k: 4 },
        // 2: BPF_LD|BPF_B|BPF_ABS — load IP protocol at offset 23 (14 ETH + 9 IP)
        SockFilter { code: 0x30, jt: 0, jf: 0, k: 23 },
        // 3: BPF_JMP|BPF_JEQ|BPF_K — if GRE (47), jt→+0(=4 accept), jf→+1(=5 drop)
        SockFilter { code: 0x15, jt: 0, jf: 1, k: 47 },
        // 4: BPF_RET — accept (return max snaplen)
        SockFilter { code: 0x06, jt: 0, jf: 0, k: 0x0000_FFFF },
        // 5: BPF_RET — drop
        SockFilter { code: 0x06, jt: 0, jf: 0, k: 0 },
    ];

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

    tracing::info!(
        "created AF_PACKET socket: SOCK_DGRAM, ETH_P_IP, BPF filter (inbound GRE only)"
    );
    Ok(sock_fd)
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

    // Match MikroTik hop limit of 255 (IPv6 equivalent of TTL).
    // Non-fatal if this fails (some kernels/configs don't support it).
    if let Err(e) = sock.set_unicast_hops_v6(255) {
        tracing::warn!("set_unicast_hops_v6(255) failed: {e} (non-critical)");
    }

    tracing::info!("created raw socket: AF_INET6, SOCK_RAW, proto=97 (EtherIP), hops=255");
    Ok(OwnedFd::from(sock))
}
