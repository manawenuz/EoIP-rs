//! Privileged helper for EoIP-rs.
//!
//! Creates TAP interfaces and raw sockets that require root/CAP_NET_ADMIN,
//! then passes them to the unprivileged daemon via SCM_RIGHTS over a Unix
//! domain socket. Audit surface: ~200 lines of privileged code.

pub mod fdpass;
pub mod privdrop;
pub mod rawsock;
pub mod tap;
