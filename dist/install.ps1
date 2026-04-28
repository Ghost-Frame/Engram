# Kleos installer for Windows -- downloads the correct binaries.
#
# Usage (PowerShell):
#   irm https://raw.githubusercontent.com/Ghost-Frame/Engram/main/dist/install.ps1 | iex
#
# Options (via environment variables):
#   $env:KLEOS_VERSION   -- version to install (default: latest)
#   $env:KLEOS_INSTALL   -- installation directory (default: ~\.kleos\bin)
#   $env:KLEOS_PROFILE   -- "server" (default), "agent-host", or "full"
#   $env:KLEOS_BINARIES  -- comma-separated list (overrides KLEOS_PROFILE if set)

$ErrorActionPreference = "Stop"

$Repo = "Ghost-Frame/Engram"
$Version = if ($env:KLEOS_VERSION) { $env:KLEOS_VERSION } else { "" }
$InstallDir = if ($env:KLEOS_INSTALL) { $env:KLEOS_INSTALL } else { Join-Path $HOME ".kleos\bin" }
$Profile = if ($env:KLEOS_PROFILE) { $env:KLEOS_PROFILE } else { "server" }
$Suffix = "windows-x64"

# Resolve binary list from profile if not set explicitly
if ($env:KLEOS_BINARIES) {
    $BinList = $env:KLEOS_BINARIES -split ","
} else {
    switch ($Profile) {
        { $_ -in "agent-host", "agent" } {
            $BinList = @("kleos-cli", "kleos-sh", "kr", "kw", "ke", "agent-forge", "eidolon-supervisor", "kleos-cred", "kleos-credd")
        }
        "full" {
            $BinList = @("kleos-server", "kleos-cli", "kleos-sidecar", "kleos-credd", "kleos-cred", "kleos-mcp", "kleos-sh", "kr", "kw", "ke", "agent-forge", "eidolon-supervisor")
        }
        default {
            $BinList = @("kleos-server", "kleos-cli", "kleos-mcp")
        }
    }
}

# ─── Resolve latest version ─────────────────────────────────────────────────

if (-not $Version) {
    Write-Host "Fetching latest release..."
    try {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -UseBasicParsing
        $Version = $release.tag_name -replace "^v", ""
    }
    catch {
        Write-Error "Could not determine latest version. Set `$env:KLEOS_VERSION manually."
        exit 1
    }
}

# ─── Install ─────────────────────────────────────────────────────────────────

Write-Host "Installing Kleos v$Version ($Suffix)"
Write-Host "  Binaries: $($BinList -join ', ')"
Write-Host "  Target:   $InstallDir"
Write-Host ""

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

$BaseUrl = "https://github.com/$Repo/releases/download/v$Version"
$Failed = @()

foreach ($bin in $BinList) {
    $url = "$BaseUrl/$bin-$Suffix.exe"
    $dest = Join-Path $InstallDir "$bin.exe"

    Write-Host -NoNewline ("  {0,-20}" -f $bin)
    try {
        Invoke-WebRequest -Uri $url -OutFile $dest -UseBasicParsing
        Write-Host "OK"
    }
    catch {
        Write-Host "FAILED"
        $Failed += $bin
    }
}

Write-Host ""

if ($Failed.Count -gt 0) {
    Write-Warning "Failed to download: $($Failed -join ', ')"
    Write-Warning "Check https://github.com/$Repo/releases/tag/v$Version"
}

# Agent-host profile: drop sample supervisor config if none exists
if ($Profile -in "agent-host", "agent", "full") {
    $SupervisorDir = Join-Path $HOME ".config\eidolon"
    $SupervisorConfig = Join-Path $SupervisorDir "supervisor.json"
    if (-not (Test-Path $SupervisorConfig)) {
        New-Item -ItemType Directory -Force -Path $SupervisorDir | Out-Null
        @'
[
  {"id":"no-force-push","check_type":"rule_match","pattern":"git\\s+push\\s+.*--force","severity":"critical","cooldown_secs":300,"message":"Force push detected"},
  {"id":"no-reboot","check_type":"rule_match","pattern":"reboot|shutdown|systemctl\\s+(reboot|poweroff)","severity":"critical","cooldown_secs":600,"message":"Reboot or shutdown command detected"},
  {"id":"retry-loop","check_type":"retry_loop","pattern":"","severity":"warning","cooldown_secs":120,"message":"Agent stuck in retry loop (3+ identical failing commands)"}
]
'@ | Set-Content $SupervisorConfig
        Write-Host "  Dropped sample config: $SupervisorConfig"
    }
    Write-Host ""
}

# Check if install dir is on PATH
$currentPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($currentPath -notlike "*$InstallDir*") {
    Write-Host "Add to PATH (run once):"
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', `"$InstallDir;`$env:Path`", 'User')"
    Write-Host ""
}

$VerifyCmd = if ($Profile -eq "server") { "kleos-server --version" } else { "kleos-cli --version" }
Write-Host "Done. Verify with: $VerifyCmd"
