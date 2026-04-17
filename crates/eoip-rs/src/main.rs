//! EoIP-rs daemon entry point.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use eoip_rs::config::parse_config;
use eoip_rs::shutdown::ShutdownCoordinator;
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

    // Initialize tracing early with env filter, reconfigure after config load
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
    // Load and validate config
    let config = parse_config(&args.config)?;
    tracing::info!(
        tunnels = config.tunnels.len(),
        helper_mode = %config.daemon.helper_mode,
        "configuration loaded"
    );

    // Initialize subsystems
    let registry = Arc::new(TunnelRegistry::new());
    let shutdown = ShutdownCoordinator::new();
    shutdown.spawn_signal_handler();

    // Pre-populate registry with configured tunnels (in Initializing state)
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

    tracing::info!(
        registered = registry.len(),
        "all tunnels registered, waiting for helper connection"
    );

    // TODO: Phase 4 — Connect to helper, receive fds, start packet processing
    // For now, wait for shutdown signal
    shutdown.token().cancelled().await;

    tracing::info!("shutting down gracefully");
    Ok(())
}
