//! Network I/O: TAP device wrapper, MTU detection, and helper communication.

pub mod mtu;
pub mod pmtud;
#[cfg(unix)]
pub mod tap;
#[cfg(target_os = "windows")]
pub mod tap_windows;
