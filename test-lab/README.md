# EoIP-rs Performance Test Lab

Reproducible benchmark environment for userspace performance work (Phase 11).

## Infrastructure

Two Hetzner Cloud VMs in the same datacenter (`fsn1`), connected via public IPv4.

```
┌─────────────────────────────┐         ┌─────────────────────────────┐
│  build-vm (node1)           │         │  test-vm (node2)            │
│  CPX22 (2 vCPU, 4GB)       │         │  CPX22 (2 vCPU, 4GB)       │
│  Ubuntu 22.04, Linux 5.15  │         │  Ubuntu 22.04, Linux 5.15  │
│                             │  GRE/47 │                             │
│  eth0: ${NODE1_IP}  ◄──────┼─────────┼──────► eth0: ${NODE2_IP}   │
│                             │         │                             │
│  eoip100: 10.200.0.1/24    │  EoIP   │  eoip100: 10.200.0.2/24    │
│         (TAP)               │ tunnel  │         (TAP)               │
│                             │  id=100 │                             │
│  /opt/eoip-new/  (current) │         │  /opt/eoip-new/  (current)  │
│  /opt/eoip-old/  (v0.1.0a) │         │  /opt/eoip-old/  (v0.1.0a) │
│  /root/EoIP-rs/  (source)  │         │                             │
│  Rust 1.95, protoc 28.3    │         │  iperf3                     │
└─────────────────────────────┘         └─────────────────────────────┘
```

IPs and SSH details are in `env.sh` (gitignored). See `env.sh.example` for the template.

## Quick Start

### 1. Source the environment

```bash
source test-lab/env.sh
```

### 2. Provision VMs (if destroyed)

```bash
hcloud server create --name build-vm --type $HCLOUD_SERVER_TYPE --image $HCLOUD_IMAGE --location $HCLOUD_LOCATION --ssh-key <your-key>
hcloud server create --name test-vm  --type $HCLOUD_SERVER_TYPE --image $HCLOUD_IMAGE --location $HCLOUD_LOCATION --ssh-key <your-key>

# Update env.sh with new IPs from:
hcloud server list
```

### 3. Bootstrap build-vm (first time)

```bash
ssh root@$NODE1_IP "apt-get update -qq && apt-get install -y -qq build-essential iperf3"

# Install Rust
ssh root@$NODE1_IP "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"

# Install protoc (needs >= 3.15 for proto3 optional)
ssh root@$NODE1_IP 'curl -sSL https://github.com/protocolbuffers/protobuf/releases/download/v28.3/protoc-28.3-linux-x86_64.zip -o /tmp/protoc.zip && apt-get install -y -qq unzip && unzip -o /tmp/protoc.zip -d /usr/local bin/protoc'
```

### 4. Bootstrap test-vm (first time)

```bash
ssh root@$NODE2_IP "apt-get update -qq && apt-get install -y -qq iperf3"
ssh root@$NODE2_IP "mkdir -p /etc/eoip-rs /run/eoip-rs /opt/eoip-new /opt/eoip-old"
```

### 5. Build and deploy

```bash
# Sync source and build
rsync -az --exclude target --exclude .git . root@$NODE1_IP:$REMOTE_BUILD_DIR/
ssh root@$NODE1_IP "source ~/.cargo/env && cd $REMOTE_BUILD_DIR && cargo build --release"

# Deploy to both nodes
scp root@$NODE1_IP:$REMOTE_BUILD_DIR/target/release/eoip-rs root@$NODE1_IP:$REMOTE_BUILD_DIR/target/release/eoip-helper /tmp/eoip-new/
scp /tmp/eoip-new/* root@$NODE1_IP:$REMOTE_NEW_DIR/
scp /tmp/eoip-new/* root@$NODE2_IP:$REMOTE_NEW_DIR/
```

### 6. Deploy old pre-release binaries (for A/B testing)

```bash
gh release download v0.1.0-alpha.1 --repo manawenuz/EoIP-rs --dir /tmp/eoip-old
cd /tmp/eoip-old && tar xzf eoip-rs-0.1.0-linux-x86_64.tar.gz
scp /tmp/eoip-old/eoip-rs-0.1.0-linux-x86_64/eoip-rs /tmp/eoip-old/eoip-rs-0.1.0-linux-x86_64/eoip-helper root@$NODE1_IP:$REMOTE_OLD_DIR/
scp /tmp/eoip-old/eoip-rs-0.1.0-linux-x86_64/eoip-rs /tmp/eoip-old/eoip-rs-0.1.0-linux-x86_64/eoip-helper root@$NODE2_IP:$REMOTE_OLD_DIR/
```

## Tunnel Configuration

Deployed to `/etc/eoip-rs/config.toml` on each node.

**Node 1:**
```toml
[daemon]
helper_mode = "persist"
helper_socket = "/run/eoip-rs/helper.sock"

[api]
listen = "[::1]:50051"

[performance]
low_water_mark = 8
high_water_mark = 256
max_batch_size = 64
batch_timeout_us = 50
channel_buffer = 1024

[[tunnel]]
tunnel_id = 100
local = "<NODE1_IP>"
remote = "<NODE2_IP>"
iface_name = "eoip100"
mtu = 1500
enabled = true
keepalive_interval_secs = 10
keepalive_timeout_secs = 30
```

**Node 2:** Same but `local`/`remote` swapped.

## VM Scripts

Three scripts are deployed to `/opt/` on each VM:

### `eoip-start-quiet.sh <new|old>`

Starts the specified binary variant. Backgrounds helper + daemon, configures the tunnel interface. Requires `NODE_ID` env var (1 or 2).

```bash
ssh root@$NODE1_IP "NODE_ID=1 bash /opt/eoip-start-quiet.sh new"
ssh root@$NODE2_IP "NODE_ID=2 bash /opt/eoip-start-quiet.sh new"
```

### `eoip-stop.sh`

Kills all eoip processes and deletes the tunnel interface.

```bash
ssh root@$NODE1_IP "bash /opt/eoip-stop.sh"
```

### `eoip-start.sh <new|old>`

Verbose version of start — outputs daemon logs to terminal. Useful for debugging but blocks the SSH session (daemon stdout isn't redirected). Use `eoip-start-quiet.sh` for benchmarks.

## Benchmarking

### Quick throughput test

```bash
source test-lab/env.sh

# Start tunnel
ssh root@$NODE1_IP "NODE_ID=1 bash /opt/eoip-start-quiet.sh new"
ssh root@$NODE2_IP "NODE_ID=2 bash /opt/eoip-start-quiet.sh new"
sleep 3

# Verify
ssh root@$NODE1_IP "ping -c 2 -W 2 $NODE2_TUNNEL_IP"

# TX test (node1 sends → node2 receives)
ssh root@$NODE2_IP "killall iperf3 2>/dev/null; iperf3 -s -D"
ssh root@$NODE1_IP "iperf3 -c $NODE2_TUNNEL_IP -t $BENCH_DURATION"

# RX test (node2 sends → node1 receives)
ssh root@$NODE1_IP "iperf3 -c $NODE2_TUNNEL_IP -t $BENCH_DURATION -R"
```

### Full A/B benchmark

Runs all 4 combinations (new+new, new+old, old+new, old+old) with JSON output for CPU metrics:

```bash
run_bench() {
    local name="$1" v1="$2" v2="$3"
    ssh root@$NODE1_IP "bash /opt/eoip-stop.sh" &>/dev/null
    ssh root@$NODE2_IP "bash /opt/eoip-stop.sh" &>/dev/null
    sleep 1
    ssh root@$NODE1_IP "NODE_ID=1 bash /opt/eoip-start-quiet.sh $v1"
    ssh root@$NODE2_IP "NODE_ID=2 bash /opt/eoip-start-quiet.sh $v2"
    sleep 3

    ssh root@$NODE2_IP "killall iperf3 2>/dev/null; iperf3 -s -D" &>/dev/null
    sleep 1

    TX=$(ssh root@$NODE1_IP "iperf3 -c $NODE2_TUNNEL_IP -t $BENCH_DURATION -J")
    TX_MBPS=$(echo "$TX" | python3 -c "import sys,json;d=json.load(sys.stdin);print(f\"{d['end']['sum_sent']['bits_per_second']/1e6:.1f}\")")
    TX_CPU=$(echo "$TX" | python3 -c "import sys,json;d=json.load(sys.stdin);print(f\"{d['end']['cpu_utilization_percent']['host_total']:.1f}\")")

    RX=$(ssh root@$NODE1_IP "iperf3 -c $NODE2_TUNNEL_IP -t $BENCH_DURATION -R -J")
    RX_MBPS=$(echo "$RX" | python3 -c "import sys,json;d=json.load(sys.stdin);print(f\"{d['end']['sum_received']['bits_per_second']/1e6:.1f}\")")
    RX_CPU=$(echo "$RX" | python3 -c "import sys,json;d=json.load(sys.stdin);print(f\"{d['end']['cpu_utilization_percent']['host_total']:.1f}\")")

    ssh root@$NODE2_IP "killall iperf3" &>/dev/null
    echo "$name: TX=${TX_MBPS}Mbps(cpu:${TX_CPU}%) RX=${RX_MBPS}Mbps(cpu:${RX_CPU}%)"
}

run_bench "new+new" new new
run_bench "new+old" new old
run_bench "old+new" old new
run_bench "old+old" old old
```

### Baseline numbers (2026-04-18)

Commit `064486b` (main, IPv6 fix only, no PACKET_MMAP).

| Test | TX (Mbps) | TX CPU | RX (Mbps) | RX CPU |
|------|-----------|--------|-----------|--------|
| new+new | 500 | 1.3% | 424 | 20.3% |
| new+old | 339 | 0.7% | 501 | 28.3% |
| old+new | 471 | 1.2% | 234 | 21.4% |
| old+old | 406 | 1.1% | 313 | 21.9% |

## Profiling

```bash
# CPU flamegraph (install perf + inferno on build-vm first)
ssh root@$NODE1_IP "perf record -g --call-graph=dwarf -p \$(pgrep -of 'eoip-rs --config') -- sleep 10"
ssh root@$NODE1_IP "perf script" | inferno-collapse-perf | inferno-flamegraph > flame.svg

# Syscall counts
ssh root@$NODE1_IP "perf stat -e 'syscalls:sys_enter_recvmmsg,syscalls:sys_enter_write,syscalls:sys_enter_sendto' -p \$(pgrep -of 'eoip-rs --config') -- sleep 10"

# Thread check
ssh root@$NODE1_IP "cat /proc/\$(pgrep -of 'eoip-rs --config')/status | grep Threads"

# Strace (check for stuck syscalls)
ssh root@$NODE1_IP "timeout 3 strace -p \$(pgrep -of 'eoip-rs --config') -e recvmsg,poll -c"
```

## Teardown

```bash
ssh root@$NODE1_IP "bash /opt/eoip-stop.sh"
ssh root@$NODE2_IP "bash /opt/eoip-stop.sh"

# Destroy VMs (they're cheap to recreate)
hcloud server delete build-vm
hcloud server delete test-vm
```
