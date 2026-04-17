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

## Bridging (L2 Forwarding)

EoIP carries full Ethernet frames, making it ideal for L2 bridges:

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
