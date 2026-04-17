# EoIP-rs Design Documentation

## Project Goal

EoIP-rs is a **userspace** EoIP (Ethernet over IP) and EoIPv6 implementation in Rust, compatible with MikroTik RouterOS. It enables Layer 2 (Ethernet frame) tunneling over IP networks, designed to chain with VPN protocols like WireGuard, SSTP, and ZeroTier for long-distance bridging.

## Non-Goals

- **Kernel module**: This is intentionally userspace for portability and safety.
- **Built-in encryption**: Security is delegated to the underlying VPN transport.
- **MPLS/VPLS**: Only point-to-point L2 tunnels, not multipoint.
- **Routing**: EoIP-rs bridges Ethernet frames, it does not route IP packets.

## Design Documents

| Document | Description |
|----------|-------------|
| [Protocol Specification](protocol.md) | Wire format for EoIP, EoIPv6, and UDP encapsulation modes |
| [System Architecture](architecture.md) | Component design, crate structure, data flow, configuration |
| [Threading Model](threading.md) | Async/thread layout, adaptive batching, lock-free structures |
| [Performance Design](performance.md) | Syscall optimization, buffer pools, benchmarking strategy |
| [Platform Abstraction](platform.md) | Cross-platform TAP/TUN, raw sockets, privilege models |
| [Security Model](security.md) | Privilege separation, threat model, attack surface |

## Decision Log

| # | Decision | Choice | Rationale |
|---|----------|--------|-----------|
| 1 | MikroTik interop | Native GRE (proto 47) + EtherIP (proto 97) | Primary use case is bridging with MikroTik routers |
| 2 | UDP encapsulation | Designed, built later | NAT traversal for VPN chaining; not MikroTik-compatible |
| 3 | Platform priority | Linux first | Best TAP/L2 support; macOS/Windows/Android incremental |
| 4 | Dual-stack | Full (IPv4/IPv6 any combination) | MikroTik supports both; users need flexibility |
| 5 | Performance target | Balanced (adaptive batching) | Sub-ms latency idle, high throughput under load |
| 6 | Encryption | None (VPN handles it) | Avoids complexity; EoIP is always inside a VPN tunnel |
| 7 | Privilege model | Separation (root helper + unprivileged daemon) | Minimizes attack surface of the packet-processing daemon |
| 8 | Socket model | Shared raw socket + userspace demux | Scales to thousands of tunnels with 2 FDs total |
| 9 | Management API | gRPC (tonic) with streaming | Strongly typed, streaming for live events, good tooling |
| 10 | Virtual interface | TAP (L2) on Linux; TUN+bridge elsewhere | EoIP is L2; TAP is the natural fit where available |

## Reference Implementations

- [amphineko/eoip](https://github.com/amphineko/eoip) — Linux kernel module, MikroTik-compatible, GRE demux patch
- [bbonev/eoip](https://github.com/bbonev/eoip) — Linux kernel module, actively maintained, netlink management
- [agustim/openwrt-linux-eoip](https://github.com/agustim/openwrt-linux-eoip) — OpenWrt package wrapper for userspace linux-eoip

## MikroTik EoIP Protocol

MikroTik's EoIP is a proprietary Layer 2 tunneling protocol that encapsulates Ethernet frames inside a non-standard GRE header (IP protocol 47). MikroTik also supports EoIPv6 using EtherIP (IP protocol 97, RFC 3378) with a MikroTik-specific tunnel ID encoding. Neither protocol is formally specified — the wire format was reverse-engineered from packet captures.
