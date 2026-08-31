# Error Reference

This document lists every `ContractError` variant across the three core contracts — **AssetRegistry**, **EngineerRegistry**, and **Lifecycle** — together with the **Lending** contract. For each code you'll find the numeric value, when it is raised, and how to resolve it.

---

## Table of Contents

- [AssetRegistry](#assetregistry)
- [EngineerRegistry](#engineerregistry)
- [Lifecycle](#lifecycle)
- [Lending](#lending)

---

## AssetRegistry

| Code | Variant | Description |
|------|---------|-------------|
| 1 | `AssetNotFound` | No asset exists with the given ID. |
| 2 | `DuplicateAsset` | The same owner attempted to register an asset with identical metadata or the same serial number already exists in the registry. |
| 3 | `UnauthorizedAdmin` | The caller is not the stored admin. |
| 4 | `UnauthorizedOwner` | The caller is not the asset's owner (or admin) for a write operation. |
| 5 | `NotInitialized` | The contract has not been initialized. |
| 6 | `AdminAlreadyInitialized` | `initialize` was called a second time. |
| 7 | `Paused` | The contract is paused; all mutating calls are blocked. |
| 8 | `InvalidAssetType` | The supplied `asset_type` symbol is not in the admin-managed allowlist. |
| 9 | `PendingAdminAlreadyExists` | `propose_admin` was called while a previous proposal is still pending. |
| 10 | `TypeInUse` | The admin attempted to remove an asset type that still has registered assets. |
| 11 | `EmptyMetadata` | The metadata or serial-number string supplied to a registration call was empty. |
| 12 | `SameOwner` | A transfer was requested where the new owner equals the current owner. |
| 13 | `TimelockNotExpired` | A timelocked operation was executed before the 48-hour delay elapsed. |
| 14 | `ProposalNotFound` | No active (non-executed) timelock proposal exists for the requested operation and asset. |
| 15 | `AssetDecommissioned` | The asset has been decommissioned and the requested operation is not permitted on decommissioned assets. |
| 16 | `ProposalAlreadyExists` | A pending deregister proposal already exists for this asset; wait for it to expire or execute it first. |
| 17 | `AssetAlreadyDeprecated` | The asset has already been deprecated and cannot be deprecated again. |
| 18 | `BatchTooLarge` | The batch passed to `batch_register_assets` exceeds the maximum of 50 assets per call. |
| 30 | `SpecializationMismatch` | Engineer's specialization does not match the asset's type. Shares its discriminant with `Lifecycle`'s `SpecializationMismatch` so the two contracts decode this error consistently. |

### Resolution guidance

| Variant | How to resolve |
|---------|----------------|
| `AssetNotFound` | Verify the `asset_id` with `asset_exists` before calling any read or write function. |
| `DuplicateAsset` | Check the existing serial-number dedup index with `asset_exists` or query by serial number before registering. Each physical asset (identified by serial number) may only be registered once. |
| `UnauthorizedAdmin` | Ensure the signing address matches the current admin returned by `get_admin`. |
| `UnauthorizedOwner` | Confirm the caller owns the asset via `get_asset().owner` before mutating it. |
| `NotInitialized` | Call `initialize` once immediately after deployment, signed by the deployer. |
| `AdminAlreadyInitialized` | `initialize` has already been called. No action required; the contract is ready. |
| `Paused` | Wait for the admin to call `unpause`, or contact the admin if unexpected. |
| `InvalidAssetType` | Call `get_asset_types` to retrieve the current allowlist; use a type from that list or ask the admin to add the new type first. |
| `PendingAdminAlreadyExists` | Cancel or complete the existing admin transfer before proposing a new one. |
| `TypeInUse` | Deregister all assets of that type before the admin can remove it from the allowlist. |
| `EmptyMetadata` | Ensure both the `metadata` and `serial_number` fields are non-empty strings with lengths ≤ 256 and ≤ 64 characters respectively. |
| `SameOwner` | The destination address of a transfer must differ from the current owner. |
| `TimelockNotExpired` | Wait 48 hours after calling `propose_deregister_asset` (or the relevant global proposal) before calling the corresponding `execute_*` function. |
| `ProposalNotFound` | Call the appropriate `propose_*` function before calling `execute_*`. If a proposal was already executed, create a new one. |
| `AssetDecommissioned` | Decommissioned assets are write-protected. No further maintenance or ownership changes can be made to them. |
| `ProposalAlreadyExists` | Wait for the existing 48-hour window to pass and then call `execute_deregister_asset`, or allow the proposal to lapse before re-proposing. |
| `AssetAlreadyDeprecated` | Query the asset's `deprecation_status` field before calling deprecate. |
| `BatchTooLarge` | Split the batch into chunks of ≤ 50 assets per call. |
| `SpecializationMismatch` | Verify the engineer's specializations (via the engineer registry) include the asset's `asset_type` before submitting maintenance. |

---

## EngineerRegistry

| Code | Variant | Description |
|------|---------|-------------|
| 1 | `CredentialAlreadyRevoked` | The credential was already revoked before the current call. |
| 2 | `UnauthorizedAdmin` | The caller is not the stored admin. |
| 3 | `EngineerNotFound` | No engineer record exists for the given address. |
| 4 | `NotInitialized` | The contract has not been initialized (`initialize_admin` not yet called). |
| 5 | `AdminAlreadyInitialized` | `initialize_admin` was called a second time. |
| 6 | `UntrustedIssuer` | The issuer address is not in the trusted-issuers list. |
| 7 | `InvalidCredentialHash` | The supplied `credential_hash` is all-zero bytes (the null hash is disallowed). |
| 8 | `Paused` | The contract is paused; all mutating calls are blocked. |
| 9 | `CredentialRevoked` | The credential targeted by `renew_credential` is already revoked and cannot be renewed. |
| 10 | `EngineerAlreadyRegistered` | An active engineer record already exists; re-registration is only allowed after revocation. |
| 11 | `IssuerNotFound` | The issuer was not found in the trusted-issuers list during a lookup. |
| 12 | `PendingAdminAlreadyExists` | `propose_admin` was called while a previous proposal is still pending. |
| 13 | `InvalidValidityPeriod` | The supplied `validity_period` is 0 or below the minimum of 86,400 seconds (1 day). |
| 14 | `IssuerRemoved` | The issuer that originally registered the engineer has since been removed from the trusted list, blocking the renewal. |
| 15 | `TimelockNotExpired` | `execute_revoke_credential` was called before the 48-hour timelock elapsed. |
| 16 | `ProposalNotFound` | No active revocation proposal exists for the engineer, or it was already executed. |
| 17 | `CredentialSuspended` | The credential has been temporarily suspended. |
| 18 | `EngineerAlreadySuspended` | The engineer is already suspended and cannot be suspended again. |
| 19 | `InvalidSuspensionPeriod` | The supplied suspension period is invalid or out of range. |
| 20 | `BatchRevokeTooLarge` | The batch passed to a batch-revoke call exceeds the maximum of 50 engineers per call. |
| 21 | `CredentialExpired` | The engineer's credential has expired and cannot be used for maintenance submissions. |

### Resolution guidance

| Variant | How to resolve |
|---------|----------------|
| `CredentialAlreadyRevoked` | Call `get_engineer_status` to confirm current status before revoking. |
| `UnauthorizedAdmin` | Ensure the signing address matches `get_admin()`. |
| `EngineerNotFound` | Confirm the engineer's on-chain address with `get_engineer` before operating on the record. |
| `NotInitialized` | Call `initialize_admin` once immediately after deployment, signed by the deployer. |
| `AdminAlreadyInitialized` | Admin is already set; no action needed. |
| `UntrustedIssuer` | Only an address present in the trusted-issuers list (managed by the admin) can register engineers. Ask the admin to add the issuer via `add_trusted_issuer`. |
| `InvalidCredentialHash` | Supply the real SHA-256 hash of the credential document; ensure it is not the 32-byte zero array. |
| `Paused` | Wait for the admin to call `unpause`. |
| `CredentialRevoked` | A revoked credential cannot be renewed. Register a new credential for the engineer instead. |
| `EngineerAlreadyRegistered` | Check status with `get_engineer_status`; if `Active`, the engineer is already registered. |
| `IssuerNotFound` | The issuer lookup will only succeed for addresses added via `add_trusted_issuer`. |
| `PendingAdminAlreadyExists` | Complete or cancel the pending admin transfer before proposing a new one. |
| `InvalidValidityPeriod` | Use a validity period ≥ 86,400 seconds (1 day). |
| `IssuerRemoved` | The original issuer has been removed from the trusted list. The admin must re-add the issuer before renewal, or the engineer must be re-registered under a currently trusted issuer. |
| `TimelockNotExpired` | Call `propose_revoke_credential` first; then wait 48 hours before calling `execute_revoke_credential`. |
| `ProposalNotFound` | Call `propose_revoke_credential` before `execute_revoke_credential`. If already executed, create a new proposal. |
| `BatchRevokeTooLarge` | Split the batch into chunks of ≤ 50 engineers per call. |

---

## Lifecycle

| Code | Variant | Description |
|------|---------|-------------|
| 1 | `NoMaintenanceHistory` | The asset has no recorded maintenance entries. |
| 2 | `UnauthorizedEngineer` | The engineer failed the registry verification check (credential invalid, expired, or revoked). |
| 3 | `UnauthorizedAdmin` | The caller does not match the stored admin address. |
| 4 | `HistoryCapReached` | The asset's maintenance history has hit the configured `max_history` cap and the oldest entry could not be pruned. |
| 5 | `AssetNotFound` | The cross-contract call to AssetRegistry found no asset with the given ID. |
| 6 | `NotInitialized` | The contract has not been initialized or a required registry address is missing. |
| 7 | `AlreadyInitialized` | `initialize` was called a second time. |
| 8 | `InvalidConfig` | A configuration value failed validation (e.g. `score_increment` = 0, `max_history` > 10,000, or asset and engineer registries share the same address). |
| 9 | `Paused` | The contract is paused; all mutating calls are blocked. |
| 10 | `InvalidTaskType` | The `task_type` symbol is not recognized or not in the configured weights map. |
| 11 | `PendingAdminAlreadyExists` | `propose_admin` was called while a previous proposal is still pending. |
| 12 | `ZeroAddress` | The address supplied is the Stellar zero address and is not permitted. |
| 13 | `SameRegistryAddress` | The asset registry and engineer registry addresses supplied to `initialize` are identical. |
| 14 | `IndexOutOfBounds` | An index into an internal vector exceeded the valid range. |
| 15 | `UnauthorizedOwner` | The caller is not the asset's owner for an owner-gated operation (e.g. `authorize_engineer`). |
| 16 | `EngineerNotAuthorized` | The engineer has not been granted per-asset authorization by the asset owner via `authorize_engineer`. |
| 17 | `TimelockNotExpired` | An `execute_*` function was called before the 48-hour delay elapsed since the corresponding `propose_*` call. |
| 18 | `ProposalNotFound` | No active timelock proposal exists for the operation, or it was already executed. |
| 19 | `ScoreOverflow` | A score arithmetic operation would overflow a `u32`. |
| 20 | `NotesTooLong` | The `notes` field is empty or exceeds the configured `max_notes_length` (default 256 characters). |
| 21 | `ScoreFrozen` | The asset has been decommissioned; score decay and mutation are blocked. |
| 22 | `AssetDecommissioned` | The asset is decommissioned and cannot accept maintenance records. |
| 23 | `BatchTooLarge` | Batch submission exceeds the maximum allowed batch size (DoS / gas-limit guard). |
| 24 | `InsufficientSigners` | Fewer valid signers were provided than the configured `admin_threshold` requires. |
| 25 | `Reentrancy` | A reentrant call was detected: the contract is already executing `submit_maintenance`. |
| 26 | `DuplicateAdmin` | The admins list supplied to `set_admin_quorum` contains a duplicate address. |
| 27 | `SnapshotNotFound` | The requested health snapshot index does not exist for the given asset. |
| 28 | `InsufficientPredictionData` | Fewer than 2 matching task-type records exist; cannot compute a prediction. |
| 29 | `WeightProposalAlreadyExists` | A weight-change proposal already exists and has not been executed yet. |
| 30 | `SpecializationMismatch` | Engineer's specialization does not match the asset's type. |
| 31 | `RecurringTaskNotFound` | No recurring task exists with the given `task_id` for this asset. |
| 32 | `RecurringTaskInactive` | The recurring task exists but is not active. |
| 33 | `DuplicateRecurringTask` | A recurring task with an equivalent schedule already exists for this asset. |
| 34 | `InvalidRecurringSchedule` | The recurring task `interval_type`/`interval_value` combination is invalid. |
| 35 | `DuplicateRecordNotFound` | No duplicate maintenance record exists with the given timestamp. |
| 36 | `StandardAlreadyRegistered` | A compliance standard is already registered for this asset type. |
| 37 | `RateLimitExceeded` | Engineer has exceeded the configured `max_submissions_per_hour` rate limit. |
| 38 | `BatchRevokeTooLarge` | Bulk engineer authorization revocation exceeds the maximum allowed batch size. |

### Resolution guidance

| Variant | How to resolve |
|---------|----------------|
| `NoMaintenanceHistory` | Submit at least one maintenance record via `submit_maintenance` before querying history-dependent functions. |
| `UnauthorizedEngineer` | Verify the engineer's credential status with `EngineerRegistry.get_credential_status`; ensure the credential is `Valid` or `GracePeriod`. |
| `UnauthorizedAdmin` | Use the address returned by `get_config().admin` to sign admin calls. |
| `HistoryCapReached` | Increase `max_history` via `propose_config_update` + `execute_update_max_history`, or wait for the automated prune to remove older entries. |
| `AssetNotFound` | Verify the asset exists in AssetRegistry with `asset_exists(asset_id)` before calling lifecycle functions. |
| `NotInitialized` | Call `initialize` once after deployment with valid registry addresses and an admin address. |
| `AlreadyInitialized` | The contract is already set up; no further initialization is required. |
| `InvalidConfig` | Ensure `score_increment` > 0, `max_history` ≤ 10,000, and the two registry addresses are different. |
| `Paused` | Wait for the admin to call `unpause`. |
| `InvalidTaskType` | Call the function that returns the registered task types and use a symbol from that list, or ask the admin to add the new type. |
| `PendingAdminAlreadyExists` | Complete or cancel the existing admin transfer before proposing a new one. |
| `ZeroAddress` | Replace the zero address (`CAAAAAA…AAABSC4`) with a valid contract or account address. |
| `SameRegistryAddress` | Provide distinct addresses for the asset registry and the engineer registry parameters. |
| `IndexOutOfBounds` | This indicates an internal bug; report it. As a workaround, do not call the function with an index beyond the length of the relevant list. |
| `UnauthorizedOwner` | Confirm the signer is the current owner via `AssetRegistry.get_asset(asset_id).owner`. |
| `EngineerNotAuthorized` | The asset owner must call `authorize_engineer(owner, asset_id, engineer)` before the engineer can submit maintenance for that asset. |
| `TimelockNotExpired` | A proposal must be created with the corresponding `propose_*` function and then 48 hours must elapse before calling `execute_*`. |
| `ProposalNotFound` | Call the matching `propose_*` function before `execute_*`. Proposals that have already been executed cannot be reused; create a new one. |
| `ScoreOverflow` | This indicates accumulated scoring data has saturated a `u32`. Contact the admin to reset the score via `reset_score`. |
| `NotesTooLong` | Keep maintenance notes between 1 and `max_notes_length` characters (configurable, default 256). Empty strings are rejected. |
| `ScoreFrozen` | The asset was decommissioned in AssetRegistry. Scores and maintenance records for decommissioned assets are frozen and cannot be updated. |
| `AssetDecommissioned` | The asset has been decommissioned. No new maintenance records can be submitted for decommissioned assets. |
| `BatchTooLarge` | Split the batch into chunks of ≤ MAX_BATCH_SIZE records per call. |
| `InsufficientSigners` | Ensure the required number of co-signers from the admin quorum have signed the transaction. Check `admin_threshold` in the contract config. |
| `Reentrancy` | Do not call `submit_maintenance` recursively (e.g. via a callback triggered mid-execution). Wait for the outer call to complete. |
| `DuplicateAdmin` | Remove duplicate addresses from the list passed to `set_admin_quorum`; each admin address must appear exactly once. |
| `SnapshotNotFound` | Call `get_health_snapshots` to list valid indices before requesting a specific snapshot. |
| `InsufficientPredictionData` | Submit at least 2 maintenance records of the same `task_type` before requesting a prediction for that type. |
| `WeightProposalAlreadyExists` | Wait for the pending weight-change proposal to be executed or expire before proposing a new one. |
| `SpecializationMismatch` | Verify the engineer's specializations (via the engineer registry) include the asset's `asset_type` before submitting maintenance. |
| `RecurringTaskNotFound` | Call `get_recurring_tasks(asset_id)` to confirm the `task_id` exists before referencing it. |
| `RecurringTaskInactive` | Reactivate the recurring task, or create a new one, before calling `execute_recurring_task`. |
| `DuplicateRecurringTask` | Query existing recurring tasks for the asset before adding a new one with the same schedule. |
| `InvalidRecurringSchedule` | Ensure `interval_type` is a supported unit and `interval_value` is a positive integer within the allowed range. |
| `DuplicateRecordNotFound` | Verify the timestamp against `get_maintenance_history` before calling the duplicate-flagging function. |
| `StandardAlreadyRegistered` | Query the asset type's registered compliance standards before attempting to register a new one. |
| `RateLimitExceeded` | Wait until the current hourly window resets, or reduce submission frequency to stay within `max_submissions_per_hour`. |
| `BatchRevokeTooLarge` | Split the batch into chunks within the maximum allowed batch-revoke size. |

---

## Lending

| Code | Variant | Description |
|------|---------|-------------|
| 1 | `LoanAlreadyActive` | The borrower already has an active loan that has not been repaid or defaulted. |
| 2 | `NoActiveLoan` | No active loan exists for the borrower, or the loan's status is not `Active`. |
| 3 | `DuplicateVouch` | The voucher already vouched for this borrower, or the borrower attempted to vouch for themselves. |
| 4 | `ZeroStake` | The stake supplied to `vouch` is 0. |
| 5 | `NotInitialized` | The contract has not been initialized. |
| 6 | `AlreadyInitialized` | `initialize` was called a second time. |
| 7 | `UnauthorizedAdmin` | The caller does not match the stored admin address. |
| 8 | `InsufficientFunds` | The contract's token balance is too low to disburse the loan or cover the total yield payout. |
| 9 | `StakeBelowMinimum` | The stake is below the minimum of 50 stroops. Below this threshold the yield formula truncates to zero, so the call is rejected early. |
| 10 | `StakeSummationOverflow` | Pre-calculating total yield for all vouchers would overflow an `i128`. |
| 11 | `InvalidAdminAddress` | The admin address provided to `initialize` is the zero address. |
| 12 | `InvalidTokenAddress` | The token address provided to `initialize` is the zero address. |
| 13 | `ContractPaused` | The contract is paused; all mutating calls are blocked. |
| 14 | `TooManyVouchers` | The borrower's loan already has 100 vouchers (the DoS protection cap). |
| 15 | `VouchWithdrawNotAllowed` | The voucher attempted to withdraw their stake while an active loan is in progress. |
| 16 | `UnauthorizedBorrower` | The caller is not the authorized borrower for this loan. |

### Known bugs

- `record_lien` and `release_lien` in `contracts/lending/src/lib.rs` panic with `ContractError::LienAlreadyExists` and `ContractError::LienNotFound` respectively, but neither variant is defined in the `ContractError` enum above. This is a pre-existing latent bug (undefined-variant reference, not a duplicate discriminant) that will surface as a compile error the first time those code paths are exercised in a build. Tracked for a follow-up fix.

### Resolution guidance

| Variant | How to resolve |
|---------|----------------|
| `LoanAlreadyActive` | Check the current loan status with `get_loan(borrower)` before requesting a new one. A new loan can only be requested after the existing one is repaid or marked defaulted. |
| `NoActiveLoan` | Confirm an active loan exists via `get_loan(borrower)` before calling `repay` or `slash`. |
| `DuplicateVouch` | Query `get_vouches(borrower)` to check whether the voucher already appears in the list. Vouchers may not vouch twice for the same borrower; borrowers may not vouch for themselves. |
| `ZeroStake` | Supply a stake value > 0. The minimum is 50 stroops (`StakeBelowMinimum` will fire for values between 1 and 49). |
| `NotInitialized` | Call `initialize` once immediately after deployment, signed by the deployer. |
| `AlreadyInitialized` | The contract is already set up; no further initialization is required. |
| `UnauthorizedAdmin` | Use the address returned by `get_admin()` to sign admin operations. |
| `InsufficientFunds` | Fund the contract with enough tokens before disbursing a loan or verify that accumulated yield is covered before calling `repay`. |
| `StakeBelowMinimum` | Use a stake ≥ 50 stroops. The current configurable minimum can be read from `MIN_STAKE_KEY` storage. |
| `StakeSummationOverflow` | Reduce the number or size of vouches so that the total yield calculation fits within an `i128`. In practice this limit is extremely unlikely to be hit. |
| `InvalidAdminAddress` | Provide a non-zero admin address to `initialize`. |
| `InvalidTokenAddress` | Provide a non-zero token contract address to `initialize`. |
| `ContractPaused` | Wait for the admin to call `unpause`. |
| `TooManyVouchers` | A single loan accepts at most 100 vouchers. Split the vouching pool across multiple borrowers or reduce the voucher count. |
| `VouchWithdrawNotAllowed` | Vouchers can only withdraw their stake when no active loan exists for the borrower. Wait for the loan to be repaid or defaulted. |
| `UnauthorizedBorrower` | Only the borrower on record for the loan may call loan-mutating functions (e.g. `repay`). |

---

## Common patterns across all contracts

### Timelock flow (`TimelockNotExpired` / `ProposalNotFound`)

All four contracts use a two-step, 48-hour timelock for sensitive operations:

1. Call `propose_*` — records the proposal with `proposed_at = ledger.timestamp()`.
2. Wait at least 48 hours (172,800 seconds).
3. Call `execute_*` — verifies the delay has passed and marks the proposal as executed.

Calling `execute_*` before step 2 raises `TimelockNotExpired`. Calling it without a matching `propose_*` (or after a proposal was already executed) raises `ProposalNotFound`.

### Admin transfer (`PendingAdminAlreadyExists`)

Admin transfers follow a two-step accept pattern:

1. Current admin calls `propose_admin(admin, new_admin)`.
2. New admin calls `accept_admin()` to complete the transfer.

Only one pending proposal may exist at a time. Calling `propose_admin` again before the pending proposal is accepted raises `PendingAdminAlreadyExists`.

### Pause / unpause (`Paused` / `ContractPaused`)

Contracts can be paused by the admin as an emergency measure. All state-mutating functions check the pause flag at entry and reject with `Paused` (or `ContractPaused` in Lending) if set. Read-only view functions are unaffected. To resume operations the admin calls `unpause`.
