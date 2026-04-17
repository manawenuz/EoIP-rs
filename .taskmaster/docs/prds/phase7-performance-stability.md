# Phase 7: Performance & Stability Testing

**Status:** Draft  
**Priority:** High  
**Dependencies:** Phase 6  
**Estimated Duration:** 3-5 days  
**Cost:** May need P1 CHR license ($45/yr) for throughput > 1 Mbps

---

## Objective

Push the system to its limits. Measure throughput, latency, packet rate, and stability under sustained load. Compare against PRD targets. Identify and fix bottlenecks.

## Performance Targets (from PRD)

| Metric | Target | Measurement |
|--------|--------|-------------|
| TCP throughput | 3-8 Gbps (MTU 1500) | iperf3 |
| Packet rate | 500K-1.2M pps (64B frames) | Custom UDP flood |
| Added latency (idle) | < 150µs | ping RTT delta |
| Added latency p99 (loaded) | < 500µs | ping under iperf3 load |
| Memory (100 tunnels) | < 50MB RSS | `/proc/pid/status` |

**Note:** CHR free license caps at 1 Mbps. For throughput testing beyond protocol correctness, we need either:
- P1 license ($45/yr, 1 Gbps cap) — sufficient for most tests
- Linux-to-Linux testing (Phase 8) — no license cap
- Both MikroTik tests prove correctness, Linux tests prove performance

## Requirements

### 7.1 Throughput Testing

**iperf3 over EoIP tunnel:**
```bash
# On eoip-server (iperf3 server):
iperf3 -s -B 10.255.0.2

# On mk-a (iperf3 client):
# Note: RouterOS has built-in bandwidth test, not iperf3
# Use /tool bandwidth-test instead
/tool bandwidth-test 10.255.0.2 protocol=tcp duration=30
```

For Linux-to-Linux (Phase 8), use proper iperf3:
```bash
iperf3 -c 10.255.0.2 -t 30 -P 4  # 4 parallel streams
```

### 7.2 Latency Testing

**Idle latency (tunnel overhead only):**
```bash
# Baseline: direct ping between VMs (no tunnel)
ping -c 100 <mk-b-public-ip> | tail -1

# Tunnel ping
ping -c 100 10.255.0.2 | tail -1

# Delta = EoIP overhead
```

**Loaded latency (p99 under throughput):**
```bash
# Terminal 1: iperf3 background load
iperf3 -c 10.255.0.2 -t 60 -P 4

# Terminal 2: ping during load
ping -c 1000 -i 0.01 10.255.0.2 > latency-loaded.txt
# Process for p50, p95, p99
```

### 7.3 Packet Rate Testing

Custom UDP flood to measure raw pps:
```bash
# Small UDP packets (18B payload = 64B on wire with headers)
iperf3 -c 10.255.0.2 -u -b 0 -l 18 -t 30
```

Monitor daemon stats via gRPC during test.

### 7.4 Stability Soak Test

**24-hour soak:**
- 10 active tunnels with mixed traffic
- Background iperf3 on 2 tunnels
- Ping on all 10 tunnels
- Monitor: memory, CPU, packet loss, tunnel state changes
- Any crash, memory leak, or state corruption = fail

Collect every 5 minutes:
```bash
# Memory
cat /proc/<pid>/status | grep VmRSS

# Stats via gRPC
grpcurl localhost:50051 eoip.v1.StatsService/GetGlobalStats

# Tunnel states
grpcurl localhost:50051 eoip.v1.TunnelService/ListTunnels
```

### 7.5 Adaptive Batching Validation

Verify the batching FSM transitions correctly:
1. **Low load** (1 pps): Verify Immediate mode — latency < 150µs
2. **Ramp up** (1K pps → 100K pps): Verify transition to Batching
3. **High load** (max pps): Verify sendmmsg batching active
4. **Ramp down**: Verify return to Immediate mode

### 7.6 Buffer Pool Stress

- Set pool size to 256 (small)
- Flood 100K pps
- Verify: pool exhaustion → standalone alloc → no crash
- Verify: after flood stops, pool refills to capacity
- Measure: overhead of standalone alloc vs pooled

### 7.7 Profiling

On the Linux VM:
```bash
# Flamegraph
perf record -g -p <daemon-pid> -- sleep 30
perf script | flamegraph > flamegraph.svg

# Syscall count
strace -c -p <daemon-pid> -e trace=sendto,recvfrom,read,write
```

Identify top hotspots. Optimize if needed.

## Success Criteria

- [ ] TCP throughput ≥ 3 Gbps (Linux-to-Linux)
- [ ] Idle latency overhead < 150µs
- [ ] p99 latency under load < 500µs
- [ ] No memory growth over 24-hour soak
- [ ] No packet loss under moderate load
- [ ] Graceful degradation under overload (drops, no crash)
- [ ] Flamegraph shows no unexpected hotspots

## Artifacts

- `tests/perf/results/throughput-{date}.json`
- `tests/perf/results/latency-{date}.json`
- `tests/perf/flamegraph-{date}.svg`
- `tests/perf/soak-{date}.log`
