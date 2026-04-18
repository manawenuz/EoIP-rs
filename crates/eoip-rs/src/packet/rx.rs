//! RX packet path: raw socket → decode → DashMap demux → crossbeam channel → TAP write.
//!
//! Uses dedicated OS threads with `recvmmsg` for batched packet receive.
//! Per-tunnel consumer tasks run as tokio tasks writing to TAP via `AsyncFd`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crossbeam::channel::Sender;
use tokio_util::sync::CancellationToken;

use eoip_proto::gre::{self, EOIP_HEADER_LEN};
use eoip_proto::etherip;
use eoip_proto::DemuxKey;

use crate::packet::buffer::{BufferPool, PacketBuf, MAX_FRAME_SIZE};
use crate::tunnel::registry::TunnelRegistry;

/// Handle for a running RX pipeline.
pub struct RxPipelineHandle {
    pub v4_threads: Vec<std::thread::JoinHandle<()>>,
    pub v6_thread: Option<std::thread::JoinHandle<()>>,
}

/// Per-tunnel TX channel for delivering decoded frames to TAP writer tasks.
pub type TunnelRxSender = Sender<PacketBuf>;

/// Batch size for recvmmsg.
const RECV_BATCH: usize = 32;

/// Per-buffer size for recvmmsg. Max EoIP frame = 20 (IP) + 8 (GRE) + 1500 (MTU) + 14 (ETH) + 4 (VLAN) = 1546.
/// Round up to 2048 for alignment. Was 65536 — 42x oversized, wasting cache.
const RECV_BUF_SIZE: usize = 2048;

/// Start the RX pipeline: spawn dedicated OS threads for v4 and v6 raw sockets.
///
/// If `af_packet_v4` is provided, uses AF_PACKET socket for IPv4 RX instead
/// of the raw GRE socket. This enables PACKET_MMAP zero-copy RX.
pub fn start_rx_pipeline(
    raw_v4: Option<BorrowedFd<'_>>,
    raw_v6: Option<BorrowedFd<'_>>,
    af_packet_v4: Option<BorrowedFd<'_>>,
    registry: Arc<TunnelRegistry>,
    pool: Arc<BufferPool>,
    shutdown: CancellationToken,
    rx_workers: usize,
) -> RxPipelineHandle {
    let rx_workers = rx_workers.max(1);

    // IPv4 RX: prefer PACKET_MMAP ring → AF_PACKET recvmmsg → raw socket recvmmsg
    let v4_threads = if let Some(af_fd) = af_packet_v4 {
        // AF_PACKET path — single worker, try ring first, fall back to recvmmsg
        let raw_fd = af_fd.as_raw_fd();
        let registry = Arc::clone(&registry);
        let pool = Arc::clone(&pool);
        let shutdown = shutdown.clone();
        vec![
            std::thread::Builder::new()
                .name("rx-v4-afp".into())
                .spawn(move || {
                    // Try PACKET_MMAP ring first
                    #[cfg(target_os = "linux")]
                    {
                        rx_loop_v4_mmap(raw_fd, &registry, &pool, &shutdown);
                        // If mmap returned (error or shutdown), we're done
                        return;
                    }
                    // Non-linux fallback: AF_PACKET recvmmsg
                    #[cfg(not(target_os = "linux"))]
                    rx_loop_v4_afpacket(raw_fd, &registry, &pool, &shutdown);
                })
                .expect("failed to spawn RX AF_PACKET thread"),
        ]
    } else if let Some(fd) = raw_v4 {
        // Fallback: raw socket recvmmsg with N workers
        let raw_fd = fd.as_raw_fd();
        (0..rx_workers)
            .map(|i| {
                let registry = Arc::clone(&registry);
                let pool = Arc::clone(&pool);
                let shutdown = shutdown.clone();
                std::thread::Builder::new()
                    .name(format!("rx-v4-{i}"))
                    .spawn(move || rx_loop_v4(raw_fd, &registry, &pool, &shutdown))
                    .expect("failed to spawn RX v4 thread")
            })
            .collect()
    } else {
        vec![]
    };

    let v6_thread = raw_v6.map(|fd| {
        let raw_fd = fd.as_raw_fd();
        let registry = Arc::clone(&registry);
        let pool = Arc::clone(&pool);
        let shutdown = shutdown.clone();
        std::thread::Builder::new()
            .name("rx-v6".into())
            .spawn(move || rx_loop_v6(raw_fd, &registry, &pool, &shutdown))
            .expect("failed to spawn RX v6 thread")
    });

    RxPipelineHandle { v4_threads, v6_thread }
}

/// Rate-limited demux miss logging.
static LAST_MISS_LOG: AtomicU64 = AtomicU64::new(0);
static MISS_COUNT: AtomicU64 = AtomicU64::new(0);

fn log_demux_miss(key: &DemuxKey) {
    let count = MISS_COUNT.fetch_add(1, Ordering::Relaxed);
    let now = Instant::now();
    let now_ms = now.elapsed().as_millis() as u64;
    let last = LAST_MISS_LOG.load(Ordering::Relaxed);
    if now_ms.wrapping_sub(last) > 1000
        && LAST_MISS_LOG
            .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        tracing::info!(
            tunnel_id = key.tunnel_id,
            peer = %key.peer_addr,
            missed_since_last_log = count,
            "RX demux miss — no matching tunnel"
        );
        MISS_COUNT.store(0, Ordering::Relaxed);
    }
}

/// Get coarse timestamp (ms since epoch) via CLOCK_REALTIME_COARSE (vDSO, ~4ns).
/// Refreshes every 64 packets to amortize even the vDSO cost.
#[inline(always)]
fn coarse_timestamp_ms() -> i64 {
    thread_local! {
        static CACHED: std::cell::Cell<(u64, i64)> = const { std::cell::Cell::new((0, 0)) };
    }
    CACHED.with(|c| {
        let (count, ts) = c.get();
        if count % 64 == 0 {
            let now = {
                #[cfg(target_os = "linux")]
                {
                    let mut tp = libc::timespec { tv_sec: 0, tv_nsec: 0 };
                    unsafe { libc::clock_gettime(libc::CLOCK_REALTIME_COARSE, &mut tp) };
                    tp.tv_sec as i64 * 1000 + tp.tv_nsec as i64 / 1_000_000
                }
                #[cfg(not(target_os = "linux"))]
                {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0)
                }
            };
            c.set((count + 1, now));
            now
        } else {
            c.set((count + 1, ts));
            ts
        }
    })
}

/// Process a single received IPv4 packet containing EoIP.
#[inline]
fn process_v4_packet(
    buf: &[u8],
    n: usize,
    registry: &TunnelRegistry,
    pool: &BufferPool,
) {
    if n < 20 {
        return;
    }

    let ihl = (buf[0] & 0x0F) as usize;
    let ip_hdr_len = ihl * 4;
    if n < ip_hdr_len + EOIP_HEADER_LEN {
        return;
    }

    let src_ip = IpAddr::V4(Ipv4Addr::new(buf[12], buf[13], buf[14], buf[15]));
    let eoip_data = &buf[ip_hdr_len..n];

    let (tunnel_id, payload_len, hdr_len) = match gre::decode_eoip_header(eoip_data) {
        Ok(v) => v,
        Err(_) => return,
    };

    let key = DemuxKey { tunnel_id, peer_addr: src_ip };

    let guard = match registry.get_ref(&key) {
        Some(g) => g,
        None => {
            log_demux_miss(&key);
            return;
        }
    };
    let handle = guard.value();

    let frame_data = &eoip_data[hdr_len..];

    // Update stats (atomic, lock-free)
    handle.stats.rx_packets.fetch_add(1, Ordering::Relaxed);
    handle.stats.rx_bytes.fetch_add(frame_data.len() as u64, Ordering::Relaxed);
    handle.stats.last_rx_timestamp.store(coarse_timestamp_ms(), Ordering::Relaxed);

    // Keepalive: stats only
    if payload_len == 0 || frame_data.is_empty() {
        return;
    }

    // Deliver frame to TAP writer
    if let Some(ref tx) = handle.rx_channel {
        let mut pbuf = pool.get();
        let dest = pbuf.payload_mut();
        let copy_len = frame_data.len().min(MAX_FRAME_SIZE);
        dest[..copy_len].copy_from_slice(&frame_data[..copy_len]);
        pbuf.set_len(copy_len);

        if tx.try_send(pbuf).is_err() {
            handle.stats.rx_errors.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// EoIP (IPv4, protocol 47) receive loop using recvmmsg for batch receive.
fn rx_loop_v4(
    raw_fd: std::os::fd::RawFd,
    registry: &TunnelRegistry,
    pool: &BufferPool,
    shutdown: &CancellationToken,
) {
    tracing::info!("RX v4 worker started");

    // Try recvmmsg first (Linux only), fall back to blocking read
    #[cfg(target_os = "linux")]
    if try_rx_loop_recvmmsg(raw_fd, registry, pool, shutdown) {
        return;
    }

    // Fallback: blocking read loop (no sleep, fd should be blocking)
    let mut recv_buf = vec![0u8; RECV_BUF_SIZE];
    loop {
        if shutdown.is_cancelled() {
            break;
        }

        let n = match nix::unistd::read(raw_fd, &mut recv_buf) {
            Ok(n) => n,
            Err(nix::errno::Errno::EAGAIN | nix::errno::Errno::EINTR) => continue,
            Err(e) => {
                if !shutdown.is_cancelled() {
                    tracing::error!(%e, "RX v4 read error");
                }
                break;
            }
        };

        process_v4_packet(&recv_buf, n, registry, pool);
    }

    tracing::info!("RX v4 worker stopped");
}

/// Attempt to use recvmmsg for batched receives. Returns true if it ran
/// (even if shutdown), false if recvmmsg isn't available.
#[cfg(target_os = "linux")]
fn try_rx_loop_recvmmsg(
    raw_fd: std::os::fd::RawFd,
    registry: &TunnelRegistry,
    pool: &BufferPool,
    shutdown: &CancellationToken,
) -> bool {
    // Allocate batch buffers
    let mut bufs: Vec<Vec<u8>> = (0..RECV_BATCH).map(|_| vec![0u8; RECV_BUF_SIZE]).collect();
    let mut iovs: Vec<libc::iovec> = bufs
        .iter_mut()
        .map(|buf| libc::iovec {
            iov_base: buf.as_mut_ptr() as *mut _,
            iov_len: buf.len(),
        })
        .collect();

    let mut msg_hdrs: Vec<libc::mmsghdr> = iovs
        .iter_mut()
        .map(|iov| {
            let mut hdr: libc::mmsghdr = unsafe { std::mem::zeroed() };
            hdr.msg_hdr.msg_iov = iov as *mut _;
            hdr.msg_hdr.msg_iovlen = 1;
            hdr
        })
        .collect();

    // Test if recvmmsg works (some kernels/platforms don't support it)
    let timeout = libc::timespec { tv_sec: 0, tv_nsec: 100_000_000 }; // 100ms
    let ret = unsafe {
        libc::recvmmsg(
            raw_fd,
            msg_hdrs.as_mut_ptr(),
            1, // just test with 1
            libc::MSG_DONTWAIT,
            &timeout as *const _ as *mut _,
        )
    };
    if ret < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ENOSYS) {
            tracing::warn!("recvmmsg not supported, falling back to read()");
            return false;
        }
        // EAGAIN is fine — means it works but no data yet
    }

    tracing::info!("RX v4 using recvmmsg (batch={})", RECV_BATCH);

    loop {
        if shutdown.is_cancelled() {
            break;
        }

        // Receive batch of packets
        let timeout = libc::timespec { tv_sec: 1, tv_nsec: 0 };
        let count = unsafe {
            libc::recvmmsg(
                raw_fd,
                msg_hdrs.as_mut_ptr(),
                RECV_BATCH as u32,
                libc::MSG_WAITFORONE, // return after first packet if others aren't ready
                &timeout as *const _ as *mut _,
            )
        };

        if count < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if err.kind() == std::io::ErrorKind::WouldBlock {
                continue;
            }
            if !shutdown.is_cancelled() {
                tracing::error!(%err, "recvmmsg error");
            }
            break;
        }

        // Process all received packets
        for i in 0..count as usize {
            let n = msg_hdrs[i].msg_len as usize;
            process_v4_packet(&bufs[i], n, registry, pool);
        }
    }

    true
}

/// EoIP (IPv4) receive loop using AF_PACKET socket.
///
/// Currently uses recvmmsg on the AF_PACKET socket to validate the socket
/// + BPF filter under load. The ring-based path (rx_loop_v4_mmap) will
/// replace this once validated.
///
/// AF_PACKET SOCK_RAW delivers full Ethernet frames. We strip the 14-byte
/// L2 header to get IP data for process_v4_packet().
#[cfg(target_os = "linux")]
fn rx_loop_v4_afpacket(
    raw_fd: std::os::fd::RawFd,
    registry: &TunnelRegistry,
    pool: &BufferPool,
    shutdown: &CancellationToken,
) {
    const ETH_HLEN: usize = 14;

    tracing::info!("RX v4 AF_PACKET worker started (recvmmsg validation)");

    let mut bufs: Vec<Vec<u8>> = (0..RECV_BATCH).map(|_| vec![0u8; RECV_BUF_SIZE]).collect();
    let mut iovs: Vec<libc::iovec> = bufs
        .iter_mut()
        .map(|buf| libc::iovec {
            iov_base: buf.as_mut_ptr() as *mut _,
            iov_len: buf.len(),
        })
        .collect();

    let mut msg_hdrs: Vec<libc::mmsghdr> = iovs
        .iter_mut()
        .map(|iov| {
            let mut hdr: libc::mmsghdr = unsafe { std::mem::zeroed() };
            hdr.msg_hdr.msg_iov = iov as *mut _;
            hdr.msg_hdr.msg_iovlen = 1;
            hdr
        })
        .collect();

    loop {
        if shutdown.is_cancelled() {
            break;
        }

        let timeout = libc::timespec { tv_sec: 1, tv_nsec: 0 };
        let count = unsafe {
            libc::recvmmsg(
                raw_fd,
                msg_hdrs.as_mut_ptr(),
                RECV_BATCH as u32,
                libc::MSG_WAITFORONE,
                &timeout as *const _ as *mut _,
            )
        };

        if count < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted
                || err.kind() == std::io::ErrorKind::WouldBlock
            {
                continue;
            }
            if !shutdown.is_cancelled() {
                tracing::error!(%err, "AF_PACKET recvmmsg error");
            }
            break;
        }

        for i in 0..count as usize {
            let n = msg_hdrs[i].msg_len as usize;
            // AF_PACKET SOCK_RAW: first 14 bytes are Ethernet header, skip them
            if n <= ETH_HLEN {
                continue;
            }
            process_v4_packet(&bufs[i][ETH_HLEN..], n - ETH_HLEN, registry, pool);
        }
    }

    tracing::info!("RX v4 AF_PACKET worker stopped");
}

/// EoIP (IPv4) receive loop using AF_PACKET + TPACKET_V3 ring buffer.
///
/// Zero-copy path: kernel writes packets directly into mmap'd ring buffer.
/// We process blocks in-place, calling `process_v4_packet()` with IP data
/// from `tp_net` offset (L2 header already skipped by the ring abstraction).
#[cfg(target_os = "linux")]
fn rx_loop_v4_mmap(
    af_packet_fd: std::os::fd::RawFd,
    registry: &TunnelRegistry,
    pool: &BufferPool,
    shutdown: &CancellationToken,
) {
    use crate::packet::packet_mmap::PacketMmapRing;

    let mut ring = match PacketMmapRing::new(af_packet_fd) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(%e, "failed to set up PACKET_MMAP ring");
            return;
        }
    };

    tracing::info!("RX v4 PACKET_MMAP worker started");

    loop {
        if shutdown.is_cancelled() {
            break;
        }

        ring.process_block(1000, |data, data_len| {
            process_v4_packet(data, data_len, registry, pool);
        });
    }

    tracing::info!("RX v4 PACKET_MMAP worker stopped");
}

/// EoIPv6 (IPv6, protocol 97) receive loop.
///
/// Uses `recvfrom()` instead of `read()` to obtain the source IPv6 address,
/// which the kernel provides via the sockaddr (IPv6 raw sockets strip the
/// IP header from the payload).
fn rx_loop_v6(
    raw_fd: std::os::fd::RawFd,
    registry: &TunnelRegistry,
    pool: &BufferPool,
    shutdown: &CancellationToken,
) {
    tracing::info!("RX v6 worker started");
    let mut recv_buf = vec![0u8; RECV_BUF_SIZE];
    let mut sockaddr: libc::sockaddr_in6 = unsafe { std::mem::zeroed() };

    loop {
        if shutdown.is_cancelled() {
            break;
        }

        let mut addr_len = std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t;
        let n = unsafe {
            libc::recvfrom(
                raw_fd,
                recv_buf.as_mut_ptr() as *mut libc::c_void,
                recv_buf.len(),
                0,
                &mut sockaddr as *mut _ as *mut libc::sockaddr,
                &mut addr_len,
            )
        };

        if n < 0 {
            let err = std::io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EAGAIN) | Some(libc::EINTR) => continue,
                _ => {
                    if !shutdown.is_cancelled() {
                        tracing::error!(%err, "RX v6 recvfrom error");
                    }
                    break;
                }
            }
        }

        let n = n as usize;
        if n < 2 {
            continue;
        }

        let src_ip = IpAddr::V6(Ipv6Addr::from(sockaddr.sin6_addr.s6_addr));

        let (tunnel_id, hdr_len) = match etherip::decode_eoipv6_header(&recv_buf[..n]) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let key = DemuxKey {
            tunnel_id,
            peer_addr: src_ip,
        };

        let guard = match registry.get_ref(&key) {
            Some(g) => g,
            None => {
                log_demux_miss(&key);
                continue;
            }
        };
        let handle = guard.value();

        let frame_data = &recv_buf[hdr_len..n];
        let is_keepalive = frame_data.is_empty();

        handle.stats.rx_packets.fetch_add(1, Ordering::Relaxed);
        handle.stats.rx_bytes.fetch_add(frame_data.len() as u64, Ordering::Relaxed);
        handle.stats.last_rx_timestamp.store(coarse_timestamp_ms(), Ordering::Relaxed);

        if is_keepalive {
            continue;
        }

        if let Some(ref tx) = handle.rx_channel {
            let mut buf = pool.get();
            let payload = buf.payload_mut();
            let copy_len = frame_data.len().min(MAX_FRAME_SIZE);
            payload[..copy_len].copy_from_slice(&frame_data[..copy_len]);
            buf.set_len(copy_len);

            if tx.try_send(buf).is_err() {
                handle.stats.rx_errors.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    tracing::info!("RX v6 worker stopped");
}
