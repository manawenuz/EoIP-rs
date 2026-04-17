# Installation Guide

## Linux

### From Binary Release

```bash
# Download latest release
curl -sLO https://github.com/manawenuz/EoIP-rs/releases/latest/download/eoip-rs-0.1.0-linux-x86_64.tar.gz
tar xzf eoip-rs-0.1.0-linux-x86_64.tar.gz
cd eoip-rs-0.1.0-linux-x86_64

# Install binaries
sudo cp eoip-rs eoip-helper eoip-cli eoip-analyzer /usr/local/bin/

# Create config directory
sudo mkdir -p /etc/eoip-rs
sudo cp eoip-rs.example.toml /etc/eoip-rs/config.toml

# Create runtime directory
sudo mkdir -p /run/eoip-rs
```

Or use the install script:

```bash
sudo ./scripts/install-linux.sh
```

### From Source

**Prerequisites:**
- Rust 1.75+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- protoc (`apt install protobuf-compiler` or download from [GitHub](https://github.com/protocolbuffers/protobuf/releases))
- C compiler (`apt install build-essential`)

```bash
git clone https://github.com/manawenuz/EoIP-rs.git
cd EoIP-rs
cargo build --release

# Binaries are in target/release/
ls target/release/eoip-{rs,helper,cli,analyzer}
```

### Systemd Setup

```bash
# Copy service files
sudo cp systemd/eoip-helper.service /etc/systemd/system/
sudo cp systemd/eoip-rs.service /etc/systemd/system/

# Create service user
sudo useradd -r -s /usr/sbin/nologin eoip

# Enable and start
sudo systemctl daemon-reload
sudo systemctl enable --now eoip-helper
sudo systemctl enable --now eoip-rs

# Check status
sudo systemctl status eoip-rs
```

### Manual Start

```bash
# Terminal 1: Start helper (needs root)
sudo eoip-helper --mode persist

# Terminal 2: Start daemon
sudo eoip-rs --config /etc/eoip-rs/config.toml

# Terminal 3: Configure TAP interface
sudo ip link set eoip100 up
sudo ip addr add 10.255.0.2/30 dev eoip100

# Verify
eoip-cli print
ping 10.255.0.1
```

### Quick Setup Script

For a fast setup with a single tunnel:

```bash
sudo scripts/setup-tunnel.sh \
  --tunnel-id 100 \
  --local 192.168.1.10 \
  --remote 192.168.1.1 \
  --ip 10.255.0.2/30
```

---

## Windows

### Prerequisites

1. **tap-windows6 driver** — install via OpenVPN:
   ```powershell
   # Download and install OpenVPN (includes TAP driver)
   Invoke-WebRequest -Uri "https://swupdate.openvpn.org/community/releases/OpenVPN-2.6.12-I001-amd64.msi" -OutFile "$env:TEMP\openvpn.msi"
   msiexec /i "$env:TEMP\openvpn.msi" /quiet /norestart ADDLOCAL=Drivers.TAPWindows6,Drivers
   ```

2. **Administrator privileges** — required for TAP and WinDivert

### From Binary Release

```powershell
# Download and extract
Invoke-WebRequest -Uri "https://github.com/manawenuz/EoIP-rs/releases/latest/download/eoip-rs-0.1.0-windows-x86_64.zip" -OutFile "$env:TEMP\eoip-rs.zip"
Expand-Archive -Path "$env:TEMP\eoip-rs.zip" -DestinationPath "C:\eoip-rs"

# Create config
Set-Content -Path "C:\eoip-rs\config.toml" -Value @"
[daemon]
user = "root"
group = "root"

[logging]
level = "info"

[[tunnel]]
tunnel_id = 100
local = "YOUR_WINDOWS_IP"
remote = "REMOTE_PEER_IP"
mtu = 1458
keepalive_interval_secs = 10
keepalive_timeout_secs = 100
"@

# Start daemon (from the directory with WinDivert files)
cd C:\eoip-rs\eoip-rs-0.1.0-windows-x86_64
.\eoip-rs-win.exe --config C:\eoip-rs\config.toml
```

Or use the setup script:

```powershell
.\scripts\setup-tunnel.ps1 -TunnelId 100 -Local "10.0.0.2" -Remote "10.0.0.1" -TunnelIP "10.255.0.2" -PrefixLength 30
```

### Configure TAP Adapter

After the daemon starts, assign an IP to the TAP adapter:

```powershell
New-NetIPAddress -InterfaceAlias "OpenVPN TAP-Windows6" -IPAddress 10.255.0.2 -PrefixLength 30
```

### Cross-Compile from Linux

```bash
# Install cross-compile toolchain
rustup target add x86_64-pc-windows-gnu
apt install gcc-mingw-w64-x86-64 cmake

# Configure linker
mkdir -p .cargo
echo '[target.x86_64-pc-windows-gnu]
linker = "x86_64-w64-mingw32-gcc"' > .cargo/config.toml

# Build
cargo build --release --target x86_64-pc-windows-gnu \
  --bin eoip-rs-win --bin eoip-cli --bin eoip-analyzer \
  --features windows
```

---

## Configuration Reference

See [config/eoip-rs.example.toml](../../config/eoip-rs.example.toml) for a fully commented example.

### Minimal Config

```toml
[[tunnel]]
tunnel_id = 100
local = "192.168.1.10"
remote = "192.168.1.1"
```

All other fields have sensible defaults (MTU 1458, keepalive 10s/100s, helper socket `/run/eoip-rs/helper.sock`).

### Multiple Tunnels

```toml
[[tunnel]]
tunnel_id = 100
local = "10.0.0.1"
remote = "10.0.0.2"
iface_name = "eoip-dc1"

[[tunnel]]
tunnel_id = 200
local = "10.0.0.1"
remote = "10.0.0.3"
iface_name = "eoip-dc2"
```

### Key Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `tunnel_id` | (required) | 0-65535 for IPv4, 0-4095 for IPv6 |
| `local` | (required) | Local bind IP address |
| `remote` | (required) | Remote peer IP address |
| `iface_name` | `eoip{id}` | TAP interface name (max 15 chars) |
| `mtu` | 1458 | Tunnel interface MTU |
| `keepalive_interval_secs` | 10 | Keepalive send interval |
| `keepalive_timeout_secs` | 100 | Time before declaring tunnel stale |
| `api.listen` | `[::1]:50051` | gRPC API bind address |
