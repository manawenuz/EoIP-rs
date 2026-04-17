mod client;
mod commands;
mod output;
mod parse;
mod repl;

use clap::Parser;

/// MikroTik-style CLI for managing EoIP-rs tunnels.
///
/// Usage:
///   eoip-cli                                    # Interactive REPL
///   eoip-cli /interface/eoip/print              # One-shot command
///   eoip-cli /interface/eoip/print detail       # Detailed view
///   eoip-cli --json /interface/eoip/stats       # JSON output
#[derive(Parser, Debug)]
#[command(name = "eoip-cli", version)]
struct Args {
    /// gRPC endpoint address
    #[arg(long, default_value = "http://[::1]:50051")]
    address: String,

    /// Output as JSON
    #[arg(long)]
    json: bool,

    /// Command tokens (e.g., /interface/eoip/print detail)
    command: Vec<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    if args.command.is_empty() {
        // REPL mode
        if let Err(e) = repl::run_repl(args.address, args.json).await {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    } else {
        // One-shot mode
        let cmd = match parse::parse_command(&args.command) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("parse error: {e}");
                std::process::exit(1);
            }
        };

        // Help and quit don't need a connection
        if matches!(cmd, parse::Command::Help) {
            commands::execute_local(cmd);
            return;
        }

        let mut client = match client::GrpcClient::connect(&args.address).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("connection error: {e}");
                std::process::exit(1);
            }
        };

        if let Err(e) = commands::execute(&mut client, cmd, args.json).await {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
