//! VICI client wrapper around the `rustici` crate.

use std::time::Duration;

use rustici::{Client, Message};

/// Default VICI socket paths (tried in order).
const VICI_SOCKET_PATHS: &[&str] = &[
    "/run/strongswan/charon.vici",
    "/var/run/charon.vici",
    "/var/run/strongswan/charon.vici",
];

/// Wrapper around `rustici::Client` for strongSwan VICI communication.
pub struct ViciClient {
    inner: Client,
}

impl ViciClient {
    /// Connect to the strongSwan VICI socket.
    ///
    /// Tries several common socket paths. Returns an error if none are available.
    pub fn connect() -> Result<Self, String> {
        for path in VICI_SOCKET_PATHS {
            if std::path::Path::new(path).exists() {
                match Client::connect(path) {
                    Ok(mut c) => {
                        let _ = c.set_read_timeout(Some(Duration::from_secs(10)));
                        let _ = c.set_write_timeout(Some(Duration::from_secs(5)));
                        tracing::debug!(path, "connected to VICI socket");
                        return Ok(Self { inner: c });
                    }
                    Err(e) => {
                        tracing::debug!(path, %e, "VICI connect failed, trying next");
                    }
                }
            }
        }

        Err(format!(
            "no VICI socket found at: {}",
            VICI_SOCKET_PATHS.join(", ")
        ))
    }

    /// Send a simple VICI command (request → response).
    pub fn call(&mut self, command: &str, msg: &Message) -> Result<Message, String> {
        self.inner
            .call(command, msg)
            .map_err(|e| format!("VICI {command}: {e}"))
    }

    /// Send a streaming VICI command, ignoring intermediate events.
    pub fn call_streaming_ignore(
        &mut self,
        command: &str,
        msg: &Message,
    ) -> Result<Message, String> {
        self.inner
            .call_streaming(command, msg, |_event_name, _event_msg| {
                // Ignore intermediate events (control-log, etc.)
            })
            .map_err(|e| format!("VICI {command}: {e}"))
    }

    /// Check if an SA is established for a given connection name.
    pub fn has_established_sa(&mut self, conn_name: &str) -> bool {
        let msg = Message::new().kv_str("ike", conn_name);
        let mut found = false;

        let _ = self.inner.call_streaming("list-sas", &msg, |_name, sa_msg| {
            // Look for "state" = "ESTABLISHED" in the SA elements
            for el in sa_msg.elements() {
                if let rustici::Element::KeyValue(ref k, ref v) = el {
                    if k == "state" {
                        let state = String::from_utf8_lossy(v);
                        if state == "ESTABLISHED" {
                            found = true;
                        }
                    }
                }
            }
        });

        found
    }
}
