# Phase 8: Linux-to-Linux EoIP-rs Communication

**Status:** Draft  
**Priority:** High  
**Dependencies:** Phase 6 (can run in parallel with Phase 7)  
**Estimated Duration:** 1-2 days  
**Cost:** Reuse existing VMs or add one more

---

## Objective

Validate EoIP-rs daemon ↔ daemon communication (no MikroTik involved). This tests our full TX+RX path end-to-end, both sides running our code. Essential for CI/CD — Linux-to-Linux tests can run in automated pipelines without MikroTik licensing.

## Background

MikroTik interop proves wire format correctness. Linux-to-Linux proves our daemon works as a complete system — both encoding and decoding our own packets. It's also where we can push performance without CHR license limits.

## Requirements

### 8.1 Infrastructure

Two Linux VMs running eoip-rs:
```
linux-a (eoip-rs) ──── tunnel 100 ────► linux-b (eoip-rs)
```

Each VM runs:
- `eoip-helper` (root) — creates TAP + raw sockets
- `eoip-rs` (eoip user) — daemon

### 8.2 Basic Connectivity

1. Configure tunnel on both sides (matching tunnel_id, pointing at each other)
2. Assign IPs to TAP interfaces
3. Ping both directions
4. Verify keepalives work (both sides send and receive)
5. Verify tunnel state: Active on both

### 8.3 Protocol Analyzer Verification

Capture traffic between the two Linux hosts:
```bash
tcpdump -i any -w linux-linux.pcap 'ip proto 47'
```

Run through `eoip-analyzer`:
- Verify our encoded packets match expected MikroTik-compatible format
- Zero deviations expected (we encode what we decode)
- Verify keepalive format matches MikroTik captures from Phase 3

### 8.4 Symmetric Testing Matrix

| Test | linux-a → linux-b | linux-b → linux-a |
|------|-------------------|-------------------|
| Ping | ✓ | ✓ |
| TCP (iperf3) | ✓ | ✓ |
| UDP flood | ✓ | ✓ |
| Large MTU | ✓ | ✓ |
| ARP | ✓ | ✓ |

### 8.5 Multi-Tunnel Linux-to-Linux

Scale to 10, 50, 100 tunnels between two Linux hosts:
- Verify all tunnels active
- Concurrent iperf3 on multiple tunnels
- Measure aggregate throughput (no CHR license limit here)

### 8.6 Bridge Mode Testing

Test L2 bridging (the primary use case):
- Bridge TAP interfaces with a Linux bridge (`brctl` or `ip link set master`)
- Verify MAC learning across the EoIP tunnel
- Verify broadcast/multicast forwarding

### 8.7 Fault Injection

| Fault | How | Expected behavior |
|-------|-----|-------------------|
| Kill daemon on one side | `kill -9` | Other side detects stale after timeout, recovers on restart |
| Network partition | `iptables -A OUTPUT -p 47 -j DROP` | Both sides go Stale, recover when rule removed |
| High packet loss | `tc qdisc add netem loss 10%` | Throughput degrades gracefully, no crash |
| Latency spike | `tc qdisc add netem delay 100ms` | Keepalive still works, tunnel stays Active |
| Reorder packets | `tc qdisc add netem reorder 25%` | Packets delivered (EoIP is stateless, order doesn't matter) |

### 8.8 CI Integration

Create GitHub Actions workflow:
```yaml
# .github/workflows/integration.yml
# Spin up two containers/VMs
# Deploy eoip-rs on both
# Run test suite
# Collect results
```

This is the foundation for automated regression testing.

## Success Criteria

- [ ] Bidirectional ping with 0% loss
- [ ] iperf3 TCP ≥ 3 Gbps (between co-located VMs)
- [ ] 100 tunnels active simultaneously
- [ ] All fault injection scenarios handled gracefully
- [ ] Bridge mode works (L2 forwarding across tunnel)
- [ ] Protocol analyzer shows zero deviations
- [ ] CI workflow runs successfully

## Artifacts

- `tests/captures/linux-linux/`
- `.github/workflows/integration.yml`
- `tests/integration/linux_pair_test.sh`
