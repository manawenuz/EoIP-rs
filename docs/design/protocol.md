# EoIP-rs Protocol Specification

**Version:** 1.0-draft  
**Date:** 2026-04-17

---

## 1. Overview

EoIP-rs implements three encapsulation modes for tunneling Ethernet frames (Layer 2) over IP networks:

| Mode | Transport | IP Protocol | Header Size | MikroTik Compatible |
|------|-----------|-------------|-------------|---------------------|
| EoIP | IPv4 | 47 (GRE) | 8 bytes | Yes |
| EoIPv6 | IPv6 | 97 (EtherIP) | 2 bytes | Yes |
| UDP Encapsulation | IPv4 or IPv6 | 17 (UDP) | 8B UDP + 4B shim + inner header | No (EoIP-rs extension) |

All modes carry complete Ethernet frames including the 14-byte Ethernet header (6B dst MAC + 6B src MAC + 2B EtherType). VLAN-tagged frames (802.1Q) add 4 bytes to the Ethernet header.

---

## 2. EoIP Header (IPv4 Transport, IP Protocol 47)

### 2.1 Wire Format

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|     0x20      |     0x01      |     0x64      |     0x00      |  Bytes 0-3: Magic
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|       Payload Length (BE)     |       Tunnel ID (LE)          |  Bytes 4-7
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
|                    Ethernet Frame Payload                     |
|                         (variable)                            |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

### 2.2 Field Definitions

| Field | Offset | Size | Endianness | Description |
|-------|--------|------|------------|-------------|
| Magic | 0 | 4 bytes | Fixed | `0x20016400`. Identifies the packet as MikroTik EoIP. |
| Payload Length | 4 | 2 bytes | **Big-endian** | Length of the Ethernet frame that follows. Valid range: 14–65535. |
| Tunnel ID | 6 | 2 bytes | **Little-endian** | Tunnel identifier. Range: 0–65535. Both peers must agree. |

**Critical: mixed endianness.** Payload Length is big-endian, Tunnel ID is little-endian. This is intentional MikroTik behavior.

### 2.3 Magic Bytes as GRE Header

The magic `0x20016400` overlaps with the GRE header structure:

```
Byte 0 (0x20): C=0 R=0 K=1 S=0 s=0 Recur=000  (Key bit set)
Byte 1 (0x01): Flags=00000 Ver=001              (Version 1)
Bytes 2-3 (0x6400): Protocol Type = 0x6400       (non-standard)
```

This is why EoIP uses IP Protocol 47 — the kernel treats it as GRE. The non-standard protocol type `0x6400` and version `1` distinguish EoIP from legitimate GRE.

### 2.4 Tunnel ID Encoding/Decoding

```rust
// Encode (host u16 → wire)
buf[6..8].copy_from_slice(&tunnel_id.to_le_bytes());

// Decode (wire → host u16)
let tunnel_id = u16::from_le_bytes([buf[6], buf[7]]);
```

---

## 3. EoIPv6 Header (IPv6 Transport, IP Protocol 97)

### 3.1 Wire Format

```
 0                   1
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| TID_hi  |0 0 1 1|   TID_lo   |  Bytes 0-1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                |
|      Ethernet Frame Payload    |
|           (variable)           |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

Bit-level layout:

```
Byte 0:  [T T T T V V V V]
Byte 1:  [t t t t t t t t]

TTTT     = Tunnel ID bits 11..8   (high nibble of byte 0)
VVVV     = 0x3 (0011)             (low nibble of byte 0, EtherIP version)
tttttttt = Tunnel ID bits 7..0    (all of byte 1)
```

### 3.2 Field Definitions

| Field | Bit Offset | Size | Description |
|-------|-----------|------|-------------|
| TID_high | Byte 0, bits 7-4 | 4 bits | Upper 4 bits of the 12-bit Tunnel ID |
| Version | Byte 0, bits 3-0 | 4 bits | Fixed `0x3`. Per RFC 3378. |
| TID_low | Byte 1, bits 7-0 | 8 bits | Lower 8 bits of the 12-bit Tunnel ID |

**Tunnel ID range: 0–4095** (12 bits). Significantly smaller than EoIP's 16-bit range.

### 3.3 Relationship to RFC 3378

RFC 3378 defines EtherIP with a 2-byte header: `[version(4)=0x3][reserved(12)=0x000]`. MikroTik encodes the Tunnel ID into the reserved bits. Tunnel ID 0 produces a valid RFC 3378 packet (`0x03 0x00`); non-zero IDs would be rejected by strict RFC 3378 implementations.

### 3.4 Tunnel ID Encoding/Decoding

```rust
// Encode (host u16 → wire, only bottom 12 bits used)
let tid = tunnel_id & 0x0FFF;
buf[0] = ((tid >> 8) as u8) << 4 | 0x03;
buf[1] = (tid & 0xFF) as u8;

// Decode (wire → host u16)
let tunnel_id = ((buf[0] as u16 >> 4) << 8) | buf[1] as u16;
```

**Worked example — Tunnel ID 300 (0x12C):**

```
Encode:
  tid = 0x12C
  buf[0] = (0x01 << 4) | 0x03 = 0x13
  buf[1] = 0x2C
  Wire: [0x13, 0x2C]

Decode:
  (0x13 >> 4) = 0x01
  tid = (0x01 << 8) | 0x2C = 0x12C = 300
```

---

## 4. UDP Encapsulation Mode (EoIP-rs Extension)

### 4.1 Rationale

Raw IP protocols (47, 97) cannot traverse NAT or many VPN tunnels. UDP encapsulation wraps the EoIP packet inside a UDP datagram for NAT/firewall traversal.

**This mode is NOT compatible with MikroTik RouterOS.**

### 4.2 Wire Format

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|          Source Port          |       Destination Port        |  UDP Header
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+  (8 bytes)
|            Length             |           Checksum            |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|     0x45      |     0x4F      |      Type     |   Reserved    |  Shim (4 bytes)
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
|         Inner EoIP or EoIPv6 header + Ethernet Payload        |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

### 4.3 Shim Header Fields

| Field | Offset | Size | Value | Description |
|-------|--------|------|-------|-------------|
| Magic | 0 | 2 bytes | `0x45 0x4F` ("EO") | Identifies UDP payload as EoIP-rs. |
| Type | 2 | 1 byte | `0x04` = EoIP, `0x06` = EoIPv6 | Inner encapsulation format. |
| Reserved | 3 | 1 byte | `0x00` | Must be zero on TX. Ignored on RX. |

**Default UDP port:** 26969 (configurable).

---

## 5. Complete Packet Diagrams

### Mode A: EoIP over IPv4 (MikroTik-compatible)

```
+------------------+---------------------+-------------------------------+
| IPv4 Header      | EoIP Header         | Ethernet Frame                |
| 20 bytes         | 8 bytes             | 14+ bytes                     |
| Proto=47         | Magic+Len+TID       | DstMAC SrcMAC EtherType Data  |
+------------------+---------------------+-------------------------------+
```

### Mode B: EoIPv6 over IPv6 (MikroTik-compatible)

```
+------------------+---------------------+-------------------------------+
| IPv6 Header      | EoIPv6 Header       | Ethernet Frame                |
| 40 bytes         | 2 bytes             | 14+ bytes                     |
| NextHdr=97       | TID+Version         | DstMAC SrcMAC EtherType Data  |
+------------------+---------------------+-------------------------------+
```

### Mode C: EoIP over UDP over IPv4

```
+------------------+-----------+----------+---------------------+-------------------+
| IPv4 Header      | UDP Hdr   | Shim     | EoIP Header         | Ethernet Frame    |
| 20 bytes         | 8 bytes   | 4 bytes  | 8 bytes             | 14+ bytes         |
| Proto=17         | Port=cfg  | "EO"+04  | Magic+Len+TID       |                   |
+------------------+-----------+----------+---------------------+-------------------+
```

### Mode D: EoIPv6 over UDP over IPv6

```
+------------------+-----------+----------+---------------------+-------------------+
| IPv6 Header      | UDP Hdr   | Shim     | EoIPv6 Header       | Ethernet Frame    |
| 40 bytes         | 8 bytes   | 4 bytes  | 2 bytes             | 14+ bytes         |
| NextHdr=17       | Port=cfg  | "EO"+06  | TID+Version         |                   |
+------------------+-----------+----------+---------------------+-------------------+
```

### Mode E: EoIPv6 over UDP over IPv4 (cross-stack)

```
+------------------+-----------+----------+---------------------+-------------------+
| IPv4 Header      | UDP Hdr   | Shim     | EoIPv6 Header       | Ethernet Frame    |
| 20 bytes         | 8 bytes   | 4 bytes  | 2 bytes             | 14+ bytes         |
| Proto=17         | Port=cfg  | "EO"+06  | TID+Version         |                   |
+------------------+-----------+----------+---------------------+-------------------+
```

### Mode F: EoIP over UDP over IPv6 (cross-stack)

```
+------------------+-----------+----------+---------------------+-------------------+
| IPv6 Header      | UDP Hdr   | Shim     | EoIP Header         | Ethernet Frame    |
| 40 bytes         | 8 bytes   | 4 bytes  | 8 bytes             | 14+ bytes         |
| NextHdr=17       | Port=cfg  | "EO"+04  | Magic+Len+TID       |                   |
+------------------+-----------+----------+---------------------+-------------------+
```

---

## 6. MTU Calculations

### 6.1 Overhead Summary

| Component | Size |
|-----------|------|
| IPv4 header | 20 bytes |
| IPv6 header | 40 bytes |
| EoIP header | 8 bytes |
| EoIPv6 header | 2 bytes |
| UDP header | 8 bytes |
| UDP shim header | 4 bytes |
| Ethernet header (untagged) | 14 bytes |
| 802.1Q VLAN tag | 4 bytes (additional) |

### 6.2 Tunnel Interface MTU

The tunnel interface MTU determines the maximum L3 payload through the tunnel:

```
tunnel_mtu = path_mtu - transport_overhead - encap_header - ethernet_header
```

| Mode | Formula | MTU (path=1500) |
|------|---------|-----------------|
| A: EoIP/IPv4 | `path - 20 - 8 - 14` | **1458** |
| B: EoIPv6/IPv6 | `path - 40 - 2 - 14` | **1444** |
| C: EoIP/UDP/IPv4 | `path - 20 - 8 - 4 - 8 - 14` | **1446** |
| D: EoIPv6/UDP/IPv6 | `path - 40 - 8 - 4 - 2 - 14` | **1432** |
| E: EoIPv6/UDP/IPv4 | `path - 20 - 8 - 4 - 2 - 14` | **1452** |
| F: EoIP/UDP/IPv6 | `path - 40 - 8 - 4 - 8 - 14` | **1426** |

### 6.3 Recommended Defaults

| Scenario | Recommended TAP MTU |
|----------|-------------------|
| Direct internet, 1500B path | Calculated per mode |
| Over WireGuard (~1420B path) | 1374 |
| Over OpenVPN/IPsec (variable) | 1300 |
| Unknown path | 1374 |

When VLAN-tagged frames are expected, subtract an additional 4 bytes.

---

## 7. Packet Validation Rules (RX Path)

All checks are fail-fast: drop on first failure.

### 7.1 EoIP (Mode A)

| # | Check | Condition | On Failure |
|---|-------|-----------|------------|
| 1 | Min length | `ip_payload_len >= 8` | Drop (runt) |
| 2 | Magic | `buf[0..4] == [0x20, 0x01, 0x64, 0x00]` | Drop (not EoIP) |
| 3 | Payload length | `payload_len_field <= ip_payload_len - 8` | Drop (corrupted) |
| 4 | Min payload | `payload_len_field >= 14` | Drop (no Ethernet header) |
| 5 | Tunnel ID | `tunnel_id == configured_id` | Drop (wrong tunnel) |
| 6 | Source IP | `src_ip == configured_peer` | Drop (optional, for fixed peers) |

**Note:** Use `payload_len_field` (not IP length minus header) to determine frame size — this strips padding bytes.

### 7.2 EoIPv6 (Mode B)

| # | Check | Condition | On Failure |
|---|-------|-----------|------------|
| 1 | Min length | `payload_len >= 16` (2B header + 14B Ethernet) | Drop |
| 2 | Version | `(buf[0] & 0x0F) == 0x03` | Drop |
| 3 | Tunnel ID | `tunnel_id == configured_id` | Drop |
| 4 | Source IP | `src_ipv6 == configured_peer` | Drop (optional) |

Frame length = `ipv6_payload_length - 2`.

### 7.3 UDP Encapsulation (Modes C-F)

| # | Check | Condition | On Failure |
|---|-------|-----------|------------|
| 1 | Min UDP payload | `len >= 4` | Drop |
| 2 | Shim magic | `buf[0..2] == [0x45, 0x4F]` | Drop |
| 3 | Type | `buf[2] == 0x04 \|\| buf[2] == 0x06` | Drop |
| 4 | Inner min length | Type 0x04: `remaining >= 22`; Type 0x06: `remaining >= 16` | Drop |
| 5 | Inner validation | Apply Mode A or B checks | Drop per inner rules |

### 7.4 Ethernet Frame (All Modes)

| # | Check | Condition | On Failure |
|---|-------|-----------|------------|
| 1 | Min frame | `frame_len >= 14` | Drop |
| 2 | Max frame | `frame_len <= mtu + 14` (or +18 for VLAN) | Drop, log warning |

---

## 8. PMTU Discovery

### 8.1 Outer Path MTU

**Raw IP modes (A, B):**
- Set `IP_MTU_DISCOVER = IP_PMTUDISC_DO` on the raw socket.
- Kernel returns ICMP "Fragmentation Needed" / ICMPv6 "Packet Too Big".
- On PMTU change: update cached path MTU, recalculate TAP interface MTU, log.

**UDP modes (C-F):**
- Set `IP_MTU_DISCOVER = IP_PMTUDISC_DO` on the UDP socket.
- Use `IP_RECVERR` / `IPV6_RECVERR` to receive ICMP errors asynchronously.
- On `EMSGSIZE`: read new MTU from error queue, adjust.

### 8.2 Inner Payload

EoIP operates at L2 — the TAP interface MTU controls inner IP packet size. Inner hosts perform their own PMTU discovery. If an inner frame exceeds the tunnel capacity after encapsulation:

- **Optional**: Generate ICMP "Fragmentation Needed" back into the TAP with the correct MTU.
- **Default**: Drop and let inner PMTU discovery handle it (simpler, requires conservative MTU).

---

## 9. Fragmentation Handling

### 9.1 Principles

1. **Outer packets SHOULD NOT be fragmented.** Set DF bit on IPv4; IPv6 does not fragment at routers.
2. **Rely on PMTU discovery** to size the TAP MTU so encapsulated frames fit.
3. **No reassembly** by EoIP-rs for outer packets — the OS stack reassembles before delivering to the socket.

### 9.2 On EMSGSIZE

If `sendmsg()` returns `EMSGSIZE`:
- Drop the frame. The inner sender will reduce packet size via PMTU discovery.
- Optionally generate ICMP back into the TAP.

### 9.3 Jumbo Frames

If the path supports jumbo frames (MTU > 1500):
- TAP MTU can be increased.
- EoIP's 16-bit payload length supports frames up to 65535 bytes.
- EoIPv6 relies on IPv6's 16-bit Payload Length (65535 - 2 = 65533 byte frames).

### 9.4 Fallback: Fragmentation-Permissive Mode

Optional config to clear the DF bit. Trades performance for compatibility when PMTU discovery is broken (e.g., ICMP blackholed). Logs a warning at startup.

```toml
allow_fragmentation = false  # default
```

---

## 10. Keepalive

### 10.1 EoIP Keepalive Format

Keepalive packets are EoIP packets with `payload_length = 0` and no Ethernet frame:

```
Wire: [0x20, 0x01, 0x64, 0x00, 0x00, 0x00, TID_lo, TID_hi]  (8 bytes total)
```

- Accept without delivering to TAP.
- Send at configurable interval (MikroTik default: 10 seconds).
- Declare tunnel stale after timeout (MikroTik default: 100 seconds / 10 retries).

### 10.2 EoIPv6 Keepalive

Not documented by MikroTik. EoIP-rs sends the 2-byte header with zero-length payload:

```
Wire: [TID_encoded_byte0, TID_encoded_byte1]  (2 bytes, no Ethernet payload)
```

---

## 11. Tunnel ID Constraints

| Protocol | Bits | Range | Endianness | Notes |
|----------|------|-------|------------|-------|
| EoIP (IPv4) | 16 | 0–65535 | Little-endian | Full u16 |
| EoIPv6 (IPv6) | 12 | 0–4095 | Big-endian (nibble-packed) | Upper 4 bits in byte 0 |
| UDP mode | Inherited | Per inner format | Per inner format | Shim has no TID |

**Dual-stack constraint:** If a tunnel may operate in both EoIP and EoIPv6, the ID must be 0–4095.

---

## 12. Byte-Level Worked Examples

### 12.1 EoIP: 64-byte Ethernet Frame, Tunnel ID 42

```
Tunnel ID 42 = 0x002A → LE bytes: [0x2A, 0x00]
Payload length 64 = 0x0040 → BE bytes: [0x00, 0x40]

Offset  Hex                                           Description
------  --------------------------------------------  -----------
0x00    20 01 64 00                                   Magic
0x04    00 40                                         Payload length = 64 (BE)
0x06    2A 00                                         Tunnel ID = 42 (LE)
0x08    FF FF FF FF FF FF 00 11 22 33 44 55 08 00     Ethernet header
0x16    [... 50 bytes inner IP ...]                   Payload
```

Total on wire: 20 (IP) + 8 (EoIP) + 64 (Ethernet) = 92 bytes.

### 12.2 EoIPv6: Tunnel ID 1000 (0x3E8)

```
TID 1000 = 0x3E8
  high = 0x03, low = 0xE8
  byte 0 = (0x03 << 4) | 0x03 = 0x33
  byte 1 = 0xE8

Offset  Hex                                           Description
------  --------------------------------------------  -----------
0x00    33 E8                                         EoIPv6 header (TID=1000)
0x02    FF FF FF FF FF FF 00 11 22 33 44 55 86 DD     Ethernet header (IPv6)
0x10    [... inner IPv6 packet ...]                   Payload
```

### 12.3 UDP-Encapsulated EoIP: Tunnel ID 7

```
Offset  Hex                                           Description
------  --------------------------------------------  -----------
(UDP payload starts)
0x00    45 4F                                         Shim magic ("EO")
0x02    04                                            Type = EoIP
0x03    00                                            Reserved
0x04    20 01 64 00                                   EoIP magic
0x08    00 40                                         Payload length = 64 (BE)
0x0A    07 00                                         Tunnel ID = 7 (LE)
0x0C    [... 64 bytes Ethernet frame ...]             Frame
```

UDP payload size: 4 + 8 + 64 = 76 bytes. Total on wire (IPv4): 20 + 8 + 76 = 104 bytes.

---

## 13. Security Considerations

1. **No encryption or authentication.** Must be layered over an encrypted transport.
2. **Tunnel ID is not a security mechanism.** 16-bit provides demultiplexing, not access control.
3. **Source address validation** should be enforced for fixed peers.
4. **Rate limiting** on RX protects against packet floods.
5. **UDP encapsulation** does not add authentication.
