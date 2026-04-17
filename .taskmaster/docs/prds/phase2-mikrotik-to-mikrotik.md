# Phase 2: MikroTik-to-MikroTik EoIP Baseline

**Status:** Draft  
**Priority:** Critical — establishes ground truth for wire format  
**Dependencies:** Phase 1  
**Estimated Duration:** 2-3 hours  
**Cost:** ~$2-5 additional (second CX22 VM)

---

## Objective

Deploy a second MikroTik CHR, establish an EoIP tunnel between the two, verify L2 connectivity, and create the "known-good" baseline that all our protocol analysis and interop testing will compare against.

## Requirements

### 2.1 Second CHR Deployment
- Use the automation script from Phase 1 to deploy `mk-b`
- Both VMs must be in the same Hetzner region/datacenter
- Record both VMs' public IPs

### 2.2 EoIP Tunnel Configuration

On **mk-a** (first CHR):
```routeros
/interface eoip add name=eoip-tunnel1 remote-address=<mk-b-ip> tunnel-id=100
/ip address add address=10.255.0.1/30 interface=eoip-tunnel1
```

On **mk-b** (second CHR):
```routeros
/interface eoip add name=eoip-tunnel1 remote-address=<mk-a-ip> tunnel-id=100
/ip address add address=10.255.0.2/30 interface=eoip-tunnel1
```

### 2.3 L2/L3 Connectivity Validation
- Ping from mk-a → mk-b via tunnel: `ping 10.255.0.2`
- Ping from mk-b → mk-a via tunnel: `ping 10.255.0.1`
- Verify MAC addresses visible: `/interface eoip print detail` on both sides
- Verify tunnel state shows "running": `/interface eoip monitor eoip-tunnel1`
- Test ARP resolution across tunnel

### 2.4 Multiple Tunnel IDs
- Create a second tunnel with `tunnel-id=200` between the same pair
- Verify both tunnels work independently
- Verify different tunnel IDs don't cross-talk

### 2.5 EoIPv6 Tunnel (if both VMs have IPv6)
- If Hetzner provides IPv6, create an EoIPv6 tunnel:
  ```routeros
  /interface eoip add name=eoip6-tunnel1 remote-address=<mk-b-ipv6> tunnel-id=42
  ```
- Verify it uses protocol 97 (EtherIP) instead of protocol 47 (GRE)

### 2.6 Keepalive Behavior
- Monitor keepalive state: `/interface eoip monitor eoip-tunnel1`
- Observe keepalive interval (default 10s)
- Block traffic temporarily with firewall rule, verify tunnel goes to "not running"
- Unblock, verify recovery

### 2.7 Configuration Script
- Create `tests/infra/setup-eoip-pair.sh` that:
  - Takes two VM IPs as input
  - SSHes into each and configures the EoIP tunnel
  - Runs validation pings
  - Outputs: tunnel status on both sides

## Success Criteria

- [ ] EoIP tunnel established between two CHR instances
- [ ] Bidirectional ping works across tunnel
- [ ] Multiple tunnel IDs work independently
- [ ] Keepalive behavior observed and documented
- [ ] EoIPv6 tunnel tested (if IPv6 available)
- [ ] Setup scripted for repeatability

## Artifacts

- `tests/infra/setup-eoip-pair.sh`
- `tests/captures/mk-mk-baseline/` — saved RouterOS `/export` configs from both sides
