# EoIP-rs Testing & Validation

## Overview

Testing is phased: we build confidence in the protocol by observing real MikroTik behavior before testing our own code against it. Each phase has explicit success criteria and must pass before the next begins.

## Phase Map

```
 Phase 1          Phase 2            Phase 3           Phase 4
┌──────────┐    ┌──────────────┐   ┌──────────────┐  ┌──────────────┐
│ CHR       │    │ MK ↔ MK      │   │ Protocol     │  │ Design       │
│ Bootstrap │───▶│ Baseline     │──▶│ Deep-Dive    │─▶│ Reflection   │
│           │    │              │   │              │  │              │
│ 1× CX23  │    │ 2× CX23     │   │ 3× CX23     │  │ Local only   │
└──────────┘    └──────────────┘   └──────────────┘  └──────┬───────┘
                                                            │
                                               ╔════════════╧═══════════╗
                                               ║   CONFIDENCE GATE      ║
                                               ║   Protocol matches     ║
                                               ║   our implementation?  ║
                                               ╚════════════╤═══════════╝
                                                            │
 Phase 5          Phase 6            Phase 7           Phase 8
┌──────────┐    ┌──────────────┐   ┌──────────────┐  ┌──────────────┐
│ Single    │    │ Multi-Tunnel │   │ Performance  │  │ Linux ↔      │
│ Interop   │◀──│ Scaling      │◀──│ & Stability  │  │ Linux        │
│           │    │              │   │              │  │              │
│ 3× CX23  │    │ 3× CX23     │   │ 3× CX23     │  │ 2× CX23     │
└──────────┘    └──────────────┘   └──────────────┘  └──────────────┘
      │                                                     │
      └─────────────────────┬───────────────────────────────┘
                            │
                     Phase 9: Cross-Platform
                     (Windows, macOS, etc.)
```

## Network Topologies

### Phase 1 — Single CHR

```
┌─────────────┐
│  Your       │
│  Machine    │──── SSH ────▶ ┌─────────────────┐
│  (macOS)    │               │  chr-test-1      │
│             │               │  CX23 / fsn1     │
└─────────────┘               │  RouterOS 7.18   │
                              │  78.47.55.197    │
                              └─────────────────┘
```

### Phase 2 — MikroTik ↔ MikroTik EoIP

```
┌─────────────────┐           ┌─────────────────┐
│  chr-test-1      │           │  chr-test-2      │
│  RouterOS 7.18   │           │  RouterOS 7.18   │
│                  │           │                  │
│  eoip1 ──────────╬═══ GRE ══╬── eoip1          │
│  tunnel-id=100   │  proto 47 │  tunnel-id=100   │
│                  │           │                  │
│  bridge1         │           │  bridge1         │
│  10.0.100.1/24   │           │  10.0.100.2/24   │
└─────────────────┘           └─────────────────┘

Validation: ping 10.0.100.2 from chr-test-1 through EoIP tunnel
```

### Phase 3 — Protocol Analysis (middlebox capture)

```
┌─────────────────┐           ┌─────────────────┐
│  chr-test-1      │           │  chr-test-2      │
│  RouterOS        │           │  RouterOS        │
│                  │           │                  │
│  eoip1 ──────────╬═══ GRE ══╬── eoip1          │
│  tunnel-id=100   │           │  tunnel-id=100   │
└────────┬────────┘           └────────┬────────┘
         │                             │
         │         ┌────────────┐      │
         └─────────┤ middlebox  ├──────┘
                   │ (Ubuntu)   │
                   │            │
                   │ tcpdump    │
                   │ eoip-      │
                   │  analyzer  │
                   └────────────┘

Captures: GRE headers, keepalives, tunnel-id encoding,
          MTU behavior, error responses
```

### Phase 5 — Single Tunnel Interop (our code!)

```
┌─────────────────┐           ┌─────────────────┐
│  chr-test-1      │           │  linux-test-1    │
│  RouterOS        │           │  Ubuntu 22.04    │
│                  │           │                  │
│  eoip1 ──────────╬═══ GRE ══╬── eoip-rs        │
│  tunnel-id=100   │  proto 47 │  tunnel-id=100   │
│                  │           │                  │
│  bridge1         │           │  TAP: eoip0      │
│  10.0.100.1/24   │           │  10.0.100.2/24   │
└─────────────────┘           └─────────────────┘

First real interop test: eoip-rs daemon ↔ MikroTik CHR
```

### Phase 6 — Multi-Tunnel Scaling

```
┌─────────────────┐                ┌─────────────────┐
│  chr-test-1      │                │  linux-test-1    │
│  RouterOS        │                │  eoip-rs daemon  │
│                  │                │                  │
│  eoip1 (tid=1) ──╬════ GRE ═════╬── eoip0          │
│  eoip2 (tid=2) ──╬════ GRE ═════╬── eoip1          │
│  ...             │                │  ...             │
│  eoipN (tid=N) ──╬════ GRE ═════╬── eoipN          │
│                  │                │                  │
│  N = 2,10,50,100 │                │  All concurrent  │
└─────────────────┘                └─────────────────┘

Scaling gates: 2 → 10 → 50 → 100 tunnels
```

### Phase 8 — Linux ↔ Linux (no MikroTik)

```
┌─────────────────┐           ┌─────────────────┐
│  linux-test-1    │           │  linux-test-2    │
│  eoip-rs         │           │  eoip-rs         │
│                  │           │                  │
│  eoip0 ──────────╬═══ GRE ══╬── eoip0          │
│  tunnel-id=100   │  proto 47 │  tunnel-id=100   │
│                  │           │                  │
│  10.0.100.1/24   │           │  10.0.100.2/24   │
└─────────────────┘           └─────────────────┘

CI-ready: no MikroTik dependency, pure eoip-rs both sides
```

## Current Infrastructure

| VM | IP | Role | Status |
|----|-----|------|--------|
| chr-test-1 | 78.47.55.197 | RouterOS 7.18.2 CHR | Phase 1 ✓ |

## Scripts

| Script | Purpose |
|--------|---------|
| [`tests/infra/deploy-chr.sh`](../../tests/infra/deploy-chr.sh) | Deploy a MikroTik CHR VM on Hetzner |
| [`tests/infra/teardown-chr.sh`](../../tests/infra/teardown-chr.sh) | Destroy CHR VMs |

See [`tests/infra/README.md`](../../tests/infra/README.md) for usage details.

## Cost Budget

```
Phases 1-6:  ~$15  (hourly CX23 VMs, CHR free license)
Phase 7:     +$45  (MikroTik P1 license for throughput)
Phase 8:      $0   (reuse VMs)
Phase 9:     +$20  (Windows VM)
─────────────────────
Total:       ~$80
```

## Principles

1. **Never skip the confidence gate** — Phase 4→5 boundary is mandatory
2. **Protocol analyzer runs continuously** — every phase captures packets
3. **Phases are sequential** — each must pass before the next (except 7∥8)
4. **Captures are archived** — they become regression test data forever
5. **Rollback on failure** — fix at current phase, don't push forward
6. **Teardown when idle** — `./teardown-chr.sh --all` to stop billing
