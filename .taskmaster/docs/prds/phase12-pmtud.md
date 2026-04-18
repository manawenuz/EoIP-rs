# Phase 12: Path MTU Discovery + Auto-MTU

**Status:** Planned
**Priority:** High — prerequisite for PACKET_MMAP (Phase 11 Route 4) and correct MikroTik behavior
**Dependencies:** None (standalone feature)
**Blocked by this:** PACKET_MMAP zero-copy RX (IP fragmentation breaks AF_PACKET)

---

## Problem

EoIP-rs currently uses a static MTU (default 1458, configurable per-tunnel). MikroTik RouterOS auto-detects the correct overlay MTU using PMTUD and adjusts dynamically. When the overlay MTU is too large, GRE-encapsulated packets exceed the physical path MTU and get IP-fragmented. This:

1. **Breaks AF_PACKET / PACKET_MMAP** — AF_PACKET sees pre-reassembly fragments, only the first has a GRE header
2. **Wastes CPU** — kernel must reassemble fragments before delivering to raw sockets
3. **Reduces throughput** — fragment reassembly adds latency and can cause drops under load
4. **Breaks in complex paths** — NAT, WireGuard, PPPoE, VLAN all reduce effective MTU

MikroTik example: direct link → Actual MTU 1458, over WireGuard → Actual MTU 1378.

## Overhead Calculation

```
overlay_mtu = path_mtu - 20 (IP header) - 8 (GRE/EoIP header) - 14 (inner Ethernet header)
            = path_mtu - 42
```

For physical MTU 1500: overlay = 1458
For WireGuard MTU 1420: overlay = 1378

---

## Implementation Plan

### Step 1: Auto-MTU from outgoing interface (simple case)

**What:** At tunnel creation, look up the outgoing interface for `remote` IP via routing table. Read its MTU. Calculate `overlay_mtu = iface_mtu - 42`. Use this as the default when config `mtu` is not explicitly set (or set to `auto`).

**Files:**
- `crates/eoip-rs/src/net/mtu.rs` (new) — `fn detect_interface_mtu(remote: IpAddr) -> Result<u16>`
  - Use `libc::socket` + `libc::connect` (UDP, unbound) to trigger route lookup
  - Then `ioctl(SIOCGIFMTU)` on the resulting interface, OR
  - Read `/proc/net/route` and match, then read `/sys/class/net/<iface>/mtu`
  - Platform-gated: Linux only, fallback to 1458 on other platforms
- `crates/eoip-rs/src/config.rs` — change `mtu` default from `1458` to `0` (meaning "auto")
- `crates/eoip-rs/src/main.rs` — if `mtu == 0`, call `detect_interface_mtu()`, set on config
- `crates/eoip-rs/src/net/mod.rs` — add `pub mod mtu;`

**Test:** Deploy with `mtu` not set in config. Check logs for detected MTU. Verify TAP interface gets correct MTU. Run iperf3.

**Estimated:** ~80 lines new code.

### Step 2: Set TAP MTU at creation time

**What:** Currently MTU is set via `ip link set` in the start script after daemon starts. Move MTU setting into the daemon — after TAP creation, set MTU via `ioctl(SIOCSIFMTU)`.

**Files:**
- `crates/eoip-helper/src/tap.rs` — add `fn set_interface_mtu(iface: &str, mtu: u16)`
- `crates/eoip-helper/src/main.rs` — call after TAP creation, using MTU from `CreateTunnel` message
- `crates/eoip-proto/src/wire.rs` — add `mtu: u16` field to `DaemonMsg::CreateTunnel`

**Test:** Remove MTU from start script. Daemon should set it. `ip link show eoip100` shows correct MTU.

**Estimated:** ~40 lines.

### Step 3: PMTUD probing

**What:** Active path MTU discovery. Send ICMP Echo or UDP probes with DF=1 at varying sizes to discover the maximum non-fragmenting packet size on the path to `remote`.

**Algorithm:**
1. At tunnel startup, probe with sizes from 1500 down to 576 (binary search)
2. Send IP packet with DF=1 to remote peer
3. If we get ICMP "Fragmentation Needed" (type 3, code 4) → too large
4. If we get echo reply or no error → this size works
5. Calculate `overlay_mtu = probe_mtu - 42`
6. Re-probe periodically (every 10 minutes) to detect path changes

**Implementation approach:**
- Use raw ICMP socket (already have CAP_NET_RAW)
- Send ICMP Echo Request with DF=1 to `remote` IP
- Binary search between 576 and 1500 (6-7 probes)
- Timeout 2s per probe, 3 retries
- Store discovered MTU in `TunnelHandle` as `AtomicU16`
- Update TAP MTU when discovered MTU changes

**Files:**
- `crates/eoip-rs/src/net/pmtud.rs` (new) — `PmtudProber` struct
  - `fn probe_path_mtu(remote: IpAddr) -> Result<u16>`
  - `fn spawn_pmtud_task(handle: Arc<TunnelHandle>, ...)`  (periodic re-probe)
- `crates/eoip-rs/src/tunnel/handle.rs` — add `actual_mtu: AtomicU16`
- `crates/eoip-rs/src/tunnel/manager.rs` — spawn PMTUD task per tunnel

**Fallback:** If PMTUD fails (ICMP blocked, no response), use Step 1 interface MTU. If that fails, use 1458.

**Test:** 
- Direct Hetzner path: should discover 1458
- Over WireGuard: should discover ~1378
- Blocked ICMP: should fall back gracefully

**Estimated:** ~200 lines.

### Step 4: Config override

**What:** Allow explicit `mtu = 1400` in config to override auto-detection. Add `mtu = "auto"` option.

**Config syntax:**
```toml
[[tunnel]]
tunnel_id = 100
# mtu = "auto"    # default — PMTUD then interface detection
# mtu = 1458      # explicit override
```

**Files:**
- `crates/eoip-rs/src/config.rs` — change `mtu` to enum `MtuConfig { Auto, Fixed(u16) }`
- Serde deserialize: number → Fixed, "auto" or absent → Auto

**Test:** Set explicit MTU, verify it overrides auto-detection.

**Estimated:** ~30 lines.

### Step 5: CLI updates

**What:** Show actual MTU in `print` and `print detail`. Add to `set` command.

**Display:**
```
print:
 #  NAME     MTU   ACTUAL-MTU  REMOTE-ADDRESS    TUNNEL-ID  STATE
 0  eoip100  auto  1458        128.140.114.175   100        running

print detail:
              mtu: auto
       actual-mtu: 1458
```

**Files:**
- `crates/eoip-api/proto/eoip.proto` — add `uint32 actual_mtu` field to `Tunnel` message
- `crates/eoip-rs/src/api/tunnel_svc.rs` — populate `actual_mtu` from handle
- `crates/eoip-cli/src/commands.rs` — display actual_mtu in print/detail

**Estimated:** ~40 lines.

### Step 6: TCP MSS clamping

**What:** Automatically add iptables rule to clamp TCP MSS on the tunnel interface, matching MikroTik's `Clamp TCP MSS = yes`.

**Rule:**
```bash
iptables -t mangle -A FORWARD -o eoip100 -p tcp --tcp-flags SYN,RST SYN -j TCPMSS --clamp-mss-to-pmtu
```

**Files:**
- `crates/eoip-helper/src/tap.rs` or new `mss.rs` — run iptables after TAP creation
- Add cleanup rule on tunnel destroy
- Config flag: `clamp_tcp_mss: bool` (default true)

**Alternative:** Do MSS clamping in-daemon by modifying TCP SYN packets in the data path. More complex but avoids iptables dependency.

**Estimated:** ~50 lines (iptables approach), ~150 lines (in-daemon).

---

## Implementation Order

1. **Step 1** (auto-MTU from interface) — unblocks basic correctness
2. **Step 2** (set TAP MTU in daemon) — removes script dependency
3. **Step 4** (config override) — small, do alongside Step 2
4. **Step 5** (CLI) — user visibility
5. **Step 3** (PMTUD probing) — full MikroTik-compatible auto-detection
6. **Step 6** (TCP MSS clamping) — polish

After Steps 1-2: retry PACKET_MMAP (no fragmentation on standard paths).
After Step 3: PACKET_MMAP works on all paths including NAT/VPN.

---

## Key Files

| File | Role |
|------|------|
| `crates/eoip-rs/src/net/mtu.rs` | Interface MTU detection (new) |
| `crates/eoip-rs/src/net/pmtud.rs` | PMTUD probing (new) |
| `crates/eoip-rs/src/config.rs` | MTU config (MtuConfig enum) |
| `crates/eoip-helper/src/tap.rs` | Set TAP MTU via ioctl |
| `crates/eoip-rs/src/tunnel/handle.rs` | actual_mtu field |
| `crates/eoip-api/proto/eoip.proto` | actual_mtu proto field |
| `crates/eoip-cli/src/commands.rs` | CLI display |

## Success Criteria

- Auto-MTU matches MikroTik for same path (1458 direct, 1378 over WireGuard)
- Config override works and takes precedence
- CLI shows both configured and actual MTU
- No IP fragmentation on GRE outer packets (verify with tcpdump)
- TCP MSS clamped automatically
- PACKET_MMAP unblocked (iperf3 works over AF_PACKET with auto-MTU)

## Test Lab

VMs: build-vm (167.235.241.221), test-vm (128.140.114.175) — may need recreation.
PMTUD validation: test between VMs (direct), and via Pi (wifi/NAT path).
