# EoIP-rs Test Infrastructure

MikroTik CHR (Cloud Hosted Router) VMs on Hetzner Cloud for protocol testing.

## Prerequisites

1. **hcloud CLI** — `brew install hcloud` / [github.com/hetznercloud/cli](https://github.com/hetznercloud/cli)
2. **Hetzner API token** — [console.hetzner.cloud](https://console.hetzner.cloud) → Project → Security → API Tokens
3. **hcloud context** — `hcloud context create eoip-test` (paste token when prompted)
4. **SSH key in Hetzner** — `hcloud ssh-key create --name mykey --public-key-from-file ~/.ssh/id_ed25519.pub`

Verify setup:
```bash
hcloud context active    # should print your context name
hcloud ssh-key list      # should show your key
```

## Quick Start

```bash
# Deploy first CHR VM
./deploy-chr.sh -k <your-ssh-key-name>

# SSH into RouterOS
ssh -o HostKeyAlgorithms=+ssh-rsa admin@<VM_IP>

# Tear down when done
./teardown-chr.sh -n chr-test-1

# Tear down ALL eoip-testing VMs
./teardown-chr.sh --all
```

### Deploy options

| Flag | Default | Description |
|------|---------|-------------|
| `-n, --name` | `chr-test-1` | VM name |
| `-k, --ssh-key` | (required) | Hetzner SSH key name |
| `-l, --location` | `fsn1` | Datacenter (`fsn1`, `nbg1`, `hel1`, etc.) |
| `-t, --type` | `cx23` | Server type (2 vCPU, 4GB RAM) |
| `-v, --ros-version` | `7.18.2` | RouterOS version |

## SSH to RouterOS

RouterOS uses older SSH algorithms. You'll need:

```bash
ssh -o HostKeyAlgorithms=+ssh-rsa -o PubkeyAcceptedAlgorithms=+ssh-rsa admin@<IP>
```

Or add to `~/.ssh/config`:
```
Host chr-*
    HostKeyAlgorithms +ssh-rsa
    PubkeyAcceptedAlgorithms +ssh-rsa
    User admin
```

## Cost

| Resource | Cost |
|----------|------|
| CX23 (2 vCPU, 4GB) | ~€0.007/hr (~€5/mo) |
| CHR free license | €0 (1 Mbps limit — fine for protocol testing) |
| **Per session (2-3 hrs)** | **~€0.02** |

Always teardown when not testing: `./teardown-chr.sh --all`

## Troubleshooting

### RouterOS SSH not responding after install
- CHR may need network config via Hetzner VNC console: `hcloud server request-console <name>`
- In console, set IP manually: `/ip address add address=<HETZNER_IP>/32 interface=ether1`
- Set gateway: `/ip route add dst-address=0.0.0.0/0 gateway=<GATEWAY>`
- Gateway is usually the `.1` of the server's subnet (check Hetzner console → Networking)

### SSH key rejected
- RouterOS only supports RSA keys by default (ed25519 works on RouterOS 7.x)
- Ensure the key in Hetzner matches your local key

### CHR image download fails
- Check RouterOS version exists: `https://download.mikrotik.com/routeros/`
- The script uses raw `.img.zip` format

## Phase 2: MikroTik-to-MikroTik EoIP Baseline

Once you have two CHR VMs from Phase 1, set up EoIP tunnels between them:

```bash
# Deploy two CHR VMs
./deploy-chr.sh -n mk-a -k <your-ssh-key>
./deploy-chr.sh -n mk-b -k <your-ssh-key>

# Get their IPs
MK_A=$(hcloud server ip mk-a)
MK_B=$(hcloud server ip mk-b)

# Configure EoIP tunnel pair (tunnel-id=100, 10.255.0.0/30)
./setup-eoip-pair.sh -a $MK_A -b $MK_B

# Full test: primary + second tunnel + EoIPv6 + keepalive + export configs
./setup-eoip-pair.sh -a $MK_A -b $MK_B --multi --ipv6 --keepalive-test --export

# Remove all EoIP tunnels from both VMs
./setup-eoip-pair.sh -a $MK_A -b $MK_B --teardown
```

### setup-eoip-pair.sh options

| Flag | Description |
|------|-------------|
| `-a, --mk-a IP` | First CHR VM IP (required) |
| `-b, --mk-b IP` | Second CHR VM IP (required) |
| `-u, --user` | RouterOS SSH user (default: `admin`) |
| `--multi` | Add second tunnel (tunnel-id=200, 10.255.1.0/30) |
| `--ipv6` | Add EoIPv6 tunnel (tunnel-id=42, 10.255.2.0/30) if IPv6 available |
| `--keepalive-test` | Block/unblock GRE traffic to test keepalive failover |
| `--export` | Save RouterOS `/export` to `tests/captures/mk-mk-baseline/` |
| `--teardown` | Remove all EoIP tunnels from both VMs |

### Captures

Config exports and EoIP details are saved to `tests/captures/mk-mk-baseline/` with timestamps. These serve as the "known-good" baseline for protocol analysis in Phase 3.

## Architecture

```
Your Machine                    Hetzner Cloud (fsn1)
┌──────────┐                   ┌──────────────────┐
│ hcloud   │───── API ────────▶│  CX23 VM         │
│ CLI      │                   │  MikroTik CHR    │
│          │◀──── SSH ────────▶│  RouterOS 7.x    │
└──────────┘                   └──────────────────┘
```

Phase 2 establishes the MikroTik-to-MikroTik EoIP baseline — the ground truth for protocol analysis.

```
Your Machine                    Hetzner Cloud (fsn1)
┌──────────┐                   ┌──────────┐     ┌──────────┐
│ hcloud   │───── API ────────▶│  mk-a    │────▶│  mk-b    │
│ CLI      │                   │  CHR     │EoIP │  CHR     │
│          │◀──── SSH ────────▶│ .0.1/30  │◀────│ .0.2/30  │
└──────────┘                   └──────────┘     └──────────┘
```
