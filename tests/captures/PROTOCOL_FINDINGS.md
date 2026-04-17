# Protocol Findings — MikroTik EoIP Wire Format Analysis

**Date:** 2026-04-17
**RouterOS Version:** 7.18.2 (stable)
**Capture Source:** MikroTik CHR-to-CHR EoIP tunnels on Hetzner CX23 VMs
**Analyzer:** eoip-analyzer (eoip-proto crate)

---

## 1. GRE/EoIP Header (IP Protocol 47)

### 1.1 Magic Bytes
- **Confirmed:** `20 01 64 00` in every single packet
- Byte 0: `0x20` = GRE flags (key present, no checksum, no sequence)
- Byte 1: `0x01` = version 0, protocol type high byte
- Bytes 2-3: `0x6400` = GRE protocol type (EoIP marker)

### 1.2 Payload Length (bytes 4-5)
- **Big-endian** as expected
- Keepalive: `00 00` (payload_len=0)
- ARP frame: `00 2a` (payload_len=42) — 14-byte Ethernet header + 28-byte ARP
- ICMP ping (56 data): `00 46` (payload_len=70) — 14 Eth + 20 IP + 8 ICMP + 28 data
- Matches actual inner Ethernet frame size exactly

### 1.3 Tunnel ID (bytes 6-7)
- **Little-endian** confirmed
- tunnel-id=100 encodes as `64 00`
- tunnel-id=200 encodes as `c8 00`
- No ambiguity — byte order is definitively LE

### 1.4 Total EoIP Header Size
- **8 bytes** (4-byte GRE header + 2-byte payload_len + 2-byte tunnel_id)
- Keepalive packet: IP header (20) + EoIP header (8) = 28 bytes total

## 2. IP Header Behavior

### 2.1 TTL
- **Local originator:** TTL=255 (mk-a sending)
- **Remote after transit:** TTL=247 (mk-b's packets arriving at mk-a)
- Delta: 8 hops — consistent with Hetzner internal datacenter routing
- **Implication:** MikroTik sets TTL=255 for EoIP packets. Our implementation should do the same (or make configurable).

### 2.2 Don't Fragment (DF) Bit
- **DF=0** (not set) on all observed packets
- MikroTik's `dont-fragment=no` setting confirmed in RouterOS config
- Large packets (1500+ inner) are **not fragmented at IP level** — RouterOS sends them as-is

### 2.3 DSCP/ToS
- **ToS=0x00** on all EoIP packets
- `dscp=inherit` is set in RouterOS but ToS=0 observed — may only inherit when inner has non-zero DSCP
- No DSCP copying observed in our test scenarios

### 2.4 IP ID Field
- Monotonically increasing per-direction (e.g., mk-a: `4e80`, `9800`, etc.)
- Standard kernel IP stack behavior

## 3. Keepalive Behavior

### 3.1 Timing
- **Configured:** `keepalive=10s,10` (10-second interval, 10 retries before down)
- **Observed intervals:** ~10.0s between keepalives (very stable, no jitter)
- Both sides send keepalives independently — **bidirectional, not request/response**

### 3.2 Keepalive Packet Structure
- Identical to data packets but with `payload_len=0` and no inner Ethernet frame
- Just 8 bytes of EoIP header after the IP header
- No special keepalive flag or indicator — it's purely identified by zero payload length

### 3.3 Multi-Tunnel Keepalives
- Each tunnel sends its own keepalive independently
- Keepalives for different tunnel IDs may be sent back-to-back (within microseconds)
- tunnel-id=100 and tunnel-id=200 keepalives observed in consecutive packets

### 3.4 Tunnel Down Detection
- Disabling interface on one side: other side loses `R` flag after keepalive timeout
- Recovery: re-enabling restores `R` flag within one keepalive interval
- Default timeout: 10s * 10 = 100s to declare tunnel down

### 3.5 Firewall Interaction
- **Critical finding:** RouterOS `ip firewall filter` AND `ip firewall raw` chains **cannot block EoIP/GRE**
- EoIP is processed at the driver/kernel level before any firewall chain
- Keepalive test requires disabling the interface, not firewall rules

## 4. Inner Ethernet Frames

### 4.1 Frame Structure
- Full 14-byte Ethernet header present (6 dst + 6 src + 2 ethertype)
- **No FCS** — the 4-byte Ethernet FCS is stripped before encapsulation
- **No padding** added by MikroTik

### 4.2 MAC Addresses
- EoIP interfaces get auto-generated MACs starting with `FE:xx`
- Different MAC per tunnel interface (not shared)
- ARP properly resolves across tunnel using these MACs

### 4.3 Observed Inner Protocols
MikroTik sends these through the tunnel automatically (even when "idle"):
- **IPv6 Router Advertisements** (dst: `33:33:00:00:00:01`, ethertype `0x86dd`)
- **Gratuitous ARP / IP announcements** (dst: `ff:ff:ff:ff:ff:ff`, ethertype `0x0800`)
- **CDP-like discovery** (dst: `01:00:0c:cc:cc:cc`, ethertype `0x0074`) — MikroTik Neighbor Discovery Protocol (MNDP)
- **LLDP** (dst: `01:80:c2:00:00:0e`, ethertype `0x88cc`)

**Implication:** An "idle" EoIP tunnel is never truly idle. Our daemon must handle these background L2 frames even with no user traffic.

## 5. MTU and Fragmentation

### 5.1 Effective MTU
- RouterOS reports `actual-mtu=1458` for EoIP interfaces
- Calculated: 1500 (ether MTU) - 20 (outer IP) - 8 (EoIP header) - 14 (inner Ethernet) = **1458 bytes of inner IP payload**
- Matches RouterOS's reported value exactly

### 5.2 Observed Packet Sizes
| Inner ping size | Inner Eth frame | EoIP payload_len | Outer IP len | Outer total |
|----------------|----------------|-----------------|-------------|-------------|
| 56 (default) | 70 | 70 | 98 | 98 |
| 1400 | 1414 | 1414 | 1442 | 1442 |
| 1450 | 1464 | 1464 | 1492 | 1492 |
| 1472 | 1466+54 | split | 1494+82 | fragmented |
| 1500 | 1466+82 | split | 1494+110 | fragmented |
| 1501 | 1466+83 | split | 1494+111 | fragmented |

### 5.3 Fragmentation Behavior
- Packets exceeding 1458 inner IP MTU are **fragmented at the inner IP level** by RouterOS
- The inner ICMP is split into multiple inner IP fragments
- Each fragment is encapsulated separately in its own EoIP packet
- **No IP-level fragmentation of the outer packet** — outer packets stay under 1500
- Maximum observed EoIP payload_len: 1466 bytes (1458 MTU + 8 ICMP header)

## 6. Multi-Tunnel Demultiplexing

- Multiple tunnels to the same remote IP work correctly
- Tunnel ID in EoIP header is the sole demux key
- No observed interference between tunnel-id=100 and tunnel-id=200
- Each tunnel maintains independent MAC addresses, ARP tables, and keepalive state

## 7. Deviations from Our Spec

### 7.1 Confirmed Correct
- Magic bytes `20 01 64 00`
- Payload length big-endian
- Tunnel ID little-endian
- 8-byte EoIP header
- Keepalive = zero payload length

### 7.2 Items to Verify in Our Implementation
1. **TTL should be 255** (not 64 or other default)
2. **DF bit should be 0** by default (allow fragmentation)
3. **DSCP should be 0** unless explicitly configured to copy
4. **No FCS in inner frame** — strip before encap, don't add on decap
5. **Handle background L2 traffic** — MNDP, LLDP, IPv6 RA will arrive even when idle
6. **Inner fragmentation, not outer** — when payload exceeds MTU, fragment the inner packet

### 7.3 No Deviations Found
Our protocol codec correctly matches the observed wire format. Zero decode errors across all captures (168 total packets analyzed).

## 8. Capture Inventory

| File | Scenario | Packets | Key Observation |
|------|----------|---------|-----------------|
| mk-mk-idle.pcap | Idle keepalives | 24 | 10s bidirectional keepalive, background L2 traffic |
| mk-mk-ping.pcap | Single ping | 14 | ARP + ICMP encapsulation verified |
| mk-mk-arp.pcap | ARP resolution | 4 | Clean ARP req/reply through tunnel |
| mk-mk-mtu.pcap | MTU probes 1400-1501 | 35 | Inner fragmentation at 1458 boundary |
| mk-mk-updown.pcap | Tunnel disable/enable | 51 | Keepalive cessation and recovery |
| mk-mk-multi.pcap | Two tunnels active | 41 | Independent demux by tunnel ID |
