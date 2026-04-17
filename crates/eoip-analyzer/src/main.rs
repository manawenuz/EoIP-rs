mod cli;
mod decode;
mod deviation;
mod ethernet;
mod ip;
mod output;
mod pcap_reader;
mod stats;

use std::fs::File;
use std::process;

use clap::Parser;

use crate::cli::Cli;
use crate::decode::decode_packet;
use crate::output::{render_packet, render_summary};
use crate::pcap_reader::PcapSource;
use crate::stats::SessionStats;

#[derive(Debug, thiserror::Error)]
pub enum AnalyzerError {
    #[error("I/O error: {0}")]
    Io(String),

    #[error("pcap parse error: {0}")]
    PcapParse(String),

    #[error("packet too short: {0}")]
    PacketTooShort(String),

    #[error("unsupported protocol: {0}")]
    UnsupportedProtocol(String),

    #[error("decode error: {0}")]
    Decode(String),
}

fn main() {
    let cli = Cli::parse();

    let file = match File::open(&cli.file) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: cannot open {:?}: {e}", cli.file);
            process::exit(1);
        }
    };

    let source = match PcapSource::open(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to read pcap: {e}");
            process::exit(1);
        }
    };

    let mut stats = SessionStats::new();
    let mut packet_number: usize = 0;

    for raw in source {
        let raw = match raw {
            Ok(r) => r,
            Err(e) => {
                eprintln!("warning: failed to read packet: {e}");
                stats.record_error();
                continue;
            }
        };

        packet_number += 1;

        let decoded = decode_packet(packet_number, &raw, cli.udp_port);

        // Apply tunnel ID filter
        if let Some(filter_tid) = cli.tunnel_id {
            match &decoded {
                Ok(pkt) if pkt.tunnel_id != filter_tid => {
                    stats.record_skipped();
                    continue;
                }
                _ => {}
            }
        }

        stats.record(&decoded);

        if !cli.summary_only {
            render_packet(&decoded, &cli);
        }

        if cli.limit > 0 && packet_number >= cli.limit {
            break;
        }
    }

    render_summary(&stats, &cli);
}
