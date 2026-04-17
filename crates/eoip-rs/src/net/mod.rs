//! Network I/O: TAP device wrapper and helper communication.

#[cfg(unix)]
pub mod tap;
#[cfg(target_os = "windows")]
pub mod tap_windows;
