//! Windows EoIP-rs daemon entry point.
//!
//! Self-contained — no helper binary needed. Runs as Administrator,
//! creates TAP device and raw sockets directly.

#![cfg(target_os = "windows")]

use std::net::{IpAddr, SocketAddr, SocketAddrV4, UdpSocket};
use std::os::windows::io::{FromRawSocket, IntoRawSocket};
use std::sync::Arc;
use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use eoip_proto::gre::{self, EOIP_HEADER_LEN};
use eoip_proto::DemuxKey;

use eoip_rs::config::parse_config;
use eoip_rs::net::tap_windows::{self, WinTapDevice};
/// Maximum Ethernet frame size (same as packet::buffer::MAX_FRAME_SIZE).
const MAX_FRAME_SIZE: usize = 1522;
use eoip_rs::tunnel::handle::TunnelHandle;
use eoip_rs::tunnel::lifecycle::TunnelState;
use eoip_rs::tunnel::registry::TunnelRegistry;

#[derive(Parser, Debug)]
#[command(name = "eoip-rs", version, about = "EoIP-rs Windows daemon")]
struct Args {
    #[arg(short, long, default_value = "C:\\eoip-rs\\config.toml")]
    config: PathBuf,
}

fn main() {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!(config = %args.config.display(), "eoip-rs-win starting");

    if let Err(e) = run(args) {
        tracing::error!(%e, "eoip-rs-win exiting with error");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_config(&args.config)?;
    tracing::info!(tunnels = config.tunnels.len(), "configuration loaded");

    if config.tunnels.is_empty() {
        return Err("no tunnels configured".into());
    }

    let registry = Arc::new(TunnelRegistry::new());

    // Create raw GRE socket (Winsock SOCK_RAW, proto 47)
    let raw_socket = create_raw_gre_socket(&config.tunnels[0].local)?;
    tracing::info!("raw GRE socket created");

    // Open TAP adapter
    let tap_guid = tap_windows::find_tap_guid(None)?;
    tracing::info!(guid = %tap_guid, "found TAP adapter");

    let tap = Arc::new(WinTapDevice::open(&tap_guid)?);
    tracing::info!("TAP device opened and connected");

    // Register tunnel
    let tunnel_cfg = &config.tunnels[0];
    let handle = Arc::new(TunnelHandle::new(tunnel_cfg.clone()));
    let key = DemuxKey {
        tunnel_id: tunnel_cfg.tunnel_id,
        peer_addr: tunnel_cfg.remote,
    };
    registry.insert(key, Arc::clone(&handle));
    let _ = handle.state.transition(TunnelState::Initializing, TunnelState::Configured);
    let _ = handle.state.transition(TunnelState::Configured, TunnelState::Active);
    tracing::info!(tunnel_id = tunnel_cfg.tunnel_id, "tunnel active");

    // Spawn TX thread: TAP read → EoIP encode → raw socket send
    let tx_tap = Arc::clone(&tap);
    let tx_remote = tunnel_cfg.remote;
    let tx_tid = tunnel_cfg.tunnel_id;
    let tx_socket = raw_socket.try_clone()?;
    let tx_stats = Arc::clone(&handle.stats);

    let tx_thread = std::thread::spawn(move || {
        let mut buf = vec![0u8; EOIP_HEADER_LEN + MAX_FRAME_SIZE];
        loop {
            match tx_tap.read_blocking(&mut buf[EOIP_HEADER_LEN..]) {
                Ok(n) if n > 0 => {
                    // Prepend EoIP header
                    if gre::encode_eoip_header(tx_tid, n as u16, &mut buf[..EOIP_HEADER_LEN]).is_ok() {
                        let dest = match tx_remote {
                            IpAddr::V4(v4) => SocketAddr::V4(SocketAddrV4::new(v4, 0)),
                            _ => continue,
                        };
                        let _ = tx_socket.send_to(&buf[..EOIP_HEADER_LEN + n], dest);
                        tx_stats.tx_packets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        tx_stats.tx_bytes.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::error!(%e, "TAP read error");
                    break;
                }
            }
        }
    });

    // Spawn keepalive thread
    let ka_socket = raw_socket.try_clone()?;
    let ka_remote = tunnel_cfg.remote;
    let ka_tid = tunnel_cfg.tunnel_id;
    let ka_interval = std::time::Duration::from_secs(tunnel_cfg.keepalive_interval_secs);

    let ka_thread = std::thread::spawn(move || {
        let mut hdr = [0u8; EOIP_HEADER_LEN];
        loop {
            if gre::encode_eoip_header(ka_tid, 0, &mut hdr).is_ok() {
                let dest = match ka_remote {
                    IpAddr::V4(v4) => SocketAddr::V4(SocketAddrV4::new(v4, 0)),
                    _ => break,
                };
                let _ = ka_socket.send_to(&hdr, dest);
            }
            std::thread::sleep(ka_interval);
        }
    });

    // Main thread: RX — raw socket recv → EoIP decode → TAP write
    tracing::info!("entering RX loop");
    let mut rx_buf = vec![0u8; 65536];
    loop {
        match raw_socket.recv_from(&mut rx_buf) {
            Ok((n, src_addr)) => {
                if n < 20 {
                    continue; // Too short for IP header
                }

                // Parse IP header to get to GRE payload
                let ip_hdr_len = ((rx_buf[0] & 0x0F) as usize) * 4;
                if n < ip_hdr_len + EOIP_HEADER_LEN {
                    continue;
                }
                let proto = rx_buf[9];
                if proto != 47 {
                    continue; // Not GRE
                }

                let gre_payload = &rx_buf[ip_hdr_len..n];
                match gre::decode_eoip_header(gre_payload) {
                    Ok((tid, payload_len, hdr_len)) => {
                        if tid != tunnel_cfg.tunnel_id {
                            continue;
                        }

                        handle.stats.rx_packets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        handle.stats.rx_bytes.fetch_add(payload_len as u64, std::sync::atomic::Ordering::Relaxed);

                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);
                        handle.stats.last_rx_timestamp.store(now_ms, std::sync::atomic::Ordering::Relaxed);

                        if payload_len > 0 {
                            let frame = &gre_payload[hdr_len..hdr_len + payload_len as usize];
                            let _ = tap.write_blocking(frame);
                        }
                    }
                    Err(_) => continue,
                }
            }
            Err(e) => {
                tracing::error!(%e, "raw socket recv error");
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

/// Create a raw socket bound to a local IP for GRE (protocol 47).
fn create_raw_gre_socket(local: &IpAddr) -> Result<UdpSocket, Box<dyn std::error::Error>> {
    use std::os::windows::io::FromRawSocket;

    // Windows raw sockets: socket(AF_INET, SOCK_RAW, IPPROTO_GRE)
    // We use the socket2 crate for this
    let domain = socket2::Domain::IPV4;
    let sock = socket2::Socket::new(domain, socket2::Type::RAW, Some(socket2::Protocol::from(47)))?;

    // Bind to local address
    let bind_addr: SocketAddr = match local {
        IpAddr::V4(v4) => SocketAddr::V4(SocketAddrV4::new(*v4, 0)),
        _ => return Err("IPv6 not supported on Windows yet".into()),
    };
    sock.bind(&bind_addr.into())?;

    // Convert to std UdpSocket for simple send_to/recv_from
    // (raw sockets work with the same API on Windows)
    let raw_fd = sock.into_raw_socket();
    Ok(unsafe { UdpSocket::from_raw_socket(raw_fd) })
}
