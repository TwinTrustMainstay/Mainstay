# Mainstay Deployment Runbook

This guide covers the deployment and initialization of Mainstay contracts on Stellar networks (Testnet, Mainnet).

Note: `scripts/deploy_testnet.sh` hard-requires `STELLAR_NETWORK=testnet` (from `.env`) and explicitly passes `--network testnet` to all Stellar CLI calls to prevent accidentally deploying to the wrong network.

## Prerequisites
- Stellar CLI installed and configured.
- A functional identity (`deployer`) with enough lumens.

## 0. Formal Security Audit Requirement
Mainstay handles real industrial asset records used as DeFi collateral. A formal Soroban security audit is required before Mainnet deployment.

### Recommended Audit Firms

See [docs/audit-report.md](audit-report.md) for the full list of SDF-vetted Soroban audit firms, including:
- **Certora** — Formal verification via *Certora Sunbeam* for Soroban WASM bytecode
- **OtterSec** — Premier Rust/WASM security ($36B+ TVL secured)
- **Veridise** — Audited Soroban Core; advanced static analysis via *AuditHub*
- **Runtime Verification**, **ChainSecurity**, **Halborn**, **Oak Security**, **Zellic**

SDF's **Soroban Security Audit Bank** may cover up to 100% of audit costs for eligible projects.

### Pre-Audit Checklist
- [ ] Finalize and freeze the contract codebase (tag a release candidate).
- [ ] Run full test suite with coverage: `./scripts/test.sh`.
- [ ] Run `cargo clippy` with all lints and resolve warnings.
- [ ] Run `cargo audit` to check dependency vulnerabilities.
- [ ] Complete internal threat modeling (STRIDE framework).
- [ ] Verify deployment runbook initialization on testnet.

### Required Actions
- Engage a Soroban-specialized audit firm (see `docs/audit-report.md` §Recommended Audit Firms).
- Address all audit findings before mainnet deployment.
- Publish the final audit report in `docs/audit-report.md`.
- Complete this deployment checklist after the audit is finished.

## 1. Build Contracts
Compile all contracts to optimized WASM:
```bash
./scripts/build.sh
```

## 2. Deploy & Bind Registries
Deploy contracts in order and store their IDs.

### 2.1 Asset Registry
```bash
stellar contract deploy --wasm target/wasm32-unknown-unknown/release/asset_registry.wasm --network testnet --source deployer
```
*Note the Asset Registry ID (AR_ID).*

### 2.2 Engineer Registry
```bash
stellar contract deploy --wasm target/wasm32-unknown-unknown/release/engineer_registry.wasm --network testnet --source deployer
```
*Note the Engineer Registry ID (ER_ID).*

### 2.3 Lifecycle Contract
```bash
stellar contract deploy --wasm target/wasm32-unknown-unknown/release/lifecycle.wasm --network testnet --source deployer
```
*Note the Lifecycle Contract ID (LC_ID).*

## 3. Initialization & TTL Setup

> **Security: deployer-only initialization**
> Each `initialize_admin` / `initialize` call now requires the `deployer` argument to sign the
> transaction. The `--source deployer` flag on the Stellar CLI satisfies this requirement.
> **Complete all three initialization steps in the same block as deployment** (or immediately
> after) to eliminate the window in which an observer could front-run initialization with their
> own address.

### 3.1 Initialize Asset Registry Admin
```bash
stellar contract invoke --id AR_ID --network testnet --source deployer -- initialize_admin \
  --deployer <DEPLOYER_ADDRESS> \
  --admin <ADMIN_ADDRESS>
```

### 3.2 Initialize Engineer Registry Admin
```bash
stellar contract invoke --id ER_ID --network testnet --source deployer -- initialize_admin \
  --deployer <DEPLOYER_ADDRESS> \
  --admin <ADMIN_ADDRESS>
```

### 3.3 Initialize Lifecycle Binding
Connect Lifecycle to AR and ER:
```bash
stellar contract invoke --id LC_ID --network testnet --source deployer -- initialize \
  --deployer <DEPLOYER_ADDRESS> \
  --asset_registry AR_ID \
  --engineer_registry ER_ID \
  --admin <ADMIN_ADDRESS> \
  --max_history 200
```

## 4. Post-Deployment Verification
Once initialized, verify the contract state and availability.

### 4.1 Verify Asset Registry
Confirm the registry is responsive and the admin is correctly set:
```bash
stellar contract invoke --id AR_ID --network testnet --source any -- get_admin
```

### 4.2 Verify Engineer Registry
Confirm the registry is responsive and the admin is correctly set:
```bash
stellar contract invoke --id ER_ID --network testnet --source any -- get_admin
```

### 4.3 Verify Lifecycle Binding
Confirm that Lifecycle can reach the Asset Registry (this triggers a cross-contract call):
```bash
# Attempt to get a non-existent asset; should return a contract error (not a panic)
stellar contract invoke --id LC_ID --network testnet --source any -- get_collateral_score --asset_id 999
```

## 5. Monitoring Recommendations
Mainstay contracts are critical for asset financing. Active monitoring is recommended.

### 5.1 Event Monitoring
Subscribe to contract events to track lifecycle transitions:
- `REG_AST`: Asset registration.
- `MAINT`: Maintenance record submissions.
- `DECAY`: Score decay updates.

### 5.2 Storage Expiration (TTL)
The project relies on **persistent storage** for all metadata and histories.

#### 5.2.1 Initial TTL Verification
Verify that the instance storage for all three contracts is extended past 30 days:
```bash
stellar contract storage extend --id LC_ID --network testnet --durability instance --ledgers-to-extend 518400
```

#### 5.2.2 Ongoing TTL Monitoring
If a contract remains inactive for long periods (near 30 days), persistent entries must be manually extended using the `stellar contract storage extend` command to prevent data loss.

Refer to [docs/ttl-strategy.md](ttl-strategy.md) for a full mapping of storage keys.

---

## 6. Testnet vs Mainnet Differences

### 6.1 Network Configuration

| Aspect | Testnet | Mainnet |
|---|---|---|
| `--network` flag | `testnet` | `mainnet` |
| RPC URL | `https://soroban-testnet.stellar.org` | `https://soroban-mainnet.stellar.org` (or your own node) |
| Lumens required | Funded via Friendbot (`stellar keys fund`) | Real XLM; obtain before deployment |
| Key management | Generated key (`stellar keys generate`) | Hardware wallet or multisig key ceremony |
| Deployment script | `./scripts/deploy_testnet.sh` | No equivalent script; use the manual steps in this runbook with `--network mainnet` |

### 6.2 Pre-Mainnet Gate: Security Audit

Mainnet deployment is gated by a completed formal security audit (see §0 above). Do **not** skip this step. Before proceeding:

- [ ] Audit firm has signed off on all findings.
- [ ] Audit report is published to `docs/audit-report.md`.
- [ ] All high- and critical-severity findings are resolved and verified.

### 6.3 Mainnet Build & Deploy Steps

Replace every `--network testnet` flag with `--network mainnet`. Do not use `./scripts/deploy_testnet.sh` — that script hard-rejects non-testnet networks.

```bash
# 1. Build (same as testnet)
./scripts/build.sh

# 2. Deploy Asset Registry
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/asset_registry.wasm \
  --network mainnet \
  --source deployer
# Save as AR_ID

# 3. Deploy Engineer Registry
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/engineer_registry.wasm \
  --network mainnet \
  --source deployer
# Save as ER_ID

# 4. Deploy Lifecycle (must come after AR and ER)
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/lifecycle.wasm \
  --network mainnet \
  --source deployer
# Save as LC_ID
```

**Deployment order is mandatory**: Lifecycle `initialize` requires both registry contract IDs, so asset-registry and engineer-registry must be deployed and their IDs noted before lifecycle is deployed.

### 6.4 Mainnet Initialization

Initialize all three contracts in the **same transaction block** as deployment to eliminate front-run risk on initialization.

```bash
# Initialize Asset Registry
stellar contract invoke --id AR_ID --network mainnet --source deployer -- initialize_admin \
  --deployer <DEPLOYER_ADDRESS> \
  --admin <ADMIN_ADDRESS>

# Initialize Engineer Registry
stellar contract invoke --id ER_ID --network mainnet --source deployer -- initialize_admin \
  --deployer <DEPLOYER_ADDRESS> \
  --admin <ADMIN_ADDRESS>

# Initialize Lifecycle (bind to registries)
stellar contract invoke --id LC_ID --network mainnet --source deployer -- initialize \
  --deployer <DEPLOYER_ADDRESS> \
  --asset_registry AR_ID \
  --engineer_registry ER_ID \
  --admin <ADMIN_ADDRESS> \
  --max_history 200
```

### 6.5 Post-Deploy Verification Checklist

Run these checks immediately after initialization. Do not hand off to operations until every item is confirmed.

**Registry checks:**
- [ ] `stellar contract invoke --id AR_ID --network mainnet --source any -- get_admin` returns the expected admin address.
- [ ] `stellar contract invoke --id ER_ID --network mainnet --source any -- get_admin` returns the expected admin address.

**Cross-contract binding check:**
- [ ] `stellar contract invoke --id LC_ID --network mainnet --source any -- get_collateral_score --asset_id 999` returns a contract error (`AssetNotFound`), not a panic or `NotInitialized` error. A `NotInitialized` error means the binding was not saved correctly.

**Config check:**
- [ ] `stellar contract invoke --id LC_ID --network mainnet --source any -- get_config` returns `max_history: 200` and the expected admin address.

**TTL extension:**
- [ ] Extend instance storage for all three contracts immediately after initialization:
  ```bash
  for ID in AR_ID ER_ID LC_ID; do
    stellar contract storage extend --id $ID --network mainnet --durability instance --ledgers-to-extend 518400
  done
  ```

**Smoke test (optional but recommended):**
- [ ] Register one asset type and one test asset via AR_ID.
- [ ] Register one engineer via ER_ID.
- [ ] Submit one maintenance record via LC_ID and confirm `get_collateral_score` returns a non-zero value.
- [ ] Remove/deregister the test data if the contract supports it, or note the test asset IDs for auditing.

### 6.6 Key Management Differences

On testnet, generated keys (`stellar keys generate`) are acceptable. On mainnet:

- Use a hardware wallet (Ledger) or a dedicated signing key stored in a secrets manager (e.g., HashiCorp Vault).
- The `deployer` identity should be a cold wallet used exclusively for deployment; transfer admin rights to a multisig account before handing off to operations.
- Store AR_ID, ER_ID, and LC_ID in a configuration management system (e.g., environment-specific `.env.mainnet`) immediately after deployment — these IDs cannot be recovered once lost without re-deployment.
