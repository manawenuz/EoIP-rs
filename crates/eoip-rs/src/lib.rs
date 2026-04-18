//! EoIP-rs daemon — userspace EoIP/EoIPv6 tunneling compatible with MikroTik RouterOS.

#[cfg(unix)]
pub mod api;
pub mod config;
#[cfg(unix)]
pub mod ipsec;
#[cfg(unix)]
pub mod keepalive;
pub mod net;
#[cfg(unix)]
pub mod packet;
#[cfg(unix)]
pub mod shutdown;
pub mod tunnel;

/// Daemon-level errors.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("helper error: {0}")]
    Helper(#[from] eoip_proto::EoipError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("shutdown")]
    Shutdown,
}
