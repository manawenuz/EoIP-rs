# EoIP-rs Platform Abstraction

## 1. Core Traits

```rust
/// Virtual network interface (TAP/TUN) for L2 or L3 packet I/O.
pub trait VirtualInterface: Send + Sync + 'static {
    fn read(&self, buf: &mut [u8]) -> io::Result<usize>;
    fn write(&self, buf: &[u8]) -> io::Result<usize>;
    fn mtu(&self) -> io::Result<u32>;
    fn set_mtu(&self, mtu: u32) -> io::Result<()>;
    fn name(&self) -> &str;
    fn as_raw_fd(&self) -> Option<RawFd>;
    fn layer(&self) -> InterfaceLayer;
    fn set_up(&self) -> io::Result<()>;
    fn set_down(&self) -> io::Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceLayer {
    L2,  // TAP: full Ethernet frames
    L3,  // TUN: IP packets only, needs userspace L2 bridge
}

/// Factory for creating virtual interfaces.
pub trait InterfaceFactory: Send + Sync {
    fn create(&self, name_prefix: &str, layer: InterfaceLayer)
        -> io::Result<Box<dyn VirtualInterface>>;
    fn open(&self, name: &str)
        -> io::Result<Box<dyn VirtualInterface>>;
}

/// Raw IP socket abstraction with batch I/O.
pub trait RawSocket: Send + Sync + 'static {
    fn recv_batch(
        &self,
        bufs: &mut [PacketBuf],
        src_addrs: &mut [SocketAddr],
    ) -> io::Result<usize>;

    fn send_batch(
        &self,
        bufs: &[PacketBuf],
        dst_addrs: &[SocketAddr],
    ) -> io::Result<usize>;

    fn as_raw_fd(&self) -> Option<RawFd>;
    fn protocol(&self) -> u8;
}
```

### Platform Detection

```rust
pub fn create_interface_factory() -> Box<dyn InterfaceFactory> {
    #[cfg(target_os = "linux")]
    { Box::new(linux::LinuxTapFactory::new()) }

    #[cfg(target_os = "macos")]
    { Box::new(macos::MacOsInterfaceFactory::new()) }

    #[cfg(target_os = "windows")]
    { Box::new(windows::WindowsInterfaceFactory::new()) }

    #[cfg(target_os = "android")]
    { Box::new(android::AndroidInterfaceFactory::new()) }
}
```

---

## 2. Linux (Primary Target)

### TAP Interface

- Device: `/dev/net/tun`
- Flags: `IFF_TAP | IFF_NO_PI | IFF_MULTI_QUEUE`
- Layer: **L2** (full Ethernet frames, no packet info header)
- Non-blocking: `O_NONBLOCK` via `fcntl`
- Async: wrapped in `tokio::io::AsyncFd`

```rust
pub struct LinuxTap {
    fd: OwnedFd,
    name: String,
}

impl LinuxTap {
    pub fn create(name_prefix: &str) -> io::Result<Self> {
        let fd = open("/dev/net/tun", O_RDWR)?;

        let mut ifr: ifreq = zeroed();
        copy_name(&mut ifr, name_prefix);
        ifr.ifr_flags = (IFF_TAP | IFF_NO_PI | IFF_MULTI_QUEUE) as i16;

        ioctl(fd, TUNSETIFF, &mut ifr)?;
        set_nonblocking(fd)?;

        Ok(LinuxTap { fd, name: read_ifname(&ifr) })
    }
}
```

### Raw Socket

- `socket(AF_INET, SOCK_RAW, IPPROTO_GRE)` for IPv4 EoIP
- `socket(AF_INET6, SOCK_RAW, 97)` for IPv6 EoIPv6
- Batch I/O: native `recvmmsg(2)` / `sendmmsg(2)`
- One socket per protocol for all tunnels

### Privilege Model

| Method | Description |
|--------|-------------|
| Root helper (recommended) | Separate binary creates TAP + sockets, passes FDs via SCM_RIGHTS |
| File capabilities | `setcap cap_net_admin,cap_net_raw+ep /usr/bin/eoip-rs` |
| Root | Run entire daemon as root (not recommended) |

---

## 3. macOS (Future)

### Challenge

macOS has **no native TAP** since Big Sur removed the tuntap kernel extension. `utun` is L3 only.

### Approach: utun + Userspace L2 Bridge

```
 ┌──────────────────────────────────────────────────────────┐
 │                    macOS EoIP-rs                          │
 │                                                          │
 │  ┌────────────┐     ┌───────────────────┐                │
 │  │  utun (L3) │◄───►│ Userspace L2/L3   │◄──► BPF socket│
 │  │  interface │     │ Bridge            │                │
 │  │            │     │ - ARP proxy       │                │
 │  │ IP packets │     │ - MAC learning    │                │
 │  │ only       │     │ - Eth hdr add/    │                │
 │  │            │     │   strip           │                │
 │  └────────────┘     └───────────────────┘                │
 └──────────────────────────────────────────────────────────┘
```

- **utun**: created via `socket(PF_SYSTEM, SOCK_DGRAM, SYSPROTO_CONTROL)` + `connect()`. L3 only.
- **L2 bridge**: strips Ethernet header on RX (tunnel→utun), prepends synthetic header on TX (utun→tunnel). Handles ARP by responding with a virtual MAC.
- **Raw packets**: BPF (`/dev/bpf*`) with filter for GRE/EtherIP. No `sendmmsg`; falls back to per-packet `write()`.

### Alternative: vmnet.framework

Apple's `vmnet.framework` can create bridged L2 interfaces but requires entitlements and is designed for VMs. May be viable for signed distributions.

---

## 4. Windows (Future)

### Options

| Driver | Layer | Status |
|--------|-------|--------|
| Wintun | L3 (TUN) | Recommended — fast, maintained, ring-buffer API |
| TAP-Windows6 | L2 (TAP) | True L2 but aging codebase, driver signing issues |
| Windows vSwitch | L2 | Complex, underdocumented |

### Recommended: Wintun + L2 Bridge

Wintun provides a shared-memory ring-buffer interface — no per-packet syscalls.

```rust
pub struct WintunInterface {
    session: wintun::Session,
    name: String,
}

impl VirtualInterface for WintunInterface {
    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let packet = self.session.receive_blocking()?;
        let len = packet.bytes().len().min(buf.len());
        buf[..len].copy_from_slice(&packet.bytes()[..len]);
        Ok(len)
    }

    fn layer(&self) -> InterfaceLayer { InterfaceLayer::L3 }
    // L2 bridge needed (same as macOS utun approach)
}
```

**Raw socket**: Winsock `SOCK_RAW` with `IPPROTO_GRE`. Administrator required. No batch syscalls — use WSA overlapped I/O for async batching.

---

## 5. Android (Future)

### VpnService Architecture

Android provides no raw sockets or TAP from userspace. The only path is `VpnService`:

```
 ┌─────────────────────────────────────────────────────┐
 │                Android App                           │
 │                                                     │
 │  ┌──────────────┐    JNI     ┌──────────────────┐   │
 │  │ Kotlin/Java  │◄──────────►│ eoip-rs (native) │   │
 │  │ VpnService   │            │ .so library      │   │
 │  │              │            │                  │   │
 │  │ TUN fd ──────┼────────────┼──► L3 + bridge   │   │
 │  │              │            │                  │   │
 │  └──────────────┘            │ UDP socket ──────┼──►│
 │                              │ (protect(fd))    │   │
 │                              └──────────────────┘   │
 └─────────────────────────────────────────────────────┘
```

**Constraints:**
- VpnService provides TUN (L3) fd, not TAP (L2).
- Must use `VpnService.protect(fd)` on sockets to prevent routing loops.
- Cannot use `SOCK_RAW` — must use UDP encapsulation (EoIP-rs extension mode).
- Rust core runs as native `.so` via JNI; TUN fd passed from Java.
- `BIND_VPN_SERVICE` permission in AndroidManifest.

---

## 6. Platform Capability Matrix

```
 Platform   │ Interface  │ Batch RX          │ Batch TX          │ Privilege
 ───────────┼────────────┼───────────────────┼───────────────────┼──────────────
 Linux      │ TAP (L2)   │ recvmmsg (native) │ sendmmsg (native) │ CAP_NET_ADMIN
 macOS      │ utun (L3)  │ BPF multi-read    │ write() loop      │ root / XPC
 Windows    │ Wintun(L3) │ WSA overlapped    │ WSA overlapped    │ Administrator
 Android    │ TUN (L3)   │ read() loop       │ write() loop      │ VpnService
 FreeBSD    │ TAP (L2)   │ recvmmsg (native) │ sendmmsg (native) │ root
```

### L2 Bridge Needed On

- **macOS**: utun is L3 → bridge adds/strips Ethernet headers, handles ARP
- **Windows**: Wintun is L3 → same bridge approach
- **Android**: VpnService TUN is L3 → same, plus UDP-only encapsulation

The bridge is implemented as a `L2Bridge` adapter that wraps an `InterfaceLayer::L3` interface and presents it as L2 to the tunnel code. This is in `crates/eoip-rs/src/net/bridge.rs`.

---

## 7. Privilege Model Per Platform

### Linux (Primary)

```
 ┌───────────────────┐     SCM_RIGHTS    ┌───────────┐
 │  Root Helper       │ ──────────────►  │ Unprivd   │
 │  (eoip-helper)    │   (passes FDs)    │ Daemon    │
 │                   │                   │ (eoip-rs) │
 │ - Creates raw     │                   │           │
 │   sockets         │                   │ - nobody  │
 │ - Creates TAP     │                   │ - all pkt │
 │ - Sets iface up   │                   │   process │
 │ - Drops privs     │                   │ - gRPC    │
 └───────────────────┘                   └───────────┘
```

Required capabilities (if no helper): `CAP_NET_ADMIN` + `CAP_NET_RAW`.

### macOS

- Requires root for BPF access.
- Alternative: privileged helper via XPC (Mach IPC).
- utun creation requires root or entitlements.

### Windows

- Administrator for Winsock raw sockets.
- Wintun adapter creation requires Administrator (or pre-installed driver).
- Can run as a Windows Service under `LocalSystem`.

### Android

- `BIND_VPN_SERVICE` permission in manifest.
- User approves VPN via system dialog.
- No root required.

### Configuration

```toml
[privilege]
mode = "helper"                    # "root", "helper", "capabilities"
helper_path = "/usr/libexec/eoip-helper"
helper_socket = "/run/eoip/helper.sock"
run_as_user = "eoip"
run_as_group = "eoip"
```
