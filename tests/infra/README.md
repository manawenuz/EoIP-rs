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

## Architecture

```
Your Machine                    Hetzner Cloud (fsn1)
┌──────────┐                   ┌──────────────────┐
│ hcloud   │───── API ────────▶│  CX23 VM         │
│ CLI      │                   │  MikroTik CHR    │
│          │◀──── SSH ────────▶│  RouterOS 7.x    │
└──────────┘                   └──────────────────┘
```

Phase 2 adds a second VM for MikroTik-to-MikroTik EoIP baseline testing.
