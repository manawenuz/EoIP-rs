# IPsec Encryption Guide

EoIP-rs supports transparent IPsec encryption via MikroTik's `ipsec-secret` mechanism. When configured, the tunnel negotiates IKEv1 main mode with ESP transport mode encryption, byte-compatible with MikroTik RouterOS.

## Overview

Setting `ipsec_secret` on a tunnel triggers automatic IPsec SA negotiation with the remote peer using the strongSwan IKE daemon. The EoIP-rs daemon communicates with strongSwan via the VICI protocol (`rustici` crate) to install and monitor security associations.

**Encryption parameters (matching MikroTik CHR live capture):**

| Parameter | Value |
|-----------|-------|
| IKE version | IKEv1 main mode |
| Phase 1 (IKE SA) | AES-256-CBC / SHA1 / modp2048 |
| Phase 2 (ESP) | AES-256-CBC / SHA1, transport mode |
| Phase 1 lifetime | 24 hours |
| Phase 2 lifetime | 30 minutes |
| Authentication | Pre-shared key (the `ipsec_secret` value) |

## Prerequisites

### strongSwan Installation

strongSwan is an external dependency. EoIP-rs does not bundle or install it.

```bash
# Debian / Ubuntu
sudo apt install strongswan-charon strongswan-swanctl

# Verify VICI socket exists after starting
sudo systemctl enable --now strongswan
ls /run/strongswan/charon.vici || ls /var/run/charon.vici
```

EoIP-rs checks both VICI socket paths:
- `/run/strongswan/charon.vici` (preferred)
- `/var/run/charon.vici` (fallback)

### Graceful Fallback

If strongSwan is not installed or not running, the tunnel still operates **unencrypted** with a warning in the log:

```
WARN  ipsec: strongSwan VICI socket not found, tunnel 100 running without encryption
```

No crash, no failure -- just unencrypted GRE as before.

## Configuration

### EoIP-rs Side (TOML)

```toml
[[tunnel]]
tunnel_id = 100
local = "203.0.113.10"
remote = "203.0.113.20"
ipsec_secret = "SecretPass"
```

### EoIP-rs Side (CLI)

```bash
eoip-cli add tunnel-id=100 remote-address=203.0.113.20 \
  local-address=203.0.113.10 ipsec-secret=SecretPass
```

### MikroTik Side (RouterOS)

```routeros
/interface eoip add name=eoip-linux remote-address=203.0.113.10 \
  tunnel-id=100 ipsec-secret=SecretPass allow-fast-path=no
```

**Important:** `allow-fast-path=no` is required on MikroTik when using IPsec. Fast-path bypasses the IPsec engine and causes packets to be sent unencrypted.

### Both Sides Must Match

| Parameter | EoIP-rs | MikroTik |
|-----------|---------|----------|
| Tunnel ID | `tunnel_id = 100` | `tunnel-id=100` |
| Remote IP | `remote = "203.0.113.20"` | `remote-address=203.0.113.10` |
| PSK | `ipsec_secret = "SecretPass"` | `ipsec-secret=SecretPass` |

## Wire Format

### Without IPsec

```
[ IP (20B) ][ GRE/EoIP (8B) ][ Inner Ethernet (14B+) ][ Payload ]
```

### With IPsec (ESP Transport Mode)

```
[ IP (20B) ][ ESP hdr (8B) ][ ESP IV (16B) ][ GRE/EoIP (8B) ][ Inner Ethernet (14B+) ][ Payload ][ ESP pad (2B) ][ ESP auth (12B) ]
```

ESP adds 38 bytes of overhead:
- 8 bytes ESP header (SPI + sequence number)
- 16 bytes IV (AES-256-CBC initialization vector)
- 2 bytes padding/next-header
- 12 bytes authentication tag (HMAC-SHA1-96)

## MTU Impact

IPsec ESP overhead reduces the usable overlay MTU by 38 bytes.

| Path | Path MTU | Overlay MTU (no IPsec) | Overlay MTU (with IPsec) |
|------|----------|------------------------|--------------------------|
| Direct Ethernet | 1500 | 1458 | 1420 |
| WireGuard | 1420 | 1378 | 1340 |
| PPPoE | 1492 | 1450 | 1412 |

EoIP-rs auto-adjusts the TAP interface MTU when `ipsec_secret` is configured:
- Without IPsec: `path_mtu - 42` (e.g., 1500 - 42 = 1458)
- With IPsec: `path_mtu - 42 - 38` (e.g., 1500 - 80 = 1420)

## SA Monitoring and Rekeying

### Automatic Rekeying

strongSwan handles rekeying transparently:

| Phase | Lifetime | Rekey Margin |
|-------|----------|--------------|
| Phase 1 (IKE SA) | 24 hours | strongSwan default |
| Phase 2 (ESP SA) | 30 minutes | strongSwan default |

### SA Health Monitor

EoIP-rs monitors the IPsec SA every 60 seconds. If the SA is lost (peer reboot, network blip), the daemon automatically re-initiates negotiation via VICI. No manual intervention required.

### CLI Status

```
$ eoip-cli print detail
 0  R name="eoip100" tunnel-id=100 local-address=203.0.113.10
      remote-address=203.0.113.20 mtu=1420 actual-mtu=1420
      keepalive=10s,100s enabled=yes state=active
      ipsec=yes ipsec-active=yes
```

Fields:
- `ipsec=yes` -- IPsec is configured for this tunnel
- `ipsec-active=yes` -- ESP SA is currently established
- `ipsec-active=no` -- SA lost, renegotiation pending

## Verification

### Check SA Status (strongSwan)

```bash
sudo swanctl --list-sas
# Should show the SA with AES_CBC-256/HMAC_SHA1_96 and correct peer IPs
```

### Verify ESP Packets on Wire

```bash
# Should see ESP (protocol 50) instead of GRE (protocol 47)
sudo tcpdump -i eth0 esp -c 5

# Confirm no unencrypted GRE leaks
sudo tcpdump -i eth0 'ip proto 47' -c 5
# Should show 0 packets (all GRE is inside ESP)
```

### EoIP-rs CLI

```bash
eoip-cli print detail    # Check ipsec=yes ipsec-active=yes
eoip-cli stats 100       # Traffic counters should be incrementing
```

### MikroTik Side

```routeros
/interface eoip print detail
# Should show: running=yes

/ip ipsec active-peers print
# Should show the peer with established Phase 1

/ip ipsec installed-sa print
# Should show ESP SAs with AES-256-CBC
```

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `ipsec-active=no` persists | strongSwan not running | `sudo systemctl start strongswan` |
| `VICI socket not found` warning | strongSwan not installed | `apt install strongswan-charon strongswan-swanctl` |
| SA establishes then drops | PSK mismatch | Verify `ipsec_secret` matches on both sides exactly |
| MikroTik shows no IPsec SA | `allow-fast-path=no` missing | Add `allow-fast-path=no` to MikroTik EoIP interface |
| Throughput lower than expected | ESP encryption overhead | Normal: ~230 Mbps encrypted vs higher unencrypted |
| Phase 2 rekey fails | Firewall blocking UDP 500/4500 | Allow IKE ports between peers |
| MTU still shows 1458 with IPsec | Auto-MTU not detecting ESP | Set `mtu = 1420` explicitly |
| One side encrypted, other not | Only one side has secret | Both sides must have matching `ipsec-secret`/`ipsec_secret` |

## Performance

Tested between two Hetzner CX23 VMs with MikroTik CHR 7.18.2:

- **Encrypted throughput:** ~230 Mbps (AES-256-CBC ESP transport mode)
- **Overhead:** 38 bytes per packet (ESP encapsulation)
- **Latency impact:** Negligible (< 0.1 ms added)

## See Also

- [Installation Guide](INSTALL.md) -- strongSwan as optional dependency
- [MikroTik Interop Guide](MIKROTIK.md) -- MikroTik-side IPsec configuration
- [PMTUD Guide](PMTUD.md) -- MTU auto-detection with IPsec overhead
- [CLI Reference](CLI.md) -- `ipsec-secret` parameter on `add` command
