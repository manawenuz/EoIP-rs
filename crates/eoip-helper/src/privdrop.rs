//! Privilege dropping after resource creation.
//!
//! SECURITY: `setgid` must be called before `setuid`. Once UID is dropped,
//! the process cannot regain root — this is irreversible by design.

use eoip_proto::EoipError;

/// Drop privileges to the specified UID/GID.
///
/// Must be called as root. Calls `setgid` first (POSIX requirement: GID
/// must be changed while still root), then `setuid`. Verifies the drop
/// succeeded.
///
/// # Errors
///
/// Returns an error if not running as root, or if the syscalls fail.
#[cfg(target_os = "linux")]
pub fn drop_privileges(target_uid: u32, target_gid: u32) -> Result<(), EoipError> {
    use nix::unistd::{getgid, getuid, setgid, setuid, Gid, Uid};

    if !getuid().is_root() {
        return Err(EoipError::ConfigError(
            "drop_privileges called but not running as root".into(),
        ));
    }

    // SECURITY: setgid MUST come before setuid — after dropping UID we
    // can no longer change GID.
    setgid(Gid::from_raw(target_gid)).map_err(|e| {
        EoipError::ConfigError(format!("setgid({target_gid}) failed: {e}"))
    })?;

    setuid(Uid::from_raw(target_uid)).map_err(|e| {
        EoipError::ConfigError(format!("setuid({target_uid}) failed: {e}"))
    })?;

    // Verify
    let actual_uid = getuid().as_raw();
    let actual_gid = getgid().as_raw();
    if actual_uid != target_uid || actual_gid != target_gid {
        return Err(EoipError::ConfigError(format!(
            "privilege drop verification failed: expected uid={target_uid}/gid={target_gid}, \
             got uid={actual_uid}/gid={actual_gid}"
        )));
    }

    tracing::warn!(uid = target_uid, gid = target_gid, "dropped privileges");
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn drop_privileges(_target_uid: u32, _target_gid: u32) -> Result<(), EoipError> {
    tracing::warn!("privilege dropping not supported on this platform");
    Ok(())
}
