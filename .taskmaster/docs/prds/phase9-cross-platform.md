# Phase 9: Cross-Platform — Windows & macOS Support

**Status:** Draft  
**Priority:** Medium (future, after Linux is stable)  
**Dependencies:** Phase 8  
**Estimated Duration:** 2-4 weeks  
**Cost:** Windows VM ($5-20/month), macOS — local dev machine or CI runner

---

## Objective

Port EoIP-rs to Windows and macOS. Both platforms lack native TAP/raw socket support identical to Linux, requiring platform-specific adaptations. The goal is functional EoIP tunneling on all three major desktop/server OSes.

## Background

The PRD lists macOS, Windows, and Android as future targets with a "TUN+bridge approach" for platforms without native TAP. This phase covers Windows and macOS; Android is deferred.

## Platform Challenges

### Windows
- No `/dev/net/tun` — need **TAP-Windows** (OpenVPN TAP driver) or **WinTun** (WireGuard)
- No raw sockets for arbitrary IP protocols — need **Npcap** (WinPcap successor) or Windows raw socket API (limited)
- No `SCM_RIGHTS` — privilege separation model must change (named pipes? service account?)
- No `sendmmsg`/`recvmmsg` — use `WSASendTo`/`WSARecvFrom` with overlapped I/O
- Alternative: Use WinDivert for packet capture/injection

### macOS
- No TAP since macOS Catalina removed `/dev/tap*` — need `utun` (L3 only) + userspace bridge
- Raw sockets exist but limited (no binding to specific IP protocols on newer macOS)
- No `sendmmsg`/`recvmmsg` — use kqueue + sendto/recvfrom
- `SCM_RIGHTS` works on macOS Unix sockets
- Alternative: Use Packet Filter (pf) + divert sockets

## Requirements

### 9.1 Windows Port

**9.1.1 TAP Driver Integration**
- Evaluate: TAP-Windows (OpenVPN) vs WinTun (WireGuard) vs WinDivert
- Recommendation: WinTun — modern, kernel-mode, maintained, MIT-licensed
- Create `src/net/tap_windows.rs` using WinTun API
- Abstract behind a `TapDevice` trait shared with Linux impl

**9.1.2 Raw Socket / Packet Injection**
- Evaluate: Winsock raw sockets vs Npcap vs WinDivert
- Winsock raw sockets: `socket(AF_INET, SOCK_RAW, IPPROTO_GRE)` — may work but limited
- Npcap: full packet capture/injection, works but adds dependency
- WinDivert: kernel-mode packet interception, powerful but complex

**9.1.3 Privilege Model**
- Windows services run as SYSTEM or specific service account
- No SCM_RIGHTS — helper may not be needed if service has sufficient privileges
- Or: use named pipes for IPC between helper service and daemon service

**9.1.4 Build & Test**
- Cross-compile from Linux: `cargo build --target x86_64-pc-windows-msvc` (needs MSVC linker)
- Or: build on Windows with `cargo build`
- Test on: Windows Server 2022 VM (Hetzner dedicated or Azure)

### 9.2 macOS Port

**9.2.1 TUN Interface**
- Use `utun` interfaces (L3 only, no L2)
- Requires userspace Ethernet frame encapsulation in IP
- Or: Use third-party TAP driver (tun-tap-osx, unmaintained)
- Recommended: `utun` + userspace L2-over-L3 bridge

**9.2.2 Raw Socket**
- `socket(AF_INET, SOCK_RAW, 47)` works but requires root
- macOS may filter certain protocols
- Test on macOS 14+ (Sonoma)

**9.2.3 Build & Test**
- Native build on macOS: `cargo build`
- CI: GitHub Actions macOS runner
- Test: local dev machine or macOS CI

### 9.3 Platform Abstraction Layer

Refactor to platform-agnostic interfaces:

```rust
// src/net/tap.rs
pub trait TapDevice: Send + Sync {
    async fn read(&self, buf: &mut [u8]) -> io::Result<usize>;
    async fn write(&self, buf: &[u8]) -> io::Result<usize>;
}

// Platform implementations:
// src/net/tap_linux.rs   — /dev/net/tun + AsyncFd
// src/net/tap_windows.rs — WinTun API
// src/net/tap_macos.rs   — utun + userspace bridge
```

Similarly for raw sockets:
```rust
pub trait RawSocket: Send + Sync {
    fn send_to(&self, buf: &[u8], dest: IpAddr) -> io::Result<usize>;
    fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, IpAddr)>;
}
```

### 9.4 Testing Matrix

| Platform | TAP/TUN | Raw Socket | Helper | Priority |
|----------|---------|------------|--------|----------|
| Linux x86_64 | /dev/net/tun | SOCK_RAW | SCM_RIGHTS | Done |
| Linux aarch64 | /dev/net/tun | SOCK_RAW | SCM_RIGHTS | Low |
| Windows x86_64 | WinTun | TBD | Named pipe | Medium |
| macOS x86_64 | utun | SOCK_RAW | SCM_RIGHTS | Medium |
| macOS aarch64 | utun | SOCK_RAW | SCM_RIGHTS | Medium |

### 9.5 Interop Matrix

Every platform combination must work:

| | Linux | Windows | macOS | MikroTik |
|--|-------|---------|-------|----------|
| **Linux** | Phase 8 ✓ | Phase 9 | Phase 9 | Phase 5-6 ✓ |
| **Windows** | Phase 9 | Phase 9 | Phase 9 | Phase 9 |
| **macOS** | Phase 9 | Phase 9 | Phase 9 | Phase 9 |

## Success Criteria

- [ ] Windows: Single tunnel working (ping, iperf3)
- [ ] macOS: Single tunnel working (ping, iperf3)
- [ ] Cross-platform: Linux ↔ Windows tunnel works
- [ ] Cross-platform: Linux ↔ macOS tunnel works
- [ ] All platforms interop with MikroTik
- [ ] CI builds for all three platforms

## Artifacts

- `src/net/tap_windows.rs`, `src/net/tap_macos.rs`
- Platform-specific build instructions in README
- CI workflows for Windows and macOS
