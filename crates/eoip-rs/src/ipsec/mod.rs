//! IPsec integration via strongSwan VICI protocol.
//!
//! When a tunnel has `ipsec_secret` configured, the daemon creates an IKEv1
//! transport-mode SA via strongSwan to encrypt GRE traffic — matching
//! MikroTik's `ipsec-secret` behavior.

pub mod config;
pub mod monitor;
pub mod vici;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;

use crate::ipsec::config::IpsecTunnelConfig;
use crate::ipsec::vici::ViciClient;

/// Manages IPsec SAs for tunnels via strongSwan's VICI protocol.
pub struct IpsecManager {
    client: Mutex<Option<ViciClient>>,
    /// Track which tunnels have IPsec configured (tunnel_id → config).
    tunnels: Mutex<HashMap<u16, IpsecTunnelConfig>>,
}

impl IpsecManager {
    /// Create a new IpsecManager. Attempts to connect to strongSwan.
    pub fn new() -> Self {
        let client = match ViciClient::connect() {
            Ok(c) => {
                tracing::info!("connected to strongSwan VICI socket");
                Some(c)
            }
            Err(e) => {
                tracing::warn!(%e, "strongSwan not available — IPsec tunnels will run unencrypted");
                None
            }
        };

        Self {
            client: Mutex::new(client),
            tunnels: Mutex::new(HashMap::new()),
        }
    }

    /// Set up IPsec for a tunnel: load connection + shared secret, then initiate.
    pub fn setup_tunnel(
        &self,
        tunnel_id: u16,
        local: IpAddr,
        remote: IpAddr,
        secret: &str,
    ) -> Result<(), String> {
        let cfg = IpsecTunnelConfig::new(tunnel_id, local, remote, secret.to_string());

        let mut client_guard = self.client.lock().map_err(|e| e.to_string())?;
        let client = client_guard.as_mut().ok_or_else(|| {
            "strongSwan not available — install strongswan-charon and start the service".to_string()
        })?;

        // Load shared secret first
        let shared_msg = cfg.to_vici_shared_secret();
        client
            .call("load-shared", &shared_msg)
            .map_err(|e| format!("load-shared failed: {e}"))?;
        tracing::info!(tunnel_id, "loaded IPsec shared secret");

        // Load connection
        let conn_msg = cfg.to_vici_connection();
        client
            .call("load-conn", &conn_msg)
            .map_err(|e| format!("load-conn failed: {e}"))?;
        tracing::info!(tunnel_id, conn = %cfg.conn_name(), "loaded IPsec connection");

        // Initiate the connection
        let init_msg = cfg.to_vici_initiate();
        match client.call_streaming_ignore("initiate", &init_msg) {
            Ok(_) => tracing::info!(tunnel_id, "IPsec SA initiated"),
            Err(e) => tracing::warn!(tunnel_id, %e, "IPsec initiate failed (peer may not be ready)"),
        }

        self.tunnels.lock().unwrap().insert(tunnel_id, cfg);
        Ok(())
    }

    /// Tear down IPsec for a tunnel: terminate SA, unload connection.
    pub fn teardown_tunnel(&self, tunnel_id: u16) -> Result<(), String> {
        let cfg = self.tunnels.lock().unwrap().remove(&tunnel_id);
        let cfg = match cfg {
            Some(c) => c,
            None => return Ok(()), // no IPsec configured for this tunnel
        };

        let mut client_guard = self.client.lock().map_err(|e| e.to_string())?;
        let client = match client_guard.as_mut() {
            Some(c) => c,
            None => return Ok(()),
        };

        // Terminate child SA
        let term_msg = cfg.to_vici_terminate();
        match client.call("terminate", &term_msg) {
            Ok(_) => tracing::info!(tunnel_id, "IPsec SA terminated"),
            Err(e) => tracing::warn!(tunnel_id, %e, "IPsec terminate failed"),
        }

        // Unload connection
        let unload_msg = cfg.to_vici_unload();
        match client.call("unload-conn", &unload_msg) {
            Ok(_) => tracing::info!(tunnel_id, "IPsec connection unloaded"),
            Err(e) => tracing::warn!(tunnel_id, %e, "IPsec unload-conn failed"),
        }

        Ok(())
    }

    /// Check if IPsec SA is established for a tunnel.
    pub fn is_sa_established(&self, tunnel_id: u16) -> bool {
        let tunnels = self.tunnels.lock().unwrap();
        let cfg = match tunnels.get(&tunnel_id) {
            Some(c) => c,
            None => return false,
        };

        let mut client_guard = self.client.lock().unwrap();
        let client = match client_guard.as_mut() {
            Some(c) => c,
            None => return false,
        };

        client.has_established_sa(&cfg.conn_name())
    }

    /// Re-initiate a tunnel's IPsec SA (used by monitor on SA loss).
    pub fn reinitiate(&self, tunnel_id: u16) {
        let tunnels = self.tunnels.lock().unwrap();
        let cfg = match tunnels.get(&tunnel_id) {
            Some(c) => c,
            None => return,
        };

        let init_msg = cfg.to_vici_initiate();
        drop(tunnels);

        let mut client_guard = self.client.lock().unwrap();
        if let Some(client) = client_guard.as_mut() {
            match client.call_streaming_ignore("initiate", &init_msg) {
                Ok(_) => tracing::info!(tunnel_id, "IPsec SA re-initiated"),
                Err(e) => tracing::warn!(tunnel_id, %e, "IPsec re-initiate failed"),
            }
        }
    }

    /// Get all tunnel IDs with IPsec configured.
    pub fn tunnel_ids(&self) -> Vec<u16> {
        self.tunnels.lock().unwrap().keys().copied().collect()
    }
}
