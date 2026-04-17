# Phase 6: Multi-Tunnel Scaling — N Tunnels, Two MikroTiks

**Status:** Draft  
**Priority:** High  
**Dependencies:** Phase 5  
**Estimated Duration:** 2-3 days  
**Cost:** Reuse existing VMs

---

## Objective

Scale from 1 tunnel to many: first 2 tunnels (one per MikroTik), then 5, 10, 50, 100. Validate that our DashMap demux, crossbeam channels, buffer pool, and adaptive batching work correctly under concurrent load. Find and fix any scaling bottlenecks.

## Requirements

### 6.1 Two Tunnels — One Per MikroTik

**Configuration:**
```
mk-a ──── tunnel 100 ────► eoip-server
mk-b ──── tunnel 200 ────► eoip-server
```

- eoip-rs config with two `[[tunnel]]` entries
- Verify both tunnels come up independently
- Ping from each MikroTik simultaneously
- Verify stats show correct per-tunnel counters via gRPC

### 6.2 Multiple Tunnels — Same MikroTik Pair

```
mk-a ──── tunnel 100 ────► eoip-server
mk-a ──── tunnel 101 ────► eoip-server
mk-a ──── tunnel 102 ────► eoip-server
...
mk-a ──── tunnel 109 ────► eoip-server
```

- 10 tunnels from mk-a to eoip-server
- Each tunnel has its own TAP interface and IP address
- Run concurrent pings on all 10
- Verify zero cross-talk (packets arrive on correct tunnel)

### 6.3 Scaling Ladder

| Step | Tunnels | Source | What to measure |
|------|---------|--------|-----------------|
| 1 | 2 | 1 per MK | Basic multi-tunnel |
| 2 | 10 | 5 per MK | DashMap performance |
| 3 | 50 | 25 per MK | Channel backpressure |
| 4 | 100 | 50 per MK | Buffer pool exhaustion |

At each step:
- All tunnels must be Active
- Concurrent ping on all tunnels: 0% loss
- Memory usage (`/proc/<pid>/status`): VmRSS < 50MB for 100 tunnels (target from PRD)
- CPU usage under idle (just keepalives): < 5%
- gRPC `GetGlobalStats`: all counters correct

### 6.4 Protocol Analyzer Continuous Monitoring

Run `eoip-analyzer` in JSON streaming mode throughout:
```bash
tcpdump -i any -w - 'ip proto 47' | eoip-analyzer /dev/stdin --json | tee scaling-log.jsonl
```

Flag ANY deviation immediately. Each deviation must be investigated before adding more tunnels.

### 6.5 Load Testing Per Step

At each scaling step, after tunnels are stable:

1. **Keepalive-only** (5 min): Verify memory stable, no leaks
2. **Light ping** (all tunnels, 1 pps each): Verify packet delivery
3. **Moderate load** (10 concurrent iperf3 streams, 1 per tunnel): Measure aggregate throughput
4. **Burst** (all tunnels flood simultaneously for 30s): Verify buffer pool handles it

### 6.6 Rollback Criteria

If any step fails:
- Stop, capture diagnostics
- Fix the issue
- Re-run the SAME step (don't skip ahead)
- Only proceed when current step is 100% clean

### 6.7 Configuration Generator

Create `tests/infra/gen-multi-tunnel-config.py`:
- Input: number of tunnels, mk-a IP, mk-b IP, server IP
- Output: eoip-rs config.toml + RouterOS commands for both MikroTiks
- Auto-assigns tunnel IDs (100, 101, ...) and IP addresses (10.255.N.1/30)

## Success Criteria

- [ ] 100 tunnels active simultaneously
- [ ] Zero packet loss under light load across all tunnels
- [ ] Memory < 50MB RSS at 100 tunnels
- [ ] No protocol deviations detected by analyzer
- [ ] gRPC stats accurate for all tunnels
- [ ] Clean shutdown: all tunnels torn down, no orphaned TAP interfaces

## Artifacts

- `tests/infra/gen-multi-tunnel-config.py`
- `tests/captures/scaling-{2,10,50,100}-tunnels/`
- Performance measurement logs
