# examples/credential_management.ps1
# Credential rotation workflow for the Mainstay Engineer Registry.
# Equivalent to examples/credential_management.sh for Windows users.
#
# Prerequisites: Stellar CLI installed and available on PATH.
# Usage: .\examples\credential_management.ps1

param(
    [string]$Network   = "testnet",
    [string]$RpcUrl    = "https://soroban-testnet.stellar.org",
    [string]$Contract  = $env:CONTRACT_ENGINEER_REGISTRY,
    [string]$Admin     = $env:ADMIN_ADDRESS,
    [string]$Issuer    = $env:ISSUER_ADDRESS,
    [string]$Engineer  = $env:ENGINEER_ADDRESS,
    [string]$CredHash  = $env:CREDENTIAL_HASH,
    # Validity period in seconds (default: 1 year)
    [long]$ValidityPeriod = 31536000
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Invoke-Contract {
    param([string[]]$Args)
    stellar contract invoke `
        --network $Network `
        --rpc-url $RpcUrl `
        --id $Contract `
        -- @Args
}

# ── 1. Register engineer ──────────────────────────────────────────────────────
Write-Host "Registering engineer $Engineer ..."
Invoke-Contract register_engineer `
    --engineer  $Engineer `
    --credential_hash $CredHash `
    --issuer    $Issuer `
    --validity_period $ValidityPeriod

# ── 2. Verify credential is active ───────────────────────────────────────────
Write-Host "Verifying engineer credential ..."
$active = Invoke-Contract verify_engineer --engineer $Engineer
if ($active -ne "true") {
    Write-Error "Verification failed: engineer is not active."
}
Write-Host "Credential verified: $active"

# ── 3. Renew credential (rotation) ───────────────────────────────────────────
Write-Host "Renewing credential for $Engineer ..."
Invoke-Contract renew_credential `
    --engineer        $Engineer `
    --new_validity_period $ValidityPeriod

# ── 4. Confirm status after renewal ──────────────────────────────────────────
Write-Host "Checking engineer status after renewal ..."
$status = Invoke-Contract get_engineer_status --engineer $Engineer
Write-Host "Status: $status"

# ── 5. Revoke credential ──────────────────────────────────────────────────────
Write-Host "Revoking credential for $Engineer ..."
Invoke-Contract revoke_credential --engineer $Engineer

# ── 6. Confirm revocation ────────────────────────────────────────────────────
Write-Host "Confirming revocation ..."
$revoked = Invoke-Contract verify_engineer --engineer $Engineer
Write-Host "Active after revocation: $revoked"
if ($revoked -ne "false") {
    Write-Error "Revocation check failed: engineer should not be active."
}

Write-Host "Credential management workflow completed successfully."
