//! EoIP-rs privileged helper binary.
//!
//! Two operational modes:
//! - **persist**: Stays alive, listens for `DaemonMsg` requests to create
//!   tunnels dynamically. Cannot fully drop privileges (needs root for TAP).
//! - **exit**: Creates all resources from initial config, passes fds to
//!   the daemon, then exits. Minimal attack surface.

use std::os::fd::AsFd;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use tracing_subscriber::EnvFilter;

use eoip_helper::fdpass;
use eoip_helper::rawsock;
use eoip_helper::tap;
use eoip_proto::wire::{DaemonMsg, HelperMsg};
use eoip_proto::EoipError;

// AF_INET = 2, AF_INET6 = 10
const AF_INET: u16 = 2;
const AF_INET6: u16 = 10;

#[derive(Parser, Debug)]
#[command(name = "eoip-helper", version, about = "EoIP-rs privileged helper")]
struct Args {
    /// Operational mode
    #[arg(long, value_enum, default_value = "persist")]
    mode: Mode,

    /// Path for the Unix domain socket
    #[arg(long, default_value = "/run/eoip-rs/helper.sock")]
    socket_path: PathBuf,
}

#[derive(Debug, Clone, ValueEnum)]
enum Mode {
    /// Stay alive, handle dynamic tunnel creation requests
    Persist,
    /// Create initial resources, pass to daemon, exit
    Exit,
}

fn main() {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    tracing::info!(mode = ?args.mode, socket = %args.socket_path.display(), "eoip-helper starting");

    if let Err(e) = run(args) {
        tracing::error!(%e, "eoip-helper exiting with error");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), EoipError> {
    // Ensure parent directory exists
    if let Some(parent) = args.socket_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            EoipError::ConfigError(format!(
                "cannot create directory {}: {e}",
                parent.display()
            ))
        })?;
    }

    // Remove stale socket file if present
    let _ = std::fs::remove_file(&args.socket_path);

    // Create and listen on Unix socket
    let listener = UnixListener::bind(&args.socket_path).map_err(|e| {
        EoipError::ConfigError(format!(
            "cannot bind Unix socket at {}: {e}",
            args.socket_path.display()
        ))
    })?;

    tracing::info!(path = %args.socket_path.display(), "listening for daemon connection");

    // Accept single connection from daemon
    let (stream, _addr) = listener.accept().map_err(|e| {
        EoipError::ConfigError(format!("accept failed: {e}"))
    })?;

    let sock = stream.as_fd();
    tracing::info!("daemon connected");

    // Create shared raw sockets (one per address family, shared by all tunnels)
    let mut raw_v4_created = false;
    let mut raw_v6_created = false;

    match args.mode {
        Mode::Exit => run_exit_mode(sock, &mut raw_v4_created, &mut raw_v6_created),
        Mode::Persist => run_persist_mode(sock, &mut raw_v4_created, &mut raw_v6_created),
    }
}

fn run_exit_mode(
    sock: std::os::fd::BorrowedFd<'_>,
    raw_v4_created: &mut bool,
    raw_v6_created: &mut bool,
) -> Result<(), EoipError> {
    // In exit mode, we wait for commands until we get Shutdown
    fdpass::send_msg(sock, &HelperMsg::HelperReady)?;
    tracing::info!("sent HelperReady, waiting for commands");

    loop {
        let msg = fdpass::recv_msg(sock)?;
        match msg {
            DaemonMsg::CreateTunnel {
                iface_name,
                tunnel_id,
            } => {
                handle_create_tunnel(
                    sock,
                    &iface_name,
                    tunnel_id,
                    raw_v4_created,
                    raw_v6_created,
                )?;
            }
            DaemonMsg::DestroyTunnel { iface_name } => {
                tracing::info!(interface = %iface_name, "destroy tunnel (no-op in exit mode)");
            }
            DaemonMsg::Shutdown => {
                tracing::info!("received Shutdown, exiting");
                return Ok(());
            }
        }
    }
}

fn run_persist_mode(
    sock: std::os::fd::BorrowedFd<'_>,
    raw_v4_created: &mut bool,
    raw_v6_created: &mut bool,
) -> Result<(), EoipError> {
    fdpass::send_msg(sock, &HelperMsg::HelperReady)?;
    tracing::info!("sent HelperReady, entering persist loop");

    loop {
        let msg = match fdpass::recv_msg(sock) {
            Ok(m) => m,
            Err(EoipError::HelperDisconnected) => {
                tracing::info!("daemon disconnected, exiting");
                return Ok(());
            }
            Err(e) => {
                tracing::error!(%e, "error receiving message");
                // Send error back and continue
                let _ = fdpass::send_msg(
                    sock,
                    &HelperMsg::Error {
                        msg: e.to_string(),
                    },
                );
                continue;
            }
        };

        match msg {
            DaemonMsg::CreateTunnel {
                iface_name,
                tunnel_id,
            } => {
                if let Err(e) = handle_create_tunnel(
                    sock,
                    &iface_name,
                    tunnel_id,
                    raw_v4_created,
                    raw_v6_created,
                ) {
                    tracing::error!(%e, interface = %iface_name, "failed to create tunnel");
                    let _ = fdpass::send_msg(
                        sock,
                        &HelperMsg::Error {
                            msg: e.to_string(),
                        },
                    );
                }
            }
            DaemonMsg::DestroyTunnel { iface_name } => {
                tracing::info!(interface = %iface_name, "tunnel destroyed (fd will be closed by daemon)");
            }
            DaemonMsg::Shutdown => {
                tracing::info!("received Shutdown, exiting");
                return Ok(());
            }
        }
    }
}

fn handle_create_tunnel(
    sock: std::os::fd::BorrowedFd<'_>,
    iface_name: &str,
    tunnel_id: u16,
    raw_v4_created: &mut bool,
    raw_v6_created: &mut bool,
) -> Result<(), EoipError> {
    tracing::info!(interface = %iface_name, tunnel_id, "creating tunnel resources");

    // Create TAP interface
    let tap_fd = tap::create_tap_interface(iface_name)?;
    fdpass::send_msg_with_fd(
        sock,
        &HelperMsg::TapCreated {
            iface_name: iface_name.to_string(),
            tunnel_id,
        },
        tap_fd.as_fd(),
    )?;

    // Create raw sockets if not already created (shared across tunnels)
    if !*raw_v4_created {
        let raw_v4 = rawsock::create_raw_socket_v4()?;
        fdpass::send_msg_with_fd(
            sock,
            &HelperMsg::RawSocket {
                address_family: AF_INET,
            },
            raw_v4.as_fd(),
        )?;
        *raw_v4_created = true;
    }

    if !*raw_v6_created {
        let raw_v6 = rawsock::create_raw_socket_v6()?;
        fdpass::send_msg_with_fd(
            sock,
            &HelperMsg::RawSocket {
                address_family: AF_INET6,
            },
            raw_v6.as_fd(),
        )?;
        *raw_v6_created = true;
    }

    Ok(())
}
