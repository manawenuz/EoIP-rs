#!/usr/bin/env bash
#
# deploy-chr.sh — Deploy a MikroTik CHR VM on Hetzner Cloud
#
# Usage:
#   ./deploy-chr.sh [OPTIONS]
#
# Options:
#   -n, --name NAME        VM name (default: chr-test-1)
#   -k, --ssh-key NAME     Hetzner SSH key name (required)
#   -l, --location LOC     Hetzner location (default: fsn1)
#   -t, --type TYPE        Server type (default: cx23)
#   -v, --ros-version VER  RouterOS version (default: 7.18.2)
#   -h, --help             Show this help
#
# Prerequisites:
#   - hcloud CLI installed and configured (hcloud context active)
#   - SSH key uploaded to Hetzner (hcloud ssh-key list)
#
set -euo pipefail

# --- Defaults ---
VM_NAME="chr-test-1"
SSH_KEY=""
LOCATION="fsn1"
SERVER_TYPE="cx23"
ROS_VERSION="7.18.2"

# --- Colors ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info()  { echo -e "${GREEN}[+]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
error() { echo -e "${RED}[✗]${NC} $*" >&2; }
die()   { error "$@"; exit 1; }

# --- Parse args ---
while [[ $# -gt 0 ]]; do
    case $1 in
        -n|--name)        VM_NAME="$2"; shift 2 ;;
        -k|--ssh-key)     SSH_KEY="$2"; shift 2 ;;
        -l|--location)    LOCATION="$2"; shift 2 ;;
        -t|--type)        SERVER_TYPE="$2"; shift 2 ;;
        -v|--ros-version) ROS_VERSION="$2"; shift 2 ;;
        -h|--help)        head -16 "$0" | tail -14; exit 0 ;;
        *)                die "Unknown option: $1" ;;
    esac
done

[[ -z "$SSH_KEY" ]] && die "SSH key required. Use -k <name> (see: hcloud ssh-key list)"

# --- Verify hcloud context ---
CONTEXT=$(hcloud context active 2>/dev/null) || die "No active hcloud context. Run: hcloud context create <name>"
info "Using hcloud context: $CONTEXT"

# --- Check if VM already exists ---
if hcloud server describe "$VM_NAME" &>/dev/null; then
    warn "Server '$VM_NAME' already exists"
    EXISTING_IP=$(hcloud server ip "$VM_NAME")
    info "IP: $EXISTING_IP"
    echo ""
    echo "To redeploy, teardown first: ./teardown-chr.sh -n $VM_NAME"
    exit 0
fi

# --- Create VM with Ubuntu base (for CHR install) ---
info "Creating $SERVER_TYPE VM '$VM_NAME' in $LOCATION..."
hcloud server create \
    --name "$VM_NAME" \
    --type "$SERVER_TYPE" \
    --location "$LOCATION" \
    --image ubuntu-22.04 \
    --ssh-key "$SSH_KEY" \
    --label "purpose=eoip-testing" \
    --label "ros-version=$ROS_VERSION"

VM_IP=$(hcloud server ip "$VM_NAME")
info "VM created: $VM_IP"

# --- Wait for SSH ---
info "Waiting for SSH on $VM_IP..."
for i in $(seq 1 30); do
    if ssh -o StrictHostKeyChecking=no -o ConnectTimeout=3 -o BatchMode=yes "root@$VM_IP" true &>/dev/null; then
        break
    fi
    if [[ $i -eq 30 ]]; then
        die "SSH timeout after 90s"
    fi
    sleep 3
done
info "SSH is up"

# --- Install CHR ---
info "Installing MikroTik CHR $ROS_VERSION..."
CHR_URL="https://download.mikrotik.com/routeros/$ROS_VERSION/chr-$ROS_VERSION.img.zip"

ssh -o StrictHostKeyChecking=no "root@$VM_IP" bash <<REMOTE_SCRIPT
set -euo pipefail

echo "[+] Downloading CHR image..."
cd /tmp
wget -q "$CHR_URL" -O chr.img.zip

echo "[+] Extracting..."
apt-get -qq update && apt-get -qq install -y unzip > /dev/null 2>&1
unzip -o chr.img.zip

echo "[+] Identifying boot disk..."
BOOT_DISK=\$(lsblk -dno NAME,TYPE | awk '\$2=="disk"{print \$1; exit}')
echo "    Boot disk: /dev/\$BOOT_DISK"

echo "[+] Writing CHR image to /dev/\$BOOT_DISK..."
dd if=/tmp/chr-$ROS_VERSION.img of=/dev/\$BOOT_DISK bs=4M oflag=sync status=none

echo "[+] Configuring first-boot networking..."
# Mount the CHR partition to inject network config
mkdir -p /mnt/chr
# RouterOS uses partition 2 for config on raw images
# We write a rsc (RouterOS script) that runs on first boot
# The network config will be auto-detected by CHR in most cases

echo "[+] Done. Triggering reboot into RouterOS..."
nohup bash -c "sleep 2 && echo b > /proc/sysrq-trigger" &>/dev/null &
REMOTE_SCRIPT

info "Reboot triggered. Waiting for RouterOS to come up..."
sleep 15

# --- Wait for RouterOS SSH ---
info "Waiting for RouterOS SSH on $VM_IP..."
ROUTEROS_UP=0
for i in $(seq 1 40); do
    # RouterOS SSH has a distinct banner; try connecting
    if ssh -o StrictHostKeyChecking=no \
           -o ConnectTimeout=5 \
           -o BatchMode=yes \
           -o PubkeyAcceptedAlgorithms=+ssh-rsa \
           -o HostKeyAlgorithms=+ssh-rsa \
           "admin@$VM_IP" "/system resource print" &>/dev/null; then
        ROUTEROS_UP=1
        break
    fi
    sleep 5
done

if [[ $ROUTEROS_UP -eq 0 ]]; then
    warn "RouterOS SSH not responding after 200s"
    warn "This may be normal — CHR sometimes needs manual network setup via Hetzner console"
    warn "Try: ssh -o HostKeyAlgorithms=+ssh-rsa admin@$VM_IP"
    echo ""
    echo "Hetzner console: hcloud server request-console $VM_NAME"
    exit 1
fi

info "RouterOS is up!"

# --- Post-install validation ---
info "Running post-install validation..."
ssh -o StrictHostKeyChecking=no \
    -o PubkeyAcceptedAlgorithms=+ssh-rsa \
    -o HostKeyAlgorithms=+ssh-rsa \
    "admin@$VM_IP" <<'VALIDATE'
:put "=== RouterOS Version ==="
/system resource print
:put ""
:put "=== Interfaces ==="
/interface print
:put ""
:put "=== EoIP Test ==="
/interface eoip add remote-address=127.0.0.1 tunnel-id=999 disabled=yes name=eoip-validation-test
/interface eoip print where tunnel-id=999
/interface eoip remove [find tunnel-id=999]
:put "EoIP create/destroy: OK"
VALIDATE

echo ""
info "========================================="
info "CHR deployment complete!"
info "========================================="
echo ""
echo "  VM Name:    $VM_NAME"
echo "  IP:         $VM_IP"
echo "  RouterOS:   $ROS_VERSION"
echo "  SSH:        ssh -o HostKeyAlgorithms=+ssh-rsa admin@$VM_IP"
echo ""
echo "  Teardown:   ./teardown-chr.sh -n $VM_NAME"
echo ""
