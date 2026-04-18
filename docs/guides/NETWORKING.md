# Networking Guide

EoIP tunnels create virtual Ethernet (TAP) interfaces that behave like physical NICs. You can assign IPs, bridge them to other interfaces, run DHCP, or use them in any standard networking configuration.

## IP Assignment

### Static IP

**Linux:**

```bash
sudo ip addr add 10.200.0.2/24 dev eoip100
sudo ip link set eoip100 up
```

**Windows (PowerShell):**

```powershell
New-NetIPAddress -InterfaceAlias "EoIP Tunnel" -IPAddress 10.200.0.2 -PrefixLength 24
```

Or via **Network Connections** GUI: right-click adapter → Properties → IPv4 → set IP.

### DHCP Client

If the remote side (e.g., MikroTik) has a DHCP server on the bridged network, the EoIP interface can get an IP via DHCP — as if it were physically plugged into that remote LAN.

**Linux:**

```bash
# Remove any static IP first
sudo ip addr flush dev eoip100

# Request DHCP lease
sudo dhclient eoip100
# or with systemd-networkd:
# sudo networkctl reconfigure eoip100
```

**Windows:**

```powershell
# Set adapter to DHCP
Set-NetIPInterface -InterfaceAlias "EoIP Tunnel" -Dhcp Enabled
ipconfig /renew "EoIP Tunnel"
```

Or via GUI: right-click adapter → Properties → IPv4 → "Obtain an IP address automatically".

**MikroTik side** — the EoIP interface must be bridged with a network that has a DHCP server:

```routeros
/interface bridge add name=br-lan
/interface bridge port add bridge=br-lan interface=eoip-linux
/interface bridge port add bridge=br-lan interface=ether2
/ip dhcp-server
# (DHCP server must be configured on br-lan or ether2's subnet)
```

## Bridging

Bridging connects the EoIP tunnel with a local physical interface at Layer 2. All devices on both sides see each other as if on the same LAN segment — MAC addresses, ARP, broadcast, DHCP all pass through transparently.

### Linux

```bash
# Create bridge
sudo ip link add br0 type bridge
sudo ip link set br0 up

# Add tunnel and physical port
sudo ip link set eoip100 master br0
sudo ip link set eth1 master br0

# Remove IPs from member interfaces (bridge owns the IP)
sudo ip addr flush dev eoip100
sudo ip addr flush dev eth1

# Option A: Static IP on bridge
sudo ip addr add 192.168.1.100/24 dev br0

# Option B: DHCP on bridge
sudo dhclient br0
```

To make persistent, add to `/etc/network/interfaces` or create a netplan config:

```yaml
# /etc/netplan/99-eoip-bridge.yaml
network:
  version: 2
  bridges:
    br0:
      interfaces:
        - eoip100
        - eth1
      dhcp4: true
```

To remove the bridge:

```bash
sudo ip link set eoip100 nomaster
sudo ip link set eth1 nomaster
sudo ip link del br0
```

### Windows

**GUI method (simplest):**

1. Open **Network Connections** (`ncpa.cpl`)
2. Select both adapters: hold Ctrl, click the EoIP TAP adapter and the physical NIC
3. Right-click → **Bridge Connections**
4. Windows creates a "Network Bridge" adapter
5. Configure IP/DHCP on the bridge adapter

**PowerShell:**

```powershell
# Create bridge (requires Hyper-V feature or use netsh)
New-NetSwitchTeam -Name "EoIPBridge" -TeamMembers "EoIP Tunnel", "Ethernet 2"
```

To remove:

```powershell
Remove-NetSwitchTeam -Name "EoIPBridge"
```

### MikroTik Side

The MikroTik must also bridge its EoIP interface for L2 forwarding to work:

```routeros
# Create bridge
/interface bridge add name=br-eoip

# Add EoIP tunnel and LAN port
/interface bridge port add bridge=br-eoip interface=eoip-linux
/interface bridge port add bridge=br-eoip interface=ether2

# Assign IP to bridge (not individual ports)
/ip address add address=192.168.1.1/24 interface=br-eoip
```

## Routing

For routed (non-bridged) setups, add routes for the remote network:

**Linux:**

```bash
# Route 192.168.88.0/24 through the tunnel peer
sudo ip route add 192.168.88.0/24 via 10.200.0.1 dev eoip100
```

**Windows:**

```powershell
route add 192.168.88.0 mask 255.255.255.0 10.200.0.1
```

**MikroTik:**

```routeros
/ip route add dst-address=10.200.0.0/24 gateway=eoip-linux
```

## Common Topologies

### Point-to-Point (Routed)

Simplest setup. Each side gets an IP on a /30 subnet, routes added for remote networks.

```
[Linux]                          [MikroTik]
eoip100: 10.255.0.2/30  ──────  eoip-linux: 10.255.0.1/30
  └─ route 192.168.88.0/24       └─ route 10.200.0.0/24
       via 10.255.0.1                  via 10.255.0.2
```

### L2 Bridge (Transparent)

Both sides bridge the EoIP interface with a physical port. All devices share one broadcast domain.

```
[eth1] ─── [br0] ─── [eoip100]  ══════  [eoip-linux] ─── [br-eoip] ─── [ether2]
              │                                               │
        192.168.1.0/24 (single broadcast domain)        192.168.1.0/24
```

Devices on `eth1` and `ether2` can ping each other directly, get DHCP from the same server, etc.

### Hub-and-Spoke (Multiple Tunnels)

Central site connects to multiple remote sites, each with its own tunnel ID:

```
                    [MikroTik Hub]
                   /      |       \
          eoip100 /  eoip200  eoip300\
                /        |           \
     [Site A]      [Site B]      [Site C]
     Linux         MikroTik      Windows
```

```toml
# Site A config
[[tunnel]]
tunnel_id = 100
remote = "<HUB_IP>"

# Hub MikroTik
/interface eoip add name=site-a remote-address=<SITE_A_IP> tunnel-id=100
/interface eoip add name=site-b remote-address=<SITE_B_IP> tunnel-id=200
/interface eoip add name=site-c remote-address=<SITE_C_IP> tunnel-id=300
```

## MTU Considerations for Bridging

When bridging, all member interfaces should have matching MTUs. EoIP-rs sets the TAP MTU to the auto-detected overlay MTU (e.g., 1458). If you bridge with a physical interface that has MTU 1500, the bridge will use the lowest member MTU (1458).

This means devices on the physical LAN will be limited to 1458-byte frames when communicating through the bridge. This is usually fine — TCP MSS clamping (enabled by default) ensures TCP sessions negotiate the correct segment size.

For non-TCP traffic (UDP, custom protocols), ensure applications can handle the reduced MTU or enable path MTU discovery on the endpoints.
