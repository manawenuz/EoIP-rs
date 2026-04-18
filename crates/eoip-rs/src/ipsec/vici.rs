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
                    Ok(c) => {
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

    /// Send a VICI command and check the response for `success=yes`.
    pub fn call(&mut self, command: &str, msg: &Message) -> Result<Message, String> {
        let resp = self
            .inner
            .call(command, msg)
            .map_err(|e| format!("VICI {command}: {e}"))?;

        check_vici_response(command, &resp)?;
        Ok(resp)
    }

    /// Send a streaming VICI command, ignoring intermediate events.
    pub fn call_streaming_ignore(
        &mut self,
        command: &str,
        msg: &Message,
    ) -> Result<Message, String> {
        let resp = self
            .inner
            .call_streaming(command, msg, |event_name, event_msg| {
                // Log control-log events at debug level
                if event_name == "control-log" {
                    for el in event_msg.elements() {
                        if let rustici::wire::Element::KeyValue(ref k, ref v) = el {
                            if k == "msg" {
                                let msg_str = String::from_utf8_lossy(v);
                                tracing::debug!(event = event_name, "VICI: {}", msg_str);
                            }
                        }
                    }
                }
            })
            .map_err(|e| format!("VICI {command}: {e}"))?;

        check_vici_response(command, &resp)?;
        Ok(resp)
    }

    /// Check if an IKE SA is established for a given connection name.
    ///
    /// The `list-sas` streaming response emits events with nested sections.
    /// We look for the connection name section containing `state=ESTABLISHED`.
    pub fn has_established_sa(&mut self, conn_name: &str) -> bool {
        let msg = Message::new().kv_str("ike", conn_name);
        let mut found = false;

        let _ = self
            .inner
            .call_streaming("list-sas", &msg, |_name, sa_msg| {
                // The SA event contains nested sections. We check if any
                // KeyValue "state" has value "ESTABLISHED" anywhere in the
                // message — this covers both IKE SA state and child SA state.
                let mut in_conn = false;
                for el in sa_msg.elements() {
                    match el {
                        rustici::wire::Element::SectionStart(ref name) => {
                            if name == conn_name {
                                in_conn = true;
                            }
                        }
                        rustici::wire::Element::KeyValue(ref k, ref v) => {
                            if k == "state" {
                                let state = String::from_utf8_lossy(v);
                                if state == "ESTABLISHED" || state == "INSTALLED" {
                                    found = true;
                                }
                            }
                            // Also check uniqueid to confirm we got a real SA
                            if in_conn && k == "uniqueid" {
                                found = true;
                            }
                        }
                        _ => {}
                    }
                }
            });

        found
    }
}

/// Check a VICI response message for `success=yes`. Log and return error otherwise.
fn check_vici_response(command: &str, resp: &Message) -> Result<(), String> {
    let mut success = None;
    let mut errmsg = None;

    for el in resp.elements() {
        if let rustici::wire::Element::KeyValue(ref k, ref v) = el {
            match k.as_str() {
                "success" => success = Some(String::from_utf8_lossy(v).to_string()),
                "errmsg" => errmsg = Some(String::from_utf8_lossy(v).to_string()),
                _ => {}
            }
        }
    }

    match success.as_deref() {
        Some("yes") => Ok(()),
        Some("no") => {
            let err = errmsg.unwrap_or_else(|| "unknown error".to_string());
            tracing::error!(command, %err, "VICI command failed");
            Err(format!("VICI {command} failed: {err}"))
        }
        _ => {
            // Some commands don't return success/errmsg (like list-sas)
            tracing::debug!(command, "VICI response has no success field (OK for query commands)");
            Ok(())
        }
    }
}
