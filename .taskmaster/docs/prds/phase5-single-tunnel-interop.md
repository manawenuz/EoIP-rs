# Phase 5: Single Tunnel Interop — EoIP-rs ↔ MikroTik

**Status:** Draft  
**Priority:** Critical — first live interop test  
**Dependencies:** Phase 4 (confidence gate passed)  
**Estimated Duration:** 1-2 days  
**Cost:** ~$2-5 (reuse Phase 2 MikroTik VMs + 1 Linux VM for eoip-rs)

---

## Objective

Establish the first working EoIP tunnel between our `eoip-rs` daemon and a real MikroTik CHR. One tunnel, one direction, then bidirectional. This is the moment of truth for the entire project.

## Requirements

### 5.1 Infrastructure

- **mk-a**: MikroTik CHR (from Phase 2), already configured
- **eoip-server**: New Linux VM (Ubuntu 22.04), our daemon will run here
- Build `eoip-rs` and `eoip-helper` for Linux x86_64
- Deploy binaries to eoip-server

### 5.2 eoip-rs Server Setup

```bash
# On eoip-server:
# 1. Create eoip user
sudo useradd -r -s /usr/sbin/nologin eoip

# 2. Deploy binaries
sudo cp eoip-helper /usr/local/bin/
sudo cp eoip-rs /usr/local/bin/

# 3. Create config
sudo mkdir -p /etc/eoip-rs
sudo cp config.toml /etc/eoip-rs/config.toml
# Edit: set tunnel_id=100, local=<eoip-server-ip>, remote=<mk-a-ip>

# 4. Start helper (as root)
sudo eoip-helper --mode persist &

# 5. Start daemon (as eoip user or root initially for debugging)
sudo eoip-rs --config /etc/eoip-rs/config.toml
```

### 5.3 MikroTik Configuration

On **mk-a**:
```routeros
/interface eoip add name=eoip-linux remote-address=<eoip-server-ip> tunnel-id=100
/ip address add address=10.255.1.1/30 interface=eoip-linux
```

### 5.4 Incremental Validation

**Step 1: Receive only**
- Start eoip-rs with tcpdump running
- MikroTik sends keepalives → verify our RX path decodes them
- Check `eoip-analyzer` on live capture shows keepalive packets
- Check daemon logs show tunnel going Active

**Step 2: Send keepalives**
- Verify our keepalive packets reach MikroTik
- On MikroTik: `/interface eoip monitor eoip-linux` should show "running"
- Capture our outbound packets, verify with `eoip-analyzer` — zero deviations

**Step 3: Ping MikroTik → eoip-rs**
- From mk-a: `ping 10.255.1.2`
- Our daemon should receive the ICMP-in-EoIP, deliver to TAP, kernel responds
- If this works, L2+L3 connectivity is proven

**Step 4: Ping eoip-rs → MikroTik**
- From eoip-server: `ping 10.255.1.1`
- Our TX path encodes ICMP into EoIP, MikroTik receives and responds

**Step 5: Bidirectional sustained**
- Run ping with count=100, verify 0% loss
- Run for 10 minutes, verify tunnel stays Active on both sides

### 5.5 Diagnostics Protocol

At each step, simultaneously capture on both sides:
```bash
# On eoip-server:
tcpdump -i any -w /tmp/step-N-server.pcap 'ip proto 47'

# On mk-a (via RouterOS):
/tool sniffer quick ip-protocol=47 file-name=step-N-mk
```

Feed all captures through `eoip-analyzer` after each step. Any deviation = stop, diagnose, fix before proceeding.

### 5.6 Common Failure Modes & Debugging

| Symptom | Likely Cause | Debug Steps |
|---------|-------------|-------------|
| MikroTik shows "not running" | Our keepalives malformed | Capture outbound, check magic/endianness |
| Ping from MK works, reply lost | TX path encoding wrong | Capture our TX, compare with MK format |
| Tunnel up but no ping | TAP not configured / ARP issue | Check `ip addr` on TAP, check ARP tables |
| Intermittent drops | Keepalive timeout race | Check timing, increase timeout |
| Permission denied | Helper not running / wrong socket | Check helper logs, socket permissions |

## Success Criteria

- [ ] MikroTik shows tunnel as "running"
- [ ] Bidirectional ping with 0% loss over 10 minutes
- [ ] All captures show zero protocol deviations
- [ ] Daemon logs clean (no errors, no panics)
- [ ] gRPC API shows correct stats: `grpcurl localhost:50051 eoip.v1.StatsService/GetStats`

## Artifacts

- `tests/captures/interop-step1-keepalive.pcap` through `interop-step5-sustained.pcap`
- `tests/infra/deploy-eoip-server.sh`
- `tests/infra/config-interop-single.toml`
