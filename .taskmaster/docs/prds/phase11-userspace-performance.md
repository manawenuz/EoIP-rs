# Phase 11: Userspace Performance Optimization

**Status:** Active
**Priority:** High — squeeze maximum throughput from the userspace data plane before jumping to XDP
**Dependencies:** Phase 8 (baseline throughput numbers), Phase 10 PRD (context for ceiling)
**Branch:** `feat/packet-mmap-wip` (WIP, not merged — see Post-Mortem below)
**Baseline:** v0.1.0-alpha.1 + IPv6 fix (commit `064486b`)

---

## Objective

Maximize EoIP-rs userspace RX/TX throughput on commodity Linux VMs (Hetzner CPX22, 2 shared vCPU) before investing in kernel-bypass approaches (XDP/eBPF, io_uring). The current architecture leaves performance on the table with single-`sendto` TX, untuned socket buffers, and per-packet TAP writes.

## Current Baseline

**Test Environment:** 2x Hetzner CPX22 (2 vCPU, 4 GB RAM, shared x86_64), Ubuntu 22.04, Linux 5.15, Rust 1.95.0.

| Metric | Value | Notes |
|--------|-------|-------|
| TX throughput (iperf3) | **500 Mbps** | Single stream, node1 → node2 |
| RX throughput (iperf3) | **424 Mbps** | Single stream, node2 → node1 |
| TX CPU | 1.3% | Negligible — TX is not the bottleneck |
| RX CPU | 20.3% | This is where optimization matters |
| Latency overhead | ~190 us | Acceptable for L2 tunnel |
| Cross-compat | Full | new+old, old+new all work |

Previous Phase 8 numbers on CX23 were 346 Mbps TX / 135 Mbps RX. The improvement to 500/424 is from recompiling with Rust 1.95 + the `recvmmsg` batching optimization (commit `bce0fb5`). The RX CPU is the primary target.

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

## Optimization Routes

### Route 1: `sendmmsg` for TX (Low Risk, Medium Impact)

**Current state:** TX uses individual `sendto()` calls per packet via `nix::sys::socket::sendto()` in a loop (`flush_batch` in `tx.rs`). Despite the "batch" name, each packet is a separate syscall.

**Proposed:** Replace the `sendto` loop with `sendmmsg()`, matching the RX path's `recvmmsg` batching. This amortizes syscall overhead across the entire batch.

```
Before: N packets → N sendto() syscalls
After:  N packets → 1 sendmmsg() syscall
```

**Expected impact:** TX is currently at 1.3% CPU for 500 Mbps. The headroom is large, but `sendmmsg` would reduce syscall overhead and improve throughput ceiling when the link allows more than 500 Mbps (e.g., dedicated VMs, bare metal).

**Complexity:** Low. The batch infrastructure already exists. Replace the for loop in `flush_batch()` with `libc::sendmmsg()`.

### Route 2: `SO_RCVBUF` / `SO_SNDBUF` Tuning (Low Risk, Low-Medium Impact)

**Current state:** Socket buffer sizes use kernel defaults (~212 KB on most systems). Under burst traffic, the kernel drops packets before userspace can drain them.

**Proposed:** Set `SO_RCVBUF` to 4 MB and `SO_SNDBUF` to 4 MB on the raw GRE socket. This matches the design doc's recommendation (`docs/design/performance.md` line 34-35) which was never implemented.

```rust
sock.set_recv_buffer_size(4 * 1024 * 1024)?;
sock.set_send_buffer_size(4 * 1024 * 1024)?;
```

**Expected impact:** Reduces packet drops under burst load. Most visible during iperf3 RX where the daemon processes packets in batches — larger socket buffers absorb inter-batch gaps.

**Complexity:** Trivial. Two `setsockopt` calls in `rawsock.rs`.

### Route 3: `writev` / `readv` on TAP (Medium Risk, Medium Impact)

**Current state:** TAP reads use single `AsyncFd::read()` calls (one frame per syscall). TAP writes use `libc::write()` in a dedicated thread (one frame per syscall).

**Proposed:** Use `readv()` / `writev()` to batch multiple Ethernet frames per TAP syscall. Linux TAP devices support this when `IFF_MULTI_QUEUE` is set.

**Expected impact:** Reduces per-frame syscall overhead on the TAP side. Currently each received GRE packet triggers one TAP write syscall. Batching 8-32 writes into one `writev()` would reduce context switches proportionally.

**Complexity:** Medium. Requires:
- Enabling `IFF_MULTI_QUEUE` on TAP creation
- Accumulating frames before writing (need a mini-batcher or drain the crossbeam channel in batches)
- Verifying frame boundaries are preserved (each `iovec` entry must be one complete Ethernet frame)

**Risk:** TAP `writev` behavior varies by kernel version. Needs testing.

### Route 4: AF_PACKET + PACKET_MMAP (High Risk, High Impact)

**Current state:** Failed attempt on `feat/packet-mmap-wip`. See Post-Mortem above.

**Proposed:** Revisit with the lessons learned. Specifically:
1. Use `SOCK_RAW` (not `SOCK_DGRAM`)
2. Use `PACKET_IGNORE_OUTGOING` (not BPF pkttype extension)
3. Use simple GRE-only BPF filter at offset 23
4. Create socket directly in daemon (not via helper)
5. **Validate under sustained iperf3 load before considering it done**

The ring buffer itself (`packet_mmap.rs`) is structurally sound. The bugs were all in socket setup and filtering, not in the ring processing logic. However, the ring's behavior under sustained load was never successfully validated.

**Expected impact:** Eliminates one memcpy per RX packet (kernel → recv buffer). Reduces syscall count to near-zero (poll only when ring is empty). Theoretical 2x RX throughput improvement.

**Complexity:** High. Four bugs encountered in first attempt. The TPACKET_V3 API has poor documentation and many kernel-version-specific behaviors.

**Prerequisite:** Validate `PACKET_IGNORE_OUTGOING` + simple BPF + SOCK_RAW + TPACKET_V3 in a standalone test program before integrating.

### Route 5: Needs Research

The following optimizations require further investigation before a concrete plan can be formed:

| Optimization | Description | Research Needed |
|-------------|-------------|-----------------|
| **io_uring** | Replace `recvmmsg`/`sendmmsg` with io_uring SQE batches. Zero syscall overhead. | Requires kernel >= 5.19 for multi-shot recv. Need to validate with raw sockets and TAP devices. |
| **GRO/GSO on TAP** | Generic Receive/Segmentation Offload on the TAP device. Kernel aggregates small packets into large ones before delivery. | `ethtool -K tap0 gro on` — does this work with EoIP's GRE encap? Need to test. |
| **XDP redirect** | Full kernel data plane (Phase 10 PRD). Bypasses all userspace for data packets. | Already has a PRD. Significant complexity. Requires eBPF C code, map management, fallback path. |
| **CPU pinning / NUMA** | Pin RX thread to specific CPU core, set IRQ affinity to match. | `taskset` + `/proc/irq/*/smp_affinity`. Relevant on bare metal, likely no benefit on shared vCPU. |
| **Busy-poll** | `SO_BUSY_POLL` / `SO_PREFER_BUSY_POLL` on the raw socket. Kernel spins in poll instead of sleeping. | Trades CPU for latency. Useful for dedicated machines, wasteful on shared VMs. |
| **Zero-copy TAP** | `IFF_VNET_HDR` + `TUNSNDBUF` tuning. Potentially allow the kernel to DMA directly from pool buffers. | Needs investigation into Linux TAP zero-copy support. |

---

## Implementation Order

Recommended sequence based on risk/reward:

1. **SO_RCVBUF/SO_SNDBUF tuning** — trivial change, immediate measurable impact
2. **sendmmsg for TX** — low risk, completes the batch I/O story
3. **writev on TAP** — medium risk, attacks the TAP write bottleneck
4. **PACKET_MMAP (revisit)** — high risk, but highest potential payoff before XDP
5. **io_uring / XDP** — future, requires significant architecture changes

Each route should be implemented as a **single commit**, benchmarked against the baseline, and only merged if it shows measurable improvement with no regression. The `feat/packet-mmap-wip` experience demonstrated what happens when multiple changes are bundled.

---

## Success Criteria

| Metric | Current | Target | Stretch |
|--------|---------|--------|---------|
| TX throughput | 500 Mbps | 550 Mbps | Link-speed |
| RX throughput | 424 Mbps | 550 Mbps | Link-speed |
| RX CPU (per Gbps) | ~48% | < 35% | < 20% |
| Latency overhead | 190 us | < 150 us | < 100 us |
| Cross-compat | Full | Full | Full |

All measurements on Hetzner CPX22 with iperf3 single-stream TCP.
