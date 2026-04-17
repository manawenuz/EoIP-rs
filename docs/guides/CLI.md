# CLI Reference

`eoip-cli` is a MikroTik RouterOS-style management tool for EoIP-rs.

## Connection

```bash
# Connect to local daemon (default)
eoip-cli print

# Connect to remote daemon
eoip-cli --address http://10.0.0.1:50051 print

# JSON output mode
eoip-cli --json print
```

## Interactive REPL

Run without arguments to enter interactive mode:

```
$ eoip-cli
EoIP-rs CLI v0.1.0
Connected to http://[::1]:50051
Type 'help' for commands, 'quit' to exit.

[admin@eoip-rs] > print
Flags: X - disabled; R - running; S - stale; I - initializing
 #   NAME             TUNNEL-ID   LOCAL-ADDR       REMOTE-ADDR        MTU
 0 R eoip100                100   192.168.1.10     192.168.1.1       1458

[admin@eoip-rs] > stats 100
        tunnel-id: 100
       tx-packets: 1542
         tx-bytes: 125.3 KiB
       rx-packets: 1538
         rx-bytes: 120.1 KiB
        tx-errors: 0
        rx-errors: 0
          last-rx: 2s ago
          last-tx: 3s ago

[admin@eoip-rs] > quit
bye.
```

## Command Reference

### print

List tunnels in RouterOS-style table format.

```bash
eoip-cli print                           # All tunnels
eoip-cli print detail                    # Detailed properties
eoip-cli print where tunnel-id=100       # Filter by ID
eoip-cli print where name=eoip-dc1       # Filter by name
```

**Flags:** `R` = running, `X` = disabled, `S` = stale, `I` = initializing

### add

Create a new tunnel dynamically (no daemon restart needed).

```bash
eoip-cli add tunnel-id=200 remote-address=10.0.0.2 local-address=10.0.0.1
eoip-cli add tunnel-id=300 remote-address=10.0.0.3 local-address=10.0.0.1 name=eoip-dc2 mtu=1400
```

After adding, configure the TAP interface:

```bash
sudo ip link set eoip200 up
sudo ip addr add 10.255.1.2/30 dev eoip200
```

### remove

Delete a tunnel.

```bash
eoip-cli remove 200
```

### enable / disable

Control tunnel state.

```bash
eoip-cli disable 100
eoip-cli enable 100
```

### set

Modify tunnel properties.

```bash
eoip-cli set 100 mtu=1400
eoip-cli set 100 keepalive-interval=5 keepalive-timeout=25
eoip-cli set 100 enabled=no
```

### monitor

Stream real-time tunnel events.

```bash
eoip-cli monitor
# [1713369600.000000] CREATED tunnel-id=100 name="eoip100" state=3
# [1713369601.000000] STATE_CHANGED tunnel-id=100 name="eoip100" state=3
# (Ctrl-C to stop)
```

### stats

View traffic statistics.

```bash
eoip-cli stats                    # Global stats
eoip-cli stats 100                # Per-tunnel stats
```

### health

Check daemon health.

```bash
eoip-cli health
# status: SERVING
```

## Full Path Syntax

Commands support MikroTik-style path prefixes:

```bash
eoip-cli /interface/eoip/print
eoip-cli /interface/eoip/add tunnel-id=200 remote-address=1.2.3.4
eoip-cli /interface/eoip/stats 100
eoip-cli /system/health
```

The `/interface/eoip/` prefix is optional — bare commands work in both one-shot and REPL mode.
