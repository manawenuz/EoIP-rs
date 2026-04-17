use std::path::PathBuf;

use clap::Parser;

/// EoIP protocol analyzer — decode MikroTik EoIP pcap captures layer by layer.
#[derive(Parser, Debug)]
#[command(name = "eoip-analyzer", version)]
pub struct Cli {
    /// Path to pcap or pcapng capture file
    pub file: PathBuf,

    /// Output NDJSON instead of colored text
    #[arg(long)]
    pub json: bool,

    /// Only show session summary statistics (no per-packet output)
    #[arg(long)]
    pub summary_only: bool,

    /// Filter packets by tunnel ID
    #[arg(long)]
    pub tunnel_id: Option<u16>,

    /// Show hex dump of raw packet bytes
    #[arg(long)]
    pub hexdump: bool,

    /// Maximum number of packets to process (0 = unlimited)
    #[arg(long, default_value = "0")]
    pub limit: usize,

    /// UDP port for EoIP-rs UDP encapsulation detection
    #[arg(long, default_value = "26969")]
    pub udp_port: u16,
}
