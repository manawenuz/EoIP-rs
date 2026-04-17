use colored::Colorize;

use crate::cli::Cli;
use crate::decode::{DecodedPacket, EoipVariant};
use crate::deviation::Severity;
use crate::ethernet::{ethertype_name, format_mac};
use crate::stats::SessionStats;
use crate::AnalyzerError;

/// Render a single decoded packet to stdout.
pub fn render_packet(result: &Result<DecodedPacket, AnalyzerError>, cli: &Cli) {
    if cli.json {
        render_packet_json(result);
    } else {
        render_packet_text(result, cli);
    }
}

/// Render session summary to stdout.
pub fn render_summary(stats: &SessionStats, cli: &Cli) {
    if cli.json {
        render_summary_json(stats);
    } else {
        render_summary_text(stats);
    }
}

// ── Text rendering ──────────────────────────────────────────────

fn render_packet_text(result: &Result<DecodedPacket, AnalyzerError>, cli: &Cli) {
    match result {
        Err(e) => {
            println!("  {} {e}", "ERROR".red().bold());
        }
        Ok(pkt) => {
            let variant_label = match &pkt.variant {
                EoipVariant::Eoip { .. } => "EoIP (proto 47)".cyan(),
                EoipVariant::EoipV6 { .. } => "EoIPv6 (proto 97)".magenta(),
                EoipVariant::UdpEncap { .. } => "EoIP/UDP".yellow(),
                EoipVariant::StandardGre { .. } => "GRE (non-EoIP)".dimmed(),
                EoipVariant::NonEoipUdp { .. } => "UDP (non-EoIP)".dimmed(),
                EoipVariant::Skipped { protocol } => {
                    format!("proto {protocol} (skipped)").dimmed()
                }
            };

            let ts = format!("{:.6}s", pkt.timestamp.as_secs_f64());
            let keepalive = if pkt.is_keepalive {
                " KEEPALIVE".yellow().bold().to_string()
            } else {
                String::new()
            };

            println!(
                "#{:<5} [{}]  {} -> {}  {}{}",
                pkt.packet_number.to_string().bold(),
                ts.dimmed(),
                pkt.ip_header.src(),
                pkt.ip_header.dst(),
                variant_label,
                keepalive,
            );

            // IP layer
            let ip_ver = match &pkt.ip_header {
                crate::ip::IpHeader::V4(_) => "IPv4",
                crate::ip::IpHeader::V6(_) => "IPv6",
            };
            println!(
                "  {}  {}  ttl={}  proto={}  len={}",
                "IP:".dimmed(),
                ip_ver,
                pkt.ip_header.ttl(),
                pkt.ip_header.protocol(),
                pkt.ip_header.total_length(),
            );

            // EoIP layer
            match &pkt.variant {
                EoipVariant::Eoip {
                    magic,
                    payload_len,
                    tunnel_id,
                } => {
                    println!(
                        "  {}  magic={:02x}{:02x}{:02x}{:02x}  payload_len={}  tunnel_id={} (LE: {:02x}{:02x})",
                        "EoIP:".dimmed(),
                        magic[0], magic[1], magic[2], magic[3],
                        payload_len,
                        tunnel_id,
                        tunnel_id.to_le_bytes()[0],
                        tunnel_id.to_le_bytes()[1],
                    );
                }
                EoipVariant::EoipV6 {
                    version_nibble,
                    tunnel_id,
                } => {
                    println!(
                        "  {}  version=0x{:x}  tunnel_id={} (12-bit)",
                        "EoIPv6:".dimmed(),
                        version_nibble,
                        tunnel_id,
                    );
                }
                EoipVariant::UdpEncap {
                    udp_src_port,
                    udp_dst_port,
                    inner_type,
                    reserved_byte,
                    inner,
                } => {
                    let type_name = match inner_type {
                        0x04 => "EoIP",
                        0x06 => "EoIPv6",
                        _ => "unknown",
                    };
                    println!(
                        "  {}  {}:{} -> {}  shim: type=0x{:02x} ({}) reserved=0x{:02x}",
                        "UDP:".dimmed(),
                        pkt.ip_header.src(),
                        udp_src_port,
                        udp_dst_port,
                        inner_type,
                        type_name,
                        reserved_byte,
                    );
                    render_inner_variant(inner);
                }
                EoipVariant::StandardGre { first_bytes } => {
                    println!(
                        "  {}  first_bytes={:02x}{:02x}{:02x}{:02x} (not MikroTik EoIP)",
                        "GRE:".dimmed(),
                        first_bytes[0],
                        first_bytes[1],
                        first_bytes[2],
                        first_bytes[3],
                    );
                }
                EoipVariant::NonEoipUdp {
                    udp_src_port,
                    udp_dst_port,
                } => {
                    println!(
                        "  {}  {}:{} -> {} (no EO shim)",
                        "UDP:".dimmed(),
                        pkt.ip_header.src(),
                        udp_src_port,
                        udp_dst_port,
                    );
                }
                EoipVariant::Skipped { protocol } => {
                    println!("  {}  protocol={}", "Skip:".dimmed(), protocol);
                }
            }

            // Inner Ethernet
            if let Some(ref eth) = pkt.inner_ethernet {
                let et_name = ethertype_name(eth.ethertype);
                let vlan_str = if let Some(ref vlan) = eth.vlan {
                    format!("  VLAN={}", vlan.vid)
                } else {
                    String::new()
                };
                println!(
                    "  {}  {} -> {}  {} (0x{:04x}){}",
                    "Inner:".dimmed(),
                    format_mac(&eth.src_mac).green(),
                    format_mac(&eth.dst_mac).green(),
                    et_name,
                    eth.ethertype,
                    vlan_str,
                );
            }

            // Deviations
            for dev in &pkt.deviations {
                let sev = match dev.severity {
                    Severity::Warn => "WARN".yellow().bold(),
                    Severity::Error => "ERROR".red().bold(),
                };
                println!(
                    "  {} [{}] {}: expected={}, actual={}",
                    sev, dev.field, dev.message, dev.expected, dev.actual,
                );
            }

            // Hex dump
            if cli.hexdump {
                render_hexdump(&pkt.raw_bytes);
            }

            println!(); // blank line between packets
        }
    }
}

fn render_inner_variant(variant: &EoipVariant) {
    match variant {
        EoipVariant::Eoip {
            magic,
            payload_len,
            tunnel_id,
        } => {
            println!(
                "  {}  magic={:02x}{:02x}{:02x}{:02x}  payload_len={}  tunnel_id={}",
                "  EoIP:".dimmed(),
                magic[0],
                magic[1],
                magic[2],
                magic[3],
                payload_len,
                tunnel_id,
            );
        }
        EoipVariant::EoipV6 {
            version_nibble,
            tunnel_id,
        } => {
            println!(
                "  {}  version=0x{:x}  tunnel_id={}",
                "  EoIPv6:".dimmed(),
                version_nibble,
                tunnel_id,
            );
        }
        _ => {}
    }
}

fn render_hexdump(data: &[u8]) {
    println!("  {}:", "Hex".dimmed());
    for (i, chunk) in data.chunks(16).enumerate() {
        let offset = i * 16;
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&b| if (0x20..=0x7e).contains(&b) { b as char } else { '.' })
            .collect();

        // Group hex bytes in pairs of 8
        let hex_left = hex[..chunk.len().min(8)].join(" ");
        let hex_right = if chunk.len() > 8 {
            hex[8..].join(" ")
        } else {
            String::new()
        };

        println!(
            "  {:04x}  {:<23}  {:<23}  |{}|",
            offset, hex_left, hex_right, ascii,
        );
    }
}

// ── JSON rendering ──────────────────────────────────────────────

fn render_packet_json(result: &Result<DecodedPacket, AnalyzerError>) {
    match result {
        Err(e) => {
            let obj = serde_json::json!({
                "type": "error",
                "message": e.to_string(),
            });
            println!("{}", serde_json::to_string(&obj).unwrap_or_default());
        }
        Ok(pkt) => {
            let variant_name = match &pkt.variant {
                EoipVariant::Eoip { .. } => "eoip",
                EoipVariant::EoipV6 { .. } => "eoipv6",
                EoipVariant::UdpEncap { .. } => "udp_encap",
                EoipVariant::StandardGre { .. } => "standard_gre",
                EoipVariant::NonEoipUdp { .. } => "non_eoip_udp",
                EoipVariant::Skipped { .. } => "skipped",
            };

            let mut obj = serde_json::json!({
                "type": "packet",
                "number": pkt.packet_number,
                "timestamp_us": pkt.timestamp.as_micros() as u64,
                "src": pkt.ip_header.src().to_string(),
                "dst": pkt.ip_header.dst().to_string(),
                "ip_proto": pkt.ip_header.protocol(),
                "ttl": pkt.ip_header.ttl(),
                "ip_len": pkt.ip_header.total_length(),
                "variant": variant_name,
                "tunnel_id": pkt.tunnel_id,
                "is_keepalive": pkt.is_keepalive,
            });

            // Add variant-specific fields
            match &pkt.variant {
                EoipVariant::Eoip { payload_len, .. } => {
                    obj["payload_len"] = serde_json::json!(payload_len);
                }
                EoipVariant::EoipV6 {
                    version_nibble, ..
                } => {
                    obj["version_nibble"] = serde_json::json!(version_nibble);
                }
                EoipVariant::UdpEncap {
                    udp_src_port,
                    udp_dst_port,
                    inner_type,
                    ..
                } => {
                    obj["udp_src_port"] = serde_json::json!(udp_src_port);
                    obj["udp_dst_port"] = serde_json::json!(udp_dst_port);
                    obj["inner_type"] = serde_json::json!(inner_type);
                }
                _ => {}
            }

            // Inner Ethernet
            if let Some(ref eth) = pkt.inner_ethernet {
                obj["inner_ethernet"] = serde_json::json!({
                    "src_mac": format_mac(&eth.src_mac),
                    "dst_mac": format_mac(&eth.dst_mac),
                    "ethertype": format!("0x{:04x}", eth.ethertype),
                    "ethertype_name": ethertype_name(eth.ethertype),
                    "vlan": eth.vlan.as_ref().map(|v| serde_json::json!({
                        "vid": v.vid,
                        "pcp": v.pcp,
                        "dei": v.dei,
                    })),
                });
            }

            // Deviations
            if !pkt.deviations.is_empty() {
                obj["deviations"] = serde_json::json!(pkt.deviations);
            }

            println!("{}", serde_json::to_string(&obj).unwrap_or_default());
        }
    }
}

fn render_summary_json(stats: &SessionStats) {
    let mut tunnels = serde_json::Map::new();
    for (tid, ts) in &stats.tunnels {
        let peers: Vec<String> = ts.peers.iter().map(|p| p.to_string()).collect();
        let ethertypes: serde_json::Map<String, serde_json::Value> = ts
            .inner_ethertypes
            .iter()
            .map(|(et, count)| {
                (
                    format!("0x{et:04x}"),
                    serde_json::json!({
                        "name": ethertype_name(*et),
                        "count": count,
                    }),
                )
            })
            .collect();

        tunnels.insert(
            tid.to_string(),
            serde_json::json!({
                "packets": ts.packet_count,
                "keepalives": ts.keepalive_count,
                "bytes": ts.byte_count,
                "peers": peers,
                "first_seen_us": ts.first_seen.as_micros() as u64,
                "last_seen_us": ts.last_seen.as_micros() as u64,
                "ethertypes": ethertypes,
            }),
        );
    }

    let obj = serde_json::json!({
        "type": "summary",
        "total_packets": stats.total_packets,
        "eoip_packets": stats.eoip_packets,
        "eoipv6_packets": stats.eoipv6_packets,
        "udp_encap_packets": stats.udp_encap_packets,
        "standard_gre_packets": stats.standard_gre_packets,
        "skipped_packets": stats.skipped_packets,
        "error_packets": stats.error_packets,
        "keepalive_packets": stats.keepalive_packets,
        "deviation_count": stats.deviation_count,
        "total_bytes": stats.total_bytes,
        "duration_us": stats.duration().map(|d| d.as_micros() as u64),
        "tunnels": tunnels,
    });

    println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
}

fn render_summary_text(stats: &SessionStats) {
    println!("{}", "═══ Session Summary ═══".bold());
    println!(
        "  Total packets:     {}",
        stats.total_packets.to_string().bold()
    );
    if stats.eoip_packets > 0 {
        println!("  EoIP (proto 47):   {}", stats.eoip_packets.to_string().cyan());
    }
    if stats.eoipv6_packets > 0 {
        println!(
            "  EoIPv6 (proto 97): {}",
            stats.eoipv6_packets.to_string().magenta()
        );
    }
    if stats.udp_encap_packets > 0 {
        println!(
            "  UDP encap:         {}",
            stats.udp_encap_packets.to_string().yellow()
        );
    }
    if stats.standard_gre_packets > 0 {
        println!(
            "  Standard GRE:      {}",
            stats.standard_gre_packets.to_string().dimmed()
        );
    }
    if stats.skipped_packets > 0 {
        println!(
            "  Skipped:           {}",
            stats.skipped_packets.to_string().dimmed()
        );
    }
    if stats.error_packets > 0 {
        println!(
            "  Errors:            {}",
            stats.error_packets.to_string().red()
        );
    }
    println!("  Keepalives:        {}", stats.keepalive_packets);
    println!("  Total bytes:       {}", stats.total_bytes);
    if let Some(dur) = stats.duration() {
        println!("  Capture duration:  {:.3}s", dur.as_secs_f64());
    }
    if stats.deviation_count > 0 {
        println!(
            "  Deviations:        {}",
            stats.deviation_count.to_string().red().bold()
        );
    }

    if !stats.tunnels.is_empty() {
        println!("\n{}", "─── Per-Tunnel Breakdown ───".bold());
        let mut tids: Vec<u16> = stats.tunnels.keys().copied().collect();
        tids.sort();
        for tid in tids {
            let ts = &stats.tunnels[&tid];
            let peers: Vec<String> = ts.peers.iter().map(|p| p.to_string()).collect();
            println!(
                "  Tunnel {}: {} packets ({} keepalive), {} bytes",
                tid.to_string().bold(),
                ts.packet_count,
                ts.keepalive_count,
                ts.byte_count,
            );
            println!("    Peers: {}", peers.join(", "));
            if !ts.inner_ethertypes.is_empty() {
                let types: Vec<String> = ts
                    .inner_ethertypes
                    .iter()
                    .map(|(et, c)| format!("{}(0x{:04x})x{}", ethertype_name(*et), et, c))
                    .collect();
                println!("    Inner: {}", types.join(", "));
            }
        }
    }
}
