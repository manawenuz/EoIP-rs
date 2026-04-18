# Phase 11: Userspace Performance Optimization

**Status:** Routes 1-3 complete, IPv6 fixed. Remaining: hot-path micro-optimizations + PACKET_MMAP.
**Priority:** High — squeeze maximum throughput from the userspace data plane before jumping to XDP
**Dependencies:** Phase 8 (baseline throughput numbers), Phase 10 PRD (context for ceiling)
**Branches:** `feat/phase11-userspace-perf` (merged), `fix/ipv6-transport` (merged), `feat/packet-mmap-wip` (WIP, not merged)
**Current release:** v0.1.0-alpha.3 (includes all Route 1-3 + IPv6 fixes)

---

## Objective

Maximize EoIP-rs userspace RX/TX throughput on commodity Linux VMs (Hetzner CPX22, 2 shared vCPU) before investing in kernel-bypass approaches (XDP/eBPF, io_uring).

## Performance History

**Test Environment:** 2x Hetzner CPX22 (2 vCPU, 4 GB RAM, shared x86_64), Ubuntu 22.04, Linux 5.15, Rust 1.95.0.

### Pre-optimization baseline (v0.1.0-alpha.1)

| Metric | Value | Notes |
|--------|-------|-------|
| TX throughput (iperf3) | **369 Mbps** | 3-round average, Latin square |
| RX throughput (iperf3) | **279 Mbps** | 3-round average, Latin square |
| TX CPU | 1.1% | |
| RX CPU | 20.7% | |

### Post-optimization (v0.1.0-alpha.3, Routes 1-3 + IPv6 fix)

| Metric | IPv4 | IPv6 | Notes |
|--------|------|------|-------|
| TX throughput | **570 Mbps** (+54%) | **301 Mbps** | 3-round avg |
| RX throughput | **456 Mbps** (+63%) | **452 Mbps** | 3-round avg |
| TX CPU | 1.6% | 1.1% | |
| RX CPU | 24.9% | 31.9% | |

### btest results (MikroTik bandwidth-test protocol)

| Peer | TX | RX | Notes |
|------|----|----|-------|
| Hetzner CHR (RouterOS 7.18.2) | 226 Mbps | ~1 Mbps | RX limited by CHR free license |
| MikroTik hardware (LAN) | 350 Mbps avg | 260 Mbps avg | Via user's MikroTik router |
| Raspberry Pi 4 (wifi) | — | — | Tunnel works, wifi-limited |

### Cross-compatibility matrix (all pass)

| Config | TX | RX |
|--------|----|----|
| alpha.3 + alpha.3 | 570 | 456 |
| alpha.3 + alpha.1 | 412 | 456 |
| alpha.1 + alpha.3 | 477 | 270 |
| MikroTik CHR ↔ alpha.3 | pass | pass |
| Raspberry Pi 4 ↔ MikroTik | pass | pass (wifi) |

---

## Post-Mortem: `feat/packet-mmap-wip` Branch

### What Was Attempted

A full AF_PACKET + TPACKET_V3 (PACKET_MMAP) zero-copy RX path. The goal was to eliminate the `recvmmsg` kernel-to-userspace copy by sharing a 4 MB mmap'd ring buffer between the kernel and the daemon.

### What Happened

The implementation went through multiple iterations and hit **four distinct bugs** before being shelved:

#### Bug 1: Unix STREAM Socket Message Coalescing

The initial design passed the AF_PACKET socket fd from the privileged helper to the unprivileged daemon via SCM_RIGHTS over a Unix STREAM socket. The helper sends multiple messages in quick succession (TapCreated, RawSocket v4, Error v6, RawSocket AF_PACKET). On a STREAM socket, `recvmsg` can coalesce multiple `sendmsg` payloads into a single read. The daemon read the v6 Error + AF_PACKET data in one `recvmsg`, deserialized only the Error, and lost the AF_PACKET fd. The daemon then blocked forever on the next `recvmsg` waiting for a message that was already consumed.

**Fix applied:** Moved AF_PACKET socket creation directly into the daemon (it already runs as root). Eliminated the helper protocol change entirely.

#### Bug 2: SOCK_DGRAM Incompatible with TPACKET_V3 Ring Delivery

The AF_PACKET socket was initially created with `SOCK_DGRAM | ETH_P_IP`, which strips L2 headers. While `recvfrom()` on this socket worked fine (confirmed via Python test), **packets never appeared in the TPACKET_V3 ring buffer**. The `poll()` call would return ready, but all blocks remained `TP_STATUS_KERNEL`. Only keepalive packets (one every 10 seconds) occasionally appeared.

The root cause was never fully determined. Possible explanations:
- Kernel bug in TPACKET_V3 + SOCK_DGRAM combination on 5.15
- The BPF filter offset mismatch (see Bug 3) masking the real issue
- SOCK_DGRAM cooked header interaction with ring buffer frame layout

**Fix applied:** Switched to `SOCK_RAW` which includes the full Ethernet frame. Adjusted data extraction to use `tp_net` offset (skip L2 header) in the ring buffer.

#### Bug 3: SKF_AD_PKTTYPE BPF Extension Drops All Packets

The BPF filter used the `SKF_AD_PKTTYPE` ancillary data extension (opcode `BPF_LD|BPF_W|BPF_ABS` with `k = 0xFFFFF004`) to filter out `PACKET_OUTGOING` packets. On the test kernel (5.15.0-173-generic, Ubuntu 22.04), this instruction caused the filter to **drop every packet**, including inbound ones.

The classic BPF interpreter should handle `SKF_AD_OFF` extensions by translating them to eBPF during filter attachment (`sk_convert_filter` → `convert_bpf_extensions`). The reason this translation failed is not determined — possibly a kernel configuration issue (CONFIG_BPF_JIT), missing eBPF verifier capability, or a genuine kernel bug.

**Fix applied:** Replaced with `PACKET_IGNORE_OUTGOING` socket option (available since Linux 4.20), which is a cleaner kernel-level solution that doesn't rely on BPF extensions.

#### Bug 4: TX Flooding of Ring Buffer Under Load

With `SOCK_RAW` on AF_PACKET, the socket captures **both inbound and outbound** packets. Under iperf3 load, the daemon's own GRE TX packets flood the TPACKET_V3 ring buffer, displacing inbound packets. This caused iperf3 throughput to drop to 0 Mbps despite ping working fine (keepalives are sparse enough to fit).

This was the final symptom observed. With the `PACKET_IGNORE_OUTGOING` fix (Bug 3), this should be resolved, but the combination was never fully tested under load because the pkttype BPF filter was the primary approach and kept failing.

**Fix in branch but untested:** `PACKET_IGNORE_OUTGOING` setsockopt + GRE-only BPF filter (offset 23, no pkttype check).

### Current State of the Branch

The `feat/packet-mmap-wip` branch contains:
- `packet_mmap.rs`: TPACKET_V3 ring buffer abstraction (4 MB, 16 blocks, 256 KB each)
- AF_PACKET SOCK_RAW socket creation with BPF filter
- Ring buffer integration in `rx.rs` with fallback to `recvmmsg`
- First-packet diagnostic logging

**Not working:** High-throughput RX. Ping/keepalive works. iperf3 shows 0 Mbps or sub-1 Mbps. The branch should not be merged until the ring processing is validated under sustained load.

### Key Learnings

1. **Unix STREAM sockets are not message-oriented.** Never send multiple independent messages in quick succession and expect the receiver to read them one at a time. Use DGRAM sockets, length-prefix framing, or request-response patterns.
2. **SOCK_DGRAM + TPACKET_V3 is unreliable.** Stick with SOCK_RAW for AF_PACKET ring buffers.
3. **BPF ancillary data extensions (SKF_AD_*) are fragile.** Use dedicated socket options (`PACKET_IGNORE_OUTGOING`) instead of BPF tricks where available.
4. **Always filter outgoing packets on AF_PACKET.** Under load, TX traffic dominates and starves the ring.
5. **Test under load, not just with ping.** The PACKET_MMAP path passed ping tests at every iteration but failed catastrophically under iperf3.

---

## Completed Optimization Routes

### Route 1: SO_RCVBUF/SO_SNDBUF Tuning — DONE ✓

**Commit:** `da88e9c` | **Impact:** Biggest single win (+22% TX, +10% RX in isolation)

Set socket buffers to 4 MB on both IPv4 and IPv6 raw sockets (was ~212 KB kernel default). Absorbs burst traffic between userspace batch drains.

### Route 2: sendmmsg for TX — DONE ✓

**Commit:** `238b722` | **Impact:** Fewer syscalls, architectural correctness

Replaced per-packet `sendto()` loop with single `sendmmsg()` call per batch. Also fixed to route v4/v6 packets to correct raw socket (was sending v6 on v4 fd).

### Route 3: TAP Writer Batch Drain — DONE ✓

**Commit:** `7e06a31` | **Impact:** Reduced channel contention

TAP writer thread now drains up to 32 frames per channel wake-up. Note: `writev()` on TAP doesn't preserve frame boundaries, so each frame is still a separate `write()` syscall — but channel wake-ups are reduced.

### IPv6 Transport Fix — DONE ✓

**Commits:** `2a40e52`, `e3d9b6f`, `0402525` | **Impact:** EoIPv6 now works

Three bugs fixed: `IPV6_V6ONLY` invalid on raw sockets (EINVAL), TX batcher missing v6 fd, RX v6 using `read()` instead of `recvfrom()` (no source address for demux).

### Route 5: RX Hot-Path Micro-Optimizations — DONE ✓

**Commit:** `901bcc9` | **Impact:** +64% TX, +50% RX vs pre-Route5

Six changes: DashMap Ref guard (no Arc clone), buffer shrink 65KB→2KB, CLOCK_REALTIME_COARSE via vDSO, rx_workers config wiring, RX channel cap config wiring, proper pool sizing. The buffer shrink and Arc clone elimination were the primary drivers — better cache locality and fewer atomics per packet.

**3-round Latin square benchmark (Hetzner CPX22):**

| Version | TX avg (Mbps) | RX avg (Mbps) |
|---------|-------------|-------------|
| pre-Route5 | 430 | 416 |
| **Route 5** | **704** (+64%) | **623** (+50%) |

---

## Post-Mortem: Route 6 — Zero-Copy RX into Pool Buffers

### What Was Attempted

Eliminate the `copy_from_slice` per RX packet by receiving `recvmmsg` data directly into pool buffers. Added `raw_mut()`, `as_raw_slice()`, `set_rx_region()` to `PacketBuf`. On successful decode, swap a fresh buffer in, set the received buffer's head/len to the Ethernet frame region, and send to TAP writer — zero memcpy.

### What Happened

**Branch:** `feat/route6-zerocopy-rx` (not merged, abandoned)

Benchmarked in same Latin square as Route 5. Results:

| Version | TX avg (Mbps) | RX avg (Mbps) | RX CPU |
|---------|-------------|-------------|--------|
| Route 5 | 704 | 623 | 29.3% |
| Route 5+6 | 673 | 567 | 32.0% |

Route 6 made things **slightly worse** — lower throughput and higher CPU.

### Why It Didn't Help

1. **The memcpy it eliminates is cheap.** A 1500-byte copy fits in L1 cache (~32KB). At the speeds we operate (~700 Mbps), this is ~60K copies/sec of hot cache lines. Cost: negligible.
2. **The machinery it adds is expensive.** Per-packet `mem::swap` of `PacketBuf` structs + iovec pointer relinking + mmsghdr update adds branches and pointer chasing that aren't free.
3. **DashMap shard lock held longer.** The zero-copy path does the swap while holding the `get_ref()` guard, extending the shard lock duration vs the copy path which drops the guard before pool operations.

### Key Learning

**Don't optimize memcpy of hot cache lines.** The 1500-byte frame copy is L1-resident and essentially free. The overhead of avoiding it (buffer lifecycle management, pointer juggling) exceeds the copy cost. This applies broadly: zero-copy only wins when the copy is large (>4KB) or crosses cache/NUMA boundaries. For MTU-sized Ethernet frames in a tight loop, the copy is the right choice.

**DO NOT retry this route.** The implementation was clean and correct — the approach is fundamentally wrong for this workload.

---

## Remaining Optimization Routes

### Route 4: AF_PACKET + PACKET_MMAP (High Risk, High Impact)

**Status:** Failed attempt on `feat/packet-mmap-wip`. See Post-Mortem above. Last resort.

**Prerequisite:** Validate `PACKET_IGNORE_OUTGOING` + simple BPF + SOCK_RAW + TPACKET_V3 in a standalone test program before integrating.

### Route 7: Needs Research / Future

| Optimization | Description | Prereq |
|-------------|-------------|--------|
| **io_uring** | Replace `recvmmsg`/`sendmmsg` with io_uring SQE batches. Zero syscall overhead. | Kernel >= 5.19 |
| **GRO/GSO on TAP** | Kernel aggregates small packets into large ones before delivery to TAP. | Test with `ethtool -K tap0 gro on` |
| **XDP redirect** | Full kernel data plane (Phase 10 PRD). Bypasses all userspace. | Significant arch change |
| **CPU pinning** | Pin RX thread to CPU core, set IRQ affinity to match. | Bare metal only |
| **Busy-poll** | `SO_BUSY_POLL` on raw socket. Trades CPU for latency. | Dedicated machines |
| **Zero-copy TAP** | `IFF_VNET_HDR` + `TUNSNDBUF` tuning. | Research needed |
| **recvmmsg for IPv6** | v6 RX uses blocking `recvfrom()`. Add batched receive. | Low risk, matches v4 path |

---

## Implementation Order (Remaining)

1. **Route 4** (PACKET_MMAP revisit) — high risk, standalone validation first
2. **Route 7** (io_uring / XDP / GRO) — future, requires architecture changes or kernel upgrades

---

## Success Criteria

| Metric | Pre-opt (alpha.1) | Current (Route 5) | Target | Stretch |
|--------|-------------------|--------------------|--------|---------|
| TX throughput | 369 Mbps | **704 Mbps** ✓ | 800 Mbps | Link-speed |
| RX throughput | 279 Mbps | **623 Mbps** ✓ | 700 Mbps | Link-speed |
| RX CPU (per Gbps) | ~74% | ~47% | < 35% | < 20% |
| Latency overhead | ~190 us | ~190 us | < 150 us | < 100 us |
| Cross-compat | Full | **Full** ✓ | Full | Full |
| IPv6 transport | Broken | **Working** ✓ | Working | Working |
| aarch64 support | None | **Working** ✓ | Working | Working |

All measurements on Hetzner CPX22 with iperf3 single-stream TCP, 3-round Latin square average.
