# Phase 1: MikroTik CHR Infrastructure Bootstrap

**Status:** Draft  
**Priority:** Critical — blocks all subsequent phases  
**Estimated Duration:** 2-3 hours  
**Cost:** ~$2-5 (Hetzner CX23 VMs)

---

## Objective

Deploy the first MikroTik CHR (Cloud Hosted Router) VM on Hetzner Cloud, validate it's operational, document the exact provisioning process, and create reusable automation so subsequent VMs can be spun up in minutes.

## Background

All protocol testing depends on having real MikroTik instances running EoIP. CHR is the officially supported cloud/VM version of RouterOS. We need a repeatable, scripted process — not a one-off manual install.

## Requirements

### 1.1 Cloud Provider Setup
- Create Hetzner Cloud account (if not existing) with API token
- Choose region with low inter-VM latency (same datacenter for all test VMs)
- Budget: CX23 (2 vCPU, 4GB RAM) is sufficient for CHR (CX22 discontinued)
- Document API token storage (`.env` file, never committed)

### 1.2 First CHR VM Deployment
- Provision a VM (Ubuntu 22.04 base) via Hetzner Cloud API or `hcloud` CLI
- Install MikroTik CHR using one of the validated scripts:
  - Primary: `hreskiv/chr-on-vps` (auto-detects networking, hardened)
  - Fallback: `azadrahorg/Install-MikroTik-CHR-on-VPS` (simplest)
- RouterOS version: latest stable 7.x
- Record the exact commands and any manual steps required

### 1.3 Post-Install Validation
- SSH into CHR (default user: `admin`, no password initially)
- Verify RouterOS version: `/system resource print`
- Verify networking: ping external IPs, verify public IP matches Hetzner assignment
- Set admin password
- Verify EoIP interface creation works: `/interface eoip add remote-address=127.0.0.1 tunnel-id=999 disabled=yes`
- Remove test interface: `/interface eoip remove [find tunnel-id=999]`

### 1.4 Automation Script
- Create `tests/infra/deploy-chr.sh` that:
  - Accepts: VM name, Hetzner API token, SSH key, region
  - Creates Hetzner VM via `hcloud` CLI
  - Waits for SSH availability
  - Runs CHR install script
  - Waits for reboot into RouterOS
  - Runs post-install validation
  - Outputs: VM IP, SSH credentials
- Script must be idempotent (can re-run safely)
- Script must have a corresponding `teardown-chr.sh` to destroy VMs

### 1.5 Documentation
- `tests/infra/README.md` with:
  - Prerequisites (hcloud CLI, SSH key, API token)
  - Quick-start: single command to get a working CHR
  - Known issues / troubleshooting
  - Cost breakdown

## Success Criteria

- [ ] CHR VM boots into RouterOS and is reachable via SSH
- [ ] EoIP interface can be created and destroyed via CLI
- [ ] Deploy script provisions a new CHR in < 5 minutes
- [ ] Teardown script destroys all resources cleanly
- [ ] Process documented well enough for any team member to reproduce

## Risks

- Hetzner may block raw disk writes on some VM types (rescue mode may be needed)
- CHR free license limits throughput to 1 Mbps (sufficient for protocol testing, not perf)
- RouterOS SSH may use non-standard key exchange algorithms — document SSH client config
