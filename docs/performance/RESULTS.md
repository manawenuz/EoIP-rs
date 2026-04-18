# EoIP-rs Performance & Scaling Results

**Date:** 2026-04-17
**Version:** 0.1.0
**Platform:** Hetzner CX23 (2 vCPU, 4 GB RAM, shared x86_64)
**OS:** Ubuntu 22.04, Linux 5.15
**Rust:** 1.95.0 (release build)

---

## 1. MikroTik Interop (Phase 5)

**Setup:** EoIP-rs on Linux VM ↔ MikroTik CHR (RouterOS 7.18.2)

| Metric | Result |
|--------|--------|
| Tunnel establishment | MikroTik shows `R` (running) |
| Keepalive exchange | Bidirectional, 10s interval |
| Bidirectional ping | 0% loss, ~1ms RTT |
| Protocol deviations | Zero (168 packets analyzed) |
| Wire format match | Byte-identical to MikroTik captures |

## 2. Multi-Tunnel Scaling (Phase 6)

**Setup:** 100 EoIP tunnels between EoIP-rs and MikroTik CHR on same datacenter

| Metric | Result |
|--------|--------|
| Tunnels active | **100 / 100** |
| Concurrent ping (5 pkt x 100 tunnels) | **0% loss** |
| Sustained 30s load (30 pkt x 100 tunnels) | **0% loss, 3000/3000 delivered** |
| Memory (RSS) before load | 18,068 kB |
| Memory (RSS) after load | 18,068 kB (**zero growth**) |
| Per-tunnel memory overhead | ~176 kB |
| MikroTik CPU (100 tunnels, flood) | 1-2% |
| EoIP-rs CPU (100 tunnels, flood) | 12.3% |

## 3. Latency (Phase 7)

**Setup:** Same-datacenter CX23 VMs

| Metric | Result |
|--------|--------|
| Direct ping (no tunnel) | min/avg/max = 0.336 / 0.696 / 3.958 ms |
| EoIP tunnel ping | min/avg/max = 0.522 / 0.821 / 2.232 ms |
| **Tunnel overhead** | **~190 µs average** |
| Ping flood 1000 pkt (0.01s interval) | 0% loss |

## 4. Linux-to-Linux Throughput (Phase 8)

**Setup:** Two CX23 VMs both running EoIP-rs, single tunnel

### Single Tunnel TCP

| Test | Throughput | Retransmits |
|------|-----------|-------------|
| TCP single stream | **346 Mbps** | 1058 |
| TCP 4 parallel streams | **242 Mbps** (sum) | 2354 |
| UDP max bandwidth (sender) | 3.03 Gbps | — |
| UDP max bandwidth (receiver) | 327 Mbps | 89% loss at sender rate |

**Note:** CX23 VMs have ~350 Mbps network cap. Throughput is VM-limited, not daemon-limited.

### Multi-VM Aggregate (1 Server, 5 Clients)

| Client | Tunnel ID | Throughput |
|--------|-----------|-----------|
| linux-b | 1 | 111 Mbps |
| linux-c | 2 | 129 Mbps |
| linux-d | 3 | 108 Mbps |
| linux-e | 4 | 104 Mbps |
| linux-f | 5 | 106 Mbps |
| **Total** | **5 tunnels** | **558 Mbps aggregate** |

Server metrics during 5-client concurrent iperf3:
- **CPU:** 18.2%
- **RSS:** 11.5 MB
- **Active tunnels:** 5, zero stale

## 5. Fault Injection (Phase 8)

| Fault | Behavior | Pass |
|-------|----------|------|
| Kill daemon (one side) | Other side stays active (within 100s timeout), recovers on restart | Yes |
| Network partition (iptables -j DROP) | Ping fails during partition, recovers immediately on unblock | Yes |
| 200ms latency spike (netem) | Ping RTT increases to ~201ms, tunnel stays active | Yes |
| 10% packet loss (netem) | ~8% observed loss (consistent with 10% each way), no crash, daemon stays SERVING | Yes |

## 6. Resource Usage Summary

| Tunnels | RSS (MB) | CPU (idle) | CPU (loaded) |
|---------|----------|------------|-------------|
| 1 | ~12 | < 1% | 12% (iperf3) |
| 5 (multi-VM) | 11.5 | < 2% | 18.2% (5x iperf3) |
| 100 (single peer) | 17.6 | ~5% | 12.3% (100x ping flood) |

## 7. gRPC Management API

Validated via `eoip-cli` against live daemon at all scales:

| Command | Verified |
|---------|----------|
| `print` | RouterOS-style table output, correct at 1-100 tunnels |
| `print detail` | Full property view with keepalive, state |
| `stats <tunnel-id>` | Per-tunnel TX/RX counters, timestamps |
| `stats` (global) | Aggregate counters, active/stale counts |
| `health` | SERVING status |
| `--json` mode | Structured JSON output |

## 8. Test Environment

| VM | Role | IP | Type |
|----|------|----|------|
| linux-gw | Server (EoIP-rs) | 138.199.165.46 | CX23 |
| linux-b | Client 1 | 78.47.55.197 | CX23 |
| linux-c | Client 2 | 167.235.241.221 | CX23 |
| linux-d | Client 3 | 167.235.247.235 | CX23 |
| linux-e | Client 4 | 128.140.114.175 | CX23 |
| linux-f | Client 5 | 167.235.250.191 | CX23 |

All VMs in Hetzner `fsn1` datacenter, same private network `10.0.0.0/16`.

## 9. Phase 11 — Userspace Performance Optimization

**Date:** 2026-04-18
**Platform:** Hetzner CPX22 (2 vCPU, 4 GB RAM, shared x86_64)
**OS:** Ubuntu 22.04, Linux 5.15
**Methodology:** Each version tested 3 times in rotating order (Latin square) to control for shared-vCPU variance. Results are averages.

### Optimization Routes

| Route | Change | Commit |
|-------|--------|--------|
| R1 | SO_RCVBUF/SO_SNDBUF set to 4 MB (was ~212 KB kernel default) | `da88e9c` |
| R2 | R1 + `sendmmsg()` replacing per-packet `sendto()` loop in TX flush | `238b722` |
| R3 | R1 + R2 + batch-drain crossbeam channel in TAP writer (up to 32 frames/wake) | `7e06a31` |

### Throughput Results (3-round average, iperf3 single-stream TCP, 15s)

| Version | TX (Mbps) | TX CPU | RX (Mbps) | RX CPU |
|---------|-----------|--------|-----------|--------|
| old (v0.1.0-alpha.1) | 369 | 1.1% | 279 | 20.7% |
| R1 (4 MB bufs) | 554 | 1.8% | 473 | 26.5% |
| R2 (R1 + sendmmsg) | 516 | 1.8% | 436 | 21.4% |
| **R3 (all routes)** | **570** | **1.6%** | **456** | **24.9%** |

### Improvement vs v0.1.0-alpha.1

| Metric | Old | R3 | Improvement |
|--------|-----|-----|------------|
| TX throughput | 369 Mbps | 570 Mbps | **+54%** |
| RX throughput | 279 Mbps | 456 Mbps | **+63%** |

### Cross-Compatibility

| Config | TX (Mbps) | RX (Mbps) | Status |
|--------|-----------|-----------|--------|
| R3 + R3 | 570 | 456 | Pass |
| R3 + old | 412 | 456 | Pass |
| old + R3 | 477 | 270 | Pass |
| old + old | 369 | 279 | Pass |

Full backward compatibility maintained.

### MikroTik CHR Interop (RouterOS 7.18.2)

| Test | Result |
|------|--------|
| Tunnel establishment | CHR shows `R` (running) |
| Bidirectional ping | 0% loss, ~0.6 ms RTT |
| Large frame ping (1400 bytes) | 0% loss |
| btest TX (EoIP-rs → CHR) | 229 Mbps avg (TCP) |
| btest RX (CHR → EoIP-rs) | ~1 Mbps (CHR free license cap) |

Protocol compatibility confirmed. RX bandwidth limited by CHR free license (1 Mbps cap), not by EoIP-rs.
