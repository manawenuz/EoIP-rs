//! Command execution — dispatches parsed commands to gRPC calls.

use eoip_api::*;

use crate::client::GrpcClient;
use crate::output;
use crate::parse::{Command, Filter};

/// Execute commands that don't need a gRPC connection.
pub fn execute_local(cmd: Command) {
    match cmd {
        Command::Help => print_help(),
        _ => {}
    }
}

pub async fn execute(client: &mut GrpcClient, cmd: Command, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        Command::Print { detail, filter } => cmd_print(client, detail, filter, json).await,
        Command::Add { tunnel_id, remote, local, name, mtu } => {
            cmd_add(client, tunnel_id, remote, local, name, mtu, json).await
        }
        Command::Remove { tunnel_id } => cmd_remove(client, tunnel_id).await,
        Command::Enable { tunnel_id } => cmd_enable_disable(client, tunnel_id, true, json).await,
        Command::Disable { tunnel_id } => cmd_enable_disable(client, tunnel_id, false, json).await,
        Command::Set { tunnel_id, params } => cmd_set(client, tunnel_id, params, json).await,
        Command::Monitor => cmd_monitor(client).await,
        Command::Stats { tunnel_id } => cmd_stats(client, tunnel_id, json).await,
        Command::Health => cmd_health(client, json).await,
        Command::Help => { print_help(); Ok(()) }
        Command::Quit => Ok(()),
    }
}

async fn cmd_print(
    client: &mut GrpcClient,
    detail: bool,
    filter: Option<Filter>,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let resp = client.tunnels.list_tunnels(ListTunnelsRequest {}).await?;
    let mut tunnels = resp.into_inner().tunnels;

    // Apply filter
    if let Some(f) = filter {
        tunnels.retain(|t| match &f {
            Filter::TunnelId(id) => t.tunnel_id == *id,
            Filter::Name(name) => t.iface_name == *name,
        });
    }

    if json {
        output::print_json(&tunnels);
    } else if detail {
        output::print_detail(&tunnels);
    } else {
        output::print_table(&tunnels);
    }
    Ok(())
}

async fn cmd_add(
    client: &mut GrpcClient,
    tunnel_id: u32,
    remote: String,
    local: Option<String>,
    name: Option<String>,
    mtu: Option<u32>,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let req = CreateTunnelRequest {
        tunnel_id,
        local_addr: local.unwrap_or_else(|| "0.0.0.0".into()),
        remote_addr: remote,
        iface_name: name.unwrap_or_else(|| format!("eoip{tunnel_id}")),
        mtu: mtu.unwrap_or(1458),
    };
    let resp = client.tunnels.create_tunnel(req).await?;
    let tunnel = resp.into_inner().tunnel.unwrap();
    if json {
        output::print_json(&tunnel);
    } else {
        output::print_detail(&[tunnel]);
    }
    Ok(())
}

async fn cmd_remove(client: &mut GrpcClient, tunnel_id: u32) -> Result<(), Box<dyn std::error::Error>> {
    client.tunnels.delete_tunnel(DeleteTunnelRequest { tunnel_id }).await?;
    println!("removed tunnel {tunnel_id}");
    Ok(())
}

async fn cmd_enable_disable(
    client: &mut GrpcClient,
    tunnel_id: u32,
    enabled: bool,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let req = UpdateTunnelRequest {
        tunnel_id,
        enabled: Some(enabled),
        mtu: None,
        keepalive_interval_secs: None,
        keepalive_timeout_secs: None,
    };
    let resp = client.tunnels.update_tunnel(req).await?;
    let tunnel = resp.into_inner().tunnel.unwrap();
    if json {
        output::print_json(&tunnel);
    } else {
        let verb = if enabled { "enabled" } else { "disabled" };
        println!("{verb} tunnel {tunnel_id}");
        output::print_detail(&[tunnel]);
    }
    Ok(())
}

async fn cmd_set(
    client: &mut GrpcClient,
    tunnel_id: u32,
    params: std::collections::HashMap<String, String>,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let req = UpdateTunnelRequest {
        tunnel_id,
        enabled: params.get("enabled").map(|v| v == "yes" || v == "true"),
        mtu: params.get("mtu").and_then(|v| v.parse().ok()),
        keepalive_interval_secs: params.get("keepalive-interval").and_then(|v| v.parse().ok()),
        keepalive_timeout_secs: params.get("keepalive-timeout").and_then(|v| v.parse().ok()),
    };
    let resp = client.tunnels.update_tunnel(req).await?;
    let tunnel = resp.into_inner().tunnel.unwrap();
    if json {
        output::print_json(&tunnel);
    } else {
        output::print_detail(&[tunnel]);
    }
    Ok(())
}

async fn cmd_monitor(client: &mut GrpcClient) -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = client.tunnels.watch_tunnels(WatchTunnelsRequest {}).await?.into_inner();
    println!("watching tunnel events (Ctrl-C to stop)...");
    while let Some(event) = stream.message().await? {
        let event_type = match event.event_type {
            1 => "CREATED",
            2 => "UPDATED",
            3 => "DELETED",
            4 => "STATE_CHANGED",
            _ => "UNKNOWN",
        };
        if let Some(t) = &event.tunnel {
            println!(
                "[{}] {} tunnel-id={} name=\"{}\" state={}",
                format_ts_ms(event.timestamp_ms),
                event_type,
                t.tunnel_id,
                t.iface_name,
                t.state
            );
        }
    }
    Ok(())
}

async fn cmd_stats(
    client: &mut GrpcClient,
    tunnel_id: Option<u32>,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(tid) = tunnel_id {
        let resp = client.stats.get_stats(GetStatsRequest { tunnel_id: tid }).await?;
        let stats = resp.into_inner().stats.unwrap();
        if json {
            output::print_json(&stats);
        } else {
            output::print_stats(&stats);
        }
    } else {
        let resp = client.stats.get_global_stats(GetGlobalStatsRequest {}).await?;
        let stats = resp.into_inner().stats.unwrap();
        if json {
            output::print_json(&stats);
        } else {
            output::print_global_stats(&stats);
        }
    }
    Ok(())
}

async fn cmd_health(client: &mut GrpcClient, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let resp = client
        .health
        .check(HealthCheckRequest { service: String::new() })
        .await?;
    let status = resp.into_inner().status;
    if json {
        output::print_json(&serde_json::json!({ "status": status }));
    } else {
        let name = match status {
            1 => "SERVING".green().to_string(),
            2 => "NOT_SERVING".red().to_string(),
            _ => "UNKNOWN".yellow().to_string(),
        };
        println!("status: {name}");
    }
    Ok(())
}

fn print_help() {
    println!("EoIP-rs CLI — MikroTik-style tunnel management");
    println!();
    println!("Commands (prefix /interface/eoip/ optional):");
    println!("  print                          List all tunnels");
    println!("  print detail                   Detailed tunnel view");
    println!("  print where tunnel-id=<id>     Filter by tunnel ID");
    println!("  add tunnel-id=<id> remote-address=<ip> [name=<n>] [local-address=<ip>] [mtu=<m>]");
    println!("  remove <tunnel-id>             Delete a tunnel");
    println!("  enable <tunnel-id>             Enable a tunnel");
    println!("  disable <tunnel-id>            Disable a tunnel");
    println!("  set <tunnel-id> key=value ...  Modify tunnel properties");
    println!("  monitor                        Stream tunnel events");
    println!("  stats                          Global statistics");
    println!("  stats <tunnel-id>              Per-tunnel statistics");
    println!();
    println!("  /system/health                 Health check");
    println!("  help                           This help");
    println!("  quit                           Exit REPL");
}

fn format_ts_ms(ms: i64) -> String {
    if ms == 0 { return "?".into(); }
    let secs = ms / 1000;
    let micros = (ms % 1000) * 1000;
    format!("{secs}.{micros:06}")
}

use colored::Colorize;
