# Phase 10: XDP/eBPF Data Plane Acceleration

**Status:** Draft
**Priority:** Critical — eliminates the 135 Mbps RX throughput ceiling
**Dependencies:** Phase 5 (working userspace tunnel)
**Estimated Duration:** 1-2 weeks
**Cost:** None (reuses existing infrastructure)

---

## Objective

Move the EoIP packet forwarding hot path from userspace to the kernel using eBPF/XDP. The userspace daemon becomes a control plane that loads eBPF programs, populates BPF maps with tunnel configuration, and handles keepalives. The kernel data plane handles all packet encapsulation/decapsulation at wire speed.

## Background

### The Problem

The current userspace RX path is CPU-bound at ~135 Mbps (single core at 76%):

```
NIC → kernel → raw socket → context switch → userspace recvmmsg
→ IP parse → EoIP decode → DashMap lookup → memcpy into pool buffer
→ crossbeam channel → memcpy → TAP write syscall → kernel → TAP device
```

Every packet crosses the kernel/userspace boundary **twice** (recvmmsg + TAP write) with **2-3 memory copies**. This is the fundamental bottleneck — no amount of userspace optimization can eliminate the context switches.

### The Solution

```
NIC → XDP program (kernel) → parse EoIP → strip headers
→ bpf_redirect → TAP device (zero context switches, zero copies)
```

XDP processes packets **before sk_buff allocation**, in the NIC driver's NAPI poll loop. Measured XDP redirect throughput: 3-15 Mpps per core depending on target device type. Expected improvement: **10-50x over userspace**.

## Architecture

### Data Plane (eBPF — kernel space)

```
RX (Ingress — decapsulation):
  Physical NIC
    │
    ▼
  XDP Program (eoip_decap)
    │ Parse: Eth → IPv4 → EoIP GRE header
    │ Lookup: tunnel_id in TUNNEL_MAP (BPF HashMap)
    │ Strip: bpf_xdp_adjust_head(42 bytes)
    │
    ├─ Matched tunnel → XDP_REDIRECT → TAP device (via DEVMAP)
    ├─ Keepalive (payload_len=0) → XDP_PASS → userspace daemon
    └─ No match → XDP_PASS → userspace daemon (fallback)

TX (Egress — encapsulation):
  TAP device
    │
    ▼
  TC/eBPF Program (eoip_encap) on TAP egress
    │ Read: inner Ethernet frame
    │ Lookup: ifindex in TAP_TO_TUNNEL map
    │ Grow: bpf_skb_adjust_room(42 bytes)
    │ Write: Eth + IPv4 + EoIP headers
    │
    └─ TC_ACT_REDIRECT → Physical NIC
```

### Control Plane (Rust — userspace, unchanged)

```
eoip-rs daemon (existing)
  │
  ├─ Load & attach XDP program to physical NIC
  ├─ Load & attach TC program to each TAP device
  ├─ Populate BPF maps (TUNNEL_MAP, DEVMAP, etc.)
  ├─ Handle keepalives (receive from XDP_PASS, send via raw socket)
  ├─ gRPC API (tunnel CRUD, stats, health)
  ├─ CLI (eoip-cli)
  └─ Dynamic tunnel add/remove → update BPF maps
```

### BPF Maps

| Map Name | Type | Key | Value | Purpose |
|----------|------|-----|-------|---------|
| `TUNNEL_MAP` | `HashMap<u16, TunnelInfo>` | `tunnel_id` (u16) | `TunnelInfo` struct | Tunnel config for decap validation |
| `DECAP_DEVMAP` | `DevMapHash` | `tunnel_id` (u32) | TAP ifindex | XDP redirect target for decapsulation |
| `ENCAP_MAP` | `HashMap<u32, EncapInfo>` | TAP ifindex (u32) | `EncapInfo` struct | Encapsulation headers for TX path |
| `NIC_DEVMAP` | `DevMap` | index 0 | Physical NIC ifindex | TC redirect target for encapsulated packets |
| `STATS_MAP` | `PerCpuArray<TunnelStats>` | tunnel_id | `TunnelStats` | Per-tunnel packet/byte counters |

### Shared Types (eoip-xdp-common crate)

```rust
/// Tunnel information for RX decapsulation lookup.
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct TunnelInfo {
    pub tap_ifindex: u32,     // TAP device to redirect to
    pub remote_ip: u32,       // Expected source IP (network byte order)
    pub local_ip: u32,        // Expected destination IP
    pub tunnel_id: u16,       // For validation
    pub _pad: u16,
}

/// Encapsulation info for TX path (TC egress on TAP).
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct EncapInfo {
    pub tunnel_id: u16,
    pub _pad: u16,
    pub local_ip: u32,        // Source IP for outer header (network byte order)
    pub remote_ip: u32,       // Destination IP for outer header
    pub nic_ifindex: u32,     // Physical NIC to redirect to
    pub src_mac: [u8; 6],     // Outer Ethernet source MAC
    pub dst_mac: [u8; 6],     // Outer Ethernet destination MAC (gateway/peer)
}

/// Per-tunnel statistics (per-CPU for lock-free updates).
#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct XdpTunnelStats {
    pub rx_packets: u64,
    pub rx_bytes: u64,
    pub tx_packets: u64,
    pub tx_bytes: u64,
}

/// EoIP GRE header (8 bytes).
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct EoipHeader {
    pub magic: [u8; 4],          // [0x20, 0x01, 0x64, 0x00]
    pub payload_len_be: [u8; 2], // Big-endian
    pub tunnel_id_le: [u8; 2],  // Little-endian
}

impl EoipHeader {
    pub const LEN: usize = 8;
    pub const MAGIC: [u8; 4] = [0x20, 0x01, 0x64, 0x00];

    #[inline(always)]
    pub fn payload_len(&self) -> u16 {
        u16::from_be_bytes(self.payload_len_be)
    }

    #[inline(always)]
    pub fn tunnel_id(&self) -> u16 {
        u16::from_le_bytes(self.tunnel_id_le)
    }
}
```

**All shared types must be `#[repr(C)]` and implement `Copy + Clone`.** No heap allocation, no pointers, no padding ambiguity. Place in the `-common` crate used by both eBPF and userspace.

## Implementation

### Workspace Structure

```
crates/eoip-xdp/
├── Cargo.toml                    # Workspace root for XDP crates
├── eoip-xdp-ebpf/               # eBPF programs (target: bpfel-unknown-none)
│   ├── Cargo.toml
│   ├── rust-toolchain.toml       # nightly (required for eBPF)
│   └── src/
│       └── main.rs               # XDP decap + TC encap programs
├── eoip-xdp-common/              # Shared types
│   ├── Cargo.toml
│   └── src/
│       └── lib.rs                # EoipHeader, TunnelInfo, EncapInfo, stats
└── eoip-xdp/                     # Userspace loader
    ├── Cargo.toml
    ├── build.rs                  # Compiles eBPF crate, embeds bytecode
    └── src/
        └── lib.rs                # XdpAccelerator: load, attach, map population
```

This lives as a **sub-workspace** under `crates/eoip-xdp/`, separate from the main workspace because the eBPF crate requires nightly + `bpfel-unknown-none` target.

### Dependencies

**eBPF crate (`eoip-xdp-ebpf/Cargo.toml`):**

```toml
[package]
name = "eoip-xdp-ebpf"
edition = "2021"

[dependencies]
aya-ebpf = "0.1.1"
aya-log-ebpf = "0.1"
aya-ebpf-bindings = "0.1.2"
network-types = "0.1.0"
eoip-xdp-common = { path = "../eoip-xdp-common" }

[[bin]]
name = "eoip-xdp"
path = "src/main.rs"

[profile.release]
panic = "abort"
```

**Userspace crate (`eoip-xdp/Cargo.toml`):**

```toml
[dependencies]
aya = "0.13.1"
aya-log = "0.2.1"
tokio = { version = "1", features = ["full"] }
anyhow = "1"
log = "0.4"
env_logger = "0.11"
eoip-xdp-common = { path = "../eoip-xdp-common", features = ["user"] }
```

**Toolchain:**

```toml
# eoip-xdp-ebpf/rust-toolchain.toml
[toolchain]
channel = "nightly"
components = ["rust-src"]
```

**Build system:**

```bash
# One-time setup
cargo binstall bpf-linker   # Do NOT cargo install (takes 30+ minutes)

# Build eBPF
cd eoip-xdp-ebpf
cargo +nightly build --target bpfel-unknown-none -Z build-std=core --release

# Build userspace (auto-embeds eBPF bytecode via build.rs)
cd ../eoip-xdp
cargo build --release
```

### 10.1 XDP Ingress Program (Decapsulation)

**File: `eoip-xdp-ebpf/src/main.rs`**

Packet parsing flow:

```
1. Read Ethernet header at offset 0
   - Verify ethertype == 0x0800 (IPv4)
   - If not → XDP_PASS (let kernel handle ARP, IPv6, etc.)

2. Read IPv4 header at offset 14
   - Verify protocol == 47 (GRE)
   - Extract IHL for variable header length
   - Extract src_ip (bytes 12-15)
   - If not GRE → XDP_PASS

3. Read EoIP header at offset 14 + IHL*4
   - Verify magic == [0x20, 0x01, 0x64, 0x00]
   - Extract tunnel_id (little-endian, bytes 6-7)
   - Extract payload_len (big-endian, bytes 4-5)
   - If magic mismatch → XDP_PASS (standard GRE, not EoIP)

4. Check if keepalive (payload_len == 0)
   - If keepalive → XDP_PASS (let userspace daemon handle it)

5. Lookup tunnel_id in TUNNEL_MAP
   - If not found → XDP_PASS (unknown tunnel, userspace handles)
   - Verify src_ip matches tunnel config remote_ip

6. Update STATS_MAP (per-CPU, lock-free)

7. Strip outer headers:
   - Call bpf_xdp_adjust_head(14 + IHL*4 + 8)
   - This moves ctx.data forward, exposing only the inner Ethernet frame
   - MUST re-read ctx.data/ctx.data_end after adjust_head

8. Redirect to TAP device:
   - DECAP_DEVMAP.redirect(tunnel_id, 0) → XDP_REDIRECT
```

**Bounds checking:** Every `ptr_at::<T>(ctx, offset)` call MUST verify `offset + size_of::<T>() <= ctx.data_end() - ctx.data()`. The verifier rejects any unchecked pointer dereference.

**`bpf_xdp_adjust_head` usage:**

```rust
// aya-ebpf does NOT yet expose adjust_head as a method (PR #949 pending).
// Use the raw binding directly:
use aya_ebpf_bindings::helpers::bpf_xdp_adjust_head;

let strip_len = (14 + ip_hdr_len + 8) as i32; // ETH + IP + EoIP
unsafe {
    if bpf_xdp_adjust_head(ctx.ctx, strip_len) != 0 {
        return Err(());
    }
}
// ALL pointers are now invalid. Re-read from ctx.data().
```

### 10.2 TC Egress Program (Encapsulation)

**File: same `main.rs` or separate `tc_encap.rs`**

Attached to each TAP device's egress qdisc. Intercepts Ethernet frames from the TAP, prepends outer Eth + IPv4 + EoIP headers, and redirects to the physical NIC.

```
1. Read ifindex of the TAP device this TC program is attached to
   - ctx.skb.ifindex (available in TcContext)

2. Lookup ifindex in ENCAP_MAP
   - Get: tunnel_id, local_ip, remote_ip, nic_ifindex, src_mac, dst_mac

3. Get inner frame length (ctx.len())

4. Grow packet by 42 bytes at the head:
   - bpf_skb_adjust_room(ctx, 42, BPF_ADJ_ROOM_MAC, 0)
   - This shifts data forward, creating space for outer headers

5. Write outer Ethernet header (14 bytes):
   - dst_mac, src_mac from ENCAP_MAP
   - ethertype = 0x0800 (IPv4)

6. Write outer IPv4 header (20 bytes):
   - version=4, IHL=5, total_len=(42 + inner_len)
   - TTL=255, protocol=47 (GRE)
   - src_ip=local_ip, dst_ip=remote_ip
   - Checksum=0 (kernel recalculates, or compute inline)

7. Write EoIP header (8 bytes):
   - magic=[0x20, 0x01, 0x64, 0x00]
   - payload_len=inner_len (big-endian)
   - tunnel_id (little-endian)

8. Update STATS_MAP (tx_packets, tx_bytes)

9. Return TC_ACT_REDIRECT to physical NIC via bpf_redirect
```

**`bpf_skb_adjust_room` usage (TC):**

```rust
// TcContext provides adjust_room:
ctx.adjust_room(42, 1 /* BPF_ADJ_ROOM_MAC */, 0)?;
// After this, ctx data starts at the new (empty) header space.
// Write headers using ctx.store() or direct pointer manipulation.
```

**IPv4 header checksum:** Must be computed inline or left as 0 for kernel to fill. For eBPF, inline computation is a simple 10-word ones-complement sum over the 20-byte header:

```rust
fn ipv4_csum(hdr: &[u8; 20]) -> u16 {
    let mut sum: u32 = 0;
    for i in (0..20).step_by(2) {
        if i == 10 { continue; } // skip checksum field
        sum += u16::from_be_bytes([hdr[i], hdr[i+1]]) as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}
```

This is a bounded loop (10 iterations), verifier-safe.

### 10.3 Userspace Loader Integration

**File: `eoip-xdp/src/lib.rs`**

```rust
pub struct XdpAccelerator {
    bpf: Ebpf,
    nic_ifindex: u32,
}

impl XdpAccelerator {
    /// Load eBPF programs and attach to the physical NIC.
    pub fn new(nic_iface: &str) -> Result<Self>;

    /// Add a tunnel: populate maps, attach TC to TAP.
    pub fn add_tunnel(&mut self, tunnel_id: u16, tap_iface: &str,
                      local_ip: Ipv4Addr, remote_ip: Ipv4Addr) -> Result<()>;

    /// Remove a tunnel: delete map entries, detach TC from TAP.
    pub fn remove_tunnel(&mut self, tunnel_id: u16) -> Result<()>;

    /// Read aggregated stats from per-CPU maps.
    pub fn get_stats(&self, tunnel_id: u16) -> Result<XdpTunnelStats>;
}
```

**Attachment sequence:**

```rust
// 1. Load eBPF bytecode (embedded at compile time)
let mut bpf = Ebpf::load(include_bytes_aligned!("path/to/eoip-xdp"))?;

// 2. Attach XDP to physical NIC
let xdp: &mut Xdp = bpf.program_mut("eoip_decap")?.try_into()?;
xdp.load()?;
xdp.attach(nic_iface, XdpFlags::default())?;
// Falls back to SKB_MODE if native fails:
// xdp.attach(nic_iface, XdpFlags::SKB_MODE)?;

// 3. Per-tunnel: populate maps
let mut tunnel_map: HashMap<_, u16, TunnelInfo> =
    HashMap::try_from(bpf.map_mut("TUNNEL_MAP")?)?;
tunnel_map.insert(tunnel_id, info, 0)?;

let mut devmap: DevMapHash<_> =
    DevMapHash::try_from(bpf.map_mut("DECAP_DEVMAP")?)?;
devmap.set(tunnel_id as u32, tap_ifindex, None, 0)?;

// 4. Attach TC to TAP egress
tc::qdisc_add_clsact(tap_iface)?;
let tc_prog: &mut SchedClassifier = bpf.program_mut("eoip_encap")?.try_into()?;
tc_prog.load()?;
tc_prog.attach(tap_iface, TcAttachType::Egress)?;

// 5. Populate encap map
let mut encap_map: HashMap<_, u32, EncapInfo> =
    HashMap::try_from(bpf.map_mut("ENCAP_MAP")?)?;
encap_map.insert(tap_ifindex, encap_info, 0)?;
```

### 10.4 Integration with Existing Daemon

The `XdpAccelerator` integrates into the existing `TunnelManager`:

```rust
// In TunnelManager::create_tunnel():
if let Some(ref mut xdp) = self.xdp_accelerator {
    xdp.add_tunnel(tunnel_id, &iface_name, local_ip, remote_ip)?;
    tracing::info!(tunnel_id, "XDP fast path activated");
}
// Keepalives still handled by userspace (XDP_PASS lets them through)
```

**Fallback behavior:** If XDP is not available (no `CAP_BPF`, old kernel, unsupported driver), the daemon falls back to the existing userspace path automatically. The `XdpAccelerator` is `Option<XdpAccelerator>` — `None` means pure userspace mode.

### 10.5 Keepalive Handling

Keepalive packets (EoIP with `payload_len == 0`) are NOT redirected by XDP. Instead:

```
XDP program detects payload_len == 0 → returns XDP_PASS
→ packet reaches kernel network stack → raw socket → userspace daemon
→ existing keepalive FSM processes it (update last_rx_timestamp, state transitions)
→ userspace sends keepalive responses via raw socket (as before)
```

This means the keepalive path is unchanged — only data packets are accelerated.

## Kernel Requirements

| Feature | Minimum Kernel | Notes |
|---------|---------------|-------|
| XDP (generic/SKB) | 4.8 | Works on all NICs but slower |
| XDP (native) | 4.18 | Requires driver support (virtio_net, i40e, mlx5, etc.) |
| TC eBPF (clsact) | 4.5 | For TX encapsulation |
| BPF HashMap | 3.19 | Basic map type |
| DevMapHash | 5.4 | For XDP_REDIRECT with hash keys |
| `bpf_xdp_adjust_head` | 4.10 | For stripping outer headers |
| `bpf_skb_adjust_room` | 4.13 | For adding outer headers in TC |
| Bounded loops | 5.3 | For checksum computation |
| 1M instruction limit | 5.2 | Generous limit for EoIP programs |
| `bpf_redirect` to TAP | 4.18 | TAP driver `ndo_xdp_xmit` support |

**Minimum recommended: Linux 5.4+** (covers all required features).

**Hetzner CX23:** Uses virtio_net driver which supports native XDP since kernel 4.10.

## Gotchas and Edge Cases

1. **`bpf_xdp_adjust_head` invalidates ALL pointers.** After calling it, you MUST re-read `ctx.data()` and `ctx.data_end()`. Any previously-computed pointer is invalid.

2. **`aya_ebpf::XdpContext::adjust_head()` is NOT yet released** (PR #949 in draft). Use the raw binding: `aya_ebpf_bindings::helpers::bpf_xdp_adjust_head`.

3. **DevMap/DevMapHash entries can ONLY be modified from userspace**, not from eBPF. Tunnel add/remove happens via userspace map updates.

4. **eBPF stack limit: 512 bytes.** The parsing structs total ~42 bytes, well within limits.

5. **No GRE header in `network-types` crate.** Define `EoipHeader` in the `-common` crate.

6. **Outer MTU:** Set physical NIC MTU to at least 1542 (1500 inner + 42 overhead) to avoid fragmentation. If the NIC MTU is 1500, inner frames are limited to 1458 bytes (already our default).

7. **IP fragmented packets:** If the outer IP packet is fragmented (MF=1 or fragment_offset != 0), return `XDP_PASS` to let the kernel reassemble before userspace processing.

8. **clsact qdisc:** Must be added to TAP devices before attaching TC programs. Adding it when it already exists is harmless (`EEXIST`, aya handles this).

9. **Permissions:** Loading eBPF programs requires `CAP_BPF` + `CAP_NET_ADMIN` (or root).

10. **MAC address for TX encapsulation:** The outer Ethernet header needs the gateway's MAC (or peer's MAC on L2 adjacent networks). Resolve via ARP/neighbor lookup at tunnel creation time, store in `ENCAP_MAP`. If the MAC changes (e.g., gateway failover), userspace must update the map.

11. **`bpf-linker` installation:** Use `cargo binstall bpf-linker`. Building from source takes 30+ minutes and may fail.

12. **eBPF programs use GPL-2.0 OR MIT dual license** (kernel requirement for BPF helpers).

## Testing Strategy

### Unit Tests

- Verify EoipHeader parsing (magic, endianness) in the `-common` crate
- Verify IPv4 checksum computation
- Verify TunnelInfo/EncapInfo struct sizes match expected (use `assert_eq!(size_of::<T>(), expected)`)

### Integration Tests (Linux VM)

1. **XDP load/attach:** Load program, verify attachment via `bpftool prog list`
2. **Map population:** Insert tunnel config, verify via `bpftool map dump`
3. **Decap path:** Send an EoIP packet from a peer, verify it appears on the TAP device
4. **Encap path:** Send an Ethernet frame into the TAP, verify EoIP-encapsulated packet on the wire
5. **Keepalive passthrough:** Send a keepalive, verify it reaches userspace (not redirected)
6. **Unknown tunnel:** Send EoIP with unknown tunnel_id, verify it reaches userspace
7. **Stats:** Verify per-CPU stats counters increment

### Performance Benchmarks

| Test | Metric | Target |
|------|--------|--------|
| XDP decap (small packets) | Packets/sec | > 1 Mpps |
| XDP decap (1500B frames) | Throughput | > 5 Gbps |
| TC encap (small packets) | Packets/sec | > 500 Kpps |
| TC encap (1500B frames) | Throughput | > 3 Gbps |
| End-to-end (MikroTik ↔ Linux) | Throughput | > 800 Mbps (1G link) |
| CPU usage (1 Gbps line rate) | % | < 20% |

### Regression Tests

- All existing 148 unit tests must still pass
- MikroTik interop must work identically (keepalives, ping, btest)
- Dynamic tunnel add/remove via CLI must work with XDP enabled

## Success Criteria

- [ ] XDP program loads and attaches to virtio_net (native mode)
- [ ] TC program loads and attaches to TAP egress
- [ ] EoIP decapsulation works (MikroTik ping through XDP-accelerated tunnel)
- [ ] EoIP encapsulation works (Linux ping through TC-accelerated tunnel)
- [ ] Keepalives still handled by userspace (not broken by XDP)
- [ ] RX throughput > 500 Mbps (was 135 Mbps)
- [ ] CPU usage < 30% at 500 Mbps (was 76% at 135 Mbps)
- [ ] Fallback to userspace mode when XDP unavailable
- [ ] Dynamic tunnel add/remove updates BPF maps correctly
- [ ] CLI `eoip-cli stats` shows combined userspace + XDP counters

## Artifacts

- `crates/eoip-xdp/` — full XDP sub-workspace
- `crates/eoip-xdp/eoip-xdp-ebpf/src/main.rs` — XDP decap + TC encap eBPF programs
- `crates/eoip-xdp/eoip-xdp-common/src/lib.rs` — shared types
- `crates/eoip-xdp/eoip-xdp/src/lib.rs` — `XdpAccelerator` loader
- Updated `crates/eoip-rs/src/tunnel/manager.rs` — XDP integration
- Benchmark results in `docs/performance/RESULTS.md`

## References

- [aya-rs.dev/book](https://aya-rs.dev/book/) — Aya eBPF framework book
- [linux-gre-keepalive](https://github.com/Jamesits/linux-gre-keepalive) — eBPF GRE header parsing reference
- [xdp-mptm](https://github.com/ebpf-networking/xdp-mptm) — Multi-Protocol Tunnel Multiplexer using XDP
- [bbonev/eoip](https://github.com/bbonev/eoip) — Linux kernel module EoIP (wire format reference)
- [kernel BPF docs](https://docs.kernel.org/bpf/) — Official eBPF documentation
