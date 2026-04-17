//! TOML configuration file parsing and validation.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::DaemonError;

/// Top-level daemon configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub performance: PerformanceConfig,
    #[serde(default, rename = "tunnel")]
    pub tunnels: Vec<TunnelConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "default_user")]
    pub user: String,
    #[serde(default = "default_group")]
    pub group: String,
    #[serde(default = "default_helper_mode")]
    pub helper_mode: String,
    #[serde(default = "default_helper_socket")]
    pub helper_socket: PathBuf,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            user: default_user(),
            group: default_group(),
            helper_mode: default_helper_mode(),
            helper_socket: default_helper_socket(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiConfig {
    #[serde(default = "default_api_listen")]
    pub listen: String,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            listen: default_api_listen(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_format")]
    pub format: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PerformanceConfig {
    #[serde(default = "default_low_water_mark")]
    pub low_water_mark: usize,
    #[serde(default = "default_high_water_mark")]
    pub high_water_mark: usize,
    #[serde(default = "default_max_batch_size")]
    pub max_batch_size: usize,
    #[serde(default = "default_batch_timeout_us")]
    pub batch_timeout_us: u64,
    #[serde(default = "default_channel_buffer")]
    pub channel_buffer: usize,
    #[serde(default = "default_rx_workers")]
    pub rx_workers: usize,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            low_water_mark: default_low_water_mark(),
            high_water_mark: default_high_water_mark(),
            max_batch_size: default_max_batch_size(),
            batch_timeout_us: default_batch_timeout_us(),
            channel_buffer: default_channel_buffer(),
            rx_workers: default_rx_workers(),
        }
    }
}

/// Per-tunnel configuration from the TOML `[[tunnel]]` array.
#[derive(Debug, Clone, Deserialize)]
pub struct TunnelConfig {
    pub tunnel_id: u16,
    pub local: IpAddr,
    pub remote: IpAddr,
    #[serde(default)]
    pub iface_name: Option<String>,
    #[serde(default = "default_mtu")]
    pub mtu: u16,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_keepalive_interval_secs")]
    pub keepalive_interval_secs: u64,
    #[serde(default = "default_keepalive_timeout_secs")]
    pub keepalive_timeout_secs: u64,
}

impl TunnelConfig {
    /// Generate a default interface name from the tunnel ID if none is specified.
    pub fn effective_iface_name(&self) -> String {
        self.iface_name
            .clone()
            .unwrap_or_else(|| format!("eoip{}", self.tunnel_id))
    }
}

// ── Default value functions ─────────────────────────────────────

fn default_user() -> String { "eoip".into() }
fn default_group() -> String { "eoip".into() }
fn default_helper_mode() -> String { "persist".into() }
fn default_helper_socket() -> PathBuf { PathBuf::from("/run/eoip-rs/helper.sock") }
fn default_api_listen() -> String { "[::1]:50051".into() }
fn default_log_level() -> String { "info".into() }
fn default_log_format() -> String { "pretty".into() }
fn default_low_water_mark() -> usize { 8 }
fn default_high_water_mark() -> usize { 256 }
fn default_max_batch_size() -> usize { 64 }
fn default_batch_timeout_us() -> u64 { 50 }
fn default_channel_buffer() -> usize { 1024 }
fn default_rx_workers() -> usize { 1 }
fn default_mtu() -> u16 { 1458 }
fn default_enabled() -> bool { true }
fn default_keepalive_interval_secs() -> u64 { 10 }
fn default_keepalive_timeout_secs() -> u64 { 100 }

/// Parse and validate a TOML config file.
pub fn parse_config(path: &Path) -> Result<Config, DaemonError> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| DaemonError::Config(format!("cannot read {}: {e}", path.display())))?;

    let config: Config = toml::from_str(&contents)
        .map_err(|e| DaemonError::Config(format!("invalid TOML in {}: {e}", path.display())))?;

    validate_config(&config)?;
    Ok(config)
}

fn validate_config(config: &Config) -> Result<(), DaemonError> {
    if !matches!(config.daemon.helper_mode.as_str(), "persist" | "exit") {
        return Err(DaemonError::Config(format!(
            "helper_mode must be 'persist' or 'exit', got '{}'",
            config.daemon.helper_mode
        )));
    }

    for (i, t) in config.tunnels.iter().enumerate() {
        // Check for IPv6 transport with tunnel ID > 4095
        if t.local.is_ipv6() && t.tunnel_id > 4095 {
            return Err(DaemonError::Config(format!(
                "tunnel[{i}]: tunnel_id {} exceeds EoIPv6 maximum of 4095",
                t.tunnel_id
            )));
        }

        // Validate address family consistency
        if (t.local.is_ipv4()) != (t.remote.is_ipv4()) {
            return Err(DaemonError::Config(format!(
                "tunnel[{i}]: local ({}) and remote ({}) must be same address family",
                t.local, t.remote
            )));
        }

        // Validate interface name length
        if let Some(ref name) = t.iface_name {
            if name.len() > 15 {
                return Err(DaemonError::Config(format!(
                    "tunnel[{i}]: iface_name '{}' exceeds 15 chars (Linux IFNAMSIZ)",
                    name
                )));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_CONFIG: &str = r#"
[daemon]
user = "eoip"
group = "eoip"
helper_mode = "persist"
helper_socket = "/run/eoip-rs/helper.sock"

[api]
listen = "[::1]:50051"

[logging]
level = "info"
format = "pretty"

[performance]
low_water_mark = 8
high_water_mark = 256
max_batch_size = 64
batch_timeout_us = 50
channel_buffer = 1024
rx_workers = 1

[[tunnel]]
tunnel_id = 100
local = "192.168.1.1"
remote = "192.168.1.2"
iface_name = "eoip-dc1"
mtu = 1500
enabled = true

[[tunnel]]
tunnel_id = 200
local = "10.0.0.1"
remote = "10.0.0.2"
mtu = 1400
"#;

    #[test]
    fn parse_valid_config() {
        let config: Config = toml::from_str(VALID_CONFIG).unwrap();
        assert_eq!(config.tunnels.len(), 2);
        assert_eq!(config.tunnels[0].tunnel_id, 100);
        assert_eq!(
            config.tunnels[0].local,
            "192.168.1.1".parse::<IpAddr>().unwrap()
        );
        assert_eq!(config.tunnels[0].effective_iface_name(), "eoip-dc1");
        assert_eq!(config.tunnels[1].effective_iface_name(), "eoip200");
        assert_eq!(config.daemon.helper_mode, "persist");
        assert_eq!(config.performance.max_batch_size, 64);
    }

    #[test]
    fn parse_minimal_config() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.tunnels.is_empty());
        assert_eq!(config.daemon.user, "eoip");
        assert_eq!(config.performance.rx_workers, 1);
    }

    #[test]
    fn parse_ipv6_tunnel() {
        let toml = r#"
[[tunnel]]
tunnel_id = 42
local = "fd00::1"
remote = "fd00::2"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        validate_config(&config).unwrap();
        assert!(config.tunnels[0].local.is_ipv6());
    }

    #[test]
    fn validate_ipv6_tunnel_id_too_large() {
        let toml = r#"
[[tunnel]]
tunnel_id = 5000
local = "fd00::1"
remote = "fd00::2"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_mixed_address_families() {
        let toml = r#"
[[tunnel]]
tunnel_id = 1
local = "10.0.0.1"
remote = "fd00::2"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_invalid_helper_mode() {
        let toml = r#"
[daemon]
helper_mode = "invalid"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_iface_name_too_long() {
        let toml = r#"
[[tunnel]]
tunnel_id = 1
local = "10.0.0.1"
remote = "10.0.0.2"
iface_name = "this-name-is-way-too-long"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn parse_invalid_toml() {
        assert!(toml::from_str::<Config>("invalid [[[toml").is_err());
    }

    #[test]
    fn default_keepalive_values() {
        let toml = r#"
[[tunnel]]
tunnel_id = 1
local = "10.0.0.1"
remote = "10.0.0.2"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.tunnels[0].keepalive_interval_secs, 10);
        assert_eq!(config.tunnels[0].keepalive_timeout_secs, 100);
    }
}
