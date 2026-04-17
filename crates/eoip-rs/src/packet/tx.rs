//! TX packet path: TAP read → header encode → adaptive batch → sendmmsg.
//!
//! Per-tunnel TAP reader tasks (tokio async) read Ethernet frames from the
//! TAP device, prepend the EoIP/EtherIP header using buffer headroom,
//! and send to the TX batcher. The batcher aggregates packets and flushes
//! via raw socket.

use std::io;
use std::net::{IpAddr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use eoip_proto::gre::{self, EOIP_HEADER_LEN};
use eoip_proto::etherip::{self, ETHERIP_HEADER_LEN};

use crate::net::tap::TapDevice;
use crate::packet::buffer::{BufferPool, PacketBuf};
use crate::tunnel::handle::TunnelHandle;

/// A packet ready to be sent on the raw socket.
pub struct TxPacket {
    pub buf: PacketBuf,
    pub dest: SocketAddr,
}

/// Spawn a per-tunnel TAP reader task.
///
/// Reads Ethernet frames from the TAP device, prepends the appropriate
/// EoIP/EtherIP header, and sends to the TX batcher channel.
pub fn spawn_tap_reader(
    tap: Arc<TapDevice>,
    handle: Arc<TunnelHandle>,
    pool: Arc<BufferPool>,
    tx_sender: mpsc::Sender<TxPacket>,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let tunnel_id = handle.config.tunnel_id;
    let remote = handle.config.remote;
    let is_v6 = remote.is_ipv6();

    tokio::spawn(async move {
        tracing::debug!(tunnel_id, "TAP reader started");

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                result = read_and_encode(&tap, &handle, &pool, is_v6) => {
                    match result {
                        Ok(tx_pkt) => {
                            if tx_sender.send(tx_pkt).await.is_err() {
                                break; // Batcher shut down
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                        Err(e) => {
                            tracing::error!(tunnel_id, %e, "TAP read error");
                            break;
                        }
                    }
                }
            }
        }

        tracing::debug!(tunnel_id, "TAP reader stopped");
    })
}

async fn read_and_encode(
    tap: &TapDevice,
    handle: &TunnelHandle,
    pool: &BufferPool,
    is_v6: bool,
) -> io::Result<TxPacket> {
    let mut buf = pool.get();
    let n = tap.read(buf.payload_mut()).await?;
    buf.set_len(n);

    // Update TX stats
    handle.stats.tx_packets.fetch_add(1, Ordering::Relaxed);
    handle.stats.tx_bytes.fetch_add(n as u64, Ordering::Relaxed);

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    handle.stats.last_tx_timestamp.store(now_ms, Ordering::Relaxed);

    // Prepend protocol header into headroom
    if is_v6 {
        let hdr = buf.prepend_header(ETHERIP_HEADER_LEN);
        etherip::encode_eoipv6_header(handle.config.tunnel_id, hdr)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    } else {
        let hdr = buf.prepend_header(EOIP_HEADER_LEN);
        gre::encode_eoip_header(handle.config.tunnel_id, n as u16, hdr)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    }

    let dest = match handle.config.remote {
        IpAddr::V4(v4) => SocketAddr::V4(SocketAddrV4::new(v4, 0)),
        IpAddr::V6(v6) => SocketAddr::V6(SocketAddrV6::new(v6, 0, 0, 0)),
    };

    Ok(TxPacket { buf, dest })
}

/// Spawn a TX batcher task for a raw socket.
///
/// Receives `TxPacket`s from all TAP readers of one protocol family,
/// batches them adaptively, and flushes via the raw socket.
pub fn spawn_tx_batcher(
    raw_fd: std::os::fd::RawFd,
    mut rx: mpsc::Receiver<TxPacket>,
    config: &crate::config::PerformanceConfig,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let low_water = config.low_water_mark;
    let high_water = config.high_water_mark;
    let flush_timeout = Duration::from_micros(config.batch_timeout_us);

    tokio::spawn(async move {
        tracing::info!("TX batcher started");
        let mut batch: Vec<TxPacket> = Vec::with_capacity(high_water);

        loop {
            // Wait for at least one packet or shutdown
            let pkt = tokio::select! {
                _ = shutdown.cancelled() => break,
                pkt = rx.recv() => match pkt {
                    Some(p) => p,
                    None => break,
                },
            };

            batch.push(pkt);

            // If queue has more packets, try to fill the batch
            if batch.len() < low_water {
                // Immediate mode: send now for low latency
                flush_batch(raw_fd, &mut batch);
                continue;
            }

            // Batching mode: accumulate up to high_water or timeout
            let deadline = tokio::time::Instant::now() + flush_timeout;
            loop {
                if batch.len() >= high_water {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => break,
                    pkt = rx.recv() => match pkt {
                        Some(p) => batch.push(p),
                        None => break,
                    },
                }
            }

            flush_batch(raw_fd, &mut batch);
        }

        // Flush remaining
        if !batch.is_empty() {
            flush_batch(raw_fd, &mut batch);
        }

        tracing::info!("TX batcher stopped");
    })
}

fn flush_batch(raw_fd: std::os::fd::RawFd, batch: &mut Vec<TxPacket>) {
    for pkt in batch.drain(..) {
        let data = pkt.buf.as_slice();
        let dest = nix::sys::socket::SockaddrStorage::from(pkt.dest);

        if let Err(e) = nix::sys::socket::sendto(raw_fd, data, &dest, nix::sys::socket::MsgFlags::empty()) {
            match e {
                nix::errno::Errno::EAGAIN | nix::errno::Errno::ENOBUFS => {
                    // Drop packet under backpressure — correct behavior
                }
                _ => {
                    tracing::error!(%e, "TX sendto failed");
                }
            }
        }
    }
}

/// Send a keepalive (zero-payload) packet for a tunnel.
pub async fn send_keepalive(
    raw_fd: std::os::fd::RawFd,
    handle: &TunnelHandle,
) -> io::Result<()> {
    let is_v6 = handle.config.remote.is_ipv6();
    let mut hdr_buf = [0u8; EOIP_HEADER_LEN]; // 8 bytes is enough for both

    if is_v6 {
        etherip::encode_eoipv6_header(handle.config.tunnel_id, &mut hdr_buf[..ETHERIP_HEADER_LEN])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        send_raw(raw_fd, &hdr_buf[..ETHERIP_HEADER_LEN], handle.config.remote)?;
    } else {
        gre::encode_eoip_header(handle.config.tunnel_id, 0, &mut hdr_buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        send_raw(raw_fd, &hdr_buf, handle.config.remote)?;
    }

    Ok(())
}

fn send_raw(raw_fd: std::os::fd::RawFd, data: &[u8], dest_ip: IpAddr) -> io::Result<()> {
    let sock_addr: SocketAddr = match dest_ip {
        IpAddr::V4(v4) => SocketAddr::V4(SocketAddrV4::new(v4, 0)),
        IpAddr::V6(v6) => SocketAddr::V6(SocketAddrV6::new(v6, 0, 0, 0)),
    };
    let dest = nix::sys::socket::SockaddrStorage::from(sock_addr);

    nix::sys::socket::sendto(raw_fd, data, &dest, nix::sys::socket::MsgFlags::empty())
        .map_err(io::Error::from)?;
    Ok(())
}
