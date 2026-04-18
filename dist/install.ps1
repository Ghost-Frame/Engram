# Kleos installer for Windows -- downloads the correct binaries.
#
# Usage (PowerShell):
#   irm https://raw.githubusercontent.com/Ghost-Frame/Engram/main/dist/install.ps1 | iex
#
# Options (via environment variables):
#   $env:KLEOS_VERSION   -- version to install (default: latest)
#   $env:KLEOS_INSTALL   -- installation directory (default: ~\.kleos\bin)
#   $env:KLEOS_BINARIES  -- comma-separated list (default: kleos-server,kleos-cli,kleos-mcp)

$ErrorActionPreference = "Stop"

$Repo = "Ghost-Frame/Engram"
$Version = if ($env:KLEOS_VERSION) { $env:KLEOS_VERSION } else { "" }
$InstallDir = if ($env:KLEOS_INSTALL) { $env:KLEOS_INSTALL } else { Join-Path $HOME ".kleos\bin" }
$BinList = if ($env:KLEOS_BINARIES) { $env:KLEOS_BINARIES -split "," } else { @("kleos-server", "kleos-cli", "kleos-mcp") }
$Suffix = "windows-x64"

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

# Check if install dir is on PATH
$currentPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($currentPath -notlike "*$InstallDir*") {
    Write-Host "Add to PATH (run once):"
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', `"$InstallDir;`$env:Path`", 'User')"
    Write-Host ""
}

Write-Host "Done. Verify with: kleos-server --version"
