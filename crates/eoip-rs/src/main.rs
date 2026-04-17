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
use eoip_rs::packet::tx;
use eoip_rs::shutdown::ShutdownCoordinator;
use eoip_rs::tunnel::lifecycle::TunnelState;
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

    // Pre-populate registry
    for tunnel_cfg in &config.tunnels {
        if !tunnel_cfg.enabled {
            tracing::info!(tunnel_id = tunnel_cfg.tunnel_id, "tunnel disabled, skipping");
            continue;
        }

        let handle = Arc::new(eoip_rs::tunnel::handle::TunnelHandle::new(tunnel_cfg.clone()));
        let key = eoip_proto::DemuxKey {
            tunnel_id: tunnel_cfg.tunnel_id,
            peer_addr: tunnel_cfg.remote,
        };

        registry.insert(key, handle);
        tracing::info!(
            tunnel_id = tunnel_cfg.tunnel_id,
            iface = %tunnel_cfg.effective_iface_name(),
            local = %tunnel_cfg.local,
            remote = %tunnel_cfg.remote,
            "registered tunnel (Initializing)"
        );
    }

    // Connect to helper
    let helper_socket = &config.daemon.helper_socket;
    tracing::info!(path = %helper_socket.display(), "connecting to helper");
    let helper_stream = UnixStream::connect(helper_socket).map_err(|e| {
        DaemonError::Config(format!("cannot connect to helper at {}: {e}", helper_socket.display()))
    })?;
    let helper_fd = helper_stream.as_fd();

    // Wait for HelperReady
    let (ready_msg, _) = fdpass::recv_msg_with_fd(helper_fd)
        .map_err(|e| DaemonError::Config(format!("helper handshake failed: {e}")))?;
    match ready_msg {
        HelperMsg::HelperReady => tracing::info!("helper is ready"),
        other => return Err(DaemonError::Config(format!("unexpected helper msg: {other:?}"))),
    }

    // Request tunnel creation from helper for each configured tunnel
    let mut raw_v4_fd: Option<OwnedFd> = None;
    let mut raw_v6_fd: Option<OwnedFd> = None;
    let mut tap_devices: Vec<(u16, Arc<TapDevice>)> = Vec::new();

    for tunnel_cfg in &config.tunnels {
        if !tunnel_cfg.enabled {
            continue;
        }

        let iface_name = tunnel_cfg.effective_iface_name();
        let tunnel_id = tunnel_cfg.tunnel_id;

        // Send CreateTunnel request
        let create_msg = DaemonMsg::CreateTunnel {
            iface_name: iface_name.clone(),
            tunnel_id,
        };
        let payload = eoip_proto::wire::serialize_msg(&create_msg)
            .map_err(|e| DaemonError::Config(format!("serialize error: {e}")))?;
        let iov = [std::io::IoSlice::new(&payload)];
        nix::sys::socket::sendmsg::<()>(
            helper_stream.as_raw_fd(),
            &iov,
            &[],
            nix::sys::socket::MsgFlags::empty(),
            None,
        )
        .map_err(|e| DaemonError::Config(format!("send CreateTunnel failed: {e}")))?;

        tracing::info!(tunnel_id, iface = %iface_name, "requested tunnel creation from helper");

        // Receive TapCreated + fd
        let (tap_msg, tap_fd) = fdpass::recv_msg_with_fd(helper_fd)
            .map_err(|e| DaemonError::Config(format!("recv TapCreated failed: {e}")))?;
        match tap_msg {
            HelperMsg::TapCreated { iface_name: ref name, tunnel_id: tid } => {
                tracing::info!(iface = %name, tunnel_id = tid, "TAP interface created");
            }
            HelperMsg::Error { msg } => {
                return Err(DaemonError::Config(format!("helper error creating tunnel: {msg}")));
            }
            other => {
                return Err(DaemonError::Config(format!("unexpected msg: {other:?}")));
            }
        }

        let tap_owned_fd = tap_fd
            .map(|raw| unsafe { OwnedFd::from_raw_fd(raw) })
            .ok_or_else(|| DaemonError::Config("no TAP fd received".into()))?;

        // Set TAP fd non-blocking for async
        unsafe {
            let flags = libc::fcntl(tap_owned_fd.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(tap_owned_fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
        }

        let tap = Arc::new(TapDevice::new(tap_owned_fd).map_err(|e| {
            DaemonError::Config(format!("TapDevice::new failed: {e}"))
        })?);

        // Transition tunnel to Active
        let entries = registry.find_by_tunnel_id(tunnel_id);
        if let Some((_key, handle)) = entries.first() {
            let _ = handle.state.transition(TunnelState::Initializing, TunnelState::Configured);
            let _ = handle.state.transition(TunnelState::Configured, TunnelState::Active);
            tracing::info!(tunnel_id, "tunnel active");
        }

        // Store TAP for later use in TX/RX tasks
        tap_devices.push((tunnel_id, tap));

        // Receive raw socket fds (v4 and v6, only on first tunnel)
        if raw_v4_fd.is_none() {
            let (raw_msg, raw_fd) = fdpass::recv_msg_with_fd(helper_fd)
                .map_err(|e| DaemonError::Config(format!("recv RawSocket v4 failed: {e}")))?;
            match raw_msg {
                HelperMsg::RawSocket { address_family: 2 } => {
                    tracing::info!("received raw IPv4 socket");
                }
                other => tracing::warn!("unexpected raw socket msg: {other:?}"),
            }
            raw_v4_fd = raw_fd.map(|raw| unsafe { OwnedFd::from_raw_fd(raw) });
        }

        if raw_v6_fd.is_none() {
            let (raw_msg, raw_fd) = fdpass::recv_msg_with_fd(helper_fd)
                .map_err(|e| DaemonError::Config(format!("recv RawSocket v6 failed: {e}")))?;
            match raw_msg {
                HelperMsg::RawSocket { address_family: 10 } => {
                    tracing::info!("received raw IPv6 socket");
                }
                other => tracing::warn!("unexpected raw socket msg: {other:?}"),
            }
            raw_v6_fd = raw_fd.map(|raw| unsafe { OwnedFd::from_raw_fd(raw) });
        }
    }

    // Start packet processing
    let pool = Arc::new(BufferPool::new(config.performance.channel_buffer));

    // Start RX pipeline (raw socket → demux → TAP write)
    let _rx_handle = rx::start_rx_pipeline(
        raw_v4_fd.as_ref().map(|fd| fd.as_fd()),
        raw_v6_fd.as_ref().map(|fd| fd.as_fd()),
        Arc::clone(&registry),
        Arc::clone(&pool),
        shutdown.token().clone(),
    );
    tracing::info!("RX pipeline started");

    // Start TX pipeline (TAP read → encode → raw socket send) and keepalives
    let raw_v4_raw_fd = raw_v4_fd.as_ref().map(|fd| fd.as_raw_fd()).unwrap_or(-1);
    let (tx_sender, tx_receiver) = mpsc::channel(config.performance.channel_buffer);

    // Spawn per-tunnel TAP readers, TAP writers, and keepalive tasks
    for (tunnel_id, tap) in &tap_devices {
        let entries = registry.find_by_tunnel_id(*tunnel_id);
        if let Some((_key, handle)) = entries.first() {
            // TX: TAP read → encode → raw socket
            tx::spawn_tap_reader(
                Arc::clone(tap),
                Arc::clone(handle),
                Arc::clone(&pool),
                tx_sender.clone(),
                shutdown.token().clone(),
            );
            tracing::info!(tunnel_id, "TAP reader started");

            // RX: raw socket → demux → TAP write
            // Take the receiver from the handle (it's an Option, take it once)
            // Since handle is Arc and rx_receiver is not behind a mutex,
            // we need to use a different approach — spawn the writer with a clone of the receiver
            if let Some(ref rx_recv) = handle.rx_receiver {
                let tap_clone = Arc::clone(tap);
                let rx = rx_recv.clone();
                let cancel = shutdown.token().clone();
                let tid = *tunnel_id;
                tokio::spawn(async move {
                    tracing::debug!(tunnel_id = tid, "TAP writer started");
                    loop {
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            result = tokio::task::spawn_blocking({
                                let rx = rx.clone();
                                move || rx.recv()
                            }) => {
                                match result {
                                    Ok(Ok(buf)) => {
                                        if let Err(e) = tap_clone.write(buf.as_slice()).await {
                                            if e.kind() != std::io::ErrorKind::WouldBlock {
                                                tracing::error!(tunnel_id = tid, %e, "TAP write error");
                                            }
                                        }
                                    }
                                    _ => break,
                                }
                            }
                        }
                    }
                    tracing::debug!(tunnel_id = tid, "TAP writer stopped");
                });
                tracing::info!(tunnel_id, "TAP writer started");
            }

            // Keepalive
            let raw_fd = if handle.config.remote.is_ipv6() {
                raw_v6_fd.as_ref().map(|fd| fd.as_raw_fd()).unwrap_or(-1)
            } else {
                raw_v4_raw_fd
            };
            keepalive::spawn_keepalive_task(
                Arc::clone(handle),
                raw_fd,
                shutdown.token().clone(),
            );
            tracing::info!(tunnel_id, "keepalive task started");
        }
    }

    // TX batcher
    tx::spawn_tx_batcher(raw_v4_raw_fd, tx_receiver, &config.performance, shutdown.token().clone());
    tracing::info!("TX batcher started");

    // Start gRPC API
    let api_handle = {
        let registry = Arc::clone(&registry);
        let api_config = config.api.clone();
        let api_shutdown = shutdown.token().clone();
        tokio::spawn(async move {
            if let Err(e) = eoip_rs::api::start_grpc_server(registry, &api_config, api_shutdown).await {
                tracing::error!(%e, "gRPC server error");
            }
        })
    };
    tracing::info!(listen = %config.api.listen, "gRPC API started");

    // Wait for shutdown
    shutdown.token().cancelled().await;
    tracing::info!("shutting down gracefully");

    // Send Shutdown to helper
    let shutdown_msg = DaemonMsg::Shutdown;
    let _ = eoip_proto::wire::serialize_msg(&shutdown_msg).and_then(|payload| {
        let iov = [std::io::IoSlice::new(&payload)];
        nix::sys::socket::sendmsg::<()>(
            helper_stream.as_raw_fd(),
            &iov,
            &[],
            nix::sys::socket::MsgFlags::empty(),
            None,
        )
        .map_err(|e| eoip_proto::EoipError::RawSocketError(std::io::Error::from(e)))
    });

    api_handle.abort();
    Ok(())
}
