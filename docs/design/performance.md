# EoIP-rs Performance Design

## 1. Syscall Optimization

### Batch I/O

EoIP-rs uses `recvmmsg(2)` and `sendmmsg(2)` as the primary I/O interface for raw sockets. These are Linux-specific syscalls (kernel 2.6.33+) that amortize syscall overhead across multiple messages.

```
 Syscall comparison for N packets:

 ┌──────────────────┬────────────┬────────────────┬─────────────┐
 │ Method           │ Syscalls   │ Kernel crosses │ Best for    │
 ├──────────────────┼────────────┼────────────────┼─────────────┤
 │ write() loop     │ N          │ N              │ simplicity  │
 │ writev()         │ N          │ N              │ gather I/O  │
 │ sendmmsg()       │ 1          │ 1              │ throughput  │
 │ io_uring batched │ 0 (async)  │ 0 (ring)       │ future      │
 └──────────────────┴────────────┴────────────────┴─────────────┘
```

`sendmmsg`/`recvmmsg` send/receive **multiple independent messages** in one syscall — each a complete encapsulated frame to a different tunnel endpoint.

### Raw Socket Configuration

```rust
// GRE raw socket
let fd = socket(AF_INET, SOCK_RAW | SOCK_NONBLOCK | SOCK_CLOEXEC, IPPROTO_GRE);

// Let kernel build IP header (unless we need custom TTL/ToS per tunnel)
setsockopt(fd, IPPROTO_IP, IP_HDRINCL, &0);

// Increase socket buffers for burst absorption
setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &(4 * 1024 * 1024));  // 4MB
setsockopt(fd, SOL_SOCKET, SO_SNDBUF, &(4 * 1024 * 1024));  // 4MB

// Enable PMTU discovery
setsockopt(fd, IPPROTO_IP, IP_MTU_DISCOVER, &IP_PMTUDISC_DO);
```

### RX Loop (Dedicated Thread)

```rust
fn rx_loop(raw_fd: RawFd, demux: &DashMap<DemuxKey, TunnelHandle>) {
    const BATCH: usize = 64;

    // Pre-allocate on stack
    let mut iovecs: [iovec; BATCH] = zeroed();
    let mut msgs: [mmsghdr; BATCH] = zeroed();
    let mut addrs: [sockaddr_in; BATCH] = zeroed();
    let mut bufs: Vec<PacketBuf> = pool.get_batch(BATCH);

    // Wire up iovec → buffer, msghdr → iovec + addr
    for i in 0..BATCH {
        iovecs[i].iov_base = bufs[i].payload_ptr();
        iovecs[i].iov_len = bufs[i].capacity();
        msgs[i].msg_hdr.msg_iov = &mut iovecs[i];
        msgs[i].msg_hdr.msg_iovlen = 1;
        msgs[i].msg_hdr.msg_name = &mut addrs[i] as *mut _;
        msgs[i].msg_hdr.msg_namelen = size_of::<sockaddr_in>();
    }

    loop {
        let n = recvmmsg(raw_fd, &mut msgs, BATCH, MSG_DONTWAIT, null());

        if n <= 0 {
            if errno() == EAGAIN {
                wait_readable(raw_fd);  // epoll_wait
                continue;
            }
            continue;
        }

        for i in 0..n {
            let len = msgs[i].msg_len;
            let src_ip = addrs[i].sin_addr;
            let ip_hdr_len = (bufs[i].data[0] & 0x0F) * 4;
            let tunnel_id = parse_tunnel_id(&bufs[i].data[ip_hdr_len..]);

            let key = DemuxKey { tunnel_id, remote_ip: src_ip };
            if let Some(handle) = demux.get(&key) {
                bufs[i].set_payload(ip_hdr_len + GRE_HEADER_LEN, len);
                handle.stats.record_rx(len as u64);
                let _ = handle.tx.try_send(bufs[i].take());
            }
            // Miss: drop, increment counter
        }

        // Replenish consumed buffers from pool
        replenish_bufs(&mut bufs, &mut iovecs);
    }
}
```

---

## 2. Memory Management

### Buffer Pool

```
 ┌──────────────────────────────────────────────────────┐
 │                    BufferPool                         │
 │                                                      │
 │   ArrayQueue<PacketBuf>  (lock-free MPMC)            │
 │   [buf][buf][buf][buf] ... [buf][buf][buf]           │
 │                                                      │
 │   get() → Option<PacketBuf>    (pop)                 │
 │   put(buf)                     (push, Drop impl)     │
 │                                                      │
 │   Exhaustion → drop packet (no heap fallback)        │
 └──────────────────────────────────────────────────────┘
```

### Buffer Lifecycle

```
 Pool get → Reset head/tail → Fill (recvmmsg/TAP read) →
 Process (demux, header strip/prepend) → Send (TAP write/sendmmsg) →
 Return to pool (Drop impl)
```

### Sizing

```
buf_size = 64 (headroom) + 1522 (max frame) = 1586 bytes

pool_count = max(256, batch_size * 4 + tunnels * 2)
memory = pool_count * 1586

  100 tunnels:  ~456 bufs = ~707 KB
  1000 tunnels: ~2256 bufs = ~3.5 MB
  4096 bufs (max default):    ~6.3 MB
```

### Hot Path Invariants

1. **No `malloc` in RX loop.** All buffers from pool. Exhausted → drop.
2. **No `String` or `Vec` construction** in packet processing. Parse on byte slices.
3. **No `format!` or logging** at default level in hot path. `tracing` with compile-time filtering; `trace!` is zero-cost when disabled.
4. **Stack-allocated scratch** for header construction (GRE=8B, IP=20B — fits on stack).

---

## 3. Throughput Targets

| Metric | Target | Notes |
|--------|--------|-------|
| TCP throughput (MTU 1500) | 3–8 Gbps | Bottleneck: memcpy through TAP |
| TCP throughput (jumbo 9000) | 8–15 Gbps | Fewer packets per byte |
| Packet rate (64B frames) | 500K–1.2M pps | Small packets are syscall-bound |
| Latency added (idle) | 50–200 us | TAP + context switch dominated |
| Latency p99 (loaded) | < 500 us | Adaptive batching bounds tail latency |

### Comparison

| Implementation | Throughput | Packet Rate | Type |
|---------------|------------|-------------|------|
| MikroTik EoIP (kernel) | ~10 Gbps | ~1.5 Mpps | Kernel module |
| Linux GRE (kernel) | ~15 Gbps | ~2 Mpps | Kernel module |
| wireguard-go (userspace) | ~3–5 Gbps | ~600K pps | Userspace (similar arch) |
| **EoIP-rs target** | **3–8 Gbps** | **800K+ pps** | **Userspace** |

---

## 4. Benchmarking Strategy

### Test Setup

```
 ┌──────────┐                              ┌──────────┐
 │  Host A  │    Physical or veth link     │  Host B  │
 │ eoip-rs ─┼──────────────────────────────┼─ eoip-rs │
 │  TAP0    │                              │  TAP0    │
 │   │      │                              │   │      │
 │ bridge0  │                              │ bridge0  │
 │   │      │                              │   │      │
 │ 10.0.0.1 │                              │ 10.0.0.2 │
 └──────────┘                              └──────────┘
```

### Tests

**Throughput (iperf3):**
```bash
iperf3 -c 10.0.0.2 -t 30 -P 4         # 4 parallel TCP streams
iperf3 -c 10.0.0.2 -t 30 -P 4 -R      # reverse direction
iperf3 -c 10.0.0.2 -t 30 -u -b 10G    # UDP flood (max pps)
```

**Packet rate (small packets):**
```bash
iperf3 -c 10.0.0.2 -u -l 18 -b 0 -t 10  # 18B payload ≈ 64B on wire
```

**Latency:**
```bash
# Baseline (no tunnel):
ping -c 1000 -i 0.01 <physical_ip>

# Through tunnel:
ping -c 10000 -i 0.001 10.0.0.2 | percentile_calc
```

### Profiling

```bash
# CPU flamegraph
perf record -g --call-graph=dwarf -p $(pidof eoip-rs) -- sleep 10
perf script | inferno-collapse-perf | inferno-flamegraph > flame.svg

# Syscall overhead
perf stat -e syscalls:sys_enter_sendmmsg,syscalls:sys_enter_recvmmsg \
  -p $(pidof eoip-rs) -- sleep 10

# Cache effectiveness
perf stat -e cache-misses,cache-references,instructions \
  -p $(pidof eoip-rs) -- sleep 10

# Lock contention (DashMap validation)
perf lock record -p $(pidof eoip-rs) -- sleep 5
perf lock report
```

### Benchmark Matrix

| Test | Metric | Tool | Target |
|------|--------|------|--------|
| TCP throughput (1500) | Gbps | iperf3 | > 5 Gbps |
| TCP throughput (9000) | Gbps | iperf3 | > 10 Gbps |
| UDP pps (64B) | Kpps | iperf3 -u | > 800 Kpps |
| Ping latency (idle) | us | ping | < 150 us added |
| Ping latency (loaded) | us | ping + iperf3 | < 500 us p99 |
| Memory (100 tunnels) | MB | /proc/pid/status | < 50 MB RSS |
| CPU per Gbps | % core | perf/top | < 25% per Gbps |

### Criterion Microbenchmarks

Integrated in CI for regression detection:

- `bench_parse_gre_header` — parse 1000 GRE headers
- `bench_demux_lookup` — DashMap lookup under contention
- `bench_buffer_pool_get_put` — pool round-trip
- `bench_batch_accumulate` — batch state machine transitions
- `bench_header_prepend` — headroom-based header insertion

Track with `critcmp` for cross-commit comparison.

---

## 5. io_uring (Future)

Behind feature flag `io_uring`. Replaces:

| Current | io_uring Equivalent | Benefit |
|---------|-------------------|---------|
| `recvmmsg` | Multi-shot receive | Zero syscall overhead |
| `sendmmsg` | Send SQE batch | Async submission |
| TAP read/write | Registered fd ops | Reduced fd lookup |
| Buffer pool | Provided buffers | Kernel fills pool directly |

Requires kernel >= 5.19 for multi-shot recv. The existing buffer pool design (fixed-size, pre-allocated) is compatible with io_uring's provided-buffer ring.
