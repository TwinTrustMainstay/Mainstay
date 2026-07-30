# Collateral Lien Lifecycle Documentation

This guide provides a comprehensive technical walkthrough of collateral liens in the Mainstay protocol. It details how physical industrial assets registered on-chain are encumbered, monitored, released, or liquidated across the loan lifecycle.

---

## Table of Contents

1. [Overview & Core Concepts](#overview--core-concepts)
2. [Lien Data Architecture](#lien-data-architecture)
3. [Lien Lifecycle Stages & API Functions](#lien-lifecycle-stages--api-functions)
4. [Lifecycle Scenarios & Flow Diagrams](#lifecycle-scenarios--flow-diagrams)
   - [Scenario 1: Normal Loan Repayment](#scenario-1-normal-loan-repayment)
   - [Scenario 2: Loan Default & Voucher Slashing](#scenario-2-loan-default--voucher-slashing)
   - [Scenario 3: Liquidation & Asset Recovery](#scenario-3-liquidation--asset-recovery)
   - [Scenario 4: Asset Deprecation Under Lien](#scenario-4-asset-deprecation-under-lien)
5. [Lender Responsibilities & Risk Management](#lender-responsibilities--risk-management)
6. [Security & Governance Considerations](#security--governance-considerations)

---

## Overview & Core Concepts

In industrial DeFi financing, physical equipment (e.g., generators, turbines, heavy machinery) serves as real-world collateral for loans. A **collateral lien** represents a legal and smart-contract-enforced encumbrance against an asset registered in the Mainstay `AssetRegistry`.

When a lien is attached to an asset:
- The borrower's ability to deregister or clear the asset is restricted.
- Lenders secure priority claims on asset value.
- Maintenance compliance and collateral scores are continuously monitored via the `Lifecycle` contract.

---

## Lien Data Architecture

Liens are tracked directly in the `Lending` contract state.

### `LienRecord` Structure

```rust
pub struct LienRecord {
    pub lender: Address,   // Address of the lender holding the lien
    pub loan_id: u64,      // Unique identifier for the associated loan
    pub amount: u64,       // Encumbered value in token units (stroops)
}
```

### Key Properties
- **Multi-Lien Capability**: Multiple lenders can hold liens on a single asset up to the total asset appraisal value.
- **Uniqueness Constraint**: The tuple `(lender, loan_id)` must be unique per asset.
- **Admin Encumbrance Control**: To prevent malicious self-encumbrance by borrowers, lien creation and release functions are authorized by the protocol `admin`.

---

## Lien Lifecycle Stages & API Functions

### 1. Lien Creation (`record_lien`)

When a loan request is approved and issued, a lien is recorded against the borrower's asset.

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

- **Checks**:
  - `admin` must authorize the transaction.
  - `asset_id` must exist in `AssetRegistry` and be active (`deprecation_status == Active`).
  - No existing lien with identical `(lender, loan_id)` on `asset_id`.
- **Event Emitted**: `(LIEN_REC, asset_id)` -> `(lender, loan_id, amount)`

---

### 2. Lien Monitoring & Collateral Health

While the lien is active, lenders monitor asset condition via the `Lifecycle` contract:

```rust
let score = lifecycle.get_collateral_score(asset_id);
```

- Collateral scores range from `0` to `100`.
- Scores naturally decay over time if maintenance records are not periodically logged by certified engineers.
- Lenders set minimum score thresholds (e.g., `score >= 50`) in their credit agreements.

---

### 3. Lien Release (`release_lien`)

Upon full repayment of loan principal plus voucher yields, the lien is removed.

```rust
fn release_lien(
    env: Env,
    admin: Address,
    asset_id: u64,
    lender: Address,
    loan_id: u64,
);
```

- **Checks**:
  - `admin` must authorize the transaction.
  - The specified lien `(lender, loan_id)` must exist on `asset_id`.
- **Event Emitted**: `(LIEN_REL, asset_id)` -> `(lender, loan_id)`

---

## Lifecycle Scenarios & Flow Diagrams

### Scenario 1: Normal Loan Repayment

In the happy path, the borrower borrows against the asset, maintains the equipment, repays the loan before the deadline, and the lien is released.

```
 Borrowing Phase                 Active Loan Phase              Settlement Phase
┌──────────────┐                ┌─────────────────┐           ┌────────────────┐
│ Borrower     │                │ Engineer Logs   │           │ Borrower Repays│
│ Requests Loan│                │ Maintenance     │           │ Principal+Yield│
└──────┬───────┘                └────────┬────────┘           └───────┬────────┘
       │                                 │                            │
       ▼                                 ▼                            ▼
┌──────────────┐                ┌─────────────────┐           ┌────────────────┐
│ Admin Calls  │                │ Collateral Score│           │ Admin Calls    │
│ record_lien()│                │ Stays High      │           │ release_lien() │
└──────┬───────┘                └─────────────────┘           └───────┬────────┘
       │                                                              │
       ▼                                                              ▼
┌──────────────┐                                              ┌────────────────┐
│ Asset        │                                              │ Asset Unlocked │
│ Encumbered   │                                              │ Lien Cleared   │
└──────────────┘                                              └────────────────┘
```

#### Steps:
1. Borrower requests loan of `N` tokens.
2. Vouchers stake tokens to back the borrower.
3. Lender/Admin issues funds and executes `record_lien(admin, asset_id, lender, loan_id, amount)`.
4. Certified engineers submit regular maintenance (`submit_maintenance`), keeping collateral score high.
5. Borrower invokes `repay(borrower)`, paying principal and distributing yield to vouchers.
6. Admin executes `release_lien(admin, asset_id, lender, loan_id)`.

---

### Scenario 2: Loan Default & Voucher Slashing

If the loan deadline passes without full repayment, the loan enters default status.

```
 Loan Expiration                 Default Handling              Treasury Allocation
┌──────────────┐                ┌─────────────────┐           ┌────────────────┐
│ Loan Deadline│               │ Admin Triggers  │           │ Slashed Tokens │
│ Missed       ├──────────────►│ slash()         ├──────────►│ Sent to        │
└──────────────┘                └────────┬────────┘           │ Treasury       │
                                         │                    └────────────────┘
                                         ▼
                                ┌─────────────────┐
                                │ Voucher Stakes  │
                                │ Slashed by 50%  │
                                └─────────────────┘
```

#### Steps:
1. Loan reaches `deadline` while status is `Active`.
2. Admin or liquidator detects default state.
3. Admin invokes `slash(admin, borrower)` on `Lending` contract.
4. Slashed voucher tokens (e.g., 50%) are transferred to `slash_balance` for protocol recovery.
5. Borrower `default_count` is incremented.

---

### Scenario 3: Liquidation & Asset Recovery

When slashed voucher proceeds are insufficient to cover default losses, lenders enforce the collateral lien to seize physical/legal title of the asset.

```
 Default Confirmed               Title Transfer                Lien Discharge
┌──────────────┐                ┌─────────────────┐           ┌────────────────┐
│ Uncured      │                │ Admin Executes  │           │ Lien Released  │
│ Default      ├──────────────►│ transfer_asset()├──────────►│ After Seizure  │
└──────────────┘                │ (to Lender)     │           │ Complete       │
                                └─────────────────┘           └────────────────┘
```

#### Steps:
1. Lender verifies active lien record: `get_liens(asset_id)`.
2. Lender submits proof of default and legal claim to protocol governance/admin.
3. Admin calls `transfer_asset(asset_id, old_owner, lender_address)` in `AssetRegistry`.
4. Admin calls `release_lien(admin, asset_id, lender, loan_id)` to discharge the encumbrance after ownership transfer completes.
5. Lender takes ownership of physical equipment or sells it to recover capital.

---

### Scenario 4: Asset Deprecation Under Lien

If an asset owner attempts to deprecate an equipment asset while a lien is attached:

```
 Deprecation Request             Score Zeroed                  Lien Priority Intact
┌──────────────┐                ┌─────────────────┐           ┌────────────────┐
│ Owner Calls  │                │ Collateral Score│           │ Lien Remains   │
│ deprecate_   ├──────────────►│ Drops to 0      ├──────────►│ Active Until   │
│ asset()      │                │ Immediately     │           │ Loan Resolved  │
└──────────────┘                └─────────────────┘           └────────────────┘
```

#### Steps:
1. Asset owner invokes `deprecate_asset(owner, asset_id, reason)`.
2. `AssetRegistry` sets `deprecation_status = Deprecated`.
3. `Lifecycle.get_collateral_score(asset_id)` immediately evaluates to `0`.
4. **Crucially**, active liens on `Lending` contract **remain in force**. Deprecation does not erase existing liens.
5. Lenders receive an automated score alert (`SCORE_UPD` -> `0`) and can demand immediate loan acceleration or margin resolution.

---

## Lender Responsibilities & Risk Management

To protect capital and maintain protocol integrity, lenders must enforce the following integration practices:

### Pre-Origination Verification Checklist

- [ ] **Verify Asset Ownership**: Confirm `AssetRegistry.get_asset(asset_id).owner == borrower`.
- [ ] **Check Deprecation Status**: Ensure `deprecation_status == Active`.
- [ ] **Query Current Collateral Score**: Confirm `Lifecycle.get_collateral_score(asset_id) >= min_threshold`.
- [ ] **Audit Encumbrance Capacity**: Call `Lending.get_liens(asset_id)` to sum total existing encumbrance and ensure `(existing_liens + new_loan_amount) <= asset_appraisal_value`.
- [ ] **Validate Engineer Quality**: Inspect recent maintenance logs and verify engineer license validity in `EngineerRegistry`.

### Post-Origination Monitoring

1. **Lazy Score Decay Awareness**: Scores decay continuously. Lenders must query `get_collateral_score` on-chain at regular intervals rather than relying on stale cached values.
2. **Event Subscription**: Subscribe to `MAINT_SUB`, `SCORE_UPD`, `DEPR`, and `LOAN_SLASH` events for all encumbered asset IDs.
3. **Margin Call Triggers**: Establish clear loan covenants allowing margin calls if collateral score falls below credit agreement thresholds (e.g. score drops below 50).

---

## Security & Governance Considerations

1. **Admin Key Security**: Lien recording and releasing functions require `admin` authorization. Lenders must ensure admin functions are controlled by a multi-sig or governance timelock contract.
2. **Timelock Monitoring**: Deregistration requests (`propose_deregister_asset`) carry a mandatory 48-hour timelock. Lenders must monitor `PROP_DEREG` events to object to or block unauthorized deregistration attempts on encumbered assets.
3. **Idempotency & Nonce Checks**: Ensure loan IDs are unique across transactions to prevent duplicate lien recording panics.
