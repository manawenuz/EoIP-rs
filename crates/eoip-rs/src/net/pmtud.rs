//! Path MTU Discovery (PMTUD) via ICMP probing.
//!
//! Sends ICMP Echo Request packets with DF=1 (Don't Fragment) at varying
//! sizes to discover the maximum non-fragmenting packet size on the path
//! to a remote peer. Uses binary search for efficiency (6-7 probes).
//!
//! Falls back to interface MTU detection if ICMP is blocked.

use std::net::IpAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::tunnel::handle::TunnelHandle;

use super::mtu::{self, EOIP_OVERHEAD, MIN_OVERLAY_MTU};

/// Minimum probe size (IPv4 minimum MTU).
const MIN_PROBE: u16 = 576;

/// Maximum probe size (standard Ethernet MTU).
const MAX_PROBE: u16 = 1500;

/// Timeout per probe attempt.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Number of retries per probe size.
const PROBE_RETRIES: u32 = 3;

/// Re-probe interval (10 minutes).
const REPROBE_INTERVAL: Duration = Duration::from_secs(600);

/// Spawn a background PMTUD task for a tunnel.
///
/// Probes the path MTU at startup and re-probes every 10 minutes.
/// Updates `handle.actual_mtu` and logs changes.
pub fn spawn_pmtud_task(
    handle: Arc<TunnelHandle>,
    remote: IpAddr,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        // Initial probe
        do_pmtud(&handle, remote).await;

        // Periodic re-probe
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(REPROBE_INTERVAL) => {
                    do_pmtud(&handle, remote).await;
                }
            }
        }

        tracing::debug!(
            tunnel_id = handle.config.tunnel_id,
            "PMTUD task shutting down"
        );
    });
}

async fn do_pmtud(handle: &TunnelHandle, remote: IpAddr) {
    let tunnel_id = handle.config.tunnel_id;

    match probe_path_mtu(remote).await {
        Ok(path_mtu) => {
            let overlay = path_mtu.saturating_sub(EOIP_OVERHEAD).max(MIN_OVERLAY_MTU);
            let prev = handle.actual_mtu.swap(overlay, Ordering::Relaxed);

            if prev != overlay {
                tracing::info!(
                    tunnel_id,
                    remote = %remote,
                    path_mtu,
                    overlay_mtu = overlay,
                    prev_mtu = prev,
                    "PMTUD discovered new path MTU"
                );

                // Update TAP interface MTU if it changed.
                #[cfg(target_os = "linux")]
                {
                    let iface = handle.config.effective_iface_name();
                    if let Err(e) = eoip_helper::tap::set_interface_mtu(&iface, overlay) {
                        tracing::warn!(
                            tunnel_id,
                            interface = %iface,
                            %e,
                            "failed to update TAP MTU after PMTUD"
                        );
                    }
                }
            }
        }
        Err(e) => {
            tracing::debug!(
                tunnel_id,
                remote = %remote,
                %e,
                "PMTUD probe failed, keeping current MTU"
            );

            // If we never had a successful probe, fall back to interface detection.
            if handle.actual_mtu.load(Ordering::Relaxed) == 0 {
                let fallback = mtu::auto_overlay_mtu(remote);
                handle.actual_mtu.store(fallback, Ordering::Relaxed);
                tracing::info!(
                    tunnel_id,
                    overlay_mtu = fallback,
                    "PMTUD unavailable, using interface MTU fallback"
                );
            }
        }
    }
}

/// Probe the path MTU to `remote` using ICMP Echo with DF=1.
///
/// Returns the discovered path MTU (e.g. 1500, 1420).
/// Binary search between [`MIN_PROBE`] and [`MAX_PROBE`].
async fn probe_path_mtu(remote: IpAddr) -> Result<u16, PmtudError> {
    // We run the blocking ICMP probe on a dedicated thread to avoid
    // blocking the tokio runtime.
    tokio::task::spawn_blocking(move || probe_path_mtu_blocking(remote))
        .await
        .map_err(|e| PmtudError::Internal(e.to_string()))?
}

#[derive(Debug, thiserror::Error)]
enum PmtudError {
    #[error("ICMP socket error: {0}")]
    Socket(#[from] std::io::Error),
    #[error("all probes timed out")]
    AllTimedOut,
    #[error("internal error: {0}")]
    Internal(String),
}

// ── Platform-specific ICMP probing ──────────────────────────────

#[cfg(target_os = "linux")]
fn probe_path_mtu_blocking(remote: IpAddr) -> Result<u16, PmtudError> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    // Only IPv4 for now (IPv6 PMTUD uses ICMPv6 which is different).
    let remote_v4 = match remote {
        IpAddr::V4(v4) => v4,
        IpAddr::V6(_) => {
            // For IPv6, fall back to interface detection for now.
            return Err(PmtudError::Internal("IPv6 PMTUD not yet implemented".into()));
        }
    };

    // Create raw ICMP socket.
    let sock = unsafe {
        libc::socket(
            libc::AF_INET,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::IPPROTO_ICMP,
        )
    };
    if sock < 0 {
        return Err(PmtudError::Socket(std::io::Error::last_os_error()));
    }
    let sock = unsafe { OwnedFd::from_raw_fd(sock) };
    let fd = sock.as_raw_fd();

    // Set IP_MTU_DISCOVER to IP_PMTUDISC_DO (force DF=1).
    let val: i32 = libc::IP_PMTUDISC_DO;
    let ret = unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_IP,
            libc::IP_MTU_DISCOVER,
            &val as *const _ as *const _,
            std::mem::size_of::<i32>() as u32,
        )
    };
    if ret < 0 {
        return Err(PmtudError::Socket(std::io::Error::last_os_error()));
    }

    // Set receive timeout for the socket.
    let tv = libc::timeval {
        tv_sec: PROBE_TIMEOUT.as_secs() as _,
        tv_usec: 0,
    };
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &tv as *const _ as *const _,
            std::mem::size_of::<libc::timeval>() as u32,
        );
    }

    // Connect to remote (for ICMP DGRAM socket, this sets the destination).
    let dest = libc::sockaddr_in {
        sin_family: libc::AF_INET as u16,
        sin_port: 0,
        sin_addr: libc::in_addr {
            s_addr: u32::from(remote_v4).to_be(),
        },
        sin_zero: [0; 8],
    };
    let ret = unsafe {
        libc::connect(
            fd,
            &dest as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as u32,
        )
    };
    if ret < 0 {
        return Err(PmtudError::Socket(std::io::Error::last_os_error()));
    }

    // Binary search for the maximum non-fragmenting size.
    // We send ICMP echo payloads. The kernel adds 8 bytes ICMP header + 20 bytes IP header.
    // So to probe path MTU X, we send a payload of (X - 28) bytes.
    let mut lo = MIN_PROBE;
    let mut hi = MAX_PROBE;
    let mut best = MIN_PROBE;

    // Build ICMP echo request payload (just needs to be the right size).
    // For SOCK_DGRAM/IPPROTO_ICMP, kernel handles ICMP header construction.
    // We just send the echo data payload.
    let icmp_header_overhead = 8u16; // ICMP header
    let ip_header = 20u16;           // IP header (no options)

    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let payload_size = mid.saturating_sub(ip_header + icmp_header_overhead);

        if probe_single(fd, payload_size, PROBE_RETRIES) {
            best = mid;
            if mid == hi {
                break;
            }
            lo = mid + 1;
        } else {
            if mid == lo {
                break;
            }
            hi = mid - 1;
        }
    }

    if best <= MIN_PROBE {
        // Even the minimum probe failed — ICMP is likely blocked.
        return Err(PmtudError::AllTimedOut);
    }

    Ok(best)
}

/// Send a single ICMP echo probe and check if it succeeds (gets a reply
/// or at least doesn't get "message too long").
#[cfg(target_os = "linux")]
fn probe_single(fd: i32, payload_size: u16, retries: u32) -> bool {
    // Payload: ICMP echo identifier (2) + sequence (2) + padding.
    // For SOCK_DGRAM ICMP, we write: [id:2][seq:2][data:N]
    let total = 4 + payload_size as usize; // id + seq + payload
    let mut buf = vec![0u8; total];
    // Set identifier and sequence
    buf[0] = 0x45; // identifier high
    buf[1] = 0x00; // identifier low
    // Sequence number varies per retry to avoid caching
    for attempt in 0..retries {
        buf[2] = (attempt >> 8) as u8;
        buf[3] = attempt as u8;

        let ret = unsafe {
            libc::send(fd, buf.as_ptr() as *const _, buf.len(), 0)
        };

        if ret < 0 {
            let err = std::io::Error::last_os_error();
            // EMSGSIZE means the packet is too big for the path.
            if err.raw_os_error() == Some(libc::EMSGSIZE) {
                return false;
            }
            // Other errors (ENETUNREACH, etc.) — try next attempt.
            continue;
        }

        // Try to receive a reply (or error).
        let mut recv_buf = [0u8; 1500];
        let ret = unsafe {
            libc::recv(fd, recv_buf.as_mut_ptr() as *mut _, recv_buf.len(), 0)
        };

        if ret > 0 {
            // Got a reply — this size works.
            return true;
        }

        // Timeout or error — the probe might still have worked if ICMP echo
        // replies are filtered. Check if we got EMSGSIZE on recv.
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EMSGSIZE) {
            return false;
        }
        // Timeout (EAGAIN/EWOULDBLOCK) — inconclusive, retry.
    }

    // All retries timed out — we can't tell if the size works.
    // Be conservative: assume it works (the host may not respond to ICMP).
    // The binary search will narrow down from above if larger sizes fail.
    true
}

// ── Windows implementation ──────────────────────────────────────

#[cfg(target_os = "windows")]
fn probe_path_mtu_blocking(remote: IpAddr) -> Result<u16, PmtudError> {
    use std::mem::MaybeUninit;

    let remote_v4 = match remote {
        IpAddr::V4(v4) => v4,
        IpAddr::V6(_) => {
            return Err(PmtudError::Internal("IPv6 PMTUD not yet implemented".into()));
        }
    };

    // Use IcmpSendEcho with DF=1 for path MTU discovery.
    // Windows provides a convenient API for this via iphlpapi.
    //
    // We'll use a binary search approach similar to Linux.

    let icmp_handle = unsafe { windows_sys::Win32::NetworkManagement::IpHelper::IcmpCreateFile() };
    if icmp_handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(PmtudError::Socket(std::io::Error::last_os_error()));
    }

    // IP_OPTION_INFORMATION with DF=1 (Flags = IP_FLAG_DF = 0x02).
    let ip_opts = windows_sys::Win32::NetworkManagement::IpHelper::IP_OPTION_INFORMATION {
        Ttl: 128,
        Tos: 0,
        Flags: 0x02, // IP_FLAG_DF
        OptionsSize: 0,
        OptionsData: std::ptr::null_mut(),
    };

    let ip_header = 20u16;
    let icmp_header = 8u16;
    let mut lo = MIN_PROBE;
    let mut hi = MAX_PROBE;
    let mut best = MIN_PROBE;

    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let payload_size = mid.saturating_sub(ip_header + icmp_header) as usize;

        let send_buf = vec![0x41u8; payload_size]; // 'A' padding
        let reply_size = std::mem::size_of::<windows_sys::Win32::NetworkManagement::IpHelper::ICMP_ECHO_REPLY>()
            + payload_size
            + 8;
        let mut reply_buf = vec![0u8; reply_size];

        let dest_addr = u32::from(remote_v4).to_be();
        let ret = unsafe {
            windows_sys::Win32::NetworkManagement::IpHelper::IcmpSendEcho(
                icmp_handle,
                dest_addr,
                send_buf.as_ptr() as *mut _,
                send_buf.len() as u16,
                &ip_opts as *const _ as *mut _,
                reply_buf.as_mut_ptr() as *mut _,
                reply_buf.len() as u32,
                PROBE_TIMEOUT.as_millis() as u32,
            )
        };

        if ret > 0 {
            // Success — this size works.
            let reply = unsafe {
                &*(reply_buf.as_ptr()
                    as *const windows_sys::Win32::NetworkManagement::IpHelper::ICMP_ECHO_REPLY)
            };
            // Status 0 = IP_SUCCESS
            if reply.Status == 0 {
                best = mid;
                if mid == hi {
                    break;
                }
                lo = mid + 1;
                continue;
            }
        }

        // Failed — likely packet too big (status 11009 = IP_PACKET_TOO_BIG).
        if mid == lo {
            break;
        }
        hi = mid - 1;
    }

    unsafe {
        windows_sys::Win32::NetworkManagement::IpHelper::IcmpCloseHandle(icmp_handle);
    }

    if best <= MIN_PROBE {
        return Err(PmtudError::AllTimedOut);
    }

    Ok(best)
}

// ── Fallback for other platforms ────────────────────────────────

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn probe_path_mtu_blocking(_remote: IpAddr) -> Result<u16, PmtudError> {
    Err(PmtudError::Internal("PMTUD not supported on this platform".into()))
}
