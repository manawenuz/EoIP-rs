# Phase 13: IPsec Secret (MikroTik EoIP Encryption)

**Status:** Planned
**Priority:** Medium — needed for 100% MikroTik compatibility
**Dependencies:** None (standalone, can be done in parallel with Phase 12)

---

## Background

MikroTik EoIP supports an optional `ipsec-secret` parameter. When set on both peers, RouterOS automatically creates an IPsec transport mode SA (Security Association) between the tunnel endpoints. The EoIP traffic is then encrypted with IPsec ESP before transmission.

This is NOT a VPN tunnel wrapping — it's IPsec transport mode applied directly to the GRE/EoIP packets on the wire. The outer IP header stays plaintext; the GRE payload is encrypted.

### MikroTik Config

```
/interface eoip add name=eoip1 remote-address=1.2.3.4 tunnel-id=100 ipsec-secret=SecretPass
```

When set:
- RouterOS creates an IPsec policy: `src=local, dst=remote, proto=gre, action=encrypt`
- Uses IKEv2 for key exchange (pre-shared key = the secret)
- ESP transport mode with AES-256-CBC + SHA-256 (default)
- SA established automatically when tunnel starts

### Wire Format

```
Without IPsec:  [IP][GRE/EoIP][Ethernet Frame]
With IPsec:     [IP][ESP Header][GRE/EoIP][Ethernet Frame][ESP Trailer][ESP Auth]
```

IP protocol changes from 47 (GRE) to 50 (ESP) on the wire. After IPsec decryption, the inner protocol is GRE.

---

## Implementation Approaches

### Approach A: Delegate to Linux IPsec Subsystem (Recommended)

Use the Linux kernel's built-in IPsec (XFRM) stack via Netlink. The daemon configures IPsec SAs/policies; the kernel handles encryption/decryption transparently.

**Pros:**
- Hardware offload support (NICs with IPsec offload)
- Kernel handles fragmentation post-encryption
- Well-tested, standards-compliant
- No crypto code in our daemon
- Compatible with MikroTik's IPsec implementation

**Cons:**
- Requires IKEv2 implementation or integration with strongSwan/charon
- Complex Netlink/XFRM API
- Need to manage SA lifetime, rekeying

**Sub-approaches:**

#### A1: Shell out to strongSwan (simplest)

Configure strongSwan/swanctl via config files when `ipsec_secret` is set.

```bash
# Generate swanctl.conf per tunnel
swanctl --load-all
```

**Pros:** Minimal code, full IKEv2, rekeying, DPD
**Cons:** External dependency (strongSwan must be installed), process management

#### A2: Direct XFRM + IKEv2 via library

Use a Rust IKEv2 library or implement minimal IKE handshake.

**Pros:** No external deps, fully self-contained
**Cons:** IKEv2 is complex (RFC 7296, ~100 pages). No mature Rust IKEv2 library exists.

#### A3: Static XFRM SAs (no IKE, PSK-derived keys)

Skip IKE entirely. Derive ESP keys directly from the pre-shared secret (e.g., HKDF-SHA256). Both sides compute the same keys from the same secret. No key exchange needed.

**Pros:** Simple, no IKE complexity, self-contained
**Cons:** No perfect forward secrecy, no rekeying, not standards-compliant IKE. But MikroTik's `ipsec-secret` with simple PSK is similarly simple.

**NOTE:** Need to verify if MikroTik uses IKE or static keys when `ipsec-secret` is set. If MikroTik does IKE, we must too for interop. Capture MikroTik IKE handshake to determine.

### Approach B: Userspace Encryption (Not Recommended)

Encrypt/decrypt GRE payloads in the daemon using a Rust crypto library.

**Pros:** Full control, no kernel interaction
**Cons:** No hardware offload, CPU-intensive, must implement ESP framing, breaks PACKET_MMAP, adds latency

---

## Research Required Before Implementation

### Critical: Determine MikroTik's IKE Behavior

1. **Set up MikroTik CHR with `ipsec-secret`**
2. **Capture the IKE handshake** with tcpdump/Wireshark (UDP port 500/4500)
3. **Determine:**
   - IKEv1 or IKEv2?
   - Authentication method (PSK, which hash?)
   - ESP cipher suite (AES-CBC, AES-GCM?)
   - SA lifetime and rekeying interval
   - Is DPD (Dead Peer Detection) used?
4. **Check MikroTik docs** for `/ip/ipsec/` auto-created policies when eoip ipsec-secret is set

### Test Commands

```bash
# Deploy CHR with ipsec-secret
ssh admin@CHR_IP '/interface eoip set eoip100 ipsec-secret=TestSecret'

# Capture IKE
tcpdump -i eth0 -w ike-capture.pcap 'udp port 500 or udp port 4500 or esp'

# Check auto-created IPsec policies on MikroTik
ssh admin@CHR_IP '/ip ipsec policy print'
ssh admin@CHR_IP '/ip ipsec peer print'
ssh admin@CHR_IP '/ip ipsec sa print'
```

---

## Implementation Plan (Approach A1 — strongSwan delegation)

### Step 1: Config + CLI

**What:** Add `ipsec_secret` field to tunnel config and CLI.

```toml
[[tunnel]]
tunnel_id = 100
remote = "1.2.3.4"
ipsec_secret = "SecretPass"  # optional
```

**Files:**
- `crates/eoip-rs/src/config.rs` — add `pub ipsec_secret: Option<String>` to TunnelConfig
- `crates/eoip-api/proto/eoip.proto` — add `string ipsec_secret = 10` to Tunnel message
- `crates/eoip-cli/src/commands.rs` — add to `add`/`set` commands
- `crates/eoip-cli/src/parse.rs` — parse `ipsec-secret=X`

**Estimated:** ~40 lines.

### Step 2: Research MikroTik IKE behavior

**What:** Deploy CHR with ipsec-secret, capture IKE handshake, determine cipher suite and protocol version.

**Output:** Document exact IKE parameters needed for interop.

### Step 3: strongSwan integration

**What:** When tunnel has `ipsec_secret`, generate swanctl config and load it.

**Files:**
- `crates/eoip-rs/src/ipsec/mod.rs` (new) — IPsec manager
- `crates/eoip-rs/src/ipsec/swanctl.rs` (new) — generate swanctl.conf snippet
  ```
  connections {
    eoip-100 {
      remote_addrs = 1.2.3.4
      local { auth = psk }
      remote { auth = psk }
      children {
        eoip-100 {
          local_ts = dynamic[gre]
          remote_ts = 1.2.3.4/32[gre]
          mode = transport
          esp_proposals = aes256-sha256
        }
      }
    }
  }
  secrets {
    ike-eoip-100 { secret = "SecretPass" }
  }
  ```
- `crates/eoip-rs/src/tunnel/manager.rs` — call IPsec setup after tunnel creation

**Estimated:** ~200 lines.

### Step 4: SA lifecycle management

**What:** Monitor SA status, handle rekeying events, clean up on tunnel destroy.

**Files:**
- `crates/eoip-rs/src/ipsec/monitor.rs` — watch swanctl status
- Cleanup: `swanctl --terminate --ike eoip-100` on tunnel destroy

**Estimated:** ~100 lines.

### Step 5: Verification

- EoIP-rs with ipsec-secret ↔ MikroTik CHR with ipsec-secret
- Verify: IKE handshake completes, ESP packets on wire, data flows encrypted
- Verify: works with PACKET_MMAP (ESP packets have different IP proto, need BPF update)

---

## Alternative: Approach A3 (Static XFRM, no IKE)

If MikroTik research shows static keys (unlikely but possible):

### Step 3-alt: Direct XFRM via Netlink

**What:** Use `rtnetlink` or raw Netlink to create XFRM states and policies.

**Files:**
- `crates/eoip-rs/src/ipsec/xfrm.rs` (new) — Netlink XFRM SA/SP management
  ```rust
  fn add_ipsec_transport_sa(
      local: IpAddr, remote: IpAddr,
      spi: u32, key: &[u8], // derived from ipsec_secret via HKDF
  ) -> Result<()>
  ```

**Pros:** No strongSwan dependency
**Cons:** No rekeying, no PFS, may not interop with MikroTik IKE

---

## Key Unknowns

1. **Does MikroTik use IKEv1 or IKEv2?** — determines which we must implement/integrate
2. **What cipher suite?** — must match exactly for interop
3. **Does MikroTik auto-create IPsec peer/policy or use static keys?** — determines our approach
4. **SA lifetime?** — need to match for rekeying
5. **Is strongSwan available on typical deployment targets?** — Debian/Ubuntu yes, embedded no

## Dependencies

- None for config/CLI (Step 1)
- CHR test infrastructure for research (Step 2)
- strongSwan package for Approach A1, or Netlink for A3

## Success Criteria

- `ipsec-secret=X` in config triggers automatic IPsec SA creation
- EoIP-rs ↔ MikroTik CHR tunnel works encrypted
- `print detail` shows IPsec status
- SA rekeying works (if using IKE)
- Cleanup on tunnel destroy (no leaked SAs)
- No performance regression when ipsec-secret is not set
