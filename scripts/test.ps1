# Run Rust tests for the whole workspace (same as CI: cargo test --workspace).
# Usage:
#   .\scripts\test.ps1
#   .\scripts\test.ps1 -p lifecycle
#   .\scripts\test.ps1 -p lifecycle test_decay_score -- --nocapture

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $Root

$Cargo = $null
if (Get-Command cargo -ErrorAction SilentlyContinue) {
    $Cargo = "cargo"
} elseif (Test-Path "$env:USERPROFILE\.cargo\bin\cargo.exe") {
    $Cargo = "$env:USERPROFILE\.cargo\bin\cargo.exe"
}

if (-not $Cargo) {
    Write-Error "cargo not found. Install Rust from https://rustup.rs and ensure cargo is on PATH."
}

Write-Host "Running Mainstay tests (workspace) from $Root ..."
& $Cargo test --workspace @args
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Host "All tests passed."
