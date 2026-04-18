# Phase 13: IPsec Secret (MikroTik EoIP Encryption)

**Status:** Complete (2026-04-18)
**Priority:** Medium — needed for 100% MikroTik compatibility
**Dependencies:** Phase 12 (PMTUD) — for MTU adjustment with ESP overhead

---

## Background

MikroTik EoIP supports an optional `ipsec-secret` parameter. When set on both peers, RouterOS automatically creates an IPsec transport mode SA (Security Association) between the tunnel endpoints. The EoIP traffic is then encrypted with IPsec ESP before transmission.

This is NOT a VPN tunnel wrapping — it's IPsec transport mode applied directly to the GRE/EoIP packets on the wire. The outer IP header stays plaintext; the GRE payload is encrypted.

### MikroTik Config

```routeros
/interface eoip add name=eoip1 remote-address=1.2.3.4 tunnel-id=100 \
    ipsec-secret=SecretPass allow-fast-path=no
```

**Important:** `allow-fast-path=no` is **required** — MikroTik rejects the combination of `ipsec-secret` with fast-path enabled.

### Wire Format

```
Without IPsec:  [IP(proto=47)][GRE/EoIP][Ethernet Frame]
With IPsec:     [IP(proto=50)][ESP Hdr][IV][GRE/EoIP][Ethernet Frame][Pad][ESP Auth]
```

IP protocol changes from 47 (GRE) to 50 (ESP) on the wire. The kernel's IPsec stack handles encryption/decryption transparently — after decryption, the inner protocol is GRE.

---

## Research Results (2026-04-18, Live CHR Capture)

Two MikroTik CHR 7.18.2 instances deployed on Hetzner, EoIP tunnel with `ipsec-secret=TestSecret123`. All unknowns from the original PRD are now resolved.

### IKE Phase 1 (Auto-Created Peer)

```
exchange-mode = main            ← IKEv1 Main Mode (NOT IKEv2)
enc-algorithm = aes-128, 3des   ← offers both, negotiates best
hash-algorithm = sha1
dh-group = modp2048, modp1024   ← offers both
lifetime = 1d (24 hours)
nat-traversal = yes
dpd-interval = 8s
dpd-maximum-failures = 4
```

### ESP Phase 2 (Auto-Created Proposal "default")

```
enc-algorithms = aes-256-cbc, aes-192-cbc, aes-128-cbc   ← offers three
auth-algorithms = sha1
lifetime = 30m
pfs-group = modp1024
replay = 128
```

Negotiated result: **AES-256-CBC + SHA1**, 30-minute rekey, PFS with modp1024.

### Auto-Created IPsec Objects

MikroTik creates three dynamic objects when `ipsec-secret` is set:

1. **Peer** (dynamic):
   ```
   name="eoip-test" address=<remote>/32 profile=default exchange-mode=main
   ```

2. **Identity** (dynamic):
   ```
   peer=eoip-test auth-method=pre-shared-key secret="TestSecret123"
   ```

3. **Policy** (dynamic):
   ```
   peer=eoip-test src-address=<local>/32 dst-address=<remote>/32
   protocol=gre action=encrypt level=require ipsec-protocols=esp
   proposal=default tunnel=no
   ```
   `tunnel=no` confirms **transport mode**.

### Installed SAs (Runtime)

4 SAs observed (2 per direction — Phase 2 rekey creates new pair before old expires):

```
spi=0xD124002 auth=sha1 enc=aes-cbc key-size=256 lifetime=24m/30m replay=128
```

### MTU Impact

ESP overhead reduces the overlay MTU:
- **Without IPsec:** 1500 - 42 = **1458**
- **With IPsec:** 1500 - 42 - 38 (ESP) = **~1420** (MikroTik shows 1416)

ESP overhead breakdown: 8 (ESP header) + 16 (AES-CBC IV) + 2 (pad length + next header) + 12 (SHA1 auth tag) = 38 bytes.

---

## Implementation Plan: VICI + rustici

### Chosen Approach

Use strongSwan's **VICI control protocol** via the `rustici` Rust crate to programmatically manage IKE connections. This is cleaner than shelling out to `swanctl` and fits the daemon's socket-based IPC architecture.

**strongSwan is an external dependency** — user installs it separately (documented). An optional install script is provided.

**Why VICI over shell-out:**
- No subprocess overhead per tunnel create/destroy
- Type-safe protocol interaction
- Can monitor SA state changes via VICI events
- Fits existing architecture (socket-based IPC, like the helper)

**Why not Approach A3 (static XFRM):**
- MikroTik uses full IKEv1 with PSK — we must do IKE for interop
- IKEv1 is too complex to implement from scratch
- strongSwan handles rekeying, DPD, NAT-T automatically

### Step 1: Config + Proto + CLI (~60 lines)

**Files:**
- `crates/eoip-rs/src/config.rs` — add `pub ipsec_secret: Option<String>` to TunnelConfig
- `crates/eoip-api/proto/eoip.proto` — add `string ipsec_secret = 11` and `bool ipsec_active = 12` to Tunnel message; `string ipsec_secret = 6` to CreateTunnelRequest
- `crates/eoip-cli/src/parse.rs` — parse `ipsec-secret=X` parameter
- `crates/eoip-cli/src/commands.rs` — pass ipsec_secret in CreateTunnelRequest
- `crates/eoip-cli/src/output.rs` — show `ipsec=yes/no ipsec-active=yes/no` in detail

```toml
[[tunnel]]
tunnel_id = 100
remote = "1.2.3.4"
ipsec_secret = "SecretPass"  # optional
```

### Step 2: VICI client module (~200 lines)

**New files:**
- `crates/eoip-rs/src/ipsec/mod.rs` — IpsecManager
- `crates/eoip-rs/src/ipsec/vici.rs` — VICI client wrapper around `rustici`

```rust
pub struct ViciClient { /* wraps rustici::Client */ }

impl ViciClient {
    fn connect() -> Result<Self>;                    // /run/strongswan/charon.vici
    fn load_connection(name: &str, cfg: &IpsecTunnelConfig) -> Result<()>;
    fn unload_connection(name: &str) -> Result<()>;
    fn load_shared_secret(name: &str, secret: &str, local: &str, remote: &str) -> Result<()>;
    fn initiate(name: &str) -> Result<()>;
    fn terminate(name: &str) -> Result<()>;
    fn list_sas() -> Result<Vec<SaInfo>>;
    fn is_available() -> bool;                       // checks socket exists
}
```

**Cargo.toml:** Add `rustici` dependency (unix only)

### Step 3: VICI config generation (~80 lines)

**New file:** `crates/eoip-rs/src/ipsec/config.rs`

Generate VICI messages matching MikroTik's exact IKE/ESP parameters:

```rust
pub struct IpsecTunnelConfig {
    pub name: String,           // "eoip-{tunnel_id}"
    pub local_addr: IpAddr,
    pub remote_addr: IpAddr,
    pub secret: String,
}
```

**Key config values to match MikroTik:**

| Parameter | Value | Why |
|-----------|-------|-----|
| `version` | `1` | MikroTik uses IKEv1 main mode |
| `proposals` | `aes128-sha1-modp2048, aes128-sha1-modp1024, 3des-sha1-modp2048` | Match MikroTik's Phase 1 offers |
| `esp_proposals` | `aes256-sha1, aes192-sha1, aes128-sha1` | Match MikroTik's Phase 2 offers |
| `mode` | `transport` | Not tunnel mode |
| `local_ts` | `{local}/32[gre]` | GRE protocol selector |
| `remote_ts` | `{remote}/32[gre]` | GRE protocol selector |
| `dpd_delay` | `8s` | Match MikroTik DPD |
| `rekey_time` | `24h` | Phase 1 lifetime |
| `life_time` | `30m` | Phase 2 lifetime |

### Step 4: Tunnel lifecycle integration (~60 lines)

**Modified files:**
- `crates/eoip-rs/src/tunnel/manager.rs` — in `create_tunnel()`: if `config.ipsec_secret.is_some()`, call `ipsec_manager.setup_tunnel()`. In `destroy_tunnel()`: call `teardown_tunnel()`.
- `crates/eoip-rs/src/main.rs` — create `IpsecManager` at startup, call setup for startup tunnels with ipsec_secret
- `crates/eoip-rs/src/api/tunnel_svc.rs` — populate `ipsec_active` from manager

### Step 5: MTU adjustment (~20 lines)

**Modified file:** `crates/eoip-rs/src/net/mtu.rs`
- Add `pub const IPSEC_ESP_OVERHEAD: u16 = 38;`
- `MtuConfig::resolve()` subtracts ESP overhead when tunnel has ipsec_secret

### Step 6: SA monitoring (~80 lines)

**New file:** `crates/eoip-rs/src/ipsec/monitor.rs`
- Periodic task (every 30s) queries `list-sas` via VICI
- If SA is down, attempts `initiate`
- Updates `ipsec_active` on TunnelHandle
- Logs rekeying events

### Step 7: Documentation + optional installer (~40 lines)

**New files:**
- `docs/guides/IPSEC.md` — strongSwan setup, config examples, MikroTik side, troubleshooting
- `scripts/setup-strongswan.sh` — optional installer (detect distro, install, enable, verify)

---

## Key Files

| File | Role |
|------|------|
| `crates/eoip-rs/src/ipsec/mod.rs` | NEW — IpsecManager |
| `crates/eoip-rs/src/ipsec/vici.rs` | NEW — VICI client |
| `crates/eoip-rs/src/ipsec/config.rs` | NEW — VICI config builder |
| `crates/eoip-rs/src/ipsec/monitor.rs` | NEW — SA monitor |
| `crates/eoip-rs/src/config.rs` | Add ipsec_secret field |
| `crates/eoip-rs/src/tunnel/manager.rs` | IPsec lifecycle hooks |
| `crates/eoip-rs/src/net/mtu.rs` | ESP overhead constant |
| `crates/eoip-api/proto/eoip.proto` | IPsec proto fields |
| `crates/eoip-cli/src/parse.rs` | Parse ipsec-secret |
| `docs/guides/IPSEC.md` | NEW — setup guide |

## Success Criteria

- `ipsec_secret` in config triggers automatic IKE SA creation via strongSwan
- EoIP-rs ↔ MikroTik CHR tunnel works encrypted (ESP on wire)
- `print detail` shows `ipsec=yes ipsec-active=yes`
- SA rekeying works (Phase 2 every 30min, Phase 1 every 24h)
- Cleanup on tunnel destroy (no leaked SAs in strongSwan)
- No performance regression when ipsec_secret is not set
- Graceful fallback when strongSwan is not installed (tunnel works unencrypted, logs warning)
- MTU auto-adjusts for ESP overhead (1458 → ~1420)

## Test Lab

Deploy 2 CHR VMs + 1 Linux VM with strongSwan:
```bash
tests/infra/deploy-chr.sh -n chr-a -k wz
tests/infra/deploy-chr.sh -n chr-b -k wz
# build-vm already exists with Rust toolchain
# Install strongSwan on build-vm: apt install strongswan-charon strongswan-swanctl
```

Test matrix:
1. EoIP-rs (Linux) ↔ MikroTik CHR — primary interop test
2. MikroTik CHR ↔ MikroTik CHR — baseline reference (already verified)
3. EoIP-rs ↔ EoIP-rs — Linux-to-Linux IPsec (stretch goal)

---

## Implementation Results

**Completed:** 2026-04-18

### Approach

Used strongSwan's VICI control protocol via the `rustici` Rust crate. EoIP-rs connects to strongSwan's VICI socket to programmatically load IKE connections, shared secrets, and initiate SAs. IKEv1 main mode with AES-256-CBC/SHA1 ESP transport mode, matching MikroTik's auto-created IPsec parameters exactly.

### Performance

- Encrypted throughput: ~230 Mbps (EoIP-rs ↔ MikroTik CHR, Hetzner CX23)
- SA rekeying (Phase 2 every 30min) operates without packet loss

### Key Finding

AF_PACKET (PACKET_MMAP) RX path is skipped when IPsec is active. The kernel's XFRM/IPsec stack decrypts ESP packets and delivers the inner GRE protocol via the raw socket path, bypassing the AF_PACKET ring buffer. This is handled automatically — no special configuration needed.

### Files Created/Modified

| File | Change |
|------|--------|
| `crates/eoip-rs/src/ipsec/mod.rs` | NEW — IpsecManager lifecycle |
| `crates/eoip-rs/src/ipsec/vici.rs` | NEW — VICI client wrapper |
| `crates/eoip-rs/src/ipsec/config.rs` | NEW — VICI config builder (IKEv1 params) |
| `crates/eoip-rs/src/ipsec/monitor.rs` | NEW — SA health monitor (30s poll) |
| `crates/eoip-rs/src/config.rs` | Added `ipsec_secret` field |
| `crates/eoip-rs/src/tunnel/manager.rs` | IPsec lifecycle hooks |
| `crates/eoip-rs/src/net/mtu.rs` | ESP overhead constant (38 bytes) |
| `crates/eoip-api/proto/eoip.proto` | Added ipsec_secret, ipsec_active fields |
| `crates/eoip-cli/src/parse.rs` | Parse `ipsec-secret=X` |
| `docs/guides/IPSEC.md` | NEW — setup guide |
| `scripts/setup-strongswan.sh` | NEW — optional installer |
