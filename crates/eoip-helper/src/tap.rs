//! TAP interface creation via `/dev/net/tun` ioctl.
//!
//! Creates Layer 2 (Ethernet) virtual network interfaces using `TUNSETIFF`
//! with `IFF_TAP | IFF_NO_PI` flags. The returned fd can be passed to the
//! unprivileged daemon via SCM_RIGHTS.

use std::os::fd::OwnedFd;
#[cfg(target_os = "linux")]
use std::os::fd::FromRawFd;

use eoip_proto::EoipError;

/// Maximum interface name length (including null terminator).
#[cfg(target_os = "linux")]
const IFNAMSIZ: usize = 16;

/// Create a TAP interface with the given name.
///
/// Returns an `OwnedFd` for the TAP device. The interface is created in
/// Layer 2 mode (`IFF_TAP`) without the 4-byte packet info header (`IFF_NO_PI`).
///
/// Requires `CAP_NET_ADMIN` or root.
#[cfg(target_os = "linux")]
pub fn create_tap_interface(name: &str) -> Result<OwnedFd, EoipError> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;

    if name.is_empty() || name.len() >= IFNAMSIZ {
        return Err(EoipError::TapError {
            iface: name.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("interface name must be 1-{} chars", IFNAMSIZ - 1),
            ),
        });
    }

    // Open the TUN/TAP clone device
    let tun_path = CString::new("/dev/net/tun").unwrap();
    let fd = unsafe { libc::open(tun_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(EoipError::TapError {
            iface: name.to_string(),
            source: std::io::Error::last_os_error(),
        });
    }

    // Safety: fd is valid, we just opened it
    let owned_fd = unsafe { OwnedFd::from_raw_fd(fd) };

    // Prepare ifreq struct
    let mut ifr = Ifreq::new();
    let name_bytes = name.as_bytes();
    ifr.ifr_name[..name_bytes.len()].copy_from_slice(name_bytes);
    // IFF_TAP = 0x0002, IFF_NO_PI = 0x1000, IFF_NAPI = 0x0010
    // IFF_NAPI enables a dedicated NAPI instance for packets written to TAP,
    // allowing kernel-side batching and GRO coalescing instead of using the
    // shared per-CPU backlog queue. Available since Linux ~4.15.
    const IFF_NAPI: i32 = 0x0010;
    ifr.ifr_ifru.ifr_flags = (libc::IFF_TAP | libc::IFF_NO_PI | IFF_NAPI) as i16;

    // TUNSETIFF ioctl
    let ret = unsafe { libc::ioctl(owned_fd.as_raw_fd(), TUNSETIFF, &ifr) };
    if ret < 0 {
        return Err(EoipError::TapError {
            iface: name.to_string(),
            source: std::io::Error::last_os_error(),
        });
    }

    let actual_name = ifreq_name(&ifr);
    tracing::info!(interface = %actual_name, "created TAP interface");

    Ok(owned_fd)
}

#[cfg(not(target_os = "linux"))]
pub fn create_tap_interface(name: &str) -> Result<OwnedFd, EoipError> {
    Err(EoipError::TapError {
        iface: name.to_string(),
        source: std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "TAP interfaces are only supported on Linux",
        ),
    })
}

/// Set the MTU on an existing network interface.
///
/// Requires `CAP_NET_ADMIN` or root.
#[cfg(target_os = "linux")]
pub fn set_interface_mtu(name: &str, mtu: u16) -> Result<(), EoipError> {
    if name.is_empty() || name.len() >= IFNAMSIZ {
        return Err(EoipError::TapError {
            iface: name.to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("interface name must be 1-{} chars", IFNAMSIZ - 1),
            ),
        });
    }

    let sock = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if sock < 0 {
        return Err(EoipError::TapError {
            iface: name.to_string(),
            source: std::io::Error::last_os_error(),
        });
    }
    let _guard = unsafe { std::os::fd::OwnedFd::from_raw_fd(sock) };

    let mut ifr = Ifreq::new();
    let name_bytes = name.as_bytes();
    ifr.ifr_name[..name_bytes.len()].copy_from_slice(name_bytes);
    ifr.ifr_ifru.ifr_mtu = mtu as i32;

    let ret = unsafe { libc::ioctl(sock, SIOCSIFMTU, &ifr) };
    if ret < 0 {
        return Err(EoipError::TapError {
            iface: name.to_string(),
            source: std::io::Error::last_os_error(),
        });
    }

    tracing::info!(interface = %name, mtu = mtu, "set interface MTU");
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn set_interface_mtu(name: &str, mtu: u16) -> Result<(), EoipError> {
    tracing::debug!(interface = %name, mtu = mtu, "set_interface_mtu is a no-op on this platform");
    let _ = (name, mtu);
    Ok(())
}

// ── Linux ifreq / ioctl definitions ──────────────────────────────

/// `TUNSETIFF` ioctl request code.
#[cfg(target_os = "linux")]
const TUNSETIFF: libc::c_ulong = 0x400454CA;

/// `SIOCSIFMTU` ioctl request code (set interface MTU).
#[cfg(target_os = "linux")]
const SIOCSIFMTU: libc::c_ulong = 0x8922;

/// Minimal `ifreq` struct matching the Linux kernel layout.
#[cfg(target_os = "linux")]
#[repr(C)]
struct Ifreq {
    ifr_name: [u8; IFNAMSIZ],
    ifr_ifru: IfrIfru,
}

#[cfg(target_os = "linux")]
#[repr(C)]
union IfrIfru {
    ifr_flags: i16,
    ifr_mtu: i32,
    _padding: [u8; 24],
}

#[cfg(target_os = "linux")]
impl Ifreq {
    fn new() -> Self {
        // Safety: zero-initialized ifreq is valid
        unsafe { std::mem::zeroed() }
    }
}

/// Extract the null-terminated interface name from an ifreq.
#[cfg(target_os = "linux")]
fn ifreq_name(ifr: &Ifreq) -> String {
    let end = ifr
        .ifr_name
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(IFNAMSIZ);
    String::from_utf8_lossy(&ifr.ifr_name[..end]).to_string()
}
