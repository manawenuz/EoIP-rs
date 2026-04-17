//! Dynamic tunnel lifecycle manager.
//!
//! Holds all shared resources needed to create and destroy tunnels at runtime:
//! helper socket, raw socket fds, buffer pool, TX channel, and cancellation tokens.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use eoip_helper::fdpass;
use eoip_proto::wire::{DaemonMsg, HelperMsg};
use eoip_proto::DemuxKey;

use crate::config::TunnelConfig;
use crate::keepalive;
use crate::net::tap::TapDevice;
use crate::packet::buffer::BufferPool;
use crate::packet::tx::{self, TxPacket};
use crate::tunnel::handle::TunnelHandle;
use crate::tunnel::lifecycle::TunnelState;
use crate::tunnel::registry::TunnelRegistry;

/// Per-tunnel task handles for graceful cancellation.
struct TunnelTasks {
    cancel: CancellationToken,
    _tap: Arc<TapDevice>,
}

/// Shared tunnel lifecycle manager. Thread-safe (Arc-wrapped fields + Mutex for helper).
pub struct TunnelManager {
    helper: Mutex<UnixStream>,
    registry: Arc<TunnelRegistry>,
    pool: Arc<BufferPool>,
    tx_sender: mpsc::Sender<TxPacket>,
    raw_v4_fd: RawFd,
    raw_v6_fd: RawFd,
    shutdown: CancellationToken,
    tasks: Mutex<HashMap<u16, TunnelTasks>>,
}

impl TunnelManager {
    pub fn new(
        helper: UnixStream,
        registry: Arc<TunnelRegistry>,
        pool: Arc<BufferPool>,
        tx_sender: mpsc::Sender<TxPacket>,
        raw_v4_fd: RawFd,
        raw_v6_fd: RawFd,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            helper: Mutex::new(helper),
            registry,
            pool,
            tx_sender,
            raw_v4_fd,
            raw_v6_fd,
            shutdown,
            tasks: Mutex::new(HashMap::new()),
        }
    }

    pub fn registry(&self) -> &Arc<TunnelRegistry> {
        &self.registry
    }

    /// Create a tunnel dynamically: request TAP from helper, spawn tasks, register.
    pub async fn create_tunnel(&self, config: TunnelConfig) -> Result<(), String> {
        let tunnel_id = config.tunnel_id;
        let iface_name = config.effective_iface_name();

        // Check for duplicate
        if !self.registry.find_by_tunnel_id(tunnel_id).is_empty() {
            return Err(format!("tunnel {tunnel_id} already exists"));
        }

        // Create handle and register (Initializing state)
        let handle = Arc::new(TunnelHandle::new(config.clone()));
        let key = DemuxKey {
            tunnel_id,
            peer_addr: config.remote,
        };
        self.registry.insert(key.clone(), Arc::clone(&handle));

        // Request TAP from helper (blocking, under mutex)
        let tap = {
            let helper = self.helper.lock().map_err(|e| format!("helper lock: {e}"))?;
            let helper_fd = helper.as_fd();

            // Send CreateTunnel
            let create_msg = DaemonMsg::CreateTunnel {
                iface_name: iface_name.clone(),
                tunnel_id,
            };
            let payload = eoip_proto::wire::serialize_msg(&create_msg)
                .map_err(|e| format!("serialize: {e}"))?;
            let iov = [io::IoSlice::new(&payload)];
            nix::sys::socket::sendmsg::<()>(
                helper.as_raw_fd(),
                &iov,
                &[],
                nix::sys::socket::MsgFlags::empty(),
                None,
            )
            .map_err(|e| format!("send CreateTunnel: {e}"))?;

            // Receive TapCreated + fd
            let (msg, fd) = fdpass::recv_msg_with_fd(helper_fd)
                .map_err(|e| format!("recv TapCreated: {e}"))?;

            match msg {
                HelperMsg::TapCreated { .. } => {}
                HelperMsg::Error { msg } => {
                    self.registry.remove(&key);
                    return Err(format!("helper error: {msg}"));
                }
                other => {
                    self.registry.remove(&key);
                    return Err(format!("unexpected: {other:?}"));
                }
            }

            let tap_fd = fd
                .map(|raw| unsafe { OwnedFd::from_raw_fd(raw) })
                .ok_or_else(|| {
                    self.registry.remove(&key);
                    "no TAP fd received".to_string()
                })?;

            // Set non-blocking
            unsafe {
                let flags = libc::fcntl(tap_fd.as_raw_fd(), libc::F_GETFL);
                libc::fcntl(tap_fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
            }

            // Drain the raw socket messages the helper sends after first tunnel
            // (v4 and v6 raw sockets — already created, helper sends error for v6)
            // We need to consume these messages to keep the protocol in sync
            loop {
                match fdpass::recv_msg_with_fd(helper_fd) {
                    Ok((HelperMsg::RawSocket { .. }, _)) => continue,
                    Ok((HelperMsg::Error { .. }, _)) => continue,
                    _ => break,
                }
            }

            Arc::new(TapDevice::new(tap_fd).map_err(|e| format!("TapDevice: {e}"))?)
        };

        // Transition to Active
        let _ = handle.state.transition(TunnelState::Initializing, TunnelState::Configured);
        let _ = handle.state.transition(TunnelState::Configured, TunnelState::Active);

        // Create per-tunnel cancellation token
        let tunnel_cancel = self.shutdown.child_token();

        // Spawn TAP reader (TAP → raw socket)
        tx::spawn_tap_reader(
            Arc::clone(&tap),
            Arc::clone(&handle),
            Arc::clone(&self.pool),
            self.tx_sender.clone(),
            tunnel_cancel.clone(),
        );

        // TAP writer — dedicated OS thread for zero-overhead channel → TAP delivery
        if let Some(ref rx_recv) = handle.rx_receiver {
            let tap_fd = tap.as_fd().as_raw_fd();
            let rx = rx_recv.clone();
            std::thread::Builder::new()
                .name(format!("tap-wr-{tunnel_id}"))
                .spawn(move || {
                    while let Ok(buf) = rx.recv() {
                        let data = buf.as_slice();
                        unsafe { libc::write(tap_fd, data.as_ptr() as *const _, data.len()) };
                    }
                })
                .expect("failed to spawn TAP writer thread");
        }

        // Spawn keepalive
        let raw_fd = if config.remote.is_ipv6() {
            self.raw_v6_fd
        } else {
            self.raw_v4_fd
        };
        keepalive::spawn_keepalive_task(
            Arc::clone(&handle),
            raw_fd,
            tunnel_cancel.clone(),
        );

        // Track tasks
        self.tasks.lock().unwrap().insert(tunnel_id, TunnelTasks {
            cancel: tunnel_cancel,
            _tap: tap,
        });

        tracing::info!(tunnel_id, iface = %iface_name, "tunnel created dynamically");
        Ok(())
    }

    /// Destroy a tunnel: cancel tasks, remove from registry.
    pub fn destroy_tunnel(&self, tunnel_id: u16) -> Result<(), String> {
        // Cancel per-tunnel tasks
        if let Some(tasks) = self.tasks.lock().unwrap().remove(&tunnel_id) {
            tasks.cancel.cancel();
        }

        // Remove from registry
        let entries = self.registry.find_by_tunnel_id(tunnel_id);
        if entries.is_empty() {
            return Err(format!("tunnel {tunnel_id} not found"));
        }
        for (key, handle) in &entries {
            let _ = handle.state.transition(handle.state.load(), TunnelState::TearingDown);
            self.registry.remove(key);
        }

        tracing::info!(tunnel_id, "tunnel destroyed");
        Ok(())
    }

    /// Register a tunnel that was created during startup (already has TAP + tasks).
    pub fn register_startup_tunnel(&self, tunnel_id: u16, cancel: CancellationToken, tap: Arc<TapDevice>) {
        self.tasks.lock().unwrap().insert(tunnel_id, TunnelTasks { cancel, _tap: tap });
    }
}
