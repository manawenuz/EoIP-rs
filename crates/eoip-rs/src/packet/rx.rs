//! RX packet path: raw socket → decode → DashMap demux → crossbeam channel → TAP write.
//!
//! Uses dedicated OS threads (not tokio tasks) for tight `recvmmsg` loops.
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
    pub v4_thread: Option<std::thread::JoinHandle<()>>,
    pub v6_thread: Option<std::thread::JoinHandle<()>>,
}

/// Per-tunnel TX channel for delivering decoded frames to TAP writer tasks.
pub type TunnelRxSender = Sender<PacketBuf>;

/// Start the RX pipeline: spawn dedicated OS threads for v4 and v6 raw sockets.
pub fn start_rx_pipeline(
    raw_v4: Option<BorrowedFd<'_>>,
    raw_v6: Option<BorrowedFd<'_>>,
    registry: Arc<TunnelRegistry>,
    pool: Arc<BufferPool>,
    shutdown: CancellationToken,
) -> RxPipelineHandle {
    let v4_thread = raw_v4.map(|fd| {
        let raw_fd = fd.as_raw_fd();
        let registry = Arc::clone(&registry);
        let pool = Arc::clone(&pool);
        let shutdown = shutdown.clone();
        std::thread::Builder::new()
            .name("rx-v4".into())
            .spawn(move || rx_loop_v4(raw_fd, &registry, &pool, &shutdown))
            .expect("failed to spawn RX v4 thread")
    });

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

    RxPipelineHandle { v4_thread, v6_thread }
}

/// Rate-limited demux miss logging.
static LAST_MISS_LOG: AtomicU64 = AtomicU64::new(0);
static MISS_COUNT: AtomicU64 = AtomicU64::new(0);

fn log_demux_miss(key: &DemuxKey) {
    let count = MISS_COUNT.fetch_add(1, Ordering::Relaxed);
    let now = Instant::now();
    let now_ms = now.elapsed().as_millis() as u64; // relative
    let last = LAST_MISS_LOG.load(Ordering::Relaxed);
    // Log at most once per second
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

/// EoIP (IPv4, protocol 47) receive loop.
///
/// Runs on a dedicated OS thread. Reads raw IP packets, strips the IPv4
/// header, decodes the EoIP header, and dispatches to the appropriate tunnel.
fn rx_loop_v4(
    raw_fd: std::os::fd::RawFd,
    registry: &TunnelRegistry,
    pool: &BufferPool,
    shutdown: &CancellationToken,
) {
    tracing::info!("RX v4 worker started");
    let mut recv_buf = vec![0u8; 65536]; // max IP packet

    loop {
        if shutdown.is_cancelled() {
            break;
        }

        // Non-blocking read
        let n = match nix::unistd::read(raw_fd, &mut recv_buf) {
            Ok(n) => n,
            Err(nix::errno::Errno::EAGAIN) => {
                // Poll with short sleep to avoid busy-wait on non-blocking fd
                std::thread::sleep(std::time::Duration::from_micros(100));
                continue;
            }
            Err(e) => {
                if !shutdown.is_cancelled() {
                    tracing::error!(%e, "RX v4 read error");
                }
                break;
            }
        };

        if n < 20 {
            continue; // Too short for IPv4 header
        }

        // Parse IPv4 header to get src IP and payload offset
        let ihl = (recv_buf[0] & 0x0F) as usize;
        let ip_hdr_len = ihl * 4;
        if n < ip_hdr_len + EOIP_HEADER_LEN {
            continue;
        }

        let src_ip = IpAddr::V4(Ipv4Addr::new(
            recv_buf[12],
            recv_buf[13],
            recv_buf[14],
            recv_buf[15],
        ));

        let eoip_data = &recv_buf[ip_hdr_len..n];

        // Decode EoIP header
        let (tunnel_id, payload_len, hdr_len) = match gre::decode_eoip_header(eoip_data) {
            Ok(v) => v,
            Err(_) => continue, // Not an EoIP packet (standard GRE, etc.)
        };

        let key = DemuxKey {
            tunnel_id,
            peer_addr: src_ip,
        };

        // Demux lookup
        let handle = match registry.get(&key) {
            Some(h) => h,
            None => {
                log_demux_miss(&key);
                continue;
            }
        };

        let is_keepalive = payload_len == 0;
        let frame_data = &eoip_data[hdr_len..];

        // Update stats
        handle
            .stats
            .rx_packets
            .fetch_add(1, Ordering::Relaxed);
        handle
            .stats
            .rx_bytes
            .fetch_add(frame_data.len() as u64, Ordering::Relaxed);

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        handle.stats.last_rx_timestamp.store(now_ms, Ordering::Relaxed);

        // Keepalive packets: update stats but don't deliver to TAP
        if is_keepalive || frame_data.is_empty() {
            continue;
        }

        // Copy frame into a pool buffer and send to TAP writer
        if let Some(ref tx) = handle.rx_channel {
            let mut buf = pool.get();
            let payload = buf.payload_mut();
            let copy_len = frame_data.len().min(MAX_FRAME_SIZE);
            payload[..copy_len].copy_from_slice(&frame_data[..copy_len]);
            buf.set_len(copy_len);

            if tx.try_send(buf).is_err() {
                handle
                    .stats
                    .rx_errors
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    tracing::info!("RX v4 worker stopped");
}

/// EoIPv6 (IPv6, protocol 97) receive loop.
fn rx_loop_v6(
    raw_fd: std::os::fd::RawFd,
    registry: &TunnelRegistry,
    pool: &BufferPool,
    shutdown: &CancellationToken,
) {
    tracing::info!("RX v6 worker started");
    let mut recv_buf = vec![0u8; 65536];

    loop {
        if shutdown.is_cancelled() {
            break;
        }

        let n = match nix::unistd::read(raw_fd, &mut recv_buf) {
            Ok(n) => n,
            Err(nix::errno::Errno::EAGAIN) => {
                std::thread::sleep(std::time::Duration::from_micros(100));
                continue;
            }
            Err(e) => {
                if !shutdown.is_cancelled() {
                    tracing::error!(%e, "RX v6 read error");
                }
                break;
            }
        };

        // IPv6 raw sockets deliver the payload only (no IPv6 header),
        // but we need the source address from recvfrom. For now, use
        // recvmsg-style parsing. With raw sockets on Linux, the IPv6
        // header is NOT included in the data — we get just the EtherIP payload.
        // Source address comes from the sockaddr.
        //
        // Since we're using plain read() here, we'd need recvfrom for the
        // source address. For the initial implementation, we parse what we get.
        if n < 2 {
            continue;
        }

        let (tunnel_id, hdr_len) = match etherip::decode_eoipv6_header(&recv_buf[..n]) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // TODO: Get source IPv6 address from recvfrom/recvmsg
        // For now, use unspecified — will be fixed when we switch to recvmsg
        let key = DemuxKey {
            tunnel_id,
            peer_addr: IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        };

        let handle = match registry.get(&key) {
            Some(h) => h,
            None => {
                log_demux_miss(&key);
                continue;
            }
        };

        let frame_data = &recv_buf[hdr_len..n];
        let is_keepalive = frame_data.is_empty();

        handle.stats.rx_packets.fetch_add(1, Ordering::Relaxed);
        handle
            .stats
            .rx_bytes
            .fetch_add(frame_data.len() as u64, Ordering::Relaxed);

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        handle.stats.last_rx_timestamp.store(now_ms, Ordering::Relaxed);

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
