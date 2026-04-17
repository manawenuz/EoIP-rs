#!/usr/bin/env bash
#
# teardown-chr.sh — Destroy MikroTik CHR VMs on Hetzner Cloud
#
# Usage:
#   ./teardown-chr.sh -n <name>      Destroy a specific VM
#   ./teardown-chr.sh --all          Destroy ALL eoip-testing VMs
#
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info()  { echo -e "${GREEN}[+]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
error() { echo -e "${RED}[✗]${NC} $*" >&2; }
die()   { error "$@"; exit 1; }

VM_NAME=""
DESTROY_ALL=0

while [[ $# -gt 0 ]]; do
    case $1 in
        -n|--name) VM_NAME="$2"; shift 2 ;;
        --all)     DESTROY_ALL=1; shift ;;
        -h|--help) head -8 "$0" | tail -6; exit 0 ;;
        *)         die "Unknown option: $1" ;;
    esac
done

[[ -z "$VM_NAME" && $DESTROY_ALL -eq 0 ]] && die "Specify -n <name> or --all"

if [[ $DESTROY_ALL -eq 1 ]]; then
    SERVERS=$(hcloud server list -l purpose=eoip-testing -o noheader -o columns=name 2>/dev/null)
    if [[ -z "$SERVERS" ]]; then
        info "No eoip-testing VMs found"
        exit 0
    fi
    echo "Will destroy the following VMs:"
    echo "$SERVERS" | sed 's/^/  - /'
    echo ""
    read -rp "Confirm? [y/N] " CONFIRM
    [[ "$CONFIRM" =~ ^[Yy]$ ]] || { info "Cancelled"; exit 0; }

    while IFS= read -r name; do
        info "Destroying $name..."
        hcloud server delete "$name"
    done <<< "$SERVERS"
    info "All eoip-testing VMs destroyed"
else
    if ! hcloud server describe "$VM_NAME" &>/dev/null; then
        warn "Server '$VM_NAME' not found — already destroyed?"
        exit 0
    fi
    info "Destroying $VM_NAME..."
    hcloud server delete "$VM_NAME"
    info "Done"
fi
