# EoIP-rs Security Model

## 1. Design Philosophy

EoIP-rs provides **no built-in encryption or authentication**. Security is delegated to the underlying VPN transport (WireGuard, SSTP, ZeroTier, IPsec). This is a deliberate design choice:

- EoIP is always intended to run **inside** an encrypted VPN tunnel.
- Adding encryption would duplicate the VPN layer's work, adding latency and complexity.
- MikroTik's native EoIP has no security either — this is protocol-compatible behavior.
- Users who need standalone security should use WireGuard or IPsec as the outer transport.

---

## 2. Privilege Separation

The primary security mechanism is **privilege separation**: the packet-processing daemon runs with zero elevated privileges.

### Architecture

```
 ┌───────────────────────────────────────────────────────────┐
 │                   Privilege Boundary                      │
 │                                                           │
 │  ┌─────────────────┐         ┌─────────────────────────┐  │
 │  │  eoip-helper    │  SCM_   │  eoip-rs daemon         │  │
 │  │  (root)         │  RIGHTS │  (unprivileged)         │  │
 │  │                 │────────►│                         │  │
 │  │  Audit surface: │         │  Full attack surface:   │  │
 │  │  ~200 lines     │         │  ~5000+ lines           │  │
 │  │                 │         │                         │  │
 │  │  Can:           │         │  Can:                   │  │
 │  │  - open TAP     │         │  - read/write packets   │  │
 │  │  - open raw sock│         │  - serve gRPC API       │  │
 │  │  - set iface up │         │  - manage tunnels       │  │
 │  │                 │         │                         │  │
 │  │  Cannot:        │         │  Cannot:                │  │
 │  │  - process pkts │         │  - create TAP           │  │
 │  │  - serve API    │         │  - open raw sockets     │  │
 │  │  - access net   │         │  - modify iface config  │  │
 │  └─────────────────┘         └─────────────────────────┘  │
 └───────────────────────────────────────────────────────────┘
```

### Helper Security Properties

1. **Minimal code**: ~200 lines. Small enough for manual audit.
2. **No parsing**: Does not parse packet data. Only creates OS resources.
3. **No network exposure**: Communicates only via Unix socket with the daemon.
4. **Drops privileges**: After creating initial resources (MODE_EXIT) or drops to restricted scope (MODE_PERSIST).
5. **Validates requests**: In PERSIST mode, validates DaemonMsg requests against allowlists (e.g., max tunnel count, allowed interface name patterns).

### FD Passing via SCM_RIGHTS

File descriptors are passed using `sendmsg()`/`recvmsg()` with `SCM_RIGHTS` ancillary data over a Unix domain socket. This is a well-established privilege separation pattern used by OpenSSH, Chrome, and other security-critical software.

```
Helper creates: TAP fd, raw socket fd
    │
    │ sendmsg() with cmsg SCM_RIGHTS
    │ (fd numbers are translated by kernel)
    ▼
Daemon receives: new fd numbers referencing same kernel objects
    │
    │ Daemon has no capability to create these objects itself
    ▼
Daemon uses fds for read/write only
```

---

## 3. Threat Model

### 3.1 Attack Surface

| Surface | Exposure | Mitigations |
|---------|----------|-------------|
| Raw socket RX | All GRE/EtherIP packets on the host | Source IP validation, magic byte check, rate limiting |
| gRPC API | Network-accessible (configurable) | Bind to localhost by default, optional mTLS |
| Unix socket (helper) | Local only | File permissions (0600, owned by root) |
| TAP interface | Local bridge/apps | Standard Linux network namespace isolation |
| Config file | Local filesystem | File permissions (0640, owned by root:eoip) |

### 3.2 Threats and Mitigations

**Tunnel injection (spoofed packets):**
- An attacker on the same network sends crafted GRE packets with a valid tunnel ID.
- **Mitigation**: Source IP validation. Each tunnel is configured with a specific peer IP. Packets from unknown sources are dropped.
- **Residual risk**: If the attacker can spoof the peer's IP (e.g., on a shared LAN), they can inject frames. This is why EoIP must run inside a VPN.

**Tunnel ID brute-force:**
- Tunnel IDs are 16-bit (65536 values for EoIP, 4096 for EoIPv6).
- **Mitigation**: Source IP validation makes brute-force insufficient — attacker must also match the peer IP. Rate limiting on unknown-tunnel packets (1 log per 10s per unique key).
- **Note**: Tunnel ID is a demux key, not a security credential.

**Denial of service (packet flood):**
- Flood of GRE packets targeting the raw socket.
- **Mitigation**: recvmmsg batch processing limits CPU per packet. Global rate limiter on RX path (configurable, default: disabled). OS-level iptables/nftables rules recommended for production.
- **Mitigation**: Broadcast storm guard on TAP write (optional rate limit on broadcast/multicast frames).

**gRPC API abuse:**
- Unauthorized tunnel creation/deletion via the management API.
- **Mitigation**: Bind to `[::1]:50051` (localhost only) by default. Optional mTLS with client certificate validation. No authentication token mode (mTLS is the mechanism).

**Helper compromise:**
- If the helper is compromised, attacker gains root network capabilities.
- **Mitigation**: Helper is ~200 lines, no external dependencies beyond libc/nix. Runs with `--no-new-privs` (prctl). In MODE_EXIT, the helper process doesn't persist. Validates all DaemonMsg fields.

**Config file tampering:**
- Modified config could redirect tunnels to attacker-controlled endpoints.
- **Mitigation**: Config file should be owned by root:eoip with 0640 permissions. Daemon reads config at startup only (no hot-reload of peer addresses). gRPC tunnel creation requires API access.

---

## 4. Packet Validation

All received packets pass through a fail-fast validation pipeline before being delivered to a TAP interface:

```
 Packet arrives on raw socket
          │
          ▼
 ┌─ Check 1: Minimum length ────────────── Drop (runt)
 │
 ├─ Check 2: Magic bytes (EoIP) ────────── Drop (not EoIP)
 │           Version nibble (EoIPv6)
 │
 ├─ Check 3: Payload length sanity ──────── Drop (corrupted)
 │
 ├─ Check 4: Tunnel ID lookup ──────────── Drop (unknown tunnel)
 │           in DemuxTable
 │
 ├─ Check 5: Source IP matches ─────────── Drop (wrong peer)
 │           configured peer
 │
 ├─ Check 6: Ethernet frame minimum ────── Drop (no eth header)
 │           (>= 14 bytes)
 │
 └─ Check 7: Frame size vs MTU ─────────── Drop + warn (oversized)
          │
          ▼
 Deliver to TAP interface
```

Each check increments a specific counter (`rx_runt`, `rx_bad_magic`, `rx_unknown_tunnel`, `rx_bad_source`, etc.) for monitoring.

---

## 5. Rate Limiting

### RX Rate Limiting (Optional)

```toml
[security]
# Global RX rate limit (packets per second, 0 = disabled)
rx_rate_limit = 0

# Per-tunnel RX rate limit
per_tunnel_rx_rate_limit = 0

# Broadcast/multicast frame rate limit per tunnel
broadcast_rate_limit = 1000
```

When enabled, excess packets are dropped with counter increments. Rate limiting uses a token bucket algorithm with `AtomicU64` — no locks.

### Logging Rate Limiting

Security-relevant events (unknown tunnel, bad source, invalid header) are logged at `warn` level with rate limiting: at most 1 log line per 10 seconds per unique event key. This prevents log flooding during attacks.

---

## 6. gRPC API Security

### Default: Localhost Only

```toml
[api]
listen = "[::1]:50051"  # IPv6 loopback only
```

### Optional: mTLS

```toml
[api]
listen = "0.0.0.0:50051"
tls_cert = "/etc/eoip-rs/server.pem"
tls_key  = "/etc/eoip-rs/server-key.pem"
tls_ca   = "/etc/eoip-rs/ca.pem"  # Client CA for mutual auth
```

When `tls_ca` is set, the server requires client certificates signed by the specified CA. This provides both encryption and authentication for the management API.

---

## 7. Deployment Recommendations

1. **Always run EoIP inside a VPN tunnel.** EoIP provides zero confidentiality or integrity.
2. **Use the privilege-separated helper.** Don't run the daemon as root.
3. **Firewall GRE/EtherIP traffic.** Use iptables/nftables to allow GRE (proto 47) and EtherIP (proto 97) only from known peer IPs.
4. **Bind gRPC to localhost** unless remote management is required, in which case enable mTLS.
5. **Set config file permissions** to 0640 owned by root:eoip.
6. **Monitor** `rx_unknown_tunnel` and `rx_bad_source` counters for potential attacks.
7. **Use network namespaces** to isolate tunnel TAP interfaces from the host network if bridging is not required.
