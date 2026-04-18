//! TCP MSS clamping via iptables.
//!
//! Adds a FORWARD rule in the mangle table to clamp TCP MSS on SYN packets
//! exiting the tunnel interface, matching MikroTik's `Clamp TCP MSS = yes`.
//!
//! Rule:
//! ```text
//! iptables -t mangle -A FORWARD -o <iface> -p tcp --tcp-flags SYN,RST SYN
//!          -j TCPMSS --clamp-mss-to-pmtu
//! ```

/// Add the TCP MSS clamping rule for a tunnel interface.
///
/// Idempotent: checks if the rule already exists before adding.
#[cfg(target_os = "linux")]
pub fn add_mss_clamp_rule(iface: &str) -> Result<(), std::io::Error> {
    // Check if rule already exists (iptables -C returns 0 if present).
    let check = std::process::Command::new("iptables")
        .args([
            "-t", "mangle",
            "-C", "FORWARD",
            "-o", iface,
            "-p", "tcp",
            "--tcp-flags", "SYN,RST", "SYN",
            "-j", "TCPMSS",
            "--clamp-mss-to-pmtu",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;

    if check.success() {
        tracing::debug!(interface = %iface, "MSS clamp rule already exists");
        return Ok(());
    }

    let output = std::process::Command::new("iptables")
        .args([
            "-t", "mangle",
            "-A", "FORWARD",
            "-o", iface,
            "-p", "tcp",
            "--tcp-flags", "SYN,RST", "SYN",
            "-j", "TCPMSS",
            "--clamp-mss-to-pmtu",
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(interface = %iface, %stderr, "failed to add MSS clamp rule");
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("iptables add MSS rule failed: {stderr}"),
        ));
    }

    tracing::info!(interface = %iface, "added TCP MSS clamping rule");
    Ok(())
}

/// Remove the TCP MSS clamping rule for a tunnel interface.
#[cfg(target_os = "linux")]
pub fn remove_mss_clamp_rule(iface: &str) -> Result<(), std::io::Error> {
    let output = std::process::Command::new("iptables")
        .args([
            "-t", "mangle",
            "-D", "FORWARD",
            "-o", iface,
            "-p", "tcp",
            "--tcp-flags", "SYN,RST", "SYN",
            "-j", "TCPMSS",
            "--clamp-mss-to-pmtu",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;

    if output.success() {
        tracing::info!(interface = %iface, "removed TCP MSS clamping rule");
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn add_mss_clamp_rule(iface: &str) -> Result<(), std::io::Error> {
    tracing::debug!(interface = %iface, "MSS clamping not supported on this platform");
    let _ = iface;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn remove_mss_clamp_rule(iface: &str) -> Result<(), std::io::Error> {
    let _ = iface;
    Ok(())
}
