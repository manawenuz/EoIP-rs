//! VICI config generation matching MikroTik's exact IKE/ESP parameters.
//!
//! Based on live capture of MikroTik CHR 7.18.2 with `ipsec-secret`.

use std::net::IpAddr;

use rustici::Message;

/// Per-tunnel IPsec configuration for VICI.
pub struct IpsecTunnelConfig {
    pub tunnel_id: u16,
    pub local_addr: IpAddr,
    pub remote_addr: IpAddr,
    pub secret: String,
}

impl IpsecTunnelConfig {
    pub fn new(tunnel_id: u16, local_addr: IpAddr, remote_addr: IpAddr, secret: String) -> Self {
        Self {
            tunnel_id,
            local_addr,
            remote_addr,
            secret,
        }
    }

    /// Connection name used in strongSwan (e.g., "eoip-100").
    pub fn conn_name(&self) -> String {
        format!("eoip-{}", self.tunnel_id)
    }

    /// Child SA name (e.g., "eoip-100-gre").
    pub fn child_name(&self) -> String {
        format!("eoip-{}-gre", self.tunnel_id)
    }

    /// Build VICI `load-conn` message matching MikroTik's IKEv1 parameters.
    ///
    /// MikroTik IKE Phase 1: IKEv1 main mode, AES-128+3DES / SHA1 / modp2048+modp1024
    /// MikroTik ESP Phase 2: AES-256-CBC + AES-192-CBC + AES-128-CBC / SHA1, transport mode
    pub fn to_vici_connection(&self) -> Message {
        let conn = self.conn_name();
        let child = self.child_name();
        let local = self.local_addr.to_string();
        let remote = self.remote_addr.to_string();

        // Traffic selectors: match only GRE (proto 47) between the two endpoints
        let local_ts = format!("{}[gre]", local);
        let remote_ts = format!("{}[gre]", remote);

        Message::new()
            .section_start(&conn)
                // IKEv1 main mode
                .kv_str("version", "1")
                // Local endpoint
                .section_start("local")
                    .kv_str("auth", "psk")
                    .kv_str("id", &local)
                .section_end()
                // Remote endpoint
                .section_start("remote")
                    .kv_str("auth", "psk")
                    .kv_str("id", &remote)
                .section_end()
                // IKE proposals matching MikroTik Phase 1 offers
                .list_start("proposals")
                    .list_item_str("aes128-sha1-modp2048")
                    .list_item_str("aes128-sha1-modp1024")
                    .list_item_str("3des-sha1-modp2048")
                    .list_item_str("3des-sha1-modp1024")
                .list_end()
                // DPD matching MikroTik (8s interval)
                .kv_str("dpd_delay", "8s")
                // IKE SA lifetime (Phase 1): 24 hours
                .kv_str("rekey_time", "24h")
                // Local/remote addresses
                .kv_str("local_addrs", &local)
                .kv_str("remote_addrs", &remote)
                // Child SA (ESP)
                .section_start("children")
                    .section_start(&child)
                        // Transport mode (NOT tunnel mode)
                        .kv_str("mode", "transport")
                        // ESP proposals matching MikroTik Phase 2 offers
                        .list_start("esp_proposals")
                            .list_item_str("aes256-sha1-modp1024")
                            .list_item_str("aes192-sha1-modp1024")
                            .list_item_str("aes128-sha1-modp1024")
                        .list_end()
                        // Child SA lifetime (Phase 2): 30 minutes
                        .kv_str("rekey_time", "30m")
                        // Traffic selectors: only GRE between endpoints
                        .list_start("local_ts")
                            .list_item_str(&local_ts)
                        .list_end()
                        .list_start("remote_ts")
                            .list_item_str(&remote_ts)
                        .list_end()
                        // Replay window
                        .kv_str("replay_window", "128")
                        // Start action: initiate immediately
                        .kv_str("start_action", "start")
                        // DPD action: restart on peer timeout
                        .kv_str("dpd_action", "restart")
                    .section_end()
                .section_end()
            .section_end()
    }

    /// Build VICI `load-shared` message for the pre-shared key.
    pub fn to_vici_shared_secret(&self) -> Message {
        let id = format!("eoip-psk-{}", self.tunnel_id);

        Message::new()
            .kv_str("id", &id)
            .kv_str("type", "IKE")
            .kv_bytes("data", self.secret.as_bytes())
            .list_start("owners")
                .list_item_str(&self.local_addr.to_string())
                .list_item_str(&self.remote_addr.to_string())
            .list_end()
    }

    /// Build VICI `initiate` message.
    pub fn to_vici_initiate(&self) -> Message {
        Message::new()
            .kv_str("child", &self.child_name())
            .kv_str("ike", &self.conn_name())
    }

    /// Build VICI `terminate` message.
    pub fn to_vici_terminate(&self) -> Message {
        Message::new()
            .kv_str("child", &self.child_name())
            .kv_str("ike", &self.conn_name())
    }

    /// Build VICI `unload-conn` message.
    pub fn to_vici_unload(&self) -> Message {
        Message::new().kv_str("name", &self.conn_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conn_name_format() {
        let cfg = IpsecTunnelConfig::new(
            100,
            "10.0.0.1".parse().unwrap(),
            "10.0.0.2".parse().unwrap(),
            "secret".into(),
        );
        assert_eq!(cfg.conn_name(), "eoip-100");
        assert_eq!(cfg.child_name(), "eoip-100-gre");
    }

    #[test]
    fn vici_connection_encodes() {
        let cfg = IpsecTunnelConfig::new(
            42,
            "192.168.1.1".parse().unwrap(),
            "192.168.1.2".parse().unwrap(),
            "TestSecret123".into(),
        );
        let msg = cfg.to_vici_connection();
        // Verify it can encode without panic
        let encoded = msg.encode().expect("encode should succeed");
        assert!(!encoded.is_empty());
    }

    #[test]
    fn vici_shared_secret_encodes() {
        let cfg = IpsecTunnelConfig::new(
            42,
            "192.168.1.1".parse().unwrap(),
            "192.168.1.2".parse().unwrap(),
            "TestSecret123".into(),
        );
        let msg = cfg.to_vici_shared_secret();
        let encoded = msg.encode().expect("encode should succeed");
        assert!(!encoded.is_empty());
    }
}
