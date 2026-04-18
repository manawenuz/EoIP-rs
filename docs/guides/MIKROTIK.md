# MikroTik Interop Guide

How to configure MikroTik RouterOS to establish EoIP tunnels with EoIP-rs.

## Prerequisites

- MikroTik router running RouterOS 6.x or 7.x
- IP connectivity between MikroTik and the EoIP-rs host
- IP protocol 47 (GRE) not blocked by firewalls

## Basic Setup

### On MikroTik

```routeros
# Create EoIP tunnel
/interface eoip add name=eoip-linux remote-address=<LINUX_IP> tunnel-id=100

# Assign IP to tunnel interface
/ip address add address=10.255.0.1/30 interface=eoip-linux

# Verify tunnel status (should show R = running)
/interface eoip print
```

### On EoIP-rs (Linux)

```toml
# /etc/eoip-rs/config.toml
[[tunnel]]
tunnel_id = 100
local = "<LINUX_IP>"
remote = "<MIKROTIK_IP>"
```

```bash
sudo eoip-helper --mode persist &
sudo eoip-rs --config /etc/eoip-rs/config.toml &
sudo ip link set eoip100 up
sudo ip addr add 10.255.0.2/30 dev eoip100
```

### Verify

```bash
# From Linux
ping 10.255.0.1

# From MikroTik
/ping 10.255.0.1
```

Both should show 0% packet loss.

## Multiple Tunnels

### MikroTik

```routeros
/interface eoip add name=eoip-tun1 remote-address=<LINUX_IP> tunnel-id=100
/interface eoip add name=eoip-tun2 remote-address=<LINUX_IP> tunnel-id=200
/ip address add address=10.255.0.1/30 interface=eoip-tun1
/ip address add address=10.255.1.1/30 interface=eoip-tun2
```

### EoIP-rs

```toml
[[tunnel]]
tunnel_id = 100
local = "<LINUX_IP>"
remote = "<MIKROTIK_IP>"

[[tunnel]]
tunnel_id = 200
local = "<LINUX_IP>"
remote = "<MIKROTIK_IP>"
```

Or add dynamically via CLI:

```bash
eoip-cli add tunnel-id=200 remote-address=<MIKROTIK_IP> local-address=<LINUX_IP>
sudo ip link set eoip200 up
sudo ip addr add 10.255.1.2/30 dev eoip200
```

## Monitoring

### MikroTik Side

```routeros
# Tunnel status
/interface eoip print detail

# Monitor keepalives
/interface eoip monitor eoip-linux

# Packet capture (GRE protocol 47)
/tool sniffer quick ip-protocol=47
```

### EoIP-rs Side

```bash
# CLI monitoring
eoip-cli print                    # List tunnels
eoip-cli print detail             # Detailed view
eoip-cli stats 100                # Per-tunnel stats
eoip-cli monitor                  # Stream events

# Packet capture
sudo tcpdump -i any 'ip proto 47' -w capture.pcap
eoip-analyzer capture.pcap        # Decode EoIP headers
```

## Keepalive Behavior

| Parameter | MikroTik Default | EoIP-rs Default |
|-----------|-----------------|-----------------|
| Interval | 10s | 10s |
| Timeout | 100s (10 retries) | 100s |

Both sides send keepalives independently. A tunnel goes "not running" when no keepalives are received within the timeout.

To adjust on MikroTik:

```routeros
/interface eoip set eoip-linux keepalive=5s,5
```

To adjust on EoIP-rs:

```toml
[[tunnel]]
keepalive_interval_secs = 5
keepalive_timeout_secs = 25
```

## MTU & Path MTU Discovery

EoIP-rs auto-detects the overlay MTU, matching MikroTik's behavior. See [PMTUD Guide](PMTUD.md) for full details.

### MikroTik MTU Fields

| Field | Meaning |
|-------|---------|
| **MTU** | Configured value (blank = auto) |
| **Actual MTU** | Discovered overlay MTU (path MTU - 42 bytes overhead) |
| **L2 MTU** | Max L2 frame size (always 65535 for EoIP — virtual, no hardware limit) |

### Matching Configuration

Both sides should agree on the overlay MTU. With auto-detection on both sides, this happens automatically:

```routeros
# MikroTik: leave MTU blank (auto)
/interface eoip add name=eoip-linux remote-address=<LINUX_IP> tunnel-id=100
# Check: /interface eoip print detail
#   actual-mtu=1458 (for 1500-byte path)
#   actual-mtu=1378 (for WireGuard path)
```

```toml
# EoIP-rs: omit mtu or set to "auto"
[[tunnel]]
tunnel_id = 100
remote = "<MIKROTIK_IP>"
# mtu = "auto"   (default)
```

### Verifying MTU Match

```routeros
# On MikroTik: test with DF flag
/ping <overlay_ip> do-not-fragment size=1430
# Should succeed (fits in 1458)

/ping <overlay_ip> do-not-fragment size=1500
# Should fail: "packet too large and cannot be fragmented"
```

```bash
# On Linux: same test
ping -M do -s 1430 <overlay_ip>    # should work
ping -M do -s 1472 <overlay_ip>    # should fail (> 1458 - 28)
```

### Key Differences

MikroTik sets L2 MTU to 65535 and IP-fragments oversized outer GRE packets (when `Dont Fragment = no`). EoIP-rs sets the TAP interface MTU to the actual overlay MTU so the kernel enforces the limit — no IP fragmentation occurs. Both approaches are interoperable; the difference is only in how each side handles locally-originated oversized frames.

### TCP MSS Clamping

Both sides should have TCP MSS clamping enabled to prevent TCP sessions from using segments that would require fragmentation:

- **MikroTik:** `Clamp TCP MSS` checkbox (enabled by default)
- **EoIP-rs:** `clamp_tcp_mss = true` in config (enabled by default)

## IPsec Encryption

EoIP-rs supports MikroTik's `ipsec-secret` parameter for automatic IPsec transport mode encryption. See [IPsec Guide](IPSEC.md) for full setup and troubleshooting.

### MikroTik Side

```routeros
/interface eoip add name=eoip-linux remote-address=<LINUX_IP> tunnel-id=100 \
    ipsec-secret=SecretPass allow-fast-path=no
```

**Important:** `allow-fast-path=no` is required when using `ipsec-secret`.

### EoIP-rs Side

```toml
[[tunnel]]
tunnel_id = 100
local = "<LINUX_IP>"
remote = "<MIKROTIK_IP>"
ipsec_secret = "SecretPass"
```

EoIP-rs uses strongSwan's VICI protocol to create matching IKEv1 SAs automatically. strongSwan must be installed on the Linux host (`apt install strongswan-charon strongswan-swanctl`).

### Verification

```routeros
# On MikroTik: check IPsec is active
/interface eoip print detail
# Should show: ipsec-secret="SecretPass"

/ip ipsec installed-sa print
# Should show active SAs with enc=aes-cbc key-size=256
```

```bash
# On EoIP-rs
eoip-cli print detail
# Should show: ipsec=yes ipsec-active=yes
```

### MTU with IPsec

ESP encryption adds 38 bytes of overhead, reducing the overlay MTU from 1458 to 1420 on a standard 1500-byte path. EoIP-rs adjusts this automatically when `ipsec_secret` is set. See [PMTUD Guide](PMTUD.md) for details.

## Bridging (L2 Forwarding)

EoIP carries full Ethernet frames, making it ideal for L2 bridges. See [Networking Guide](NETWORKING.md) for detailed bridging and DHCP instructions on Linux and Windows.

### MikroTik

```routeros
/interface bridge add name=br-eoip
/interface bridge port add bridge=br-eoip interface=eoip-linux
/interface bridge port add bridge=br-eoip interface=ether2
```

### Linux

```bash
sudo ip link add br-eoip type bridge
sudo ip link set eoip100 master br-eoip
sudo ip link set eth1 master br-eoip
sudo ip link set br-eoip up
```

### Windows

Open **Network Connections**, select both the EoIP TAP adapter and the physical NIC, right-click and choose **Bridge Connections**.

### DHCP Through the Tunnel

If MikroTik has a DHCP server on the bridged network, remote devices can get IPs through the EoIP tunnel:

```bash
# Linux: request DHCP on the tunnel interface
sudo dhclient eoip100
```

```powershell
# Windows: set adapter to DHCP
Set-NetIPInterface -InterfaceAlias "EoIP Tunnel" -Dhcp Enabled
ipconfig /renew "EoIP Tunnel"
```

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| MikroTik shows "not running" | Keepalives not reaching | Check firewall allows GRE (proto 47) |
| Ping works one way only | ARP not resolving | Verify IPs are in same /30 subnet |
| High packet loss | CPU bottleneck | Check `eoip-cli stats`, reduce tunnel count |
| Tunnel ID mismatch | IDs don't match | Both sides must use identical `tunnel_id` |
| SSH to MikroTik fails | Key algorithm | Use `ssh -o HostKeyAlgorithms=+ssh-rsa` |

## Wire Format Compatibility

EoIP-rs produces byte-identical packets to MikroTik RouterOS:

- Magic bytes: `20 01 64 00`
- Payload length: big-endian
- Tunnel ID: little-endian
- TTL: 255
- DF bit: 0 (allow fragmentation)
- Keepalive: payload_len=0

Validated with 168 captured packets, zero deviations.
