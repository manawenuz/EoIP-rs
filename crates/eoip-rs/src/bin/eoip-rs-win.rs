//! Windows EoIP-rs daemon entry point.
//!
//! Self-contained — no helper binary needed. Runs as Administrator.
//! Uses tap-windows6 for the tunnel interface and WinDivert for
//! GRE packet capture/injection (bypassing Windows raw socket limitations).

#![cfg(target_os = "windows")]

use std::borrow::Cow;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;
use windivert::prelude::*;

use eoip_proto::gre::{self, EOIP_HEADER_LEN};
use eoip_proto::DemuxKey;

use eoip_rs::config::parse_config;
use eoip_rs::net::tap_windows::{self, WinTapDevice};
use eoip_rs::tunnel::handle::TunnelHandle;
use eoip_rs::tunnel::lifecycle::TunnelState;
use eoip_rs::tunnel::registry::TunnelRegistry;

const MAX_FRAME_SIZE: usize = 1522;

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
    let tunnel_cfg = &config.tunnels[0];

    // Open TAP adapter
    let tap_guid = tap_windows::find_tap_guid(None)?;
    tracing::info!(guid = %tap_guid, "found TAP adapter");
    let tap = Arc::new(WinTapDevice::open(&tap_guid)?);
    tracing::info!("TAP device opened and connected");

    // Register tunnel
    let handle = Arc::new(TunnelHandle::new(tunnel_cfg.clone()));
    let key = DemuxKey {
        tunnel_id: tunnel_cfg.tunnel_id,
        peer_addr: tunnel_cfg.remote,
    };
    registry.insert(key, Arc::clone(&handle));
    let _ = handle.state.transition(TunnelState::Initializing, TunnelState::Configured);
    let _ = handle.state.transition(TunnelState::Configured, TunnelState::Active);
    tracing::info!(tunnel_id = tunnel_cfg.tunnel_id, "tunnel active");

    // WinDivert filter: capture inbound GRE from our peer
    let remote_ip = match tunnel_cfg.remote {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(_) => return Err("IPv6 not yet supported on Windows".into()),
    };
    let filter = format!("ip.Protocol == 47 and ip.SrcAddr == {}", remote_ip);
    tracing::info!(%filter, "opening WinDivert for GRE capture");

    let divert = WinDivert::network(&filter, 0, WinDivertFlags::new())?;
    tracing::info!("WinDivert RX handle opened");

    // TX: separate send-only handle
    let tx_divert = WinDivert::network("false", 0, WinDivertFlags::new().set_send_only())?;
    tracing::info!("WinDivert TX handle opened");

    // Spawn TX thread: TAP read → EoIP encode → WinDivert inject
    let tx_tap = Arc::clone(&tap);
    let tx_remote = tunnel_cfg.remote;
    let tx_local = tunnel_cfg.local;
    let tx_tid = tunnel_cfg.tunnel_id;
    let tx_stats = Arc::clone(&handle.stats);

    let _tx_thread = std::thread::spawn(move || {
        let mut pkt = vec![0u8; 20 + EOIP_HEADER_LEN + MAX_FRAME_SIZE];
        loop {
            match tx_tap.read_blocking(&mut pkt[20 + EOIP_HEADER_LEN..]) {
                Ok(n) if n > 0 => {
                    let total_len = (20 + EOIP_HEADER_LEN + n) as u16;
                    build_ip_header(&mut pkt[..20], total_len, 47, &tx_local, &tx_remote);

                    if gre::encode_eoip_header(tx_tid, n as u16, &mut pkt[20..28]).is_ok() {
                        let packet = WinDivertPacket {
                            address: unsafe { WinDivertAddress::<NetworkLayer>::new() },
                            data: Cow::Borrowed(&pkt[..20 + EOIP_HEADER_LEN + n]),
                        };
                        match tx_divert.send(&packet) {
                            Ok(_) => {
                                tx_stats.tx_packets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                tx_stats.tx_bytes.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
                            }
                            Err(e) => tracing::warn!(%e, "WinDivert TX failed"),
                        }
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
    let ka_local = tunnel_cfg.local;
    let ka_remote = tunnel_cfg.remote;
    let ka_tid = tunnel_cfg.tunnel_id;
    let ka_interval = std::time::Duration::from_secs(tunnel_cfg.keepalive_interval_secs);
    let ka_divert = WinDivert::network("false", 0, WinDivertFlags::new().set_send_only())?;

    let _ka_thread = std::thread::spawn(move || {
        let mut pkt = [0u8; 20 + EOIP_HEADER_LEN];
        loop {
            build_ip_header(&mut pkt[..20], 28, 47, &ka_local, &ka_remote);
            if gre::encode_eoip_header(ka_tid, 0, &mut pkt[20..28]).is_ok() {
                let packet = WinDivertPacket {
                    address: unsafe { WinDivertAddress::<NetworkLayer>::new() },
                    data: Cow::Borrowed(&pkt[..]),
                };
                let _ = ka_divert.send(&packet);
            }
            std::thread::sleep(ka_interval);
        }
    });

    // Main thread: RX — WinDivert recv GRE → decode → TAP write
    tracing::info!("entering RX loop");
    let mut buf = vec![0u8; 65536];
    loop {
        match divert.recv(&mut buf) {
            Ok(packet) => {
                let data = &*packet.data;
                if data.len() < 20 {
                    continue;
                }

                let ip_hdr_len = ((data[0] & 0x0F) as usize) * 4;
                if data.len() < ip_hdr_len + EOIP_HEADER_LEN {
                    continue;
                }

                let gre_payload = &data[ip_hdr_len..];
                match gre::decode_eoip_header(gre_payload) {
                    Ok((tid, payload_len, hdr_len)) => {
                        if tid != tunnel_cfg.tunnel_id {
                            // Not our tunnel — reinject
                            let _ = divert.send(&packet);
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
                            let end = hdr_len + payload_len as usize;
                            if gre_payload.len() >= end {
                                let _ = tap.write_blocking(&gre_payload[hdr_len..end]);
                            }
                        }
                    }
                    Err(_) => {
                        let _ = divert.send(&packet);
                    }
                }
            }
            Err(e) => {
                tracing::error!(%e, "WinDivert recv error");
                break;
            }
        }
    }

    Ok(())
}

fn build_ip_header(buf: &mut [u8], total_len: u16, protocol: u8, src: &IpAddr, dst: &IpAddr) {
    let src_octets = match src { IpAddr::V4(v4) => v4.octets(), _ => [0; 4] };
    let dst_octets = match dst { IpAddr::V4(v4) => v4.octets(), _ => [0; 4] };

    buf[0] = 0x45; // IPv4, IHL=5
    buf[1] = 0x00; // DSCP=0
    buf[2..4].copy_from_slice(&total_len.to_be_bytes());
    buf[4..6].copy_from_slice(&[0x00, 0x00]); // ID
    buf[6..8].copy_from_slice(&[0x00, 0x00]); // Flags (DF=0)
    buf[8] = 255;  // TTL=255
    buf[9] = protocol;
    buf[10..12].copy_from_slice(&[0x00, 0x00]); // Checksum (WinDivert recalculates)
    buf[12..16].copy_from_slice(&src_octets);
    buf[16..20].copy_from_slice(&dst_octets);
}
