# Bugfix: Tunnel Immediately Goes Stale on Startup

**Status:** Planned
**Priority:** Medium — functional but cosmetically broken; tunnel works despite wrong state
**Discovered during:** Phase 12 PMTUD testing on Raspberry Pi

---

## Problem

Every tunnel is immediately marked **Stale** within the first keepalive interval (default 10s) after daemon startup, even when the tunnel is fully operational and passing traffic. The CLI shows `state=stale` while pings over the overlay succeed:

```
 0 S eoip121   121   0.0.0.0   172.16.81.1   1458   1458
```

Log output:
```
09:44:22 INFO  tunnel active tunnel_id=121
09:45:02 WARN  tunnel went stale (keepalive timeout) tunnel_id=121 elapsed_secs=39
```

The tunnel recovers to Active only after a MikroTik keepalive happens to arrive and the next keepalive check sees a fresh `last_rx_timestamp`. With short `keepalive_timeout_secs` (e.g. 30s) and slow peers, the tunnel may never recover — or may flip-flop between Active and Stale.

## Root Cause

`TunnelStats::new()` initializes `last_rx_timestamp` to **0** (Unix epoch):

```rust
// crates/eoip-proto/src/types.rs:87
last_rx_timestamp: AtomicI64::new(0),
```

The keepalive task spawns and does an immediate first tick. On the first staleness check:

```rust
// crates/eoip-rs/src/keepalive.rs:46-57
let last_rx_ms = handle.stats.last_rx_timestamp.load(Ordering::Relaxed);
if last_rx_ms > 0 {  // <-- guards against 0, BUT...
    let elapsed = Duration::from_millis((now_ms - last_rx_ms).max(0) as u64);
    if elapsed > timeout { /* mark stale */ }
}
```

The `last_rx_ms > 0` guard means the **very first** check is skipped (timestamp is 0). But as soon as the first packet arrives (which could be our own keepalive reflected, or any data packet), `last_rx_timestamp` gets set. If the keepalive timer fires before a peer packet arrives (typical on startup — the peer may not send for up to its own keepalive interval), the sequence is:

1. `t=0s`: Tunnel created, `last_rx_timestamp = 0`, state = Active
2. `t=10s`: Keepalive fires, sends our keepalive, checks staleness
   - `last_rx_timestamp = 0` → guard skips check. OK so far.
3. `t=11s`: First peer packet arrives → `last_rx_timestamp = t=11s`
4. `t=20s`: Keepalive fires, checks staleness
   - `elapsed = 20s - 11s = 9s` < `timeout=30s` → stays Active. OK.

But if the peer's first packet arrives **between** two keepalive checks, and then the peer goes quiet (e.g., no traffic, peer keepalive interval > our timeout), the tunnel goes stale correctly.

The **actual** bug path observed in testing:

1. `t=0s`: Daemon starts, tunnel created, state = Active
2. `t=0.1s`: First peer keepalive arrives → `last_rx_timestamp = t=0.1s`
3. Service restarts (systemd `ExecStartPost` triggers restart race)
4. `t=0s` (new process): Tunnel re-created, `last_rx_timestamp = 0`, state = Active
5. `t=3s`: `ExecStartPost` sleep finishes, runs `ip link set ... up`
6. `t=10s`: Keepalive fires. No peer packet received yet in this process.
   - `last_rx_timestamp = 0` → skipped. OK.
7. `t=15s`: Peer keepalive arrives → `last_rx_timestamp = t=15s`
8. `t=20s`: Keepalive fires. `elapsed = 5s`. OK.
9. But if peer's keepalive interval is long (MikroTik default 10s) and our timeout is 30s, a tight window exists where `elapsed` > `timeout` after the first few keepalive intervals align badly.

The core issue: **the keepalive check doesn't distinguish "no data ever received" from "data stopped coming."** A 0 timestamp is treated as "never received" but any non-zero timestamp from a prior process or a stale value causes an immediate stale transition.

## Fix

### Option A: Initialize `last_rx_timestamp` to current time (recommended)

Set `last_rx_timestamp = now()` when creating the tunnel handle, giving the peer a full timeout window to send its first packet.

**Files:**
- `crates/eoip-rs/src/main.rs` — after creating `TunnelHandle`, store current time
- `crates/eoip-rs/src/tunnel/manager.rs` — same for dynamic tunnels

```rust
let now_ms = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_millis() as i64)
    .unwrap_or(0);
handle.stats.last_rx_timestamp.store(now_ms, Ordering::Relaxed);
```

**~4 lines changed. No behavioral change after the first keepalive cycle.**

### Option B: Skip staleness check until first real packet

Add a flag `first_rx_seen: AtomicBool` to `TunnelStats`. Only check staleness after the first real RX. More explicit but adds a field.

### Option C: Grace period in keepalive task

Skip staleness checks for the first N seconds after task start:

```rust
let started_at = Instant::now();
// ...in loop:
if started_at.elapsed() < timeout {
    continue; // grace period
}
```

## Recommendation

**Option A** — simplest, fewest changes, matches the semantic intent ("tunnel was just created, peer hasn't had time to respond yet").

## Test Plan

1. Start daemon with `keepalive_timeout_secs = 30`
2. Verify tunnel stays Active for at least 30s after startup
3. Verify tunnel transitions to Stale if peer is unreachable for > timeout
4. Verify tunnel recovers from Stale when peer resumes
5. Verify `journalctl` shows no `tunnel went stale` within first timeout period

## Estimated Effort

~10 minutes. 4 lines of code + restart + verify.
