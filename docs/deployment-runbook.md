# Mainstay Deployment Runbook

This guide covers the deployment and initialization of Mainstay contracts on Stellar networks (Testnet, Mainnet).

## Prerequisites
- Stellar CLI installed and configured.
- A functional identity (`deployer`) with enough lumens.

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

### 3.1 Initialize Asset Registry Admin
```bash
stellar contract invoke --id AR_ID --network testnet --source deployer -- initialize_admin --admin <ADMIN_ADDRESS>
```

### 3.2 Initialize Engineer Registry Admin
```bash
stellar contract invoke --id ER_ID --network testnet --source deployer -- initialize_admin --admin <ADMIN_ADDRESS>
```

### 3.3 Initialize Lifecycle Binding
Connect Lifecycle to AR and ER:
```bash
stellar contract invoke --id LC_ID --network testnet --source deployer -- initialize \
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
