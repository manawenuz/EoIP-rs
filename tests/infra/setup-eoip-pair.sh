#!/usr/bin/env bash
#
# setup-eoip-pair.sh — Configure EoIP tunnels between two MikroTik CHR VMs
#
# Usage:
#   ./setup-eoip-pair.sh -a <mk-a-ip> -b <mk-b-ip> [OPTIONS]
#
# Options:
#   -a, --mk-a IP          First CHR VM IP (required)
#   -b, --mk-b IP          Second CHR VM IP (required)
#   -u, --user USER        RouterOS SSH user (default: admin)
#   --multi                Also create a second tunnel (tunnel-id=200)
#   --ipv6                 Also create EoIPv6 tunnel if IPv6 available
#   --keepalive-test       Test keepalive failover (blocks traffic briefly)
#   --export               Save RouterOS /export to tests/captures/mk-mk-baseline/
#   --teardown             Remove all EoIP tunnels from both VMs
#   -h, --help             Show this help
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CAPTURES_DIR="$SCRIPT_DIR/../captures/mk-mk-baseline"

# --- Defaults ---
MK_A=""
MK_B=""
ROS_USER="admin"
MULTI=false
IPV6=false
KEEPALIVE_TEST=false
EXPORT=false
TEARDOWN=false

# --- SSH options for RouterOS ---
SSH_OPTS=(-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o PubkeyAcceptedAlgorithms=+ssh-rsa -o HostKeyAlgorithms=+ssh-rsa -o ConnectTimeout=10)

# --- Colors ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

info()    { echo -e "${GREEN}[+]${NC} $*"; }
warn()    { echo -e "${YELLOW}[!]${NC} $*"; }
error()   { echo -e "${RED}[x]${NC} $*" >&2; }
die()     { error "$@"; exit 1; }
section() { echo -e "\n${CYAN}=== $* ===${NC}"; }

# --- Parse args ---
while [[ $# -gt 0 ]]; do
    case $1 in
        -a|--mk-a)          MK_A="$2"; shift 2 ;;
        -b|--mk-b)          MK_B="$2"; shift 2 ;;
        -u|--user)           ROS_USER="$2"; shift 2 ;;
        --multi)             MULTI=true; shift ;;
        --ipv6)              IPV6=true; shift ;;
        --keepalive-test)    KEEPALIVE_TEST=true; shift ;;
        --export)            EXPORT=true; shift ;;
        --teardown)          TEARDOWN=true; shift ;;
        -h|--help)           head -17 "$0" | tail -15; exit 0 ;;
        *)                   die "Unknown option: $1" ;;
    esac
done

[[ -z "$MK_A" ]] && die "mk-a IP required. Use -a <ip>"
[[ -z "$MK_B" ]] && die "mk-b IP required. Use -b <ip>"

# --- Helper: run RouterOS command via SSH ---
ros_cmd() {
    local host="$1"
    shift
    ssh "${SSH_OPTS[@]}" "${ROS_USER}@${host}" "$@"
}

# --- Helper: check SSH connectivity ---
check_ssh() {
    local host="$1" label="$2"
    info "Checking SSH to $label ($host)..."
    if ! ros_cmd "$host" "/system resource print" &>/dev/null; then
        die "Cannot SSH to $label ($host). Is RouterOS running?"
    fi
    info "$label is reachable"
}

# ============================================================
# TEARDOWN
# ============================================================
if [[ "$TEARDOWN" == true ]]; then
    section "Tearing down EoIP tunnels"
    for host_label in "$MK_A:mk-a" "$MK_B:mk-b"; do
        host="${host_label%%:*}"
        label="${host_label##*:}"
        info "Removing EoIP interfaces from $label ($host)..."
        ros_cmd "$host" '
/interface eoip remove [find where name~"eoip"]
' 2>/dev/null || true
        info "$label: cleaned"
    done
    info "Teardown complete"
    exit 0
fi

# ============================================================
# CONNECTIVITY CHECK
# ============================================================
section "Connectivity Check"
check_ssh "$MK_A" "mk-a"
check_ssh "$MK_B" "mk-b"

# ============================================================
# PRIMARY TUNNEL (tunnel-id=100)
# ============================================================
section "Configuring Primary EoIP Tunnel (tunnel-id=100)"

info "Configuring mk-a ($MK_A)..."
ros_cmd "$MK_A" "
/interface eoip remove [find where tunnel-id=100]
/interface eoip add name=eoip-tunnel1 remote-address=$MK_B tunnel-id=100
/ip address remove [find where interface=eoip-tunnel1]
/ip address add address=10.255.0.1/30 interface=eoip-tunnel1
"

info "Configuring mk-b ($MK_B)..."
ros_cmd "$MK_B" "
/interface eoip remove [find where tunnel-id=100]
/interface eoip add name=eoip-tunnel1 remote-address=$MK_A tunnel-id=100
/ip address remove [find where interface=eoip-tunnel1]
/ip address add address=10.255.0.2/30 interface=eoip-tunnel1
"

# --- Validate primary tunnel ---
section "Validating Primary Tunnel"
sleep 3

info "mk-a tunnel status:"
ros_cmd "$MK_A" "/interface eoip print where tunnel-id=100"

info "mk-b tunnel status:"
ros_cmd "$MK_B" "/interface eoip print where tunnel-id=100"

info "Ping mk-a -> mk-b (10.255.0.2)..."
if ros_cmd "$MK_A" "/ping address=10.255.0.2 count=5"; then
    info "mk-a -> mk-b: OK"
else
    warn "mk-a -> mk-b: ping failed (tunnel may need a moment)"
fi

info "Ping mk-b -> mk-a (10.255.0.1)..."
if ros_cmd "$MK_B" "/ping address=10.255.0.1 count=5"; then
    info "mk-b -> mk-a: OK"
else
    warn "mk-b -> mk-a: ping failed"
fi

info "ARP table on mk-a:"
ros_cmd "$MK_A" "/ip arp print where interface=eoip-tunnel1"

info "ARP table on mk-b:"
ros_cmd "$MK_B" "/ip arp print where interface=eoip-tunnel1"

# ============================================================
# MULTI-TUNNEL (tunnel-id=200) — optional
# ============================================================
if [[ "$MULTI" == true ]]; then
    section "Configuring Second Tunnel (tunnel-id=200)"

    info "Configuring mk-a..."
    ros_cmd "$MK_A" "
/interface eoip remove [find where tunnel-id=200]
/interface eoip add name=eoip-tunnel2 remote-address=$MK_B tunnel-id=200
/ip address remove [find where interface=eoip-tunnel2]
/ip address add address=10.255.1.1/30 interface=eoip-tunnel2
"

    info "Configuring mk-b..."
    ros_cmd "$MK_B" "
/interface eoip remove [find where tunnel-id=200]
/interface eoip add name=eoip-tunnel2 remote-address=$MK_A tunnel-id=200
/ip address remove [find where interface=eoip-tunnel2]
/ip address add address=10.255.1.2/30 interface=eoip-tunnel2
"

    sleep 3

    section "Validating Second Tunnel"
    info "Ping mk-a -> mk-b via tunnel2 (10.255.1.2)..."
    ros_cmd "$MK_A" "/ping address=10.255.1.2 count=3"

    info "Ping mk-b -> mk-a via tunnel2 (10.255.1.1)..."
    ros_cmd "$MK_B" "/ping address=10.255.1.1 count=3"

    section "Cross-talk Isolation Check"
    info "Verifying tunnel1 and tunnel2 are independent..."
    info "Tunnel1 status:"
    ros_cmd "$MK_A" "/interface eoip print where tunnel-id=100"
    info "Tunnel2 status:"
    ros_cmd "$MK_A" "/interface eoip print where tunnel-id=200"
    info "Both tunnels should show 'running' independently"
fi

# ============================================================
# EoIPv6 (tunnel-id=42) — optional
# ============================================================
if [[ "$IPV6" == true ]]; then
    section "Configuring EoIPv6 Tunnel (tunnel-id=42)"

    # Detect IPv6 addresses
    MK_A_V6=$(ros_cmd "$MK_A" "/ipv6 address print where global" 2>/dev/null | grep -oE '[0-9a-f:]+/[0-9]+' | head -1 | cut -d/ -f1)
    MK_B_V6=$(ros_cmd "$MK_B" "/ipv6 address print where global" 2>/dev/null | grep -oE '[0-9a-f:]+/[0-9]+' | head -1 | cut -d/ -f1)

    if [[ -z "$MK_A_V6" || -z "$MK_B_V6" ]]; then
        warn "IPv6 not available on one or both VMs — skipping EoIPv6"
        warn "  mk-a IPv6: ${MK_A_V6:-none}"
        warn "  mk-b IPv6: ${MK_B_V6:-none}"
    else
        info "mk-a IPv6: $MK_A_V6"
        info "mk-b IPv6: $MK_B_V6"

        info "Configuring mk-a..."
        ros_cmd "$MK_A" "
/interface eoip remove [find where tunnel-id=42]
/interface eoip add name=eoip6-tunnel1 remote-address=$MK_B_V6 tunnel-id=42
/ip address remove [find where interface=eoip6-tunnel1]
/ip address add address=10.255.2.1/30 interface=eoip6-tunnel1
"

        info "Configuring mk-b..."
        ros_cmd "$MK_B" "
/interface eoip remove [find where tunnel-id=42]
/interface eoip add name=eoip6-tunnel1 remote-address=$MK_A_V6 tunnel-id=42
/ip address remove [find where interface=eoip6-tunnel1]
/ip address add address=10.255.2.2/30 interface=eoip6-tunnel1
"

        sleep 3

        section "Validating EoIPv6 Tunnel"
        info "Ping mk-a -> mk-b via EoIPv6 (10.255.2.2)..."
        ros_cmd "$MK_A" "/ping address=10.255.2.2 count=3"

        info "Checking protocol type (should be EtherIP/97 for IPv6)..."
        ros_cmd "$MK_A" "/interface eoip print detail where tunnel-id=42"
    fi
fi

# ============================================================
# KEEPALIVE TEST — optional
# ============================================================
if [[ "$KEEPALIVE_TEST" == true ]]; then
    section "Keepalive Behavior Test"

    info "Setting short keepalive (3s interval, 3 retries = 9s timeout)..."
    ros_cmd "$MK_A" "/interface eoip set eoip-tunnel1 keepalive=3s,3"
    ros_cmd "$MK_B" "/interface eoip set eoip-tunnel1 keepalive=3s,3"
    sleep 5

    info "Current tunnel state on mk-a:"
    ros_cmd "$MK_A" "/interface eoip print detail where tunnel-id=100"

    info "Disabling eoip-tunnel1 on mk-b..."
    ros_cmd "$MK_B" "/interface eoip disable eoip-tunnel1"

    info "Waiting 15s for keepalive timeout on mk-a..."
    sleep 15

    info "Tunnel state on mk-a after mk-b disabled (expect NOT running):"
    ros_cmd "$MK_A" "/interface eoip print detail where tunnel-id=100"

    info "Re-enabling eoip-tunnel1 on mk-b..."
    ros_cmd "$MK_B" "/interface eoip enable eoip-tunnel1"

    info "Waiting 10s for keepalive recovery..."
    sleep 10

    info "Tunnel state on mk-a after recovery (expect running):"
    ros_cmd "$MK_A" "/interface eoip print detail where tunnel-id=100"

    info "Restoring default keepalive (10s,10)..."
    ros_cmd "$MK_A" "/interface eoip set eoip-tunnel1 keepalive=10s,10"
    ros_cmd "$MK_B" "/interface eoip set eoip-tunnel1 keepalive=10s,10"

    info "Keepalive test complete"
fi

# ============================================================
# EXPORT CONFIGS — optional
# ============================================================
if [[ "$EXPORT" == true ]]; then
    section "Exporting RouterOS Configs"
    mkdir -p "$CAPTURES_DIR"

    TIMESTAMP=$(date +%Y%m%d-%H%M%S)

    info "Exporting mk-a config..."
    ros_cmd "$MK_A" "/export" > "$CAPTURES_DIR/mk-a-export-${TIMESTAMP}.rsc"
    info "Saved: $CAPTURES_DIR/mk-a-export-${TIMESTAMP}.rsc"

    info "Exporting mk-b config..."
    ros_cmd "$MK_B" "/export" > "$CAPTURES_DIR/mk-b-export-${TIMESTAMP}.rsc"
    info "Saved: $CAPTURES_DIR/mk-b-export-${TIMESTAMP}.rsc"

    info "Exporting EoIP-specific details..."
    {
        echo "# mk-a EoIP interfaces ($(date))"
        echo "# Host: $MK_A"
        ros_cmd "$MK_A" "/interface eoip print detail"
        echo ""
        echo "# mk-b EoIP interfaces"
        echo "# Host: $MK_B"
        ros_cmd "$MK_B" "/interface eoip print detail"
    } > "$CAPTURES_DIR/eoip-detail-${TIMESTAMP}.txt"
    info "Saved: $CAPTURES_DIR/eoip-detail-${TIMESTAMP}.txt"
fi

# ============================================================
# SUMMARY
# ============================================================
section "Summary"
echo ""
echo "  mk-a:  $MK_A  (10.255.0.1)"
echo "  mk-b:  $MK_B  (10.255.0.2)"
echo "  Tunnel: eoip-tunnel1 (tunnel-id=100)"
if [[ "$MULTI" == true ]]; then
    echo "  Tunnel: eoip-tunnel2 (tunnel-id=200) — 10.255.1.0/30"
fi
if [[ "$IPV6" == true ]]; then
    echo "  Tunnel: eoip6-tunnel1 (tunnel-id=42) — 10.255.2.0/30 (EoIPv6)"
fi
echo ""
echo "  SSH to mk-a: ssh ${SSH_OPTS[*]} ${ROS_USER}@${MK_A}"
echo "  SSH to mk-b: ssh ${SSH_OPTS[*]} ${ROS_USER}@${MK_B}"
echo ""
echo "  Teardown: $0 -a $MK_A -b $MK_B --teardown"
echo ""
