# EoIP-rs Performance Optimization PRDs

**Audit Date:** 2026-04-30  
**Author:** Kimi Code CLI (performance audit)  
**Target Version:** 0.1.0+  

---

## PRD-1: Release Profile Optimizations

### Problem
The workspace `Cargo.toml` and all crate `Cargo.toml` files contain **no `[profile.release]` section**. `cargo build --release` uses Rust defaults:
- `lto = false` — no cross-crate inlining
- `codegen-units = 16` — limits optimization scope
- `panic = "unwind"` — unwinding tables bloat binary and add landing pads

For a packet-forwarding daemon where hot paths span `eoip-proto` → `eoip-rs`, missing LTO prevents inlining of small functions like `gre::decode_eoip_header`, `DashMap::get`, and atomic `fetch_add` wrappers.

### Expected Impact
- **10–30% throughput improvement** (typical for network daemons with LTO + codegen-units=1)
- **Smaller binary** (`strip = true`, `panic = "abort"`)
- **Faster build** at cost of longer release compile time (acceptable tradeoff)

### Implementation
Add to workspace root `Cargo.toml`:

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

### Verification
1. `cargo build --release` succeeds
2. Binary size is smaller (`ls -l target/release/eoip-rs`)
3. Throughput benchmark before/after (same VM, same tunnel config)

### Effort
**Tiny** — 6 lines of TOML, zero code changes.

---

## PRD-2: Fix `PacketBuf::drop` Dummy Allocation

### Problem
`PacketBuf::drop` uses `std::mem::replace` to move the buffer out of `self`, but `replace` requires a new value to put back:

```rust
let mut returned = PacketBuf {
    data: std::mem::replace(&mut self.data, Box::new([0u8; BUF_TOTAL])),
    // ...
};
```

The `Box::new([0u8; BUF_TOTAL])` (~1.6 KB) is allocated, immediately placed into `self.data`, and then `self` is dropped — freeing it. **One useless allocation per packet.**

At 30K packets/sec (346 Mbps), this is **~50 MB/sec of heap churn**, causing:
- allocator lock contention
- cache pollution
- higher latency jitter

### Design
Restructure `PacketBuf` so `drop` can hand off ownership without allocating a replacement.

**Option A (Recommended):** Make `data` an `Option<Box<[u8; BUF_TOTAL]>>`:

```rust
pub struct PacketBuf {
    data: Option<Box<[u8; BUF_TOTAL]>>,
    head: usize,
    len: usize,
    pool: Option<Arc<ArrayQueue<PacketBuf>>>,
}
```

`Drop` becomes:
```rust
impl Drop for PacketBuf {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.take() {
            let mut returned = PacketBuf {
                data: self.data.take(), // zero alloc
                head: HEADER_HEADROOM,
                len: 0,
                pool: None,
            };
            let _ = pool.push(returned);
        }
    }
}
```

All accessors become `self.data.as_mut().unwrap()[...]` or similar. `new()` becomes `data: Some(Box::new([0u8; BUF_TOTAL]))`.

**Option B:** Keep `data: Box<...>` but return to pool via a helper method called before drop, bypassing `Drop` entirely. More invasive (requires call-site changes).

### Expected Impact
- **Eliminates ~1.6 KB allocation per packet** (both RX and TX paths)
- Lower allocator pressure
- Measurable throughput gain on iperf3 (5–15% estimated)

### Verification
1. `cargo test` passes (existing pool tests cover this)
2. `pool_no_leak_after_many_cycles` still passes
3. Profile with `perf` + `dhall` or `heaptrack` shows zero allocations in `PacketBuf::drop`

### Risks
- `Option<Box<...>>` adds one extra branch in `payload_mut()` / `as_slice()`. LLVM likely optimizes this away because `data` is `Some` for all valid lifetimes.
- If any code uses `PacketBuf` after partial move, it won't compile (Rust prevents this).

### Effort
**Small** — struct + Drop refactor, update ~5 accessor methods, test fixes.

---

## PRD-3: Reuse TX Batcher Allocation Buffers

### Problem
On every batch flush, `flush_batch` and `send_batch` allocate fresh `Vec`s:

| Vec | Capacity | Allocated per flush |
|-----|----------|---------------------|
| `v4_pkts` | high_water (256) | ~2 KB |
| `v6_pkts` | high_water (256) | ~2 KB |
| `iovecs` | len | ~4 KB |
| `addrs_v4` | len | ~4 KB |
| `addrs_v6` | len | ~4 KB |
| `is_v6` | len | ~256 B |
| `msgs` | len | ~8 KB |

With 346 Mbps and batch timeout 50 µs, flushes happen frequently. Each flush triggers 7+ heap allocations.

### Design
Move reusable `Vec`s into the batcher task's async closure, pass them by mutable reference:

```rust
tokio::spawn(async move {
    let mut batch: Vec<TxPacket> = Vec::with_capacity(high_water);
    let mut iovecs: Vec<libc::iovec> = Vec::with_capacity(high_water);
    let mut addrs_v4: Vec<libc::sockaddr_in> = Vec::with_capacity(high_water);
    let mut addrs_v6: Vec<libc::sockaddr_in6> = Vec::with_capacity(high_water);
    let mut is_v6: Vec<bool> = Vec::with_capacity(high_water);
    let mut msgs: Vec<libc::mmsghdr> = Vec::with_capacity(high_water);

    loop {
        // ... recv packets ...
        flush_batch(
            raw_v4_fd, raw_v6_fd, &mut batch,
            &mut iovecs, &mut addrs_v4, &mut addrs_v6, &mut is_v6, &mut msgs,
        );
    }
})
```

`flush_batch` and `send_batch` take `&mut Vec<T>` and `clear()` them instead of creating new ones.

### Expected Impact
- **Zero heap allocations in TX flush hot path**
- Reduced cache misses (buffers stay hot in L1/L2)
- 5–10% throughput improvement under batch-heavy load

### Verification
1. `cargo test` passes
2. `heaptrack` or `valgrind --tool=massif` shows no allocations in `flush_batch` / `send_batch`
3. iperf3 throughput benchmark before/after

### Risks
- Function signatures grow longer. Can mitigate with a `TxBatchBuffers` struct.
- `libc::mmsghdr` contains raw pointers to `iovec`/`sockaddr` elements. Since `Vec` may reallocate on `push`, the pointers must be established **after** all pushes complete (current code already does this, but must be verified after refactor).

### Effort
**Small** — signature refactor, add `.clear()` calls, no logic changes.

---

## PRD-4: Cache TX Timestamp (Coarse Time)

### Problem
`read_and_encode` calls `SystemTime::now()` for **every transmitted packet**:

```rust
let now_ms = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_millis() as i64)
    .unwrap_or(0);
handle.stats.last_tx_timestamp.store(now_ms, Ordering::Relaxed);
```

Even vDSO-accelerated `clock_gettime` costs ~20–30 ns. At high PPS, this adds up. The RX path already solved this with `coarse_timestamp_ms()` (cached every 64 packets via thread-local).

### Design
Reuse the existing `coarse_timestamp_ms()` helper in `packet/rx.rs`:

1. Move `coarse_timestamp_ms()` to a shared module (e.g., `packet/time.rs` or `util/time.rs`)
2. Call it from `read_and_encode` instead of `SystemTime::now()`
3. Alternatively, only update the timestamp every N packets (e.g., 64):

```rust
let tx_count = handle.stats.tx_packets.fetch_add(1, Ordering::Relaxed);
if tx_count % 64 == 0 {
    handle.stats.last_tx_timestamp.store(coarse_timestamp_ms(), Ordering::Relaxed);
}
```

### Expected Impact
- **~20–30 ns saved per packet** (small but in the hot path)
- Consistent with RX path design

### Verification
1. `cargo test` passes
2. `perf stat -e syscalls:sys_enter_clock_gettime` shows fewer syscalls during iperf3

### Effort
**Tiny** — extract function, change one call site.

---

## PRD-5: Fix `log_demux_miss` Rate-Limiting Bug

### Problem
`log_demux_miss` has a bug in its timestamp logic:

```rust
let now = Instant::now();
let now_ms = now.elapsed().as_millis() as u64; // BUG: always ~0
```

`Instant::now().elapsed()` measures time since creation — always near zero. The CAS rate-limiting therefore **fires on every demux miss**, causing:
- Unnecessary atomic contention (`compare_exchange`)
- Tracing log spam if miss volume is high (malformed traffic, scan attacks)

### Design
Fix to use actual elapsed time since a reference point:

```rust
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn log_demux_miss(key: &DemuxKey) {
    let count = MISS_COUNT.fetch_add(1, Ordering::Relaxed);
    let now_ms = START.get_or_init(Instant::now).elapsed().as_millis() as u64;
    // ... rest unchanged
}
```

Or simpler: reuse `coarse_timestamp_ms()` (which returns wall-clock millis).

### Expected Impact
- Correct rate-limiting (1 log per second max)
- Eliminates spurious atomic CAS on every miss

### Verification
1. Unit test: simulate 1000 misses, assert only 1–2 log events
2. `cargo test` passes

### Effort
**Tiny** — one-line logic fix.

---

## PRD-6: Reduce Error-Path Allocations in `eoip-proto`

### Problem
Magic-mismatch error paths allocate `Vec<u8>` for diagnostic bytes:

```rust
Err(EoipError::InvalidMagicBytes {
    expected: EOIP_MAGIC.to_vec(),   // alloc
    got: buf[0..4].to_vec(),         // alloc
})
```

Under DoS with malformed packets, every rejected packet triggers 2 heap allocations.

### Design
Change `EoipError` to use stack-allocated / borrowed data:

```rust
#[derive(Debug)]
pub struct InvalidMagicBytes {
    pub expected: &'static [u8],
    pub got: [u8; 4],  // fixed-size array, no alloc
}
```

For `udp_shim.rs` (2-byte magic): `[u8; 2]`.

### Expected Impact
- **Zero allocations on all error paths** in packet parsing
- DoS resilience improvement

### Verification
1. `cargo test` passes
2. `proptest` regression passes
3. `heaptrack` on fuzz input shows no allocations in `gre::decode`

### Risks
- Changes public error type; if downstream code matches on `InvalidMagicBytes`, it may need updating. But `expected`/`got` are just fields — same API surface.

### Effort
**Tiny** — change 2 error variants, 4 call sites.

---

## PRD-7: Remove Unused `bytes` Dependency from `eoip-proto`

### Problem
`bytes.workspace = true` is declared in `eoip-proto/Cargo.toml` but the crate uses raw `&[u8]` / `&mut [u8]` exclusively. Adds compile time and binary bloat.

### Design
Remove `bytes` from `eoip-proto/Cargo.toml`.

### Verification
1. `cargo build` succeeds
2. `cargo check -p eoip-proto` succeeds
3. No `unused_imports` warnings

### Effort
**Trivial** — delete one line.

---

## PRD-8: Add Criterion Benchmarks for Hot Paths

### Problem
There are **no benchmarks** in the project. Performance improvements cannot be measured locally; only via full integration tests with VMs.

### Design
Add `criterion` dev-dependency and `benches/` directory:

```toml
# crates/eoip-rs/Cargo.toml
[[bench]]
name = "packet_pipeline"
harness = false
```

**Benchmark suites:**

1. **`benches/buffer_pool.rs`** — `BufferPool::get()` + `drop` cycles
2. **`benches/encode_decode.rs`** — `gre::encode` / `decode` throughput
3. **`benches/dashmap_demux.rs`** — `TunnelRegistry::get_ref` latency
4. **`benches/tx_batch.rs`** — `flush_batch` throughput with synthetic packets

Example:
```rust
fn bench_buffer_pool(c: &mut Criterion) {
    let pool = BufferPool::new(1024);
    c.bench_function("pool_get_drop", |b| {
        b.iter(|| {
            let buf = pool.get();
            black_box(buf);
            // drop returns to pool
        })
    });
}
```

### Expected Impact
- Enables local, fast performance regression testing
- Validates PRD-2, PRD-3, PRD-4 quantitatively

### Verification
1. `cargo bench` runs and produces reports
2. CI can run `cargo bench` with `--baseline` for comparisons

### Effort
**Small** — add dev-deps, write 4 benchmark files (~200 lines total).

---

## PRD-9: TX Batcher Sharding (Future / Optional)

### Problem
All tunnels funnel through **one** `tokio::sync::mpsc` channel and **one** TX batcher task. Under high aggregate load (100 tunnels × line rate), this single async task becomes a bottleneck. Tokio tasks are cooperative; one batcher cannot saturate multiple CPU cores.

### Design (High-Level)
Shard by tunnel ID hash into N batcher tasks (where N = number of CPUs or a config value):

```
TAP Reader (per tunnel)
    |
    v
hash(tunnel_id) % N  ->  select mpsc channel
    |
    v
Batcher[i] (dedicated tokio task or OS thread)
    |
    v
sendmmsg on shared raw socket (with seqlock or per-batcher socket)
```

**Alternative:** Replace tokio batcher with dedicated OS thread + `crossbeam::channel` + busy-polling, mirroring the RX architecture.

### Expected Impact
- Scales aggregate TX throughput linearly with CPU cores
- Removes tokio executor from TX data path

### Risks
- Higher complexity
- Raw socket sharing between threads requires `SO_REUSEADDR` or per-thread sockets
- Reordering risk if multiple threads send to same peer

### Effort
**Large** — architectural change, extensive testing, reordering analysis.

---

## Effort vs. Impact Chart

| PRD | Title | Effort | Performance Impact | Risk | Do First? |
|-----|-------|--------|-------------------|------|-----------|
| 1 | Release Profile Optimizations | ⭐ Tiny | 🔴 High (10–30%) | None | **YES** |
| 5 | Fix `log_demux_miss` Timestamp Bug | ⭐ Tiny | 🟡 Medium (DoS resilience) | None | **YES** |
| 7 | Remove Unused `bytes` Dep | ⭐ Trivial | 🟢 Low (compile time) | None | **YES** |
| 4 | Cache TX Timestamp | ⭐ Tiny | 🟡 Medium (hot path) | None | **YES** |
| 6 | Reduce Error-Path Allocations | ⭐ Tiny | 🟡 Medium (DoS) | Low | **YES** |
| 2 | Fix `PacketBuf::drop` Allocation | ⭐⭐ Small | 🔴 High (5–15%) | Low | **YES** |
| 3 | Reuse TX Batcher Vecs | ⭐⭐ Small | 🟠 High (5–10%) | Low | **YES** |
| 8 | Add Criterion Benchmarks | ⭐⭐ Small | 🟢 Low (enablement) | None | After code fixes |
| 9 | TX Batcher Sharding | ⭐⭐⭐⭐ Large | 🔴 High (scaling) | High | Future phase |

**Recommended execution order:** 1 → 5 → 7 → 4 → 6 → 2 → 3 → 8 → 9

All PRDs 1–8 together represent roughly **1–2 days of focused work** and should yield **20–50% combined throughput improvement** with zero architectural risk.
