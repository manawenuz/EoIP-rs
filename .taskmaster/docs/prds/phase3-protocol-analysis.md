# Phase 3: Protocol Deep-Dive with Linux Middlebox

**Status:** Draft  
**Priority:** Critical — produces the wire format ground truth  
**Dependencies:** Phase 2  
**Estimated Duration:** 4-6 hours  
**Cost:** ~$1-2 additional (third CX22 VM running Linux)

---

## Objective

Deploy a third VM running Linux, position it as a transparent capture point on the path between the two MikroTik CHRs. Capture all EoIP traffic with `tcpdump`, analyze it with our `eoip-analyzer`, and extract every protocol detail: header layout, endianness, keepalive timing, MTU behavior, fragmentation, and edge cases.

## Background

We cannot fully trust documentation alone. MikroTik's EoIP has undocumented quirks (mixed endianness, non-standard GRE). We need byte-level ground truth from real captures to validate our codec and find any behaviors our spec may have missed.

## Requirements

### 3.1 Linux Middlebox Deployment
- Deploy third VM (`linux-probe`) — Ubuntu 22.04 or Debian 12
- Install: `tcpdump`, `tshark` (Wireshark CLI), our `eoip-analyzer` binary
- Cross-compile `eoip-analyzer` for Linux x86_64 or build on the VM

### 3.2 Traffic Capture Strategy

**Option A — Packet mirroring (preferred):**
- If Hetzner supports port mirroring / traffic duplication, mirror mk-a and mk-b traffic to linux-probe
- This is non-intrusive — MikroTik pair behaves exactly as without probe

**Option B — Inline capture (fallback):**
- Reconfigure mk-a to route to mk-b via linux-probe (proxy ARP or IP forwarding)
- linux-probe forwards packets while capturing
- More complex but works on any provider

**Option C — Capture on MikroTik directly (simplest):**
- Use RouterOS packet sniffer: `/tool sniffer quick ip-protocol=47`
- Export .pcap from RouterOS: `/tool sniffer packet save`
- Transfer to linux-probe via SCP
- Limitation: may miss some kernel-level details

### 3.3 Capture Scenarios

Each scenario produces a `.pcap` file saved in `tests/captures/`:

| # | Scenario | What to capture | Expected packets |
|---|----------|----------------|-----------------|
| 1 | **Idle tunnel** | No traffic, just keepalives | Keepalive packets (payload_len=0) every 10s |
| 2 | **Single ping** | `ping 10.255.0.2 count=1` from mk-a | ICMP request + reply, each wrapped in EoIP |
| 3 | **ARP resolution** | Clear ARP, then ping | ARP request/reply + ICMP, all in EoIP |
| 4 | **Bulk transfer** | `bandwidth-test` or large file over tunnel | Many EoIP packets, see batching/fragmentation |
| 5 | **MTU probe** | Ping with increasing sizes: 1400, 1450, 1472, 1500, 1501 | Find where fragmentation occurs |
| 6 | **Tunnel up/down** | Disable/enable tunnel on one side | See keepalive failure and recovery sequence |
| 7 | **Multiple tunnels** | Two tunnels active, traffic on both | Verify tunnel_id demux in captures |
| 8 | **EoIPv6** | Same as #2-3 but over IPv6 transport | Protocol 97 packets, 2-byte header |
| 9 | **VLAN-tagged frames** | Send 802.1Q tagged traffic through tunnel | Verify inner Ethernet frame has VLAN tag |
| 10 | **Broadcast storm** | Bridge loop briefly | Many broadcast frames, stress the tunnel |

### 3.4 Analysis with eoip-analyzer

For each capture:
```bash
# Full decode with hex dumps
eoip-analyzer capture-N.pcap --hexdump > analysis-N.txt

# JSON for programmatic comparison
eoip-analyzer capture-N.pcap --json > analysis-N.json

# Summary stats
eoip-analyzer capture-N.pcap --summary-only
```

### 3.5 Deep-Dive Analysis Checklist

For each captured EoIP packet, verify and document:

**GRE/EoIP (proto 47):**
- [ ] Magic bytes are exactly `20 01 64 00`
- [ ] Payload length is big-endian
- [ ] Tunnel ID is little-endian
- [ ] Payload length matches actual Ethernet frame size
- [ ] IP header TTL value (what does MikroTik use? 64? 255?)
- [ ] IP header ToS/DSCP (does MikroTik copy inner or set to 0?)
- [ ] IP Don't Fragment bit behavior

**EoIPv6/EtherIP (proto 97):**
- [ ] Version nibble is exactly 0x3
- [ ] Tunnel ID nibble-packing matches our decode
- [ ] IPv6 hop limit value
- [ ] IPv6 flow label (0 or derived from inner?)

**Keepalive:**
- [ ] Exact interval (is it really 10s or slightly drifted?)
- [ ] Payload length in keepalive is exactly 0
- [ ] Is keepalive bidirectional or only one side initiates?
- [ ] What happens in the first seconds after tunnel creation?

**Inner Ethernet frames:**
- [ ] Full 14-byte Ethernet header present (dst MAC, src MAC, ethertype)
- [ ] No padding added/removed by MikroTik
- [ ] VLAN tags preserved if present
- [ ] FCS present or stripped?

**Edge cases:**
- [ ] Maximum tunnel_id value MikroTik allows (65535? or less?)
- [ ] Minimum/maximum MTU MikroTik enforces
- [ ] Behavior when tunnel_id=0

### 3.6 Protocol Deviation Report

Create `tests/captures/PROTOCOL_FINDINGS.md` documenting:
- Any deviation from our spec (`docs/design/protocol.md`)
- Any undocumented behavior discovered
- Recommendations for updating our implementation

## Success Criteria

- [ ] At least 10 capture scenarios completed
- [ ] All captures analyzed with eoip-analyzer (zero deviations = good)
- [ ] Protocol findings document written
- [ ] Any spec deviations fed back into `docs/design/protocol.md`
- [ ] Captures archived in `tests/captures/` for regression testing

## Artifacts

- `tests/captures/mk-mk-idle.pcap` through `tests/captures/mk-mk-vlan.pcap`
- `tests/captures/PROTOCOL_FINDINGS.md`
- Updated `docs/design/protocol.md` if deviations found
- `tests/infra/setup-probe.sh` — script to deploy and configure the Linux probe
