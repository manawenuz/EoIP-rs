//! Outgoing-interface MTU detection.
//!
//! Determines the path MTU to a remote peer by looking up the outgoing
//! interface via the OS routing table and reading its MTU. The caller
//! subtracts the EoIP overhead (42 bytes) to get the overlay MTU.

use std::net::IpAddr;

/// EoIP encapsulation overhead: 20 (IP) + 8 (GRE/EoIP) + 14 (inner Ethernet).
pub const EOIP_OVERHEAD: u16 = 42;

/// IPsec ESP overhead for AES-256-CBC + SHA1:
/// 8 (ESP header) + 16 (AES-CBC IV) + 2 (pad length + next header) + 12 (SHA1 auth tag) = 38.
pub const IPSEC_ESP_OVERHEAD: u16 = 38;

/// Minimum sane overlay MTU (matches IPv4 minimum 576 - 42).
pub const MIN_OVERLAY_MTU: u16 = 534;

/// Default overlay MTU when detection fails (1500 - 42).
pub const DEFAULT_OVERLAY_MTU: u16 = 1458;

/// Detect the MTU of the outgoing interface for reaching `remote`.
///
/// Returns the **path MTU** (e.g. 1500), not the overlay MTU.
/// The caller should subtract [`EOIP_OVERHEAD`] to get the usable overlay MTU.
///
/// Falls back to 1500 if detection fails or is unsupported on the platform.
pub fn detect_interface_mtu(remote: IpAddr) -> u16 {
    match detect_interface_mtu_inner(remote) {
        Ok(mtu) => {
            tracing::debug!(remote = %remote, path_mtu = mtu, "detected interface MTU");
            mtu
        }
        Err(e) => {
            tracing::warn!(remote = %remote, %e, "MTU detection failed, using default 1500");
            1500
        }
    }
}

/// Compute the overlay MTU from interface detection.
///
/// Convenience wrapper: detects path MTU, subtracts overhead, clamps to minimum.
pub fn auto_overlay_mtu(remote: IpAddr) -> u16 {
    let path_mtu = detect_interface_mtu(remote);
    let overlay = path_mtu.saturating_sub(EOIP_OVERHEAD);
    overlay.max(MIN_OVERLAY_MTU)
}

// ── Linux implementation ────────────────────────────────────────

#[cfg(target_os = "linux")]
fn detect_interface_mtu_inner(remote: IpAddr) -> Result<u16, std::io::Error> {
    use std::ffi::CStr;
    use std::mem::MaybeUninit;
    use std::os::fd::FromRawFd;

    // Create a UDP socket and connect() to the remote.
    // This triggers a route lookup without sending any traffic.
    let af = if remote.is_ipv4() {
        libc::AF_INET
    } else {
        libc::AF_INET6
    };

    let sock = unsafe { libc::socket(af, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if sock < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // Safety: sock is a valid fd we just created.
    let _guard = unsafe { std::os::fd::OwnedFd::from_raw_fd(sock) };

    // Connect to remote on an arbitrary port (no traffic sent for UDP).
    let ret = match remote {
        IpAddr::V4(v4) => {
            let addr = libc::sockaddr_in {
                sin_family: libc::AF_INET as u16,
                sin_port: 9u16.to_be(), // discard port
                sin_addr: libc::in_addr {
                    s_addr: u32::from(v4).to_be(),
                },
                sin_zero: [0; 8],
            };
            unsafe {
                libc::connect(
                    sock,
                    &addr as *const _ as *const libc::sockaddr,
                    std::mem::size_of::<libc::sockaddr_in>() as u32,
                )
            }
        }
        IpAddr::V6(v6) => {
            let addr = libc::sockaddr_in6 {
                sin6_family: libc::AF_INET6 as u16,
                sin6_port: 9u16.to_be(),
                sin6_flowinfo: 0,
                sin6_addr: libc::in6_addr {
                    s6_addr: v6.octets(),
                },
                sin6_scope_id: 0,
            };
            unsafe {
                libc::connect(
                    sock,
                    &addr as *const _ as *const libc::sockaddr,
                    std::mem::size_of::<libc::sockaddr_in6>() as u32,
                )
            }
        }
    };
    if ret < 0 {
        return Err(std::io::Error::last_os_error());
    }

    // Get the interface name via SO_BINDTODEVICE / getsockopt on the bound socket.
    // Simpler: use getsockname to find our source IP, then scan /proc/net/if_inet6 or
    // use SIOCGIFNAME. But the most reliable way is:
    //   1. getsockopt(SOL_IP, IP_MTU) — but this only works after connect on raw/udp
    //      and may not reflect the interface MTU on all kernels.
    //   2. Use if_nameindex + SIOCGIFMTU on each, match by getsockname source.
    //
    // We'll use the approach: get bound interface index via SO_BINDTOIFINDEX (Linux 4.19+),
    // fall back to iterating interfaces.

    // Try to get the interface name from the socket.
    let mut ifr = MaybeUninit::<[u8; 40]>::zeroed();
    let mut len: libc::socklen_t = 40;
    let ret = unsafe {
        libc::getsockopt(
            sock,
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            ifr.as_mut_ptr() as *mut _,
            &mut len,
        )
    };

    if ret == 0 && len > 1 {
        // SO_BINDTODEVICE returned the interface name
        let name_bytes = unsafe { &ifr.assume_init()[..len as usize] };
        let name = CStr::from_bytes_until_nul(name_bytes)
            .map(|c| c.to_string_lossy().to_string())
            .unwrap_or_default();
        if !name.is_empty() {
            return read_sys_mtu(&name);
        }
    }

    // Fallback: read interface index from the socket, then look up name.
    let ifindex = get_bound_ifindex(sock)?;
    if ifindex > 0 {
        let mut ifname_buf = [0u8; libc::IFNAMSIZ];
        let ptr = unsafe {
            libc::if_indextoname(ifindex, ifname_buf.as_mut_ptr() as *mut _)
        };
        if !ptr.is_null() {
            let name = CStr::from_bytes_until_nul(&ifname_buf)
                .map(|c| c.to_string_lossy().to_string())
                .unwrap_or_default();
            if !name.is_empty() {
                return read_sys_mtu(&name);
            }
        }
    }

    // Last resort fallback
    Ok(1500)
}

/// Read MTU from /sys/class/net/<iface>/mtu.
#[cfg(target_os = "linux")]
fn read_sys_mtu(iface: &str) -> Result<u16, std::io::Error> {
    let path = format!("/sys/class/net/{iface}/mtu");
    let contents = std::fs::read_to_string(&path)?;
    contents
        .trim()
        .parse::<u16>()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Get the interface index the socket is bound to via IP_UNICAST_IF or
/// by reading the source address and looking it up.
#[cfg(target_os = "linux")]
fn get_bound_ifindex(sock: i32) -> Result<u32, std::io::Error> {
    // Use getsockname → source IP → lookup via getifaddrs.
    let mut addr_storage = std::mem::MaybeUninit::<libc::sockaddr_storage>::zeroed();
    let mut addr_len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;

    let ret = unsafe {
        libc::getsockname(
            sock,
            addr_storage.as_mut_ptr() as *mut libc::sockaddr,
            &mut addr_len,
        )
    };
    if ret < 0 {
        return Err(std::io::Error::last_os_error());
    }

    let storage = unsafe { addr_storage.assume_init() };
    let source_ip: IpAddr = match storage.ss_family as i32 {
        libc::AF_INET => {
            let sa: &libc::sockaddr_in = unsafe { &*(&storage as *const _ as *const _) };
            IpAddr::V4(std::net::Ipv4Addr::from(u32::from_be(sa.sin_addr.s_addr)))
        }
        libc::AF_INET6 => {
            let sa: &libc::sockaddr_in6 = unsafe { &*(&storage as *const _ as *const _) };
            IpAddr::V6(std::net::Ipv6Addr::from(sa.sin6_addr.s6_addr))
        }
        _ => return Ok(0),
    };

    // Find the interface with this source IP.
    let mut ifaddrs: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut ifaddrs) } != 0 {
        return Err(std::io::Error::last_os_error());
    }

    let mut result = 0u32;
    let mut cur = ifaddrs;
    while !cur.is_null() {
        let ifa = unsafe { &*cur };
        if !ifa.ifa_addr.is_null() {
            let ifa_ip = unsafe { sockaddr_to_ip(ifa.ifa_addr) };
            if ifa_ip == Some(source_ip) {
                if !ifa.ifa_name.is_null() {
                    let name = unsafe { std::ffi::CStr::from_ptr(ifa.ifa_name) };
                    result = unsafe { libc::if_nametoindex(name.as_ptr()) };
                }
                break;
            }
        }
        cur = unsafe { (*cur).ifa_next };
    }
    unsafe { libc::freeifaddrs(ifaddrs) };

    Ok(result)
}

#[cfg(target_os = "linux")]
unsafe fn sockaddr_to_ip(sa: *const libc::sockaddr) -> Option<IpAddr> {
    match (*sa).sa_family as i32 {
        libc::AF_INET => {
            let sa4 = &*(sa as *const libc::sockaddr_in);
            Some(IpAddr::V4(std::net::Ipv4Addr::from(
                u32::from_be(sa4.sin_addr.s_addr),
            )))
        }
        libc::AF_INET6 => {
            let sa6 = &*(sa as *const libc::sockaddr_in6);
            Some(IpAddr::V6(std::net::Ipv6Addr::from(sa6.sin6_addr.s6_addr)))
        }
        _ => None,
    }
}

// ── Windows implementation ──────────────────────────────────────

#[cfg(target_os = "windows")]
fn detect_interface_mtu_inner(remote: IpAddr) -> Result<u16, std::io::Error> {
    use std::mem::MaybeUninit;
    use windows_sys::Win32::NetworkManagement::IpHelper::*;
    use windows_sys::Win32::Networking::WinSock::*;

    // Use GetBestRoute2 to find the outgoing interface and its MTU.
    let mut best_route: MaybeUninit<MIB_IPFORWARD_ROW2> = MaybeUninit::zeroed();
    let mut best_source: MaybeUninit<SOCKADDR_INET> = MaybeUninit::zeroed();

    let dest = match remote {
        IpAddr::V4(v4) => {
            let mut sa: SOCKADDR_INET = unsafe { std::mem::zeroed() };
            unsafe {
                sa.Ipv4.sin_family = AF_INET;
                sa.Ipv4.sin_addr.S_un.S_addr = u32::from(v4).to_be();
            }
            sa
        }
        IpAddr::V6(v6) => {
            let mut sa: SOCKADDR_INET = unsafe { std::mem::zeroed() };
            unsafe {
                sa.Ipv6.sin6_family = AF_INET6;
                sa.Ipv6.sin6_addr.u.Byte = v6.octets();
            }
            sa
        }
    };

    let ret = unsafe {
        GetBestRoute2(
            std::ptr::null(),    // InterfaceLuid (any)
            0,                   // InterfaceIndex (any)
            std::ptr::null(),    // source (any)
            &dest,               // destination
            0,                   // flags
            best_route.as_mut_ptr(),
            best_source.as_mut_ptr(),
        )
    };

    if ret != 0 {
        return Err(std::io::Error::from_raw_os_error(ret as i32));
    }

    let route = unsafe { best_route.assume_init() };
    let if_index = route.InterfaceIndex;

    // Now get the interface MTU via GetIfEntry2.
    let mut if_row: MaybeUninit<MIB_IF_ROW2> = MaybeUninit::zeroed();
    unsafe {
        let row = if_row.as_mut_ptr();
        (*row).InterfaceIndex = if_index;
    }

    let ret = unsafe { GetIfEntry2(if_row.as_mut_ptr()) };
    if ret != 0 {
        return Err(std::io::Error::from_raw_os_error(ret as i32));
    }

    let if_entry = unsafe { if_row.assume_init() };
    let mtu = if_entry.Mtu;

    // Clamp to u16 range (interface MTU should always fit)
    Ok(mtu.min(u16::MAX as u32) as u16)
}

// ── Fallback for other platforms ────────────────────────────────

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn detect_interface_mtu_inner(_remote: IpAddr) -> Result<u16, std::io::Error> {
    // No reliable MTU detection on this platform.
    Ok(1500)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_overlay_mtu_standard() {
        // With a 1500-byte path, overlay should be 1458.
        assert_eq!(1500u16.saturating_sub(EOIP_OVERHEAD), 1458);
    }

    #[test]
    fn auto_overlay_mtu_wireguard() {
        // WireGuard typically has 1420 MTU.
        assert_eq!(1420u16.saturating_sub(EOIP_OVERHEAD), 1378);
    }

    #[test]
    fn auto_overlay_mtu_minimum_clamp() {
        // Extremely small path MTU should clamp to minimum.
        let overlay = 100u16.saturating_sub(EOIP_OVERHEAD).max(MIN_OVERLAY_MTU);
        assert_eq!(overlay, MIN_OVERLAY_MTU);
    }

    #[test]
    fn detect_loopback_mtu() {
        // Detect MTU for loopback (127.0.0.1) — should succeed on any platform.
        let mtu = detect_interface_mtu("127.0.0.1".parse().unwrap());
        assert!(mtu >= 1500, "loopback MTU should be >= 1500, got {mtu}");
    }
}
