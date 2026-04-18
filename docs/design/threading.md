# EoIP-rs Threading Model

## 1. Thread Layout

EoIP-rs uses a **hybrid model**: tokio multi-threaded runtime for management, control plane, and TX path, combined with dedicated OS threads for the latency-sensitive RX hot path.

```
 +---------------------------------------------------------------------+
 |                        Process                                       |
 |                                                                      |
 |  +---------------------------------------------------------+        |
 |  |              Tokio Runtime (N worker threads)            |        |
 |  |                                                          |        |
 |  |   +----------+  +----------------+  +--------------+    |        |
 |  |   |  gRPC    |  | Tunnel Manager |  |  TX Tasks    |    |        |
 |  |   |  Server  |  | (create/del)   |  |  (per-tunnel |    |        |
 |  |   |          |  |                |  |   TAP read   |    |        |
 |  |   |          |  |                |  |   + batch)   |    |        |
 |  |   +----------+  +----------------+  +------+-------+    |        |
 |  |                                            |             |        |
 |  |   +------------------+                     |             |        |
 |  |   | IPsec Monitor    |                     |             |        |
 |  |   | (per IPsec tun)  |                     |             |        |
 |  |   | SA poll via VICI |                     |             |        |
 |  |   +------------------+                     |             |        |
 |  |                                            |             |        |
 |  +--------------------------------------------+-------------+        |
 |                                               |                      |
 |       +---------------------------------------+                      |
 |       |         Shared Raw Socket(s)                                 |
 |       |         (proto 47 / proto 97)                                |
 |       +-----------+-----------------------------------------+        |
 |                   |                                         |        |
 |  +----------------+----------------------+  +---------------+-----+  |
 |  |  Dedicated RX Thread (proto 47)       |  |  RX Thread (97)    |  |
 |  |  recvmmsg -> demux -> channel -> TAP  |  |  (same pattern)   |  |
 |  +---------------------------------------+  +---------------------+  |
 +---------------------------------------------------------------------+
```

### Why This Layout

- **RX threads are dedicated OS threads**, not tokio tasks. The RX loop calls `recvmmsg()` in a tight loop -- putting this on a tokio worker would starve other tasks due to cooperative scheduling.
- **TX path uses tokio tasks** because TAP reads are event-driven (epoll-wakeup), not busy-polling.
- **Per-tunnel threads would not scale**: all tunnels share one raw socket per protocol. There's no per-tunnel fd to epoll on for RX.

### Thread Count

| Thread | Count | Configurable |
|--------|-------|-------------|
| Tokio workers | `min(4, num_cpus)` | `--workers N` |
| RX thread (GRE/47) | 1 | `rx_workers` in config |
| RX thread (EtherIP/97) | 1 (only if IPv6 tunnels exist) | Same |
| TAP reader task | 1 per tunnel | Automatic |
| TX batcher task | 1 per protocol | Automatic |
| IPsec monitor task | 1 per IPsec-enabled tunnel | Automatic |

**Note on IPsec and RX path:** When `ipsec_secret` is active on a tunnel, the RX path uses raw socket `recvmmsg()` instead of AF_PACKET/PACKET_MMAP. AF_PACKET captures packets before kernel XFRM decryption, so it would see ESP-encrypted packets rather than decoded GRE. The raw socket path receives post-XFRM decrypted GRE packets correctly.

---

## 2. Adaptive Batching

### State Machine

```
                         +-------------+
          packet arrives |             | flush_timer fires
         +-------------->|   FILLING   +------------------+
         |               |             |                   |
         |               +------+------+                   |
         |                      |                          |
         |         batch.len >= max_batch_size              |
         |         OR queue_depth > high_water              |
         |                      |                          |
         |                      v                          v
         |               +-------------+           +-------------+
         |               |  FLUSHING   |           |  FLUSHING   |
         |               |  (full)     |           |  (timer)    |
         |               +------+------+           +------+------+
         |                      |                          |
         |               sendmmsg(batch)            sendmmsg(batch)
         |                      |                          |
         +----------------------+--------------------------+
                          reset batch, restart timer
```

### Pseudocode

```rust
struct BatchState {
    batch: ArrayVec<MmsgEntry, MAX_BATCH>,
    flush_deadline: Instant,
    config: BatchConfig,
}

struct BatchConfig {
    max_batch_size: usize,          // default: 64
    flush_interval: Duration,       // default: 100us
    queue_depth_threshold: usize,   // default: 32
}

impl BatchState {
    fn on_packet(&mut self, pkt: Packet, queue_depth: usize) -> Action {
        self.batch.push(pkt);

        // Full batch -> flush immediately
        if self.batch.len() >= self.config.max_batch_size {
            return Action::FlushNow;
        }

        // Under pressure -> tighter deadline
        if queue_depth > self.config.queue_depth_threshold {
            self.flush_deadline = min(
                self.flush_deadline,
                Instant::now() + Duration::from_micros(10),
            );
            return Action::Wait;
        }

        // First packet, no backlog -> send immediately (latency-optimized)
        if self.batch.len() == 1 && queue_depth == 0 {
            return Action::FlushNow;
        }

        // First packet with backlog -> set deadline
        if self.batch.len() == 1 {
            self.flush_deadline = Instant::now() + self.config.flush_interval;
        }

        Action::Wait
    }

    fn on_timer(&mut self) -> Action {
        if !self.batch.is_empty() { Action::FlushNow } else { Action::Wait }
    }
}

enum Action { FlushNow, Wait }
```

### Bimodal Behavior

- **Low load** (queue_depth < threshold): Every packet sent immediately with single `sendmsg`. Sub-ms latency. Common for interactive traffic.
- **High load** (queue_depth >= threshold): Packets accumulate up to `max_batch_size` or a 10us micro-deadline, then flush with `sendmmsg`. High throughput. Natural during bulk transfers.

### Tunable Parameters

| Parameter | Default | Range | Effect |
|-----------|---------|-------|--------|
| `max_batch_size` | 64 | 1-1024 | sendmmsg vlen upper bound |
| `flush_interval_us` | 100 | 1-10000 | Timer deadline for partial batches |
| `queue_depth_threshold` | 32 | 0-4096 | Backlog level triggering batch mode |

---

## 3. Lock-Free Data Structures

### Demux Table

```rust
use dashmap::DashMap;

#[derive(Hash, Eq, PartialEq, Clone)]
struct DemuxKey {
    tunnel_id: u16,
    remote_ip: IpAddr,
}

struct TunnelHandle {
    tx: crossbeam::channel::Sender<PacketBuf>,
    stats: Arc<TunnelStats>,
    tap_fd: RawFd,
}

// Global demux -- RX thread looks up every packet
static TUNNEL_MAP: Lazy<DashMap<DemuxKey, TunnelHandle>> = Lazy::new(DashMap::new);
```

**Why DashMap over `RwLock<HashMap>`:**
- RX thread does lookups on every received packet. Under 1Mpps, `RwLock` reader contention from occasional writes (tunnel create/delete) would cause priority inversion.
- DashMap uses sharded internal locking. Reads from different shards never contend. Tunnel mutations (rare) lock only one shard.

### Per-Tunnel Stats

```rust
pub struct TunnelStats {
    pub rx_packets: AtomicU64,
    pub rx_bytes: AtomicU64,
    pub tx_packets: AtomicU64,
    pub tx_bytes: AtomicU64,
    pub rx_errors: AtomicU64,
    pub tx_errors: AtomicU64,
    pub last_rx_timestamp: AtomicU64,  // epoch nanos
}

impl TunnelStats {
    fn record_rx(&self, bytes: u64) {
        self.rx_packets.fetch_add(1, Ordering::Relaxed);
        self.rx_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.last_rx_timestamp.store(now_nanos(), Ordering::Relaxed);
    }

    fn snapshot(&self) -> StatsSnapshot {
        // All Relaxed -- stats are advisory, not correctness-critical
        StatsSnapshot {
            rx_packets: self.rx_packets.load(Ordering::Relaxed),
            rx_bytes: self.rx_bytes.load(Ordering::Relaxed),
            tx_packets: self.tx_packets.load(Ordering::Relaxed),
            tx_bytes: self.tx_bytes.load(Ordering::Relaxed),
            // ...
        }
    }
}
```

### Channel Selection

| Path | Channel | Rationale |
|------|---------|-----------|
| RX thread -> per-tunnel consumer | `crossbeam::channel::bounded(512)` | RX thread is bare OS thread (not async). Crossbeam works without runtime. |
| Tunnel consumer -> TAP write | Direct `write()` | Consumer owns TAP fd, no channel needed. |
| TAP read -> TX batcher | In-task | Single task owns both sides. |
| gRPC -> TunnelManager | `tokio::sync::mpsc(64)` | Both sides async; integrates with waker system. |

**Why crossbeam for the hot path:**
- `tokio::sync::mpsc` requires `.await`, meaning the RX thread would need a tokio context. We avoid this to keep RX free from cooperative scheduling.
- `crossbeam::Sender::try_send` is wait-free on the fast path. If full, drop packet and increment `rx_errors` -- correct backpressure behavior.

---

## 4. Zero-Copy Buffer Pool

### Design

```rust
use crossbeam::queue::ArrayQueue;

struct BufferPool {
    pool: ArrayQueue<PacketBuf>,
    buf_size: usize,
}

struct PacketBuf {
    data: Box<[u8]>,       // [headroom | payload | tailroom]
    head: usize,           // start of valid data
    tail: usize,           // end of valid data
    pool: Arc<BufferPool>, // for return-on-drop
}

const HEADER_HEADROOM: usize = 64;    // IP(20-60) + GRE(8) + padding
const MAX_FRAME_SIZE: usize = 1522;   // 1500 MTU + 14 eth + 4 VLAN + 4 FCS
const BUF_TOTAL: usize = HEADER_HEADROOM + MAX_FRAME_SIZE;  // 1586 bytes
```

### Buffer Layout

```
 +------------------------------------------------------------------+
 |  headroom    |         payload (Ethernet frame)    |unused         |
 |  (64 bytes)  |         (up to 1522 bytes)         |               |
 +------------------------------------------------------------------+
 ^              ^                                    ^
 data[0]     data[head]                         data[tail]

 After header prepend:
 +------------------------------------------------------------------+
 |free|IP+GRE   |         payload (Ethernet frame)    |unused        |
 |    |header   |                                    |               |
 +------------------------------------------------------------------+
       ^        ^                                    ^
    data[head]                                  data[tail]
```

The TX path reads a frame from TAP into `data[HEADER_HEADROOM..]`, then decrements `head` by the header size and writes headers in-place. No reallocation or memmove.

### Pool Sizing

```
pool_count = max(256, rx_batch_size * 4 + max_tunnels * 2)
total_memory = pool_count * 1586 bytes

100 tunnels: ~456 buffers = ~707 KB
1000 tunnels: ~2256 buffers = ~3.5 MB
```

If the pool is exhausted, drop the packet rather than allocate.

---

## 5. CPU Affinity

### Optional Pinning

```toml
[threading]
rx_cpu_affinity = [2, 3]  # pin RX threads to these CPUs
```

- RX threads pin to specified CPUs for cache locality with NIC RSS queues.
- Tokio workers are NOT pinned (work-stealing scheduler manages them).
- When the NIC supports RSS, configure RSS queues to land on the same CPUs.

### NUMA Awareness (Future)

- Not implemented in v1.
- Future: `libnuma`-based buffer allocation on the NUMA node local to the RX thread's CPU.
- Config flag: `numa_aware = false`.

---

## 6. io_uring (Future)

Behind feature flag `io_uring`. Replaces:
- `recvmmsg` -> `io_uring` multi-shot receive
- `sendmmsg` -> `io_uring` send SQEs
- TAP read/write -> `io_uring` registered fds

Requires kernel >= 5.19 for multi-shot recv. The buffer pool design is already compatible with io_uring's provided-buffer mechanism.
