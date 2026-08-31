# Lender Integration Guide

This guide provides everything a lender or lending protocol needs to integrate with Mainstay for collateral verification, lien recording, and liquidation.

---

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Asset Search & Discovery](#asset-search--discovery)
3. [Collateral Scoring & Verification](#collateral-scoring--verification)
4. [Lien Recording & Release](#lien-recording--release)
5. [Loan Lifecycle: Request → Repay → Default](#loan-lifecycle-request--repay--default)
6. [Liquidation Flow](#liquidation-flow)
7. [Code Examples](#code-examples)
8. [Fee Structure & Cost Assumptions](#fee-structure--cost-assumptions)
9. [Security Checklist](#security-checklist)

---

## Architecture Overview

Mainstay is a suite of four Soroban smart contracts on the Stellar network:

```
┌──────────────────────────────────────────────────────────┐
│                      Mainstay                            │
│                                                          │
│  ┌──────────────┐  ┌──────────────────┐                 │
│  │ AssetRegistry│  │ EngineerRegistry │                 │
│  │              │  │                  │                 │
│  │ • Register   │  │ • Credentialing  │                 │
│  │ • Search     │  │ • Reputation     │                 │
│  │ • Transfer   │  │ • Verification   │                 │
│  └──────┬───────┘  └────────┬─────────┘                 │
│         │                   │                            │
│         └───────┬───────────┘                            │
│                 │                                        │
│         ┌───────▼────────┐    ┌──────────────┐          │
│         │   Lifecycle    │    │   Lending    │          │
│         │                │    │              │          │
│         │ • Maintenance  │    │ • Loans      │          │
│         │ • Collateral   │    │ • Vouching   │          │
│         │   Scoring      │    │ • Liens      │          │
│         │ • Score Decay  │    │ • Slashing   │          │
│         └────────────────┘    └──────────────┘          │
└──────────────────────────────────────────────────────────┘
```

**Your integration touches all four contracts** but the primary surface is `Lending` (loans + liens) and `Lifecycle` (collateral scores).

| Contract | Role in lending |
|---|---|
| `AssetRegistry` | Verify asset exists, check owner, search for eligible assets |
| `EngineerRegistry` | Verify engineer credentials, check reputation for score weighting |
| `Lifecycle` | Query collateral scores, check maintenance history |
| `Lending` | Record/release liens, manage loan lifecycle, handle defaults |

---

## Asset Search & Discovery

Use the Asset Registry's `search_assets` function to find assets eligible for lending.

### SearchFilter Structure

```rust
pub struct SearchFilter {
    pub asset_type: Option<Symbol>,       // e.g., "GENSET", "TURBINE"
    pub manufacturer: Option<String>,     // Case-sensitive substring match on metadata
    pub min_age_months: Option<u32>,      // Minimum age in months (1 month ≈ 30 days)
    pub max_age_months: Option<u32>,      // Maximum age in months
    pub sort: Option<SortOrder>,          // ByCollateralScore | ByMaintenanceDate
    pub lifecycle_contract: Option<Address>, // Required for ByCollateralScore sort
}
```

### SearchPage Response

```rust
pub struct SearchPage {
    pub assets: Vec<Asset>,  // Up to 100 assets
    pub total: u32,          // Full match count (may exceed 100)
}
```

### Common Search Patterns

**Find all GENSET assets sorted by collateral score:**
```json
{
  "asset_type": "GENSET",
  "sort": "ByCollateralScore",
  "lifecycle_contract": "<LC_ID>"
}
```

**Find recently registered assets by manufacturer:**
```json
{
  "manufacturer": "Caterpillar",
  "max_age_months": 6,
  "sort": "ByMaintenanceDate"
}
```

**Pagination**: If `total > assets.len()`, use a narrower filter or iterate by adjusting `min_age_months`/`max_age_months` to partition the result set.

---

## Collateral Scoring & Verification

### Score API

Call the Lifecycle contract to get the current collateral score:

```rust
// Returns u32 in range [0, 100]
fn get_collateral_score(env: Env, asset_id: u64) -> u32;
```

**Batch query** for multiple assets:
```rust
fn get_collateral_score_batch(env: Env, asset_ids: Vec<u64>) -> Vec<u32>;
```

### Score Interpretation

| Score | Meaning | Lending Decision |
|---|---|---|
| 0 | No maintenance history OR deprecated | **Reject** — no verifiable maintenance |
| 1–49 | History exists but below threshold | **Reject** — insufficient maintenance quality |
| 50–74 | Threshold met, moderate quality | **Accept** — standard collateral |
| 75–100 | Strong maintenance record | **Accept** — premium collateral |

> **Note:** This table applies to `get_collateral_score`. If you use `AssetRegistry::get_lifecycle_score` instead, see [`get_lifecycle_score` and the `NO_LIFECYCLE_HISTORY_SCORE` Sentinel](#get_lifecycle_score-and-the-no_lifecycle_history_score-sentinel) — it uses `u32::MAX` as a sentinel for "no history," not `0`.

### Verification Before Lending

Always verify these before issuing a loan:

1. **Asset exists and is active:**
   ```rust
   let asset = asset_registry.get_asset(&asset_id);
   // Check asset is not None
   ```

2. **Asset is not deprecated:**
   ```rust
   if asset.deprecation_status != Active { /* reject */ }
   ```

3. **Collateral score meets threshold:**
   ```rust
   let score = lifecycle.get_collateral_score(&asset_id);
   if score < 50 { /* reject */ }
   ```

4. **No existing liens above the asset's value:**
   ```rust
   let liens = lending.get_liens(&asset_id);
   let total_encumbrance: u64 = liens.iter().map(|l| l.amount).sum();
   if total_encumbrance + loan_amount > asset_value { /* reject */ }
   ```

5. **Asset owner matches the borrower:**
   ```rust
   if asset.owner != borrower { /* reject */ }
   ```

### Score Freshness

Scores decay over time. The decay is applied **lazily** — only when `get_collateral_score` is called. This means:

- **Always call `get_collateral_score` at decision time** — never cache a score from a previous block
- A score that was 55 yesterday might be 50 today if the decay interval boundary was crossed
- Gas cost is modest: the read is a cross-contract call but the decay computation is local

### `get_lifecycle_score` and the `NO_LIFECYCLE_HISTORY_SCORE` Sentinel

`AssetRegistry::get_lifecycle_score(asset_id, lifecycle_contract)` is a convenience wrapper that cross-calls the Lifecycle contract directly from the AssetRegistry, so a lender only needs to hold one contract reference. Unlike `get_collateral_score` (which returns a plain `u32` in `[0, 100]`), `get_lifecycle_score` returns a value from a **different codomain**: it uses `NO_LIFECYCLE_HISTORY_SCORE` (`u32::MAX`, i.e. `4294967295`) as a sentinel meaning *"this asset has never had a maintenance record submitted."*

**This is the single most important gotcha in this guide.** An integrator who does not check for the sentinel will observe a score of `4294967295` and may misread it as an extremely high — and therefore excellent — collateral score, when it actually means the opposite: **there is no verifiable maintenance history at all.**

| Return value | Meaning | Lending Decision |
|---|---|---|
| `NO_LIFECYCLE_HISTORY_SCORE` (`u32::MAX`) | No maintenance record has ever been submitted for this asset | **Reject** — treat identically to a score of 0, never as a high score |
| `0..=100` | A real collateral score, same semantics as `get_collateral_score` | Use the [Score Interpretation](#score-interpretation) table above |

#### Example handling code (Rust / soroban-sdk)

```rust
use asset_registry::NO_LIFECYCLE_HISTORY_SCORE;

let raw = asset_registry_client.get_lifecycle_score(&asset_id, &lifecycle_contract);

let score = match raw {
    NO_LIFECYCLE_HISTORY_SCORE => {
        // Sentinel — do NOT treat this as a valid score.
        return Err(LendingError::NoMaintenanceHistory);
    }
    valid_score => valid_score,
};

if score < 50 {
    return Err(LendingError::CollateralScoreTooLow);
}
```

#### Example handling code (TypeScript / Stellar SDK)

```typescript
const NO_LIFECYCLE_HISTORY_SCORE = 4294967295; // u32::MAX

const rawScore = await assetRegistryContract.get_lifecycle_score({
  asset_id: assetId,
  lifecycle_contract: lifecycleContractId,
});

if (rawScore === NO_LIFECYCLE_HISTORY_SCORE) {
  // Sentinel value — asset has no maintenance history. Reject, do not
  // interpret as a high score.
  throw new Error("Asset has no maintenance history; cannot use as collateral");
}

if (rawScore < 50) {
  throw new Error("Collateral score below minimum threshold");
}
```

> **Cross-reference:** See [Score Interpretation](#score-interpretation) above for the normal `0-100` score semantics used by `get_collateral_score`. Always check for `NO_LIFECYCLE_HISTORY_SCORE` **before** applying the threshold logic from that table when using `get_lifecycle_score` instead of `get_collateral_score`.

---

## Lien Recording & Release

Liens secure a lender's claim on a borrower's asset. The lending contract manages lien records through admin-gated functions.

### LienRecord Structure

```rust
pub struct LienRecord {
    pub lender: Address,   // The lender's address
    pub loan_id: u64,      // Unique loan identifier
    pub amount: u64,       // Lien amount in token units
}
```

### Recording a Lien

Only the contract admin can record liens. In practice, a lender integration would submit a signed request to the admin, who records the lien on-chain:

```rust
fn record_lien(
    env: Env,
    admin: Address,
    asset_id: u64,
    lender: Address,
    loan_id: u64,
    amount: u64,
);
```

**Constraints:**
- The `(lender, loan_id)` pair must be unique per asset
- Recording a duplicate lien panics with `LienAlreadyExists`
- Multiple lenders can have liens on the same asset

### Querying Liens

```rust
fn get_liens(env: Env, asset_id: u64) -> Vec<LienRecord>;
```

### Releasing a Lien

Release the lien after the loan is fully repaid:

```rust
fn release_lien(
    env: Env,
    admin: Address,
    asset_id: u64,
    lender: Address,
    loan_id: u64,
);
```

**Constraints:**
- The lien must exist (panics with `LienNotFound` otherwise)
- Only the contract admin can release liens

### Lien Lifecycle

```
Loan requested ──► record_lien(asset, lender, loan_id, amount)
                         │
                         ▼
              Loan is Active (lien in place)
                         │
              ┌──────────┴──────────┐
              ▼                     ▼
          Repaid               Defaulted
              │                     │
              ▼                     ▼
    release_lien(...)     Liquidation process
    (lien removed)        (lien may be released
                           after liquidation)
```

---

## Loan Lifecycle: Request → Repay → Default

The lending contract uses a voucher-based model. Borrowers request loans and vouchers stake tokens to back them.

### Request a Loan

```rust
fn request_loan(env: Env, borrower: Address, amount: u64);
```

- The contract transfers `amount` tokens to `borrower`
- A loan deadline is set: `current_time + loan_duration` (default: 30 days)
- Only one active loan per borrower
- Contract must have sufficient token balance

### Vouch for a Borrower

Lenders (or any token holder) can vouch for a borrower by staking tokens:

```rust
fn vouch(env: Env, borrower: Address, voucher: Address, stake: u64);
```

- Minimum stake: 50 stroops (configurable)
- Maximum 100 vouchers per loan (DoS protection)
- Voucher earns yield on repayment: `stake × yield_bps / 10_000` (default: 2% yield)
- Borrower cannot vouch for themselves

### Repay a Loan

```rust
fn repay(env: Env, borrower: Address);
```

- Borrower repays `loan.amount` plus yield to all vouchers
- Yield is computed as: `Σ (voucher.stake × 200 / 10_000)` for all vouchers
- Loan status changes from `Active` → `Repaid`

### Default (Slash)

If the loan is not repaid by the deadline:

```rust
fn slash(env: Env, admin: Address, borrower: Address);
```

- Admin marks the loan as `Defaulted`
- Voucher stakes are slashed at the configured rate (default: 50%)
- 50% returned to voucher, 50% accumulated in `slash_balance` for treasury
- Borrower's `default_count` is incremented

---

## Liquidation Flow

For assets that serve as collateral, liquidation occurs when a borrower defaults and the lender needs to recover value from the collateral.

### Step-by-step Liquidation

1. **Detect default**: Monitor loan deadlines or listen for the `loan_sls` event
2. **Verify lien exists**: Call `get_liens(asset_id)` to confirm the lien is recorded
3. **Initiate asset transfer**: Work with the contract admin to:
   - Release the lien: `release_lien(admin, asset_id, lender, loan_id)`
   - Transfer asset ownership: `transfer_asset(asset_id, owner, lender_address)` (on Asset Registry)
4. **Settle**: The lender now owns the asset and can sell, re-collateralize, or hold it

### Liquidation Checklist

- [ ] Loan is in `Defaulted` status
- [ ] Lien exists for this `(asset_id, lender, loan_id)` combination
- [ ] Asset's `deprecation_status` is `Active` (not already deprecated)
- [ ] No other liens take priority (FIFO or by agreement)
- [ ] Asset transfer is executed by or approved by the current owner

---

## Code Examples

### TypeScript (Stellar SDK)

```typescript
import {
  Contract,
  SorobanRpc,
  TransactionBuilder,
  Networks,
  BASE_FEE,
  nativeToScVal,
  scValToNative,
} from "@stellar/stellar-sdk";

const rpc = new SorobanRpc.Server("https://soroban-testnet.stellar.org");

// Contract IDs (replace with actual deployed addresses)
const ASSET_REGISTRY_ID = "CC...";
const LIFECYCLE_ID = "CD...";
const LENDING_ID = "CE...";

// ── Search for eligible assets ──────────────────────────────

async function findEligibleAssets(
  assetType: string
): Promise<Array<{ assetId: number; score: number }>> {
  const contract = new Contract(ASSET_REGISTRY_ID);

  const filter = {
    asset_type: assetType,
    manufacturer: null,
    min_age_months: null,
    max_age_months: null,
    sort: "ByCollateralScore",
    lifecycle_contract: LIFECYCLE_ID,
  };

  // Build and simulate the search
  const result = await simulateInvoke(contract, "search_assets", [
    nativeToScVal(filter, { type: "object" }),
  ]);

  const page = scValToNative(result);
  const eligible: Array<{ assetId: number; score: number }> = [];

  for (const asset of page.assets) {
    const score = await getCollateralScore(asset.asset_id);
    if (score >= 50) {
      eligible.push({ assetId: asset.asset_id, score });
    }
  }

  return eligible;
}

// ── Get collateral score ────────────────────────────────────

async function getCollateralScore(assetId: number): Promise<number> {
  const contract = new Contract(LIFECYCLE_ID);

  const result = await simulateInvoke(contract, "get_collateral_score", [
    nativeToScVal(assetId, { type: "u64" }),
  ]);

  return scValToNative(result) as number;
}

// ── Verify asset eligibility ─────────────────────────────────

async function verifyAssetForLending(
  assetId: number,
  borrowerAddress: string,
  loanAmount: bigint
): Promise<{ eligible: boolean; reason?: string }> {
  // 1. Asset exists
  const assetContract = new Contract(ASSET_REGISTRY_ID);
  const assetResult = await simulateInvoke(assetContract, "get_asset", [
    nativeToScVal(assetId, { type: "u64" }),
  ]);
  const asset = scValToNative(assetResult);

  if (!asset) {
    return { eligible: false, reason: "Asset not found" };
  }

  // 2. Asset is Active
  if (asset.deprecation_status !== 0) {
    // 0 = Active
    return { eligible: false, reason: "Asset is deprecated or decommissioned" };
  }

  // 3. Borrower is the owner
  if (asset.owner !== borrowerAddress) {
    return { eligible: false, reason: "Borrower is not the asset owner" };
  }

  // 4. Score meets threshold
  const score = await getCollateralScore(assetId);
  if (score < 50) {
    return {
      eligible: false,
      reason: `Collateral score too low: ${score} < 50`,
    };
  }

  // 5. Check existing liens
  const lendingContract = new Contract(LENDING_ID);
  const liensResult = await simulateInvoke(lendingContract, "get_liens", [
    nativeToScVal(assetId, { type: "u64" }),
  ]);
  const liens = scValToNative(liensResult);

  const totalEncumbered = liens.reduce(
    (sum: bigint, lien: any) => sum + BigInt(lien.amount),
    0n
  );

  // Asset value is determined off-chain (e.g., appraisal, oracle).
  // The on-chain Asset struct does not carry a `value` field.
  // Replace `assetValue` with your own valuation source.
  const assetValue = BigInt(0); // TODO: integrate your valuation oracle
  if (totalEncumbered + loanAmount > assetValue) {
    return {
      eligible: false,
      reason: `Insufficient collateral: ${totalEncumbered} already encumbered`,
    };
  }

  return { eligible: true };
}

// ── Helper: simulate a contract invoke ───────────────────────

async function simulateInvoke(
  contract: Contract,
  method: string,
  args: any[]
): Promise<any> {
  const source = await getSourceAccount(); // your keypair

  const tx = new TransactionBuilder(source, {
    fee: BASE_FEE,
    networkPassphrase: Networks.TESTNET,
  })
    .addOperation(contract.call(method, ...args))
    .setTimeout(30)
    .build();

  const sim = await rpc.simulateTransaction(tx);
  return sim.result.retval;
}
```

### Rust (soroban-sdk)

```rust
use soroban_sdk::{Address, Env, Symbol, Vec};

/// Full lending eligibility check for an asset.
pub fn verify_collateral(
    env: &Env,
    asset_registry_id: &Address,
    lifecycle_id: &Address,
    lending_id: &Address,
    asset_id: u64,
    borrower: &Address,
    loan_amount: u64,
) -> Result<(), &'static str> {
    // 1. Fetch the asset
    let asset_registry = AssetRegistryClient::new(env, asset_registry_id);
    let asset = asset_registry
        .try_get_asset(&asset_id)
        .map_err(|_| "Asset not found")?;

    // 2. Verify owner
    if asset.owner != *borrower {
        return Err("Borrower is not the asset owner");
    }

    // 3. Check deprecation status
    if asset.deprecation_status != DeprecationStatus::Active {
        return Err("Asset is deprecated or decommissioned");
    }

    // 4. Check collateral score
    let lifecycle = LifecycleClient::new(env, lifecycle_id);
    let score = lifecycle.get_collateral_score(&asset_id);

    if score < 50 {
        return Err("Collateral score below threshold");
    }

    // 5. Check existing liens
    let lending = LendingContractClient::new(env, lending_id);
    let liens = lending.get_liens(&asset_id);

    let total_encumbered: u64 = liens.iter().map(|l| l.amount).sum();
    if total_encumbered + loan_amount > asset_value {
        return Err("Insufficient unencumbered collateral");
    }

    Ok(())
}

/// Record a lien after issuing a loan (admin-only in practice).
pub fn place_lien(
    env: &Env,
    lending_id: &Address,
    admin: &Address,
    asset_id: u64,
    lender: &Address,
    loan_id: u64,
    amount: u64,
) {
    let lending = LendingContractClient::new(env, lending_id);
    lending.record_lien(admin, &asset_id, lender, &loan_id, &amount);
}

/// Release a lien after loan repayment.
pub fn remove_lien(
    env: &Env,
    lending_id: &Address,
    admin: &Address,
    asset_id: u64,
    lender: &Address,
    loan_id: u64,
) {
    let lending = LendingContractClient::new(env, lending_id);
    lending.release_lien(admin, &asset_id, lender, &loan_id);
}

/// Check if a borrower has defaulted.
pub fn check_loan_status(
    env: &Env,
    lending_id: &Address,
    borrower: &Address,
) -> Option<LoanStatus> {
    let lending = LendingContractClient::new(env, lending_id);
    lending.get_loan(borrower).map(|loan| loan.status)
}
```

---

## Fee Structure & Cost Assumptions

### Gas Costs (Stellar / Soroban)

| Operation | Estimated Cost | Notes |
|---|---|---|
| `get_collateral_score` | ~0.001 XLM | Includes cross-contract calls to AssetRegistry |
| `get_collateral_score_batch` (10 assets) | ~0.005 XLM | Scales linearly with batch size |
| `search_assets` | ~0.002–0.01 XLM | Varies with result set size; capped at 100 |
| `record_lien` | ~0.0005 XLM | Simple storage write |
| `release_lien` | ~0.0005 XLM | Storage write + potential removal |
| `request_loan` | ~0.001 XLM | Includes token transfer |
| `repay` | ~0.001–0.01 XLM | Scales with voucher count (max 100) |
| `vouch` | ~0.0005 XLM | Token transfer + storage write |

> Costs are estimates based on testnet benchmarks. Mainnet costs may vary with network congestion.

### Economic Parameters

| Parameter | Default | Description |
|---|---|---|
| `yield_bps` | 200 (2%) | Yield paid to vouchers on repayment |
| `slash_bps` | 5,000 (50%) | Percentage of stake slashed on default |
| `loan_duration` | 2,592,000 sec (30 days) | Default loan term |
| `min_vouch_stake` | 50 stroops | Minimum vouch amount |
| `max_vouchers` | 100 | Maximum vouchers per loan |

### Integration Cost Considerations

1. **No protocol fees**: Mainstay contracts do not charge protocol-level fees for loan operations
2. **Transaction fees**: All operations incur standard Stellar network fees (~100 stroops per operation)
3. **Cross-contract overhead**: `get_collateral_score` makes up to 2 cross-contract calls, each adding ~100 stroops
4. **Batch where possible**: Use `get_collateral_score_batch` instead of N individual calls when checking multiple assets

---

## Security Checklist for Lenders

### Before integrating

- [ ] **Audit the contracts**: Review the latest audit report in `docs/audit-report.md`
- [ ] **Verify contract IDs**: Confirm you're interacting with the correct, audited contract deployment
- [ ] **Understand pause risk**: Contracts can be paused by admin. Your integration should handle `ContractPaused` / `Paused` errors gracefully
- [ ] **Test on testnet first**: Run a full integration on Stellar testnet before mainnet

### Per-loan checks

- [ ] **Score freshness**: Always call `get_collateral_score` at decision time — never cache
- [ ] **Score floor handling**: Score `1` means "has history but fully decayed" — treat differently from `0` (no history)
- [ ] **Deprecation check**: Verify `deprecation_status == Active` before issuing a loan
- [ ] **Owner verification**: Confirm borrower == asset owner via `get_asset`
- [ ] **Lien deduplication**: Check for existing liens with `get_liens` before recording a new one
- [ ] **Encumbrance ratio**: Ensure `total_liens + new_loan ≤ collateral_value`
- [ ] **Loan duration**: Set appropriate deadlines; 30 days is the default

### Post-loan monitoring

- [ ] **Monitor loan deadlines**: Track `loan.deadline` and trigger collection/default processes
- [ ] **Watch for score decay**: Periodically re-check the collateral score; if it drops significantly, consider margin calls
- [ ] **Listen for events**: Subscribe to `loan_rep`, `loan_sls`, `PAUSED`, `UNPAUSED` events
- [ ] **TTL awareness**: If a contract is inactive for ~30 days, storage may expire. Coordinate with the admin to extend TTL

### Liquidation readiness

- [ ] **Asset transfer procedure**: Have a documented process for taking ownership of liquidated assets
- [ ] **Lien release procedure**: Ensure the admin can release your lien after liquidation settlement
- [ ] **Legal compliance**: Liquidation of physical industrial assets may have jurisdiction-specific requirements

### Key management

- [ ] **Lender address**: Use a dedicated lender address; do not reuse personal wallets
- [ ] **Signing authority**: Ensure your signing keys are secured (hardware wallet or HSM)
- [ ] **Multisig for large loans**: Consider requiring multiple signatures for high-value loan issuance

---

## Event Reference

Monitor these events for integration with off-chain systems:

| Event Topic | Contract | Emitted When | Data |
|---|---|---|---|
| `loan_req` | Lending | Loan requested | `(borrower, amount)` |
| `loan_rep` | Lending | Loan repaid | `(borrower, total_yield)` |
| `loan_sls` | Lending | Loan defaulted/slashed | `(borrower, slash_amount)` |
| `vouch_cr` | Lending | Voucher stakes tokens | `(voucher, borrower, stake)` |
| `PAUSED` | Lending | Contract paused | `(admin)` |
| `UNPAUSED` | Lending | Contract unpaused | `(admin)` |
| `INIT` | Lending | Contract initialized | `(admin, token)` |
| `MAINT` | Lifecycle | Maintenance submitted | `(asset_id, engineer, task_type, timestamp)` |
| `DECAY` | Lifecycle | Score decayed | `(asset_id, new_score)` |

---

## Quick Reference

| Need | Contract | Function |
|---|---|---|
| Find assets | AssetRegistry | `search_assets(filter)` |
| Verify asset exists | AssetRegistry | `get_asset(id)` |
| Check asset owner | AssetRegistry | `get_asset(id).owner` |
| Get collateral score | Lifecycle | `get_collateral_score(id)` |
| Batch scores | Lifecycle | `get_collateral_score_batch(ids)` |
| Check maintenance | Lifecycle | `get_maintenance_history(id)` |
| Record lien | Lending | `record_lien(admin, asset, lender, loan, amt)` |
| Check liens | Lending | `get_liens(asset_id)` |
| Release lien | Lending | `release_lien(admin, asset, lender, loan)` |
| Issue loan | Lending | `request_loan(borrower, amount)` |
| Check loan | Lending | `get_loan(borrower)` |
| Check pause state | Any | `is_paused()` |

---

*This guide is maintained alongside the Mainstay smart contract system. For implementation details, refer to the source code and [docs/architecture.md](architecture.md).*
