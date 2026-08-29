# Contract Upgrade Testing

This document describes how Mainstay validates that contract upgrades are safe, compatible with existing on-chain state, and recoverable from failures.

## Overview

Contract upgrades on Soroban replace the WASM bytecode of an existing contract while preserving its storage. Because storage layouts, event topics, and authorization patterns must remain compatible across versions, every upgrade must be tested before deployment.

The Mainstay CI pipeline includes a dedicated **`upgrade-test`** job that automates end-to-end upgrade testing.

## CI Pipeline

The `upgrade-test` job in `.github/workflows/ci.yml` runs after the main `test` job and executes the following stages:

### Stage 1: Build & Deploy Current Version
The current branch's contracts are compiled to WASM and deployed to a local Stellar test ledger (provided by the `stellar/quickstart` Docker image).

### Stage 2: Bootstrap Test Data
All three contracts (AssetRegistry, EngineerRegistry, Lifecycle) are:
- Initialized with admin addresses
- Seeded with test assets, engineer credentials, and maintenance records
- Configured with cross-contract bindings

### Stage 3: Snapshot Pre-Upgrade State
Contract state is read via `get_config`, `get_admin`, and similar view functions to establish a baseline.

### Stage 4: Simulate Upgrade
- Contracts are rebuilt (simulating a "new" version)
- The new WASM is installed into the local ledger via `stellar contract install`
- Upgrade proposals are submitted on all three contracts via `propose_upgrade`
- This exercises the upgrade proposal path and verifies storage is not corrupted by the proposal

### Stage 5: Storage Migration Validation
Dedicated tests (`test_storage_migration`) verify:
- **Storage key persistence**: All keys are readable after a simulated upgrade
- **Data integrity**: Values stored before an upgrade are intact after
- **Cross-contract binding**: Lifecycle→AssetRegistry and Lifecycle→EngineerRegistry references survive
- **Counter consistency**: Monotonic counters are not reset
- **Maintenance history**: Historical records persist unchanged
- **Edge cases**: Empty maintenance history, expired credentials

### Stage 6: Signature Compatibility Tests
Dedicated tests (`test_signature_compatibility`) verify:
- **Credential status consistency**: Valid → GracePeriod → HardExpired → Revoked → Suspended transitions produce consistent results
- **EngineerAuth enforcement**: Unregistered and unauthorized engineers are rejected for maintenance submissions
- **Issuer auth enforcement**: Only the original issuer can revoke or renew credentials
- **Batch verification**: `batch_verify_engineers` returns correct results across upgrade boundaries
- **Event topic consistency**: `reg_eng`, `REV_CRED`, `ADM_AUD` topics remain stable for indexers

### Stage 7: Error Recovery Tests
Dedicated tests (`test_upgrade_error_recovery`) verify:
- **Double-initialization protection**: Calling `initialize` twice panics with `AlreadyInitialized`
- **Timelock enforcement**: `execute_upgrade` before 48-hour delay panics with `TimelockNotExpired`
- **Unauthorized proposals**: Non-admin upgrade proposals are rejected
- **Paused contract blocking**: Upgrades are rejected while the contract is paused
- **Data integrity after failure**: All storage is intact after a failed upgrade attempt
- **Proposal lifecycle**: Already-executed and duplicate proposals are rejected
- **Cross-contract recovery**: Lifecycle contract continues functioning after a dependency's upgrade failure

### Stage 8: Post-Upgrade Data Integrity
After all tests pass, view functions are called on the deployed test ledger contracts to confirm state is still readable.

## Test File Reference

| Test File | Purpose |
|-----------|---------|
| `tests/test_storage_migration.rs` | Storage layout persistence and migration validation |
| `tests/test_signature_compatibility.rs` | Engineer credential verification compatibility |
| `tests/test_upgrade_error_recovery.rs` | Failed upgrade recovery and error handling |
| `tests/test_paused_contract.rs` | Paused state behavior across all contracts |
| `tests/test_emergency_pause.rs` | Emergency pause and unpause transitions |
| `tests/test_engineer_credential_expiry.rs` | Credential expiry state machine |
| `tests/test_full_lifecycle_e2e.rs` | End-to-end lifecycle flow |
| `tests/test_multi_asset_collateral.rs` | Multi-asset collateral scoring and lending |

## Running Locally

To run all upgrade-related tests locally:

```bash
# Build contracts
./scripts/build.sh

# Run the full test suite
cargo test --workspace

# Run only upgrade-related tests
cargo test --workspace -- test_storage_migration
cargo test --workspace -- test_signature_compatibility
cargo test --workspace -- test_upgrade_error_recovery
cargo test --workspace -- test_paused_contract
cargo test --workspace -- test_emergency_pause
cargo test --workspace -- test_engineer_credential_expiry
```

To run the full CI upgrade-test flow locally (requires Docker):

```bash
# Start a local Stellar test ledger
docker run -d --name stellar-local -p 8000:8000 stellar/quickstart:latest --local --enable-soroban-rpc

# Build and deploy
./scripts/build.sh

# Run contract deployment and upgrade simulation
# (see .github/workflows/ci.yml for the full script)
```

## Upgrade Workflow (Production)

For production upgrades, refer to `docs/deployment-runbook.md`. The key steps are:

1. Build the new contract WASM: `./scripts/build.sh`
2. Install the WASM on the network: `stellar contract install --wasm <path>`
3. Propose the upgrade via the contract: `stellar contract invoke --id <contract_id> -- propose_upgrade`
4. Wait for the timelock (48 hours)
5. Execute the upgrade: `stellar contract invoke --id <contract_id> -- execute_upgrade`
6. Verify data integrity: call view functions to confirm state is intact

### Safety Properties

- **Timelock**: All upgrades have a mandatory 48-hour delay, enforced on-chain
- **Admin-only**: Only the contract admin can propose and execute upgrades
- **Pause guard**: Upgrades are blocked while the contract is paused
- **Immutable storage**: Contract storage is never cleared during an upgrade — only the WASM is replaced
- **Cross-contract independence**: A failed upgrade on one contract does not affect the others

## Related Documentation

- [Deployment Runbook](deployment-runbook.md) — Production deployment procedures
- [Architecture Overview](architecture.md) — Contract architecture and cross-contract calls
- [TTL Strategy](ttl-strategy.md) — Storage expiry management
- [Error Reference](error-reference.md) — Complete error code reference
