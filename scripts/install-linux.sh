#!/usr/bin/env bash
#
# install-linux.sh — Install EoIP-rs binaries and create initial config
#
set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
CONFIG_DIR="/etc/eoip-rs"
RUN_DIR="/run/eoip-rs"
SYSTEMD_DIR="/etc/systemd/system"

info()  { echo -e "\033[0;32m[+]\033[0m $*"; }
warn()  { echo -e "\033[1;33m[!]\033[0m $*"; }
error() { echo -e "\033[0;31m[x]\033[0m $*" >&2; }

# Find binaries (either in current dir or in the release tarball)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BIN_DIR="$SCRIPT_DIR"

for bin in eoip-rs eoip-helper eoip-cli eoip-analyzer; do
    if [[ ! -f "$BIN_DIR/$bin" ]]; then
        error "$bin not found in $BIN_DIR"
        exit 1
    fi
done

info "Installing binaries to $PREFIX/bin/"
install -m 755 "$BIN_DIR/eoip-rs" "$PREFIX/bin/"
install -m 755 "$BIN_DIR/eoip-helper" "$PREFIX/bin/"
install -m 755 "$BIN_DIR/eoip-cli" "$PREFIX/bin/"
install -m 755 "$BIN_DIR/eoip-analyzer" "$PREFIX/bin/"

info "Creating config directory: $CONFIG_DIR"
mkdir -p "$CONFIG_DIR"
if [[ ! -f "$CONFIG_DIR/config.toml" ]]; then
    if [[ -f "$BIN_DIR/eoip-rs.example.toml" ]]; then
        cp "$BIN_DIR/eoip-rs.example.toml" "$CONFIG_DIR/config.toml"
        info "Example config copied to $CONFIG_DIR/config.toml"
    fi
else
    warn "Config already exists at $CONFIG_DIR/config.toml (not overwritten)"
fi

info "Creating runtime directory: $RUN_DIR"
mkdir -p "$RUN_DIR"

# Create service user
if ! id -u eoip &>/dev/null; then
    info "Creating service user: eoip"
    useradd -r -s /usr/sbin/nologin eoip 2>/dev/null || true
fi

# Install systemd services if available
if [[ -d "$SCRIPT_DIR/../systemd" ]] && command -v systemctl &>/dev/null; then
    info "Installing systemd services"
    cp "$SCRIPT_DIR/../systemd/eoip-helper.service" "$SYSTEMD_DIR/" 2>/dev/null || true
    cp "$SCRIPT_DIR/../systemd/eoip-rs.service" "$SYSTEMD_DIR/" 2>/dev/null || true
    systemctl daemon-reload 2>/dev/null || true
    info "Run: sudo systemctl enable --now eoip-helper eoip-rs"
fi

echo ""
info "Installation complete!"
echo ""
echo "  Next steps:"
echo "  1. Edit $CONFIG_DIR/config.toml (set local/remote IPs)"
echo "  2. Start: sudo eoip-helper --mode persist &"
echo "  3. Start: sudo eoip-rs --config $CONFIG_DIR/config.toml &"
echo "  4. Configure: sudo ip link set eoip100 up"
echo "  5. Verify: eoip-cli print"
echo ""
