# Builds the Rust workspace and installs minisamp.asi into the GTA San Andreas directory.
# Usage: ./scripts/deploy.ps1 [-GameDir <path>]
param([string]$GameDir = $env:GTA_SA_DIR)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

if (-not $GameDir) { throw 'Pass -GameDir or set GTA_SA_DIR to the folder containing gta_sa.exe' }
if (Get-Process gta_sa -ErrorAction SilentlyContinue) { throw 'gta_sa.exe is running; close it first (the .asi is locked while loaded)' }

Push-Location $root
try {
    # cargo reports progress on stderr; Windows PowerShell would turn that into a terminating error.
    $ErrorActionPreference = 'Continue'
    cargo build --release -p server -p minisamp-client
    $ErrorActionPreference = 'Stop'
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
} finally { Pop-Location }

Copy-Item (Join-Path $root 'target/i686-pc-windows-msvc/release/minisamp.dll') (Join-Path $GameDir 'minisamp.asi') -Force
Write-Host "installed $(Join-Path $GameDir 'minisamp.asi')"
Write-Host "server: $(Join-Path $root 'target/i686-pc-windows-msvc/release/server.exe')"
