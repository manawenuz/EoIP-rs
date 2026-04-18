//! EoIP-rs daemon entry point.

use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

use eoip_helper::fdpass;
use eoip_proto::wire::{DaemonMsg, HelperMsg};

use eoip_rs::config::parse_config;
use eoip_rs::keepalive;
use eoip_rs::net::tap::TapDevice;
use eoip_rs::packet::buffer::BufferPool;
use eoip_rs::packet::rx;
use eoip_rs::packet::tx::{self, TxPacket};
use eoip_rs::shutdown::ShutdownCoordinator;
use eoip_rs::tunnel::lifecycle::TunnelState;
use eoip_rs::tunnel::manager::TunnelManager;
use eoip_rs::tunnel::registry::TunnelRegistry;
use eoip_rs::DaemonError;

#[derive(Parser, Debug)]
#[command(name = "eoip-rs", version, about = "EoIP/EoIPv6 userspace daemon")]
struct Args {
    /// Path to TOML configuration file
    #[arg(short, long, default_value = "/etc/eoip-rs/config.toml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!(config = %args.config.display(), "eoip-rs starting");

    if let Err(e) = run(args).await {
        tracing::error!(%e, "eoip-rs exiting with error");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), DaemonError> {
    let config = parse_config(&args.config)?;
    tracing::info!(
        tunnels = config.tunnels.len(),
        helper_mode = %config.daemon.helper_mode,
        "configuration loaded"
    );

    let registry = Arc::new(TunnelRegistry::new());
    let shutdown = ShutdownCoordinator::new();
    shutdown.spawn_signal_handler();

    // Connect to helper
    let helper_socket = &config.daemon.helper_socket;
    tracing::info!(path = %helper_socket.display(), "connecting to helper");
    let helper_stream = UnixStream::connect(helper_socket).map_err(|e| {
        DaemonError::Config(format!("cannot connect to helper at {}: {e}", helper_socket.display()))
    })?;

    // Wait for HelperReady
    let helper_fd = helper_stream.as_fd();
    let (ready_msg, _) = fdpass::recv_msg_with_fd(helper_fd)
        .map_err(|e| DaemonError::Config(format!("helper handshake failed: {e}")))?;
    match ready_msg {
        HelperMsg::HelperReady => tracing::info!("helper is ready"),
        other => return Err(DaemonError::Config(format!("unexpected helper msg: {other:?}"))),
    }

    // Bootstrap: create first tunnel from config to get raw socket fds
    let mut raw_v4_fd: Option<OwnedFd> = None;
    let mut raw_v6_fd: Option<OwnedFd> = None;
    // Pool size: channel depth + recvmmsg batch headroom per RX worker + margin
    let pool_size = config.performance.channel_buffer * config.tunnels.len().max(1)
        + 32 * 4  // RECV_BATCH * num_rx_workers
        + 256;    // margin
    let pool = Arc::new(BufferPool::new(pool_size));
    let (tx_sender, tx_receiver) = mpsc::channel::<TxPacket>(config.performance.channel_buffer);

    // Create initial tunnels from config (gets raw sockets from first tunnel)
    let mut startup_tunnels: Vec<(u16, Arc<TapDevice>)> = Vec::new();

    for tunnel_cfg in &config.tunnels {
        if !tunnel_cfg.enabled {
            continue;
        }

        let handle = Arc::new(eoip_rs::tunnel::handle::TunnelHandle::with_channel_cap(
            tunnel_cfg.clone(),
            config.performance.channel_buffer,
        ));
        let key = eoip_proto::DemuxKey {
            tunnel_id: tunnel_cfg.tunnel_id,
            peer_addr: tunnel_cfg.remote,
        };
        registry.insert(key, Arc::clone(&handle));

        let iface_name = tunnel_cfg.effective_iface_name();
        let tunnel_id = tunnel_cfg.tunnel_id;

        // Request TAP from helper
        let create_msg = DaemonMsg::CreateTunnel {
            iface_name: iface_name.clone(),
            tunnel_id,
        };
        let payload = eoip_proto::wire::serialize_msg(&create_msg)?;
        let iov = [std::io::IoSlice::new(&payload)];
        nix::sys::socket::sendmsg::<()>(
            helper_stream.as_raw_fd(), &iov, &[], nix::sys::socket::MsgFlags::empty(), None,
        ).map_err(|e| DaemonError::Config(format!("send CreateTunnel: {e}")))?;

        // Receive TapCreated + fd
        let (msg, fd) = fdpass::recv_msg_with_fd(helper_fd)
            .map_err(|e| DaemonError::Config(format!("recv TapCreated: {e}")))?;
        match msg {
            HelperMsg::TapCreated { .. } => {}
            HelperMsg::Error { msg } => return Err(DaemonError::Config(format!("helper: {msg}"))),
            other => return Err(DaemonError::Config(format!("unexpected: {other:?}"))),
        }

        let tap_fd = fd.map(|raw| unsafe { OwnedFd::from_raw_fd(raw) })
            .ok_or_else(|| DaemonError::Config("no TAP fd".into()))?;

        unsafe {
            let flags = libc::fcntl(tap_fd.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(tap_fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
        }

        let tap = Arc::new(TapDevice::new(tap_fd)?);

        // Receive raw socket fds (only on first tunnel)
        if raw_v4_fd.is_none() {
            let (raw_msg, raw_fd) = fdpass::recv_msg_with_fd(helper_fd)
                .map_err(|e| DaemonError::Config(format!("recv raw v4: {e}")))?;
            match raw_msg {
                HelperMsg::RawSocket { address_family: 2 } => tracing::info!("raw IPv4 socket ready"),
                _ => tracing::warn!("unexpected raw socket msg: {raw_msg:?}"),
            }
            raw_v4_fd = raw_fd.map(|raw| unsafe { OwnedFd::from_raw_fd(raw) });

            // Try IPv6 (may fail, that's ok)
            if let Ok((msg, fd)) = fdpass::recv_msg_with_fd(helper_fd) {
                match msg {
                    HelperMsg::RawSocket { address_family: 10 } => {
                        tracing::info!("raw IPv6 socket ready");
                        raw_v6_fd = fd.map(|raw| unsafe { OwnedFd::from_raw_fd(raw) });
                    }
                    HelperMsg::Error { .. } => {
                        tracing::warn!("IPv6 raw socket not available (non-critical)");
                    }
                    _ => {}
                }
            }
        }

        let _ = handle.state.transition(TunnelState::Initializing, TunnelState::Configured);
        let _ = handle.state.transition(TunnelState::Configured, TunnelState::Active);
        tracing::info!(tunnel_id, iface = %iface_name, "tunnel active");

        startup_tunnels.push((tunnel_id, tap));
    }

    // Create AF_PACKET socket for PACKET_MMAP zero-copy RX (directly in daemon)
    #[cfg(target_os = "linux")]
    let af_packet_fd: Option<OwnedFd> = match eoip_helper::rawsock::create_af_packet_socket_v4() {
        Ok(fd) => {
            tracing::info!("AF_PACKET socket created for zero-copy RX");
            Some(fd)
        }
        Err(e) => {
            tracing::info!(%e, "AF_PACKET not available, using recvmmsg");
            None
        }
    };
    #[cfg(not(target_os = "linux"))]
    let af_packet_fd: Option<OwnedFd> = None;

    // Start RX pipeline
    let _rx_handle = rx::start_rx_pipeline(
        raw_v4_fd.as_ref().map(|fd| fd.as_fd()),
        raw_v6_fd.as_ref().map(|fd| fd.as_fd()),
        Arc::clone(&registry),
        Arc::clone(&pool),
        shutdown.token().clone(),
        config.performance.rx_workers,
    );
    tracing::info!("RX pipeline started");

    let raw_v4_raw = raw_v4_fd.as_ref().map(|fd| fd.as_raw_fd()).unwrap_or(-1);
    let raw_v6_raw = raw_v6_fd.as_ref().map(|fd| fd.as_raw_fd()).unwrap_or(-1);

    // Create TunnelManager (holds helper socket for dynamic creation)
    let manager = Arc::new(TunnelManager::new(
        helper_stream,
        Arc::clone(&registry),
        Arc::clone(&pool),
        tx_sender.clone(),
        raw_v4_raw,
        raw_v6_raw,
        shutdown.token().clone(),
    ));

    // Spawn per-tunnel tasks for startup tunnels
    for (tunnel_id, tap) in &startup_tunnels {
        let tunnel_cancel = shutdown.token().child_token();
        let entries = registry.find_by_tunnel_id(*tunnel_id);

        if let Some((_key, handle)) = entries.first() {
            // TAP reader (TAP → raw socket)
            tx::spawn_tap_reader(
                Arc::clone(tap), Arc::clone(handle), Arc::clone(&pool),
                tx_sender.clone(), tunnel_cancel.clone(),
            );

            // TAP writer — dedicated OS thread, batch-drains channel to reduce contention
            if let Some(ref rx_recv) = handle.rx_receiver {
                let tap_fd = tap.as_fd().as_raw_fd();
                let rx = rx_recv.clone();
                let tid = *tunnel_id;
                std::thread::Builder::new()
                    .name(format!("tap-wr-{tid}"))
                    .spawn(move || {
                        const MAX_BATCH: usize = 32;
                        let mut bufs = Vec::with_capacity(MAX_BATCH);

                        while let Ok(buf) = rx.recv() {
                            bufs.push(buf);
                            while bufs.len() < MAX_BATCH {
                                match rx.try_recv() {
                                    Ok(b) => bufs.push(b),
                                    Err(_) => break,
                                }
                            }
                            for b in bufs.drain(..) {
                                let data = b.as_slice();
                                unsafe { libc::write(tap_fd, data.as_ptr() as *const _, data.len()) };
                            }
                        }
                    })
                    .expect("failed to spawn TAP writer thread");
            }

            // Keepalive
            let raw_fd = if handle.config.remote.is_ipv6() { raw_v6_raw } else { raw_v4_raw };
            keepalive::spawn_keepalive_task(Arc::clone(handle), raw_fd, tunnel_cancel.clone());

            manager.register_startup_tunnel(*tunnel_id, tunnel_cancel, Arc::clone(tap));
        }

        tracing::info!(tunnel_id, "tunnel tasks started");
    }

    // TX batcher (shared by all tunnels, routes packets to correct raw socket)
    tx::spawn_tx_batcher(raw_v4_raw, raw_v6_raw, tx_receiver, &config.performance, shutdown.token().clone());
    tracing::info!("TX batcher started");

    // Start gRPC API (with TunnelManager for dynamic creation)
    let api_handle = {
        let registry = Arc::clone(&registry);
        let manager = Arc::clone(&manager);
        let api_config = config.api.clone();
        let api_shutdown = shutdown.token().clone();
        tokio::spawn(async move {
            if let Err(e) = eoip_rs::api::start_grpc_server(registry, manager, &api_config, api_shutdown).await {
                tracing::error!(%e, "gRPC server error");
            }
        })
    };
    tracing::info!(listen = %config.api.listen, "gRPC API started");

    // Wait for shutdown
    shutdown.token().cancelled().await;
    tracing::info!("shutting down gracefully");
    api_handle.abort();
    Ok(())
}
