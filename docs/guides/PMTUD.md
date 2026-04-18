# Path MTU Discovery & Auto-MTU

EoIP-rs automatically detects the correct overlay MTU to avoid IP fragmentation on the encapsulated path. This matches MikroTik RouterOS behavior.

## How It Works

### Overhead Calculation

Every EoIP packet adds 42 bytes of overhead to the inner Ethernet frame:

```
overlay_mtu = path_mtu - 20 (IP header) - 8 (GRE/EoIP header) - 14 (inner Ethernet)
            = path_mtu - 42
```

| Path | Path MTU | Overlay MTU |
|------|----------|-------------|
| Direct Ethernet | 1500 | 1458 |
| Direct Ethernet + IPsec | 1500 | 1420 |
| WireGuard | 1420 | 1378 |
| WireGuard + IPsec | 1420 | 1340 |
| PPPoE | 1492 | 1450 |
| PPPoE + WireGuard | 1412 | 1370 |

> **IPsec note:** When `ipsec_secret` is configured, ESP encryption adds 38 bytes of overhead (8 ESP header + 16 AES-CBC IV + 2 padding + 12 SHA1 auth tag). EoIP-rs subtracts this automatically from the overlay MTU.

### Detection Stages

EoIP-rs uses a three-stage fallback chain:

1. **PMTUD Probing** (best) — sends ICMP Echo with DF=1 at varying sizes using binary search. Discovers the actual path MTU regardless of intermediate hops, NAT, or VPN encapsulation. Re-probes every 10 minutes to detect path changes.

2. **Interface MTU Detection** (fallback) — looks up the outgoing interface for the remote IP via the OS routing table and reads its MTU. Works on direct paths but may overestimate when there are lower-MTU hops between the local interface and the peer.

3. **Default 1458** (last resort) — assumes standard 1500-byte Ethernet. Used only if both detection methods fail (e.g., ICMP blocked and routing table unreadable).

### What MikroTik Does

MikroTik RouterOS shows three MTU values on EoIP interfaces:

| Field | Meaning |
|-------|---------|
| **MTU** | Configured MTU (blank = auto) |
| **Actual MTU** | Discovered overlay MTU after subtracting encapsulation overhead |
| **L2 MTU** | Maximum L2 frame the interface can accept (always 65535 for EoIP) |

MikroTik's L2 MTU of 65535 means the interface accepts arbitrarily large Ethernet frames. If a frame exceeds the Actual MTU, MikroTik IP-fragments the outer GRE packet (when `Dont Fragment = no`). The remote kernel reassembles the fragments.

EoIP-rs takes a different approach: the TAP interface MTU is set to the discovered overlay MTU, so the kernel enforces the limit. Oversized frames are handled by the kernel's normal MTU enforcement (ICMP "fragmentation needed" for routed traffic, or drop for bridged). This avoids IP fragmentation entirely, which is important for performance — fragment reassembly is expensive in userspace.

### Comparison

| | MikroTik | EoIP-rs |
|-|----------|---------|
| Auto-detection | Yes | Yes |
| TAP/interface MTU | 65535 (L2 MTU) | = actual overlay MTU |
| Oversized frames | IP-fragment outer packet | Kernel enforces MTU |
| IP fragmentation | Allowed (kernel-space) | Avoided (userspace perf) |
| DF bit on outer | Configurable | 0 (matches MikroTik default) |

## Configuration

### Auto-detect (default)

```toml
[[tunnel]]
tunnel_id = 100
remote = "172.16.81.1"
# mtu is absent — auto-detect is the default
```

Or explicitly:

```toml
[[tunnel]]
tunnel_id = 100
remote = "172.16.81.1"
mtu = "auto"
```

### Explicit override

```toml
[[tunnel]]
tunnel_id = 100
remote = "172.16.81.1"
mtu = 1400
```

When set explicitly, PMTUD probing is skipped and the TAP interface is set to exactly this value.

### TCP MSS Clamping

Enabled by default. Adds an iptables rule to clamp TCP MSS on SYN packets exiting the tunnel interface, preventing TCP sessions from using segments that would require fragmentation:

```
iptables -t mangle -A FORWARD -o eoip100 -p tcp --tcp-flags SYN,RST SYN -j TCPMSS --clamp-mss-to-pmtu
```

To disable:

```toml
[[tunnel]]
tunnel_id = 100
remote = "172.16.81.1"
clamp_tcp_mss = false
```

MikroTik equivalent: `Clamp TCP MSS` checkbox on the EoIP interface (enabled by default).

## CLI Output

The CLI shows both configured and actual MTU:

```
$ eoip-cli print
Flags: X - disabled; R - running; S - stale; I - initializing
 #   NAME             TUNNEL-ID   LOCAL-ADDR       REMOTE-ADDR        MTU  ACTUAL-MTU
 0 R eoip100                100   0.0.0.0          172.16.81.1       1458        1458

$ eoip-cli print detail
 0  R name="eoip100" tunnel-id=100 local-address=0.0.0.0
      remote-address=172.16.81.1 mtu=1458 actual-mtu=1458 keepalive=10s,30s
      enabled=yes state=active
```

When PMTUD discovers a different path MTU (e.g., path goes through WireGuard), `actual-mtu` updates while `mtu` stays as configured:

```
 0 R eoip100                100   0.0.0.0          172.16.81.1       auto        1378
```

## Verifying No Fragmentation

After auto-MTU is active, verify no IP fragments on the outer path:

```bash
# Should show 0 fragments
sudo tcpdump -i eth0 'ip[6:2] & 0x1fff != 0' -c 10

# Test with maximum overlay frame
ping -c 3 -s $((1458 - 28)) 10.200.0.1    # adjust for your overlay MTU
```

On MikroTik:

```routeros
# Test with DF to verify path MTU
/ping <remote_overlay_ip> do-not-fragment size=1430
# Should succeed (fits in 1458 overlay)

/ping <remote_overlay_ip> do-not-fragment size=1500
# Should fail: "packet too large and cannot be fragmented"
```

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `overlay_mtu=1458` but path has lower MTU | ICMP blocked, interface detection used | Set `mtu` explicitly in config |
| TAP MTU doesn't match auto-detected value | `ExecStartPost` script overrides MTU | Remove `ip link set ... mtu` from systemd unit |
| Large pings fail but small ones work | MTU mismatch between sides | Check both sides show same Actual MTU |
| PMTUD log says "all probes timed out" | ICMP filtered on path | Normal — falls back to interface detection |
| `actual-mtu` changes after startup | PMTUD re-probe found different path | Expected — path MTU changed (e.g., VPN route change) |
