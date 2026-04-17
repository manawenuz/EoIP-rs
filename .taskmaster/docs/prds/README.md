# EoIP-rs Testing & Validation PRDs

## Phase Overview

```
Phase 1: CHR Bootstrap          ─── Deploy first MikroTik VM
   │
Phase 2: MK↔MK Baseline        ─── Two MikroTiks, EoIP working
   │
Phase 3: Protocol Deep-Dive     ─── Linux middlebox, tcpdump, analyzer
   │
Phase 4: Design Reflection      ─── Fix our code to match reality
   │                                 ┌── CONFIDENCE GATE ──┐
Phase 5: Single Tunnel Interop  ─── First EoIP-rs ↔ MikroTik tunnel
   │
Phase 6: Multi-Tunnel Scaling   ─── 2 → 10 → 50 → 100 tunnels
   │
Phase 7: Performance & Stability─── Throughput, latency, 24hr soak
   │
Phase 8: Linux↔Linux            ─── Our daemon on both sides, CI-ready
   │
Phase 9: Cross-Platform         ─── Windows & macOS ports
```

## PRD Documents

| Phase | Document | VMs Needed | Approx Time | Approx Cost |
|-------|----------|-----------|-------------|-------------|
| 1 | [CHR Infrastructure](phase1-chr-infrastructure.md) | 1 (CX23) | 2-3 hrs | $2-5 |
| 2 | [MK↔MK Baseline](phase2-mikrotik-to-mikrotik.md) | 2 | 2-3 hrs | $2-5 |
| 3 | [Protocol Analysis](phase3-protocol-analysis.md) | 3 | 4-6 hrs | $1-2 |
| 4 | [Design Reflection](phase4-design-reflection.md) | 0 (local) | 1-2 days | $0 |
| 5 | [Single Interop](phase5-single-tunnel-interop.md) | 3 | 1-2 days | $0 (reuse) |
| 6 | [Multi-Tunnel](phase6-multi-tunnel-scaling.md) | 3 | 2-3 days | $0 (reuse) |
| 7 | [Performance](phase7-performance-stability.md) | 3 | 3-5 days | $45 (P1 license) |
| 8 | [Linux↔Linux](phase8-linux-to-linux.md) | 2 | 1-2 days | $0 (reuse) |
| 9 | [Cross-Platform](phase9-cross-platform.md) | varies | 2-4 weeks | $5-20 |

## Total Estimated Cost

- **Phases 1-6:** Under $15 (Hetzner hourly VMs, CHR free license)
- **Phase 7:** +$45 (MikroTik P1 license for throughput testing)
- **Phase 9:** +$5-20/month (Windows VM)
- **Total through Phase 8:** ~$60

## Key Principles

1. **Never skip the confidence gate** (Phase 4→5 boundary)
2. **Protocol analyzer runs continuously** — every phase, every test
3. **Each phase must pass before the next begins** (except 7∥8)
4. **Captures are archived** — they're regression test data forever
5. **Rollback on failure** — fix at current phase, don't push forward
