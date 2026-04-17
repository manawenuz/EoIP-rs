# Phase 4: Design Reflection & Implementation Hardening

**Status:** Draft  
**Priority:** High — must complete before any interop attempt  
**Dependencies:** Phase 3  
**Estimated Duration:** 1-2 days

---

## Objective

Review all protocol findings from Phase 3 against our `eoip-proto` codec, `eoip-rs` daemon, and design documentation. Fix any discrepancies. Ensure our implementation handles every observed MikroTik behavior before attempting interop.

## Background

Phase 3 gives us ground truth. This phase is the "measure twice" before we "cut once" on interop. Cheaper to fix codec bugs now than debug them live against MikroTik.

## Requirements

### 4.1 Codec Verification

For each finding in `PROTOCOL_FINDINGS.md`, verify our code handles it:

**eoip-proto checks:**
- Run the MikroTik pcaps through `eoip_proto::decode_eoip_header` and `decode_eoipv6_header` — do they decode correctly?
- Write new unit tests using exact byte sequences from captures
- Verify `validate_eoip_packet` accepts all valid MikroTik packets
- Verify keepalive packets (payload_len=0) pass validation

**Write regression tests:**
- `tests/interop/mikrotik_wire_compat.rs` in eoip-proto
- Parse each MikroTik capture packet, re-encode, verify byte-identical output
- These tests run in CI — they catch future codec regressions

### 4.2 Daemon Behavior Review

Compare observed MikroTik behavior against our daemon design:

| Behavior | MikroTik Does | Our Design | Status |
|----------|---------------|------------|--------|
| Keepalive interval | ? (measure in Phase 3) | Configurable, default 10s | Verify match |
| Keepalive direction | ? (both sides? one side?) | Both sides send | Verify |
| TTL on outer IP | ? | Kernel default | May need config |
| DSCP handling | ? | Not implemented | Add if needed |
| DF bit | ? | Not implemented | Add if needed |
| Max tunnel_id | ? | 65535 (v4), 4095 (v6) | Verify |
| Tunnel down detection | ? timeout | 30s default | Verify |
| Recovery behavior | ? | Stale→Active on RX | Verify |

### 4.3 Implementation Fixes

For any discrepancy found:
1. Open a GitHub issue documenting the finding
2. Fix the codec or daemon code
3. Add regression test with MikroTik capture data
4. Update design docs

### 4.4 Encode Verification

Critical: verify that packets WE encode are accepted by MikroTik.
- Use our `eoip_proto::encode_eoip_header` to craft packets
- Compare byte-by-byte with MikroTik-originated packets
- Any difference in magic, endianness, or padding = bug in our encoder

### 4.5 Confidence Gate

Before proceeding to Phase 5, the team must affirm:
- [ ] Every MikroTik packet we captured decodes without errors
- [ ] Every packet we encode matches MikroTik byte layout
- [ ] Keepalive behavior matches
- [ ] All regression tests pass
- [ ] No known protocol gaps remain

## Success Criteria

- [ ] All MikroTik pcaps decode cleanly with zero `eoip-analyzer` deviations
- [ ] Encode→decode roundtrip produces byte-identical output to MikroTik
- [ ] Regression test suite added to CI
- [ ] Design docs updated with findings
- [ ] Confidence gate passed — team sign-off

## Artifacts

- `crates/eoip-proto/tests/interop/mikrotik_wire_compat.rs`
- Updated `docs/design/protocol.md`
- GitHub issues for any bugs found
