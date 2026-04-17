# EoIP-rs System Architecture

## 1. Component Diagram

```
+------------------------------------------------------------------+
|                        SYSTEM BOUNDARY                           |
|                                                                  |
|  +--------------------+        +-----------------------------+   |
|  |   eoip-helper      |        |       eoip-rs (daemon)      |   |
|  |   (root / setuid)  |        |       (unprivileged)        |   |
|  |                     |  SCM   |                             |   |
|  |  +---------------+  | RIGHTS |  +----------------------+  |   |
|  |  | TAP Creator   |--+-------+->| FD Receiver          |  |   |
|  |  +---------------+  |  Unix  |  +----------+-----------+  |   |
|  |  | Raw Socket    |--+ Socket |             |              |   |
|  |  | Creator       |  |  Pair  |  +----------v-----------+  |   |
|  |  +---------------+  |        |  | Tunnel Manager       |  |   |
|  |  | Privilege Drop |  |        |  |   - TunnelRegistry   |  |   |
|  |  +---------------+  |        |  |   - Lifecycle FSM     |  |   |
|  +--------------------+        |  +----------+-----------+  |   |
|                                |             |              |   |
|                                |  +----------v-----------+  |   |
|  +--------------------+        |  | Packet Processor     |  |   |
|  | Config Loader      |------->|  |                      |  |   |
|  |  (TOML + CLI)      |        |  |  +------+ +-------+  |  |   |
|  +--------------------+        |  |  |TX Pth| |RX Path|  |  |   |
|                                |  |  |      | |       |  |  |   |
|  +--------------------+        |  |  |TAP   | |Socket |  |  |   |
|  | gRPC API Server    |<------>|  |  | Read | | Read  |  |  |   |
|  |  (tonic)           |        |  |  |  |   | |  |    |  |  |   |
|  |  - TunnelService   |        |  |  |  v   | |  v    |  |  |   |
|  |  - StatsService    |        |  |  |Encode| |Demux  |  |  |   |
|  |  - HealthService   |        |  |  |  |   | |  |    |  |  |   |
|  +--------------------+        |  |  |  v   | |  v    |  |  |   |
|                                |  |  |Socket| |Decode |  |  |   |
|  +--------------------+        |  |  |Write | |  |    |  |  |   |
|  | Stats Collector    |<-------|  |  |      | |  v    |  |  |   |
|  |  - per-tunnel ctrs |        |  |  |      | |TAP   |  |  |   |
|  |  - global metrics  |        |  |  |      | |Write |  |  |   |
|  +--------------------+        |  |  +------+ +-------+  |  |   |
|                                |  +-----------+----------+  |   |
|                                |              |             |   |
|  +--------------------+        |  +-----------v----------+  |   |
|  | Keepalive FSM      |<-------|  | Demux Table          |  |   |
|  |  - per-tunnel timer |        |  |  DashMap<(u16,       |  |   |
|  |  - state tracking  |        |  |    IpAddr),           |  |   |
|  +--------------------+        |  |    TunnelHandle>      |  |   |
|                                |  +-----------------------+  |   |
|                                +-----------------------------+   |
+------------------------------------------------------------------+
```

---

## 2. Crate Structure

```
eoip-rs/
├── Cargo.toml                    # Workspace root
├── crates/
│   ├── eoip-proto/               # Pure protocol library (no async, no I/O)
│   │   ├── Cargo.toml            # deps: bytes, thiserror
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── gre.rs            # MikroTik GRE encode/decode
│   │       ├── etherip.rs        # EtherIP encode/decode (IPv6)
│   │       ├── udp_shim.rs       # UDP encapsulation shim
│   │       ├── types.rs          # DemuxKey, TunnelId, etc.
│   │       ├── error.rs          # EoipError enum
│   │       └── wire.rs           # Helper<->Daemon wire protocol
│   │
│   ├── eoip-api/                 # gRPC service definitions
│   │   ├── Cargo.toml            # deps: tonic, tonic-build, prost
│   │   ├── build.rs              # tonic_build::compile_protos
│   │   ├── proto/
│   │   │   └── eoip.proto        # Service + message definitions
│   │   └── src/
│   │       └── lib.rs            # Re-exports generated code
│   │
│   ├── eoip-helper/              # Privileged helper binary
│   │   ├── Cargo.toml            # deps: nix, socket2, eoip-proto
│   │   └── src/
│   │       ├── main.rs           # Entry point
│   │       ├── tap.rs            # TAP creation via ioctl
│   │       ├── rawsock.rs        # Raw socket creation
│   │       ├── fdpass.rs         # SCM_RIGHTS send logic
│   │       └── privdrop.rs       # setuid/setgid after setup
│   │
│   └── eoip-rs/                  # Main daemon binary
│       ├── Cargo.toml            # deps: tokio, dashmap, tonic, etc.
│       └── src/
│           ├── main.rs           # Config load, helper connect, run
│           ├── config.rs         # TOML deserialization
│           ├── tunnel/
│           │   ├── mod.rs        # TunnelManager
│           │   ├── lifecycle.rs  # State machine
│           │   ├── handle.rs     # TunnelHandle (Arc'd state)
│           │   └── registry.rs   # DashMap-backed registry
│           ├── packet/
│           │   ├── mod.rs
│           │   ├── rx.rs         # RX: raw socket → demux → TAP
│           │   ├── tx.rs         # TX: TAP → encode → raw socket
│           │   └── batch.rs      # Adaptive batching
│           ├── net/
│           │   ├── mod.rs
│           │   ├── fdrecv.rs     # SCM_RIGHTS receive
│           │   ├── rawsock.rs    # AsyncFd wrappers for raw sockets
│           │   └── tap.rs        # AsyncFd wrappers for TAP fds
│           ├── api/
│           │   ├── mod.rs        # gRPC server setup
│           │   ├── tunnel_svc.rs
│           │   ├── stats_svc.rs
│           │   └── health_svc.rs
│           ├── keepalive.rs
│           ├── stats.rs
│           └── shutdown.rs       # Graceful shutdown
├── config/
│   └── eoip-rs.example.toml
├── systemd/
│   ├── eoip-helper.service
│   └── eoip-rs.service
└── proto/
    └── eoip.proto                # Canonical protobuf
```

### Workspace Cargo.toml

```toml
[workspace]
resolver = "2"
members = [
    "crates/eoip-proto",
    "crates/eoip-api",
    "crates/eoip-helper",
    "crates/eoip-rs",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
rust-version = "1.75"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
nix = { version = "0.29", features = ["net", "ioctl", "socket", "user"] }
socket2 = { version = "0.5", features = ["all"] }
dashmap = "6"
bytes = "1"
tonic = "0.12"
prost = "0.13"
serde = { version = "1", features = ["derive"] }
toml = "0.8"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
clap = { version = "4", features = ["derive"] }
thiserror = "2"
crossbeam = "0.8"
```

---

## 3. Data Flow

### 3.1 TX Path: TAP → Wire

```
 Ethernet Frame from kernel (bridge/apps)
          |
          v
 +------------------+
 | TAP AsyncFd Read |  tokio::io::AsyncFd wrapping raw fd
 +--------+---------+
          |
          v
 +------------------+
 | Header Prepend   |  eoip_proto::encode()
 |                  |
 |  IPv4: 8-byte    |  [0x20 0x01 0x64 0x00][len][tid]
 |    MikroTik GRE  |
 |  IPv6: 2-byte    |  [tid_hi|0x03][tid_lo]
 |    EtherIP       |
 +--------+---------+
          |
          v
 +------------------+
 | Adaptive Batcher |
 |                  |
 |  queue empty:    |──► Immediate sendto()
 |  queue filling:  |──► Accumulate, flush via sendmmsg()
 +--------+---------+
          |
          v
 +------------------+
 | Raw Socket Write |  ONE shared socket for all tunnels
 +------------------+
          |
          v
      Kernel routes packet to peer
```

### 3.2 RX Path: Wire → TAP

```
 IP packet arrives (proto 47 or 97)
          |
          v
 +------------------+
 | Raw Socket Read  |  recvmmsg() in batch mode
 | (shared socket)  |  Returns payload + source IP
 +--------+---------+
          |
          v
 +------------------+
 | Header Parse     |  eoip_proto::decode()
 |                  |  Extract tunnel_id + payload offset
 +--------+---------+
          |
          v
 +------------------+
 | Demux Lookup     |  DashMap<(tunnel_id, src_ip), TunnelHandle>
 |                  |
 |  Miss → drop     |  Increment unknown_tunnel counter
 |  Hit  → handle   |
 +--------+---------+
          |
          v
 +------------------+
 | TAP Write        |  write() Ethernet frame to tunnel's TAP fd
 | (IFF_NO_PI)      |
 +------------------+
          |
          v
 Kernel delivers frame to bridge/applications
```

---

## 4. Privilege Separation

### 4.1 Startup Sequence

```
 1. systemd starts eoip-helper as root
          |
 2. Helper creates Unix socketpair: (helper_end, daemon_end)
          |
 3. Helper fork+exec eoip-rs daemon, passing daemon_end fd
    (or: daemon connects to well-known socket path)
          |
 4. For each tunnel in config:
    a. open("/dev/net/tun") → tap_fd
    b. ioctl(TUNSETIFF, "eoip%d" | IFF_TAP | IFF_NO_PI)
    c. Send HelperMsg::TapCreated { name, tunnel_id }
       with SCM_RIGHTS carrying tap_fd
          |
 5. Create raw sockets (once per AF):
    a. socket(AF_INET, SOCK_RAW, 47)  → raw4_fd
    b. socket(AF_INET6, SOCK_RAW, 97) → raw6_fd
    c. Send HelperMsg::RawSocket with SCM_RIGHTS
          |
 6. Helper either:
    a. MODE_PERSIST: stays alive for dynamic tunnel creation
    b. MODE_EXIT: exits (all tunnels must be in config)
          |
 7. Daemon receives FDs, wraps in AsyncFd, starts processing
```

### 4.2 Helper ↔ Daemon Wire Protocol

Length-prefixed (4 bytes LE) + serde-serialized payload. FDs in `cmsg` ancillary data.

```rust
/// Helper → Daemon
enum HelperMsg {
    TapCreated { iface_name: String, tunnel_id: u16 },
    RawSocket { address_family: u16 },
    Error { msg: String },
    HelperReady,
}

/// Daemon → Helper
enum DaemonMsg {
    CreateTunnel { iface_name: String, tunnel_id: u16 },
    DestroyTunnel { iface_name: String },
    Shutdown,
}
```

---

## 5. Tunnel Lifecycle

```
                    CreateTunnel (gRPC or config)
                           |
                           v
                    +------+------+
                    | INITIALIZING |  Request TAP from helper
                    +------+------+  Allocate TunnelState
                           |         Insert into DemuxTable
                           v
                    +------+------+
                    |  CONFIGURED  |  TAP fd ready, peer known
                    +------+------+  Not yet forwarding
                           |
                           v
                    +------+------+
                    |   ACTIVE     |  TAP read task spawned
                    +------+------+  Keepalive timer running
                          /|\
                         / | \
           Keepalive OK /  |  \ Keepalive timeout
                       v   |   v
                 (stay     |  +------+------+
                  ACTIVE)  |  |   STALE      |  Stop TX, keep RX
                           |  +------+------+  (detect recovery)
                           |         |
                           |  Keepalive resumes → back to ACTIVE
                           |
                    Admin teardown
                           |
                           v
                    +------+------+
                    | TEARING DOWN |  Cancel tasks, remove from
                    +------+------+  DemuxTable, close TAP fd
                           |
                           v
                       DESTROYED
```

### Key Types

```rust
#[derive(Clone, Copy, Hash, Eq, PartialEq)]
pub struct DemuxKey {
    pub tunnel_id: u16,
    pub peer_addr: IpAddr,
}

#[derive(Clone)]
pub struct TunnelHandle {
    pub tap_fd: Arc<AsyncFd<OwnedFd>>,
    pub stats: Arc<TunnelStats>,
    pub state: Arc<AtomicU8>,
    pub keepalive_tx: mpsc::Sender<KeepaliveEvent>,
}

pub struct TunnelStats {
    pub tx_packets: AtomicU64,
    pub tx_bytes: AtomicU64,
    pub rx_packets: AtomicU64,
    pub rx_bytes: AtomicU64,
    pub tx_errors: AtomicU64,
    pub rx_errors: AtomicU64,
    pub last_rx: AtomicI64,  // unix timestamp millis
    pub last_tx: AtomicI64,
}
```

---

## 6. Error Handling

### Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum EoipError {
    #[error("invalid GRE header: {0}")]
    InvalidGreHeader(String),
    #[error("invalid EtherIP header: {0}")]
    InvalidEtherIpHeader(String),
    #[error("unknown tunnel: id={tunnel_id} peer={peer_addr}")]
    UnknownTunnel { tunnel_id: u16, peer_addr: IpAddr },
    #[error("packet too short: got {got}, need {need}")]
    PacketTooShort { got: usize, need: usize },
    #[error("TAP error on {iface}: {source}")]
    TapError { iface: String, source: io::Error },
    #[error("raw socket error: {0}")]
    RawSocket(#[from] io::Error),
    #[error("helper disconnected")]
    HelperDisconnected,
    #[error("config error: {0}")]
    Config(String),
}
```

### Recovery Matrix

| Error | Severity | Action |
|-------|----------|--------|
| TAP read `EAGAIN` | Normal | Yield to async runtime |
| TAP read `EIO` | Critical | TAP gone. Tear down tunnel. |
| TAP write `ENOBUFS` | Transient | Drop packet, increment `tx_errors` |
| Invalid header | Warning | Drop, increment `rx_malformed` |
| Demux miss | Info (rate-limited) | Drop, increment `unknown_tunnel` |
| Helper disconnect | Critical | Disable dynamic creation. Existing tunnels continue. |
| Keepalive timeout | Operational | Transition to Stale |
| Config parse fail | Fatal | Print error, exit 1 |

**Panic policy:** No panics in library code. `expect()` only for truly impossible invariants.

---

## 7. Configuration

```toml
# /etc/eoip-rs/config.toml

[daemon]
user = "eoip"
group = "eoip"
helper_mode = "persist"           # "persist" or "exit"
helper_socket = "/run/eoip-rs/helper.sock"
pid_file = "/run/eoip-rs/eoip-rs.pid"

[api]
listen = "[::1]:50051"
# tls_cert = "/etc/eoip-rs/server.pem"
# tls_key  = "/etc/eoip-rs/server-key.pem"
# tls_ca   = "/etc/eoip-rs/ca.pem"

[logging]
level = "info"
format = "pretty"                 # "json" or "pretty"

[performance]
low_water_mark = 8
high_water_mark = 256
max_batch_size = 64
batch_timeout_us = 50
channel_buffer = 1024
rx_workers = 1

[[tunnel]]
tunnel_id = 100
local = "192.168.1.1"
remote = "192.168.1.2"
iface_name = "eoip-dc1"          # optional, auto if omitted
mtu = 1500
enabled = true

[tunnel.keepalive]
interval = "10s"
timeout = "30s"

[[tunnel]]
tunnel_id = 200
local = "fd00::1"
remote = "fd00::2"
iface_name = "eoip-dc2"
mtu = 1500
enabled = true
```

---

## 8. gRPC API

```protobuf
syntax = "proto3";
package eoip.v1;

service TunnelService {
  rpc CreateTunnel(CreateTunnelRequest) returns (CreateTunnelResponse);
  rpc DeleteTunnel(DeleteTunnelRequest) returns (DeleteTunnelResponse);
  rpc ListTunnels(ListTunnelsRequest) returns (ListTunnelsResponse);
  rpc GetTunnel(GetTunnelRequest) returns (Tunnel);
  rpc WatchTunnels(WatchTunnelsRequest) returns (stream TunnelEvent);
}

service StatsService {
  rpc GetStats(GetStatsRequest) returns (TunnelStats);
  rpc GetGlobalStats(GetGlobalStatsRequest) returns (GlobalStats);
}

service HealthService {
  rpc Check(HealthCheckRequest) returns (HealthCheckResponse);
}

message Tunnel {
  uint32 tunnel_id = 1;
  string local_addr = 2;
  string remote_addr = 3;
  string iface_name = 4;
  uint32 mtu = 5;
  TunnelState state = 6;
  TunnelStats stats = 7;
}

enum TunnelState {
  TUNNEL_STATE_UNSPECIFIED = 0;
  TUNNEL_STATE_INITIALIZING = 1;
  TUNNEL_STATE_CONFIGURED = 2;
  TUNNEL_STATE_ACTIVE = 3;
  TUNNEL_STATE_STALE = 4;
  TUNNEL_STATE_TEARING_DOWN = 5;
}

message TunnelStats {
  uint64 tx_packets = 1;
  uint64 tx_bytes = 2;
  uint64 rx_packets = 3;
  uint64 rx_bytes = 4;
  uint64 tx_errors = 5;
  uint64 rx_errors = 6;
  int64 last_rx_ms = 7;
  int64 last_tx_ms = 8;
}

message TunnelEvent {
  enum EventType {
    EVENT_TYPE_UNSPECIFIED = 0;
    CREATED = 1;
    STATE_CHANGED = 2;
    DELETED = 3;
  }
  EventType event_type = 1;
  Tunnel tunnel = 2;
}
```

---

## 9. Task Layout (Async Runtime)

```
tokio runtime (multi-threaded)
│
├── [task] gRPC server (tonic)
│     Holds Arc<TunnelManager>
│
├── [task] Helper FD Receiver
│     Reads from Unix socket, dispatches FDs
│
├── [OS thread] RX Worker (proto 47 / IPv4 GRE)
│     recvmmsg() → parse → demux → channel → TAP write
│
├── [OS thread] RX Worker (proto 97 / IPv6 EtherIP)
│     Same pattern for IPv6
│
├── [task per tunnel] TAP Reader
│     read() → channel → TX Batcher
│
├── [task] TX Batcher (proto 47)
│     Adaptive batching → sendmmsg()
│
├── [task] TX Batcher (proto 97)
│     Same for IPv6
│
├── [task] Keepalive Supervisor
│     Per-tunnel timers, state transitions
│
├── [task] Stats Aggregator
│     Periodic snapshot for logging/export
│
└── [task] Signal Handler
      SIGTERM/SIGINT → CancellationToken → graceful shutdown
```
