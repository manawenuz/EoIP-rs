//! Interactive REPL with rustyline.

use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use crate::client::GrpcClient;
use crate::commands;
use crate::parse;

pub async fn run_repl(address: String, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = GrpcClient::connect(&address).await?;

    let mut rl = DefaultEditor::new()?;
    let history_path = dirs_next().map(|d| d.join(".eoip-cli_history"));
    if let Some(ref p) = history_path {
        let _ = rl.load_history(p);
    }

    println!("EoIP-rs CLI v{}", env!("CARGO_PKG_VERSION"));
    println!("Connected to {address}");
    println!("Type 'help' for commands, 'quit' to exit.");
    println!();

    loop {
        match rl.readline("[admin@eoip-rs] > ") {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(line);

                match parse::parse_line(line) {
                    Ok(parse::Command::Quit) => break,
                    Ok(cmd) => {
                        if let Err(e) = commands::execute(&mut client, cmd, json).await {
                            eprintln!("error: {e}");
                        }
                    }
                    Err(e) => eprintln!("parse error: {e}"),
                }
            }
            Err(ReadlineError::Interrupted | ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("readline error: {e}");
                break;
            }
        }
    }

    if let Some(ref p) = history_path {
        let _ = rl.save_history(p);
    }

    println!("bye.");
    Ok(())
}

fn dirs_next() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}
