//! Async TAP device wrapper using `tokio::io::unix::AsyncFd`.

use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};

use nix::libc;

use tokio::io::unix::AsyncFd;

/// Async wrapper around a TAP file descriptor.
///
/// Uses `AsyncFd` for epoll-based readiness notification, with
/// `try_io()` for non-blocking reads and writes.
pub struct TapDevice {
    inner: AsyncFd<OwnedFd>,
}

impl TapDevice {
    /// Wrap an existing TAP `OwnedFd` (must already be non-blocking).
    pub fn new(fd: OwnedFd) -> io::Result<Self> {
        Ok(Self {
            inner: AsyncFd::new(fd)?,
        })
    }

    /// Read an Ethernet frame from the TAP device.
    pub async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.inner.readable().await?;
            match guard.try_io(|inner| {
                let raw_fd = inner.as_raw_fd();
                // Safety: we have readiness, raw_fd is valid for the lifetime of the guard
                let n = unsafe { libc::read(raw_fd, buf.as_mut_ptr().cast(), buf.len()) };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            }) {
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }

    /// Write an Ethernet frame to the TAP device.
    pub async fn write(&self, buf: &[u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.inner.writable().await?;
            match guard.try_io(|inner| {
                let raw_fd = inner.as_raw_fd();
                let n = unsafe { libc::write(raw_fd, buf.as_ptr().cast(), buf.len()) };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            }) {
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }
}

impl std::fmt::Debug for TapDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TapDevice").finish()
    }
}

impl AsFd for TapDevice {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.inner.as_fd()
    }
}
