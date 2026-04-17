# setup-tunnel.ps1 — Quick single-tunnel setup for EoIP-rs on Windows
#
# Usage:
#   .\setup-tunnel.ps1 -TunnelId 100 -Local "10.0.0.2" -Remote "10.0.0.1" -TunnelIP "10.255.0.2" -PrefixLength 30
#

param(
    [Parameter(Mandatory=$true)][int]$TunnelId,
    [Parameter(Mandatory=$true)][string]$Local,
    [Parameter(Mandatory=$true)][string]$Remote,
    [Parameter(Mandatory=$true)][string]$TunnelIP,
    [int]$PrefixLength = 30,
    [string]$ConfigPath = "C:\eoip-rs\config.toml",
    [string]$BinaryPath = "C:\eoip-rs"
)

$ErrorActionPreference = "Stop"

Write-Output "[+] Creating config: $ConfigPath"
$configDir = Split-Path $ConfigPath
if (-not (Test-Path $configDir)) { New-Item -ItemType Directory -Path $configDir -Force | Out-Null }

@"
[daemon]
user = "root"
group = "root"

[logging]
level = "info"

[[tunnel]]
tunnel_id = $TunnelId
local = "$Local"
remote = "$Remote"
mtu = 1458
keepalive_interval_secs = 10
keepalive_timeout_secs = 100
"@ | Set-Content $ConfigPath

Write-Output "[+] Checking TAP driver..."
$tap = Get-NetAdapter | Where-Object { $_.InterfaceDescription -like "*TAP*" }
if (-not $tap) {
    Write-Output "[!] No TAP adapter found. Install OpenVPN TAP driver:"
    Write-Output "    https://swupdate.openvpn.org/community/releases/OpenVPN-2.6.12-I001-amd64.msi"
    exit 1
}
Write-Output "    Found: $($tap.Name) ($($tap.InterfaceDescription))"

Write-Output "[+] Stopping existing daemon..."
Stop-Process -Name eoip-rs-win -Force -ErrorAction SilentlyContinue
Start-Sleep 1

Write-Output "[+] Starting eoip-rs-win..."
$exe = Join-Path $BinaryPath "eoip-rs-win.exe"
if (-not (Test-Path $exe)) {
    Write-Output "[!] Binary not found: $exe"
    exit 1
}

Start-Process -FilePath $exe -ArgumentList "--config", $ConfigPath -WorkingDirectory $BinaryPath -NoNewWindow
Start-Sleep 5

$proc = Get-Process eoip-rs-win -ErrorAction SilentlyContinue
if ($proc) {
    Write-Output "[+] Daemon running (PID: $($proc.Id))"
} else {
    Write-Output "[!] Daemon failed to start. Check logs."
    exit 1
}

Write-Output "[+] Configuring TAP interface ($TunnelIP/$PrefixLength)..."
$tapAlias = $tap.Name
Remove-NetIPAddress -InterfaceAlias $tapAlias -Confirm:$false -ErrorAction SilentlyContinue
New-NetIPAddress -InterfaceAlias $tapAlias -IPAddress $TunnelIP -PrefixLength $PrefixLength -ErrorAction SilentlyContinue | Out-Null

Write-Output ""
Write-Output "[+] Tunnel setup complete!"
Write-Output ""
Write-Output "  Interface: $tapAlias"
Write-Output "  Tunnel IP: $TunnelIP/$PrefixLength"
Write-Output "  Remote:    $Remote (tunnel-id=$TunnelId)"
Write-Output ""
Write-Output "  Verify:    .\eoip-cli.exe print"
Write-Output "  Ping:      ping $TunnelIP"
Write-Output ""
Write-Output "  MikroTik config:"
Write-Output "    /interface eoip add name=eoip-win remote-address=$Local tunnel-id=$TunnelId"
Write-Output "    /ip address add address=<MIKROTIK_TUNNEL_IP>/30 interface=eoip-win"
