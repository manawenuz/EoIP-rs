//! RouterOS-style output formatting.

use colored::Colorize;
use eoip_api::{GlobalStats, Tunnel, TunnelStats, TunnelState};

/// State flag character for a tunnel.
fn state_flag(tunnel: &Tunnel) -> (&str, colored::ColoredString) {
    if !tunnel.enabled {
        return ("X", "X".red());
    }
    match TunnelState::try_from(tunnel.state) {
        Ok(TunnelState::Active) => ("R", "R".green()),
        Ok(TunnelState::Stale) => ("S", "S".yellow()),
        Ok(TunnelState::Initializing) => ("I", "I".cyan()),
        Ok(TunnelState::Configured) => ("C", "C".cyan()),
        Ok(TunnelState::TearingDown) => ("T", "T".red()),
        Ok(TunnelState::Destroyed) => ("D", "D".red()),
        _ => (" ", " ".normal()),
    }
}

fn state_name(tunnel: &Tunnel) -> &str {
    match TunnelState::try_from(tunnel.state) {
        Ok(TunnelState::Active) => "active",
        Ok(TunnelState::Stale) => "stale",
        Ok(TunnelState::Initializing) => "initializing",
        Ok(TunnelState::Configured) => "configured",
        Ok(TunnelState::TearingDown) => "tearing-down",
        Ok(TunnelState::Destroyed) => "destroyed",
        _ => "unknown",
    }
}

/// Print tunnels in RouterOS table format.
pub fn print_table(tunnels: &[Tunnel]) {
    if tunnels.is_empty() {
        println!("(no tunnels)");
        return;
    }

    println!("{}", "Flags: X - disabled; R - running; S - stale; I - initializing");
    println!(
        "{:>2} {:1} {:<15} {:>10}   {:<15}  {:<15}  {:>5}",
        "#", "", "NAME", "TUNNEL-ID", "LOCAL-ADDR", "REMOTE-ADDR", "MTU"
    );

    for (i, t) in tunnels.iter().enumerate() {
        let (_, flag) = state_flag(t);
        println!(
            "{:>2} {} {:<15} {:>10}   {:<15}  {:<15}  {:>5}",
            i, flag, t.iface_name, t.tunnel_id, t.local_addr, t.remote_addr, t.mtu
        );
    }
}

/// Print tunnels in RouterOS detail format.
pub fn print_detail(tunnels: &[Tunnel]) {
    if tunnels.is_empty() {
        println!("(no tunnels)");
        return;
    }

    println!("{}", "Flags: X - disabled; R - running; S - stale; I - initializing");

    for (i, t) in tunnels.iter().enumerate() {
        let (_, flag) = state_flag(t);
        println!(
            "{:>2}  {} name=\"{}\" tunnel-id={} local-address={}",
            i, flag, t.iface_name, t.tunnel_id, t.local_addr
        );
        println!(
            "      remote-address={} mtu={} keepalive={}s,{}s",
            t.remote_addr, t.mtu, t.keepalive_interval_secs, t.keepalive_timeout_secs
        );
        println!(
            "      enabled={} state={}",
            if t.enabled { "yes" } else { "no" },
            state_name(t)
        );
        println!();
    }
}

/// Print per-tunnel stats.
pub fn print_stats(stats: &TunnelStats) {
    println!("        tunnel-id: {}", stats.tunnel_id);
    println!("       tx-packets: {}", stats.tx_packets);
    println!("         tx-bytes: {}", format_bytes(stats.tx_bytes));
    println!("       rx-packets: {}", stats.rx_packets);
    println!("         rx-bytes: {}", format_bytes(stats.rx_bytes));
    println!("        tx-errors: {}", stats.tx_errors);
    println!("        rx-errors: {}", stats.rx_errors);
    println!("          last-rx: {}", format_ts(stats.last_rx_timestamp_ms));
    println!("          last-tx: {}", format_ts(stats.last_tx_timestamp_ms));
}

/// Print global stats.
pub fn print_global_stats(stats: &GlobalStats) {
    println!("   active-tunnels: {}", stats.active_tunnels);
    println!("    stale-tunnels: {}", stats.stale_tunnels);
    println!(" total-tx-packets: {}", stats.total_tx_packets);
    println!(" total-rx-packets: {}", stats.total_rx_packets);
    println!("   total-tx-bytes: {}", format_bytes(stats.total_tx_bytes));
    println!("   total-rx-bytes: {}", format_bytes(stats.total_rx_bytes));
}

/// Print tunnels/stats as JSON.
pub fn print_json<T: serde::Serialize>(val: &T) {
    println!("{}", serde_json::to_string_pretty(val).unwrap_or_default());
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GiB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes}")
    }
}

fn format_ts(ms: i64) -> String {
    if ms == 0 {
        return "never".into();
    }
    let secs = ms / 1000;
    let ago = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64 - secs)
        .unwrap_or(0);
    if ago < 60 {
        format!("{ago}s ago")
    } else if ago < 3600 {
        format!("{}m {}s ago", ago / 60, ago % 60)
    } else {
        format!("{}h {}m ago", ago / 3600, (ago % 3600) / 60)
    }
}
