#!/usr/bin/env bash
#
# setup-tunnel.sh — Quick single-tunnel setup for EoIP-rs
#
# Usage:
#   sudo ./setup-tunnel.sh --tunnel-id 100 --local 192.168.1.10 --remote 192.168.1.1 --ip 10.255.0.2/30
#
set -euo pipefail

TUNNEL_ID=""
LOCAL=""
REMOTE=""
TUNNEL_IP=""
CONFIG="/etc/eoip-rs/config.toml"
HELPER_SOCKET="/run/eoip-rs/helper.sock"
API_LISTEN="[::1]:50051"

usage() {
    echo "Usage: $0 --tunnel-id <id> --local <ip> --remote <ip> --ip <tunnel_ip/mask>"
    echo ""
    echo "Options:"
    echo "  --tunnel-id ID       Tunnel ID (must match MikroTik side)"
    echo "  --local IP           Local bind IP"
    echo "  --remote IP          Remote peer IP (MikroTik or EoIP-rs)"
    echo "  --ip CIDR            IP address for tunnel interface (e.g., 10.255.0.2/30)"
    echo "  --config PATH        Config file path (default: $CONFIG)"
    echo "  --api-listen ADDR    gRPC API listen address (default: $API_LISTEN)"
    exit 1
}

while [[ $# -gt 0 ]]; do
    case $1 in
        --tunnel-id) TUNNEL_ID="$2"; shift 2 ;;
        --local)     LOCAL="$2"; shift 2 ;;
        --remote)    REMOTE="$2"; shift 2 ;;
        --ip)        TUNNEL_IP="$2"; shift 2 ;;
        --config)    CONFIG="$2"; shift 2 ;;
        --api-listen) API_LISTEN="$2"; shift 2 ;;
        -h|--help)   usage ;;
        *)           echo "Unknown: $1"; usage ;;
    esac
done

[[ -z "$TUNNEL_ID" ]] && echo "Error: --tunnel-id required" && usage
[[ -z "$LOCAL" ]]     && echo "Error: --local required" && usage
[[ -z "$REMOTE" ]]    && echo "Error: --remote required" && usage
[[ -z "$TUNNEL_IP" ]] && echo "Error: --ip required" && usage

IFACE_NAME="eoip${TUNNEL_ID}"

echo "[+] Creating config: $CONFIG"
mkdir -p "$(dirname "$CONFIG")"
cat > "$CONFIG" <<EOF
[daemon]
user = "root"
group = "root"
helper_mode = "persist"
helper_socket = "$HELPER_SOCKET"

[api]
listen = "$API_LISTEN"

[logging]
level = "info"

[[tunnel]]
tunnel_id = $TUNNEL_ID
local = "$LOCAL"
remote = "$REMOTE"
iface_name = "$IFACE_NAME"
mtu = 1458
keepalive_interval_secs = 10
keepalive_timeout_secs = 100
EOF

echo "[+] Stopping existing services..."
pkill -f "eoip-rs\|eoip-helper" 2>/dev/null || true
sleep 1
ip link del "$IFACE_NAME" 2>/dev/null || true

echo "[+] Creating runtime directory"
mkdir -p /run/eoip-rs
rm -f "$HELPER_SOCKET"

echo "[+] Starting eoip-helper..."
eoip-helper --mode persist --socket-path "$HELPER_SOCKET" &
HELPER_PID=$!
sleep 1

echo "[+] Starting eoip-rs daemon..."
eoip-rs --config "$CONFIG" &
DAEMON_PID=$!
sleep 3

echo "[+] Configuring interface $IFACE_NAME ($TUNNEL_IP)..."
ip link set "$IFACE_NAME" up
ip addr add "$TUNNEL_IP" dev "$IFACE_NAME" 2>/dev/null || true

echo ""
echo "[+] Tunnel setup complete!"
echo ""
echo "  Interface: $IFACE_NAME"
echo "  Tunnel IP: $TUNNEL_IP"
echo "  Remote:    $REMOTE (tunnel-id=$TUNNEL_ID)"
echo "  Helper:    PID $HELPER_PID"
echo "  Daemon:    PID $DAEMON_PID"
echo ""
echo "  Verify:    eoip-cli print"
echo "  Ping:      ping ${TUNNEL_IP%/*}"
echo ""
echo "  MikroTik config:"
echo "    /interface eoip add name=eoip-linux remote-address=$LOCAL tunnel-id=$TUNNEL_ID"
echo "    /ip address add address=<MIKROTIK_TUNNEL_IP>/30 interface=eoip-linux"
echo ""
