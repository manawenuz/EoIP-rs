//! MikroTik-style command parser.
//!
//! Accepts both full paths (`/interface/eoip/print`) and bare commands (`print`).

use std::collections::HashMap;

#[derive(Debug)]
pub enum Command {
    Print { detail: bool, filter: Option<Filter> },
    Add { tunnel_id: u32, remote: String, local: Option<String>, name: Option<String>, mtu: Option<u32> },
    Remove { tunnel_id: u32 },
    Enable { tunnel_id: u32 },
    Disable { tunnel_id: u32 },
    Set { tunnel_id: u32, params: HashMap<String, String> },
    Monitor,
    Stats { tunnel_id: Option<u32> },
    Health,
    Help,
    Quit,
}

#[derive(Debug)]
pub enum Filter {
    TunnelId(u32),
    Name(String),
}

/// Parse a sequence of tokens into a Command.
pub fn parse_command(tokens: &[String]) -> Result<Command, String> {
    if tokens.is_empty() {
        return Err("empty command".into());
    }

    // Flatten path-style input: "/interface/eoip/print" → ["print"]
    // Also handle space-separated: "/interface" "eoip" "print" → ["print"]
    let normalized = normalize_tokens(tokens);
    if normalized.is_empty() {
        return Err("empty command after normalization".into());
    }

    let verb = normalized[0].to_lowercase();
    let rest = &normalized[1..];

    match verb.as_str() {
        "print" => parse_print(rest),
        "add" => parse_add(rest),
        "remove" | "delete" => parse_remove(rest),
        "enable" => parse_enable_disable(rest, true),
        "disable" => parse_enable_disable(rest, false),
        "set" => parse_set(rest),
        "monitor" | "watch" => Ok(Command::Monitor),
        "stats" | "statistics" => parse_stats(rest),
        "health" | "check" => Ok(Command::Health),
        "help" | "?" => Ok(Command::Help),
        "quit" | "exit" => Ok(Command::Quit),
        _ => Err(format!("unknown command: {verb}")),
    }
}

/// Parse REPL input line into tokens and then a Command.
pub fn parse_line(line: &str) -> Result<Command, String> {
    let tokens: Vec<String> = shell_split(line);
    if tokens.is_empty() {
        return Err("empty input".into());
    }
    parse_command(&tokens)
}

fn normalize_tokens(tokens: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    for token in tokens {
        // Split path segments: "/interface/eoip/print" → ["interface", "eoip", "print"]
        for segment in token.split('/') {
            let s = segment.trim();
            if !s.is_empty() {
                result.push(s.to_string());
            }
        }
    }

    // Strip known path prefixes
    let prefixes = &["interface", "eoip", "system"];
    let mut start = 0;
    for (i, token) in result.iter().enumerate() {
        if prefixes.contains(&token.to_lowercase().as_str()) {
            start = i + 1;
        } else {
            break;
        }
    }

    result[start..].to_vec()
}

fn shell_split(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut quote_char = '"';

    for ch in line.chars() {
        if in_quote {
            if ch == quote_char {
                in_quote = false;
            } else {
                current.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            in_quote = true;
            quote_char = ch;
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn parse_kv(tokens: &[String]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for token in tokens {
        if let Some((k, v)) = token.split_once('=') {
            map.insert(k.to_lowercase(), v.to_string());
        }
    }
    map
}

fn parse_print(rest: &[String]) -> Result<Command, String> {
    let mut detail = false;
    let mut filter = None;
    let mut i = 0;

    while i < rest.len() {
        match rest[i].to_lowercase().as_str() {
            "detail" => detail = true,
            "where" => {
                // Parse filter: "where tunnel-id=100" or "where name=foo"
                i += 1;
                if i >= rest.len() {
                    return Err("'where' requires a condition (e.g., tunnel-id=100)".into());
                }
                let kv = parse_kv(&rest[i..]);
                if let Some(tid) = kv.get("tunnel-id") {
                    filter = Some(Filter::TunnelId(
                        tid.parse().map_err(|_| format!("invalid tunnel-id: {tid}"))?,
                    ));
                } else if let Some(name) = kv.get("name") {
                    filter = Some(Filter::Name(name.clone()));
                }
                break;
            }
            _ => {
                // Might be a filter shorthand: "print 100"
                if let Ok(tid) = rest[i].parse::<u32>() {
                    filter = Some(Filter::TunnelId(tid));
                }
            }
        }
        i += 1;
    }

    Ok(Command::Print { detail, filter })
}

fn parse_add(rest: &[String]) -> Result<Command, String> {
    let kv = parse_kv(rest);
    let tunnel_id = kv
        .get("tunnel-id")
        .ok_or("add requires tunnel-id=<id>")?
        .parse::<u32>()
        .map_err(|_| "invalid tunnel-id")?;
    let remote = kv
        .get("remote-address")
        .or(kv.get("remote"))
        .ok_or("add requires remote-address=<ip>")?
        .clone();
    let local = kv.get("local-address").or(kv.get("local")).cloned();
    let name = kv.get("name").cloned();
    let mtu = kv.get("mtu").map(|v| v.parse::<u32>()).transpose().map_err(|_| "invalid mtu")?;

    Ok(Command::Add { tunnel_id, remote, local, name, mtu })
}

fn parse_remove(rest: &[String]) -> Result<Command, String> {
    if rest.is_empty() {
        return Err("remove requires a tunnel-id".into());
    }
    let tid = rest[0]
        .parse::<u32>()
        .or_else(|_| {
            // Try key=value
            parse_kv(rest)
                .get("tunnel-id")
                .ok_or("invalid".to_string())
                .and_then(|v| v.parse::<u32>().map_err(|e| e.to_string()))
        })
        .map_err(|_| format!("invalid tunnel-id: {}", rest[0]))?;
    Ok(Command::Remove { tunnel_id: tid })
}

fn parse_enable_disable(rest: &[String], enable: bool) -> Result<Command, String> {
    if rest.is_empty() {
        return Err(format!("{} requires a tunnel-id", if enable { "enable" } else { "disable" }));
    }
    let tid = rest[0]
        .parse::<u32>()
        .map_err(|_| format!("invalid tunnel-id: {}", rest[0]))?;
    if enable {
        Ok(Command::Enable { tunnel_id: tid })
    } else {
        Ok(Command::Disable { tunnel_id: tid })
    }
}

fn parse_set(rest: &[String]) -> Result<Command, String> {
    if rest.is_empty() {
        return Err("set requires a tunnel-id and key=value pairs".into());
    }
    let tid = rest[0]
        .parse::<u32>()
        .map_err(|_| format!("invalid tunnel-id: {}", rest[0]))?;
    let params = parse_kv(&rest[1..]);
    if params.is_empty() {
        return Err("set requires at least one key=value pair".into());
    }
    Ok(Command::Set { tunnel_id: tid, params })
}

fn parse_stats(rest: &[String]) -> Result<Command, String> {
    if rest.is_empty() {
        return Ok(Command::Stats { tunnel_id: None });
    }
    let tid = rest[0]
        .parse::<u32>()
        .map_err(|_| format!("invalid tunnel-id: {}", rest[0]))?;
    Ok(Command::Stats { tunnel_id: Some(tid) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parse_bare_print() {
        let cmd = parse_command(&t("print")).unwrap();
        assert!(matches!(cmd, Command::Print { detail: false, filter: None }));
    }

    #[test]
    fn parse_full_path_print() {
        let cmd = parse_command(&t("/interface/eoip/print")).unwrap();
        assert!(matches!(cmd, Command::Print { detail: false, filter: None }));
    }

    #[test]
    fn parse_print_detail() {
        let cmd = parse_command(&t("print detail")).unwrap();
        assert!(matches!(cmd, Command::Print { detail: true, filter: None }));
    }

    #[test]
    fn parse_print_where() {
        let cmd = parse_command(&t("print where tunnel-id=100")).unwrap();
        match cmd {
            Command::Print { filter: Some(Filter::TunnelId(100)), .. } => {}
            _ => panic!("expected TunnelId filter"),
        }
    }

    #[test]
    fn parse_add() {
        let cmd = parse_command(&t("add tunnel-id=100 remote-address=1.2.3.4 name=tun1")).unwrap();
        match cmd {
            Command::Add { tunnel_id: 100, ref remote, ref name, .. } => {
                assert_eq!(remote, "1.2.3.4");
                assert_eq!(name.as_deref(), Some("tun1"));
            }
            _ => panic!("expected Add"),
        }
    }

    #[test]
    fn parse_remove() {
        let cmd = parse_command(&t("remove 100")).unwrap();
        assert!(matches!(cmd, Command::Remove { tunnel_id: 100 }));
    }

    #[test]
    fn parse_enable() {
        let cmd = parse_command(&t("enable 100")).unwrap();
        assert!(matches!(cmd, Command::Enable { tunnel_id: 100 }));
    }

    #[test]
    fn parse_stats_global() {
        let cmd = parse_command(&t("stats")).unwrap();
        assert!(matches!(cmd, Command::Stats { tunnel_id: None }));
    }

    #[test]
    fn parse_stats_tunnel() {
        let cmd = parse_command(&t("stats 100")).unwrap();
        assert!(matches!(cmd, Command::Stats { tunnel_id: Some(100) }));
    }

    #[test]
    fn parse_health() {
        let cmd = parse_command(&t("health")).unwrap();
        assert!(matches!(cmd, Command::Health));
    }

    #[test]
    fn parse_quit() {
        let cmd = parse_command(&t("quit")).unwrap();
        assert!(matches!(cmd, Command::Quit));
    }

    #[test]
    fn parse_path_with_spaces() {
        let tokens: Vec<String> = vec!["/interface", "eoip", "print", "detail"]
            .into_iter().map(String::from).collect();
        let cmd = parse_command(&tokens).unwrap();
        assert!(matches!(cmd, Command::Print { detail: true, .. }));
    }
}
