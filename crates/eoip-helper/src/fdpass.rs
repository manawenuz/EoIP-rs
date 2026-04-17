//! SCM_RIGHTS file descriptor passing over Unix domain sockets.
//!
//! File descriptors are passed as ancillary data alongside a serialized
//! `HelperMsg`/`DaemonMsg` payload. The kernel translates fd numbers during
//! transfer — the receiving process gets different fd numbers referencing
//! the same kernel objects.

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, RawFd};

use nix::sys::socket::{self, ControlMessage, MsgFlags};

use eoip_proto::wire::{deserialize_msg, serialize_msg, DaemonMsg, HelperMsg};
use eoip_proto::EoipError;

/// Send a `HelperMsg` with an attached file descriptor via SCM_RIGHTS.
///
/// The message payload is serialized into the iov data, and the fd is
/// sent as ancillary data. The receiver must use `recv_msg_with_fd` to
/// extract both.
pub fn send_msg_with_fd(
    sock: BorrowedFd<'_>,
    msg: &HelperMsg,
    fd: BorrowedFd<'_>,
) -> Result<(), EoipError> {
    let payload = serialize_msg(msg)?;
    let fds = [fd.as_raw_fd()];
    let cmsg = [ControlMessage::ScmRights(&fds)];
    let iov = [io::IoSlice::new(&payload)];

    socket::sendmsg::<()>(sock.as_raw_fd(), &iov, &cmsg, MsgFlags::empty(), None)
        .map_err(|e| EoipError::RawSocketError(io::Error::from(e)))?;

    tracing::debug!(fd = fd.as_raw_fd(), "sent fd via SCM_RIGHTS");
    Ok(())
}

/// Send a `HelperMsg` without an attached file descriptor.
pub fn send_msg(sock: BorrowedFd<'_>, msg: &HelperMsg) -> Result<(), EoipError> {
    let payload = serialize_msg(msg)?;
    let iov = [io::IoSlice::new(&payload)];

    socket::sendmsg::<()>(sock.as_raw_fd(), &iov, &[], MsgFlags::empty(), None)
        .map_err(|e| EoipError::RawSocketError(io::Error::from(e)))?;

    Ok(())
}

/// Receive a `DaemonMsg` from the Unix socket.
///
/// Reads the serialized message payload from the socket. This does NOT
/// expect any ancillary data (the daemon sends messages, not fds).
pub fn recv_msg(sock: BorrowedFd<'_>) -> Result<DaemonMsg, EoipError> {
    let mut buf = [0u8; 512];
    let mut iov = [io::IoSliceMut::new(&mut buf)];

    let msg = socket::recvmsg::<()>(sock.as_raw_fd(), &mut iov, None, MsgFlags::empty())
        .map_err(|e| EoipError::RawSocketError(io::Error::from(e)))?;

    let bytes_read = msg.bytes;
    if bytes_read == 0 {
        return Err(EoipError::HelperDisconnected);
    }

    deserialize_msg(&buf[..bytes_read])
}

/// Receive a `HelperMsg` with an optional attached file descriptor.
///
/// Used by the daemon side to receive messages and fds from the helper.
/// Returns `(msg, Option<RawFd>)`.
pub fn recv_msg_with_fd(sock: BorrowedFd<'_>) -> Result<(HelperMsg, Option<RawFd>), EoipError> {
    let mut buf = [0u8; 512];
    let mut iov = [io::IoSliceMut::new(&mut buf)];
    let mut cmsg_buf = nix::cmsg_space!(RawFd);

    let msg = socket::recvmsg::<()>(
        sock.as_raw_fd(),
        &mut iov,
        Some(&mut cmsg_buf),
        MsgFlags::empty(),
    )
    .map_err(|e| EoipError::RawSocketError(io::Error::from(e)))?;

    let bytes_read = msg.bytes;
    if bytes_read == 0 {
        return Err(EoipError::HelperDisconnected);
    }

    // Extract fd from ancillary data
    let mut received_fd: Option<RawFd> = None;
    if let Ok(cmsgs) = msg.cmsgs() {
        for cmsg in cmsgs {
            if let socket::ControlMessageOwned::ScmRights(fds) = cmsg {
                if let Some(&fd) = fds.first() {
                    received_fd = Some(fd);
                }
            }
        }
    }

    let helper_msg: HelperMsg = deserialize_msg(&buf[..bytes_read])?;
    Ok((helper_msg, received_fd))
}
