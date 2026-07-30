# Soroban Storage & TTL Strategy

Soroban persistent storage entries expire if their Time-To-Live (TTL) is not extended. To prevent silent data loss, Mainstay contracts follow a standardized TTL management approach.

## Storage Types

- **Instance Storage**: Used for shared contract configuration (admin address, trusted issuers, registry bindings, etc.). Instance storage TTL is **not** automatically extended on every call — it must be explicitly extended on every write to prevent the admin address and other critical config from expiring.
- **Persistent Storage**: Used for all asset-specific data, maintenance records, and scores. **Requires explicit extension** to ensure longevity.

## TTL Parameters

Mainstay uses a standardized 30-day extension policy:
- **Threshold**: 518,400 ledgers (~30 days at 5s/ledger)
- **Target**: 518,400 ledgers (~30 days)

## Contract Storage Keys

### 1. Asset Registry

| Key Pattern | Storage Type | TTL Extended? | Description |
| ----------- | ------------ | ------------- | ----------- |
| `(Symbol("ASSET"), id: u64)` | Persistent | ✅ Yes — every write | Full `Asset` record (metadata, owner, etc.) |
| `(Symbol("DEDUP"), owner: Address, hash: BytesN<32>)` | Persistent | ✅ Yes — every write | Mapping of unique metadata to active asset IDs |
| `(Symbol("SN_DEDUP"), hash: BytesN<32>)` | Persistent | ✅ Yes — every write | Serial-number deduplication guard → asset ID |
| `Symbol("A_COUNT")` | Persistent | ✅ Yes — every write | Global counter for total registered assets |
| `Symbol("PAUSED")` | Persistent | ✅ Yes — `pause` / `unpause` | Contract pause flag |
| `Symbol("ADMIN")` | Instance | ✅ Yes — via instance TTL | Admin address authorized for admin operations |
| `Symbol("PADMIN")` | Instance | ✅ Yes — via instance TTL | Pending admin address during 2-step transfer |
| `(Symbol("AST_TYPE"), asset_type: Symbol)` | Persistent | ✅ Yes — `add_asset_type` | Asset type allowlist entries |
| `(Symbol("AST_CNT"), asset_type: Symbol)` | Instance | ✅ Yes — via instance TTL | Per-type asset count (for `TypeInUse` guard) |
| `(Symbol("OWN_IDX"), owner: Address)` | Persistent | ✅ Yes — every write | Owner → `Vec<asset_id>` index |
| `(Symbol("TYP_IDX"), asset_type: Symbol)` | Persistent | ✅ Yes — every write | Asset type → `Vec<asset_id>` index |
| `(Symbol("AST_CATS"), asset_id: u64)` | Persistent | ✅ Yes — every write | Asset → `Vec<category_bytes>` category membership |
| `(Symbol("META_HIS"), asset_id: u64)` | Persistent | ✅ Yes — `update_metadata` | `Vec<MetadataHistoryEntry>` — metadata change log |
| `(Symbol("DECOMM"), asset_id: u64)` | Persistent | ✅ Yes — `decommission_asset` | Boolean decommission flag |
| `(Symbol("U_MAINT"), asset_id: u64)` | Persistent | ✅ Yes — `flag_for_maintenance` | Maintenance-required flag; removed on decommission |
| `(Symbol("DEP_RSN"), asset_id: u64)` | Persistent | ✅ Yes — `deprecate_asset` | Deprecation reason string |
| `(Symbol("TL_PROP"), op: Symbol, asset_id: u64)` | Persistent | ✅ Yes — `propose_*` | `TimelockProposal` for per-asset admin operations |
| `(Symbol("TL_GLOB"), op: Symbol)` | Persistent | ✅ Yes — `propose_upgrade` | `TimelockProposal` for global (upgrade) operations |
| `Symbol("PEND_UPG")` | Persistent | ✅ Yes — `propose_upgrade` | Pending WASM hash during upgrade timelock |
| `Symbol("LIFECYCLE")` | Instance | ✅ Yes — via instance TTL | Bound Lifecycle contract address |
| `Symbol("LEND_CTR")` | — | ⚠️ Dead code | Defined constant, never written or read. See [follow-up note](#follow-up-issues). |

---

### 2. Engineer Registry

| Key Pattern | Storage Type | TTL Extended? | Description |
| ----------- | ------------ | ------------- | ----------- |
| `(Symbol("ENG"), addr: Address)` | Persistent | ✅ Yes — every write | `Engineer` record (credential hash, active status, reputation, specializations) |
| `(Symbol("ISS_ENGS"), issuer: Address)` | Persistent | ✅ Yes — every write | Issuer → `Vec<engineer_address>` mapping |
| `Symbol("ENG_CNT")` | Persistent | ✅ Yes — `register_engineer` | Global engineer count counter |
| `Symbol("PAUSED")` | Persistent | ✅ Yes — `pause` / `unpause` | Contract pause flag |
| `Symbol("GRACE_P")` | Persistent | ✅ Yes — `set_grace_period` | Configurable credential grace period in seconds |
| `(Symbol("TL_RVK"), engineer: Address)` | Persistent | ✅ Yes — `propose_revoke_credential` | `TimelockProposal` for credential revocation |
| `(Symbol("TL_GLOB"), Symbol("UPGRADE"))` | Persistent | ✅ Yes — `propose_upgrade` | `TimelockProposal` for WASM upgrade |
| `Symbol("PEND_UPG")` | Persistent | ✅ Yes — `propose_upgrade` | Pending WASM hash during upgrade timelock |
| `(Symbol("TRUSTED"), issuer: Address)` | Instance | ✅ Yes — via instance TTL | Trusted issuer flag |
| `Symbol("ISS_LIST")` | Instance | ✅ Yes — via instance TTL | Authoritative list of all trusted issuer addresses |
| `Symbol("ADMIN")` | Instance | ✅ Yes — via instance TTL | Admin address authorized for trust management |
| `Symbol("PADMIN")` | Instance | ✅ Yes — via instance TTL | Pending admin address during 2-step transfer |
| `(Symbol("TRAIN"), engineer: Address)` | — | ⚠️ Dead code | Key helper defined, no `record_training` implementation exists. See [follow-up note](#follow-up-issues). |

---

### 3. Lifecycle Contract

All Lifecycle keys are stored in **persistent** storage. There is no instance storage in the Lifecycle contract — every key must be individually extended.

| Key Pattern | TTL Extended? | Extension Policy | Description |
| ----------- | ------------- | ---------------- | ----------- |
| `(Symbol("HIST"), asset_id: u64)` | ✅ Yes | Every `submit_maintenance` / `batch_submit_maintenance` write | `Vec<MaintenanceRecord>` of all verified events |
| `(Symbol("SCORE"), asset_id: u64)` | ✅ Yes | Every score write (`submit_maintenance`, `decay_score`, `get_collateral_score`) | Current accumulated collateral score (0–100) |
| `(Symbol("SCHIST"), asset_id: u64)` | ✅ Yes | Alongside every score write | `Vec<ScoreEntry>` snapshots — (timestamp, score) per maintenance event |
| `(Symbol("LUPD"), asset_id: u64)` | ✅ Yes | Every score write | Timestamp of the last maintenance submission or decay event |
| `(Symbol("XFER_HIST"), asset_id: u64)` | ✅ Yes | Every `record_transfer` | `Vec<TransferRecord>` — ownership transfer provenance log |
| `(Symbol("HLTH_SNP"), asset_id: u64)` | ✅ Yes | Every `take_health_snapshot` | `Vec<HealthSnapshot>` — cumulative health snapshot history |
| `(Symbol("ENG_AUTH"), asset_id: u64, engineer: Address)` | ✅ Yes | `authorize_engineer` | Owner-granted authorization flag for a specific (asset, engineer) pair |
| `(Symbol("ENG_HIST"), engineer: Address)` | ✅ Yes | Every `submit_maintenance` that records a new asset for the engineer | Engineer → `Vec<asset_id>` association list |
| `(Symbol("TL_PROP"), op: Symbol)` | ✅ Yes | `propose_*`; removed on execution or cancellation | `TimelockProposal` for admin configuration changes |
| `(Symbol("RVK_TL"), asset_id: u64, engineer: Address)` | ✅ Yes | `propose_revoke_engineer_auth`; removed on execution | Timelock proposal for revoking an engineer's per-asset authorization |
| `(Symbol("FROZEN"), asset_id: u64)` | ✅ Yes | `decommission_notify` | Flag indicating the asset's score has been frozen at decommission time |
| `(Symbol("FRZ_SCR"), asset_id: u64)` | ✅ Yes | `decommission_notify` | Score captured at decommission time — returned by `get_collateral_score` for frozen assets |
| `RecurringTasks(asset_id: u64)` | ✅ Yes | `add_recurring_task` / `execute_recurring_task` | `Vec<RecurringTask>` — scheduled maintenance task definitions |
| `DuplicateRecords(asset_id: u64)` | ✅ Yes | `flag_duplicate_record` | `Vec<u64>` — flagged duplicate maintenance record indices |
| `CollateralValuationHistory(asset_id: u64)` | ✅ Yes | Every collateral score write (via `valuation_history_push`) | `Vec<(timestamp, value)>` — time-series of collateral valuations |
| `Symbol("REGISTRY")` | ✅ Yes | Extended once at `initialize` | Linked Asset Registry contract address |
| `Symbol("ENG_REG")` | ✅ Yes | Extended once at `initialize` | Linked Engineer Registry contract address |
| `Symbol("CONFIG")` | ✅ Yes | `initialize` and every `update_config` | `Config` record (max history, decay rate/interval, eligibility threshold, task weights) |
| `Symbol("PAUSED")` | ✅ Yes | Every `pause` and `unpause` | Contract pause flag |
| `Symbol("PADMIN")` | ✅ Yes | `propose_admin`; removed on `accept_admin` | Pending admin address during 2-step transfer |

> **Note on `CollateralValuationHistory`**: This `DataKey` variant is used in `scoring.rs` and `lib.rs` but is absent from the `DataKey` enum definition in `types.rs`. The code currently relies on Soroban's XDR serialisation of the enum discriminant — this compiles but the missing variant is a correctness risk. See [follow-up issues](#follow-up-issues).

#### Expiry consequences — Lifecycle

| Key | If it expires |
| --- | ------------- |
| `HIST` | Maintenance history is lost. `get_collateral_score` and `decay_score` return 0. The asset loses all collateral eligibility. Historical audit trail is permanently destroyed. |
| `SCORE` | Stored accumulated score resets to 0 on next read. `get_collateral_score` falls back to `compute_decay` from history; if `HIST` is still alive the score can be recomputed, but write-back sets 0 as a starting point until `HIST` is processed. |
| `SCHIST` | Score trend history is wiped. `get_score_history` returns an empty vec. Lenders lose visibility into score trajectory but current eligibility is unaffected (it uses `HIST`). |
| `LUPD` | Last-update timestamp is lost. `apply_decay` treats `last_update` as 0 (epoch), causing the full elapsed time since epoch to be used for decay — potentially zeroing the score instantly on the next `decay_score` call. |
| `XFER_HIST` | Ownership transfer provenance log is wiped. `get_transfer_history` returns empty. No impact on current scoring, but the audit trail for DeFi lenders is lost. |
| `HLTH_SNP` | Health snapshot history is wiped. `get_health_snapshots` returns an empty vec. No impact on current score or eligibility. |
| `ENG_AUTH` | The engineer's authorization for that asset is silently revoked. Their next `submit_maintenance` call panics with `EngineerNotAuthorized`. The owner must re-call `authorize_engineer`. |
| `ENG_HIST` | The engineer's asset association list is lost. `get_engineer_assets` returns empty. No direct impact on maintenance submission. |
| `TL_PROP` | The pending timelock proposal is lost. Any queued admin config change is silently cancelled. The admin must re-propose from scratch after the TTL window passes. |
| `RVK_TL` | The revoke-engineer timelock proposal expires silently. The revocation is cancelled; `execute_revoke_engineer_auth` will panic with `ProposalNotFound`. The owner must re-propose. |
| `FROZEN` | The decommissioned asset no longer appears frozen. `get_collateral_score` falls through to live `compute_decay`, potentially returning a non-zero value that diverges from the score captured at decommission. Lending contracts using this score may see inconsistent data. |
| `FRZ_SCR` | The frozen score is lost. Frozen assets return 0 via `get_collateral_score` instead of the value captured at decommission. Any in-progress loans collateralized by this score may be under-collateralized. |
| `RecurringTasks` | All scheduled maintenance task definitions are lost. `get_recurring_tasks` returns empty; `execute_recurring_task` panics with `RecurringTaskNotFound`. |
| `DuplicateRecords` | The duplicate-record flag list is lost. Previously flagged duplicates appear clean. No direct impact on active maintenance submission. |
| `CollateralValuationHistory` | The collateral valuation time-series is wiped. `get_valuation_history` returns empty. No impact on current scoring, but historical valuation data for lenders is destroyed. |
| `REGISTRY` | All cross-contract calls to the asset registry panic with `NotInitialized`. `submit_maintenance`, `get_collateral_score`, and `is_collateral_eligible` are all blocked. The contract becomes inoperable until re-initialized (not possible — `initialize` is one-shot). |
| `ENG_REG` | All engineer credential checks panic with `NotInitialized`. `submit_maintenance` is blocked for all assets. |
| `CONFIG` | All operations that read config panic with `NotInitialized`. The contract becomes fully inoperable. |
| `PAUSED` | The pause flag silently expires as `false` (the `unwrap_or(false)` default). A contract that was deliberately paused during an incident will silently unpause, re-enabling all operations without admin action. **Critical safety hazard.** |
| `PADMIN` | The pending admin proposal disappears. The 2-step admin transfer must be restarted from `propose_admin`. No funds or access are lost, but the handover is cancelled. |

---

### 4. Lending Contract

All Lending Contract keys are stored in **persistent** storage. There is no instance storage. Every key is extended on every write using `extend_ttl(TTL_THRESHOLD, TTL_TARGET)`.

| Key Pattern | TTL Extended? | Extension Policy | Description |
| ----------- | ------------- | ---------------- | ----------- |
| `Symbol("ADMIN")` | ✅ Yes | `initialize` and every admin-transfer function | Admin address for the lending contract |
| `Symbol("TOKEN")` | ✅ Yes | `initialize` | Payment token contract address |
| `Symbol("CONFIG")` | ✅ Yes | `initialize` and `update_config` | `Config` record (yield BPS, slash BPS) |
| `Symbol("PAUSED")` | ✅ Yes | Every `pause` and `unpause` | Contract pause flag |
| `Symbol("SL_BAL")` | ✅ Yes | Whenever the slash balance changes | Accumulated slash balance in token units |
| `Symbol("SL_BPS")` | ✅ Yes | `set_slash_bps` | Slash basis points applied to voucher stakes |
| `Symbol("LOAN_DUR")` | ✅ Yes | `set_loan_duration` | Default loan duration in seconds |
| `Symbol("MIN_STK")` | ✅ Yes | `set_min_stake` | Minimum vouch stake in token units (stroops) |
| `Symbol("YIELD_BPS")` | ✅ Yes | `set_yield_bps` | Yield basis points applied to loan repayments |
| `(Symbol("LOAN"), borrower: Address)` | ✅ Yes | `request_loan`, `repay`, `auto_slash` | Active `Loan` record for a borrower |
| `(Symbol("BORR"), borrower: Address)` | ✅ Yes | `request_loan` and loan closure | Borrower credit history record |
| `(Symbol("VOUCHES"), borrower: Address)` | ✅ Yes | `vouch`, `unvouch`, loan closure | `Vec<Vouch>` — all active voucher stakes for a borrower |
| `(Symbol("V_HIST"), voucher: Address)` | ✅ Yes | `vouch` and every voucher settlement | `VoucherHistory` — running yield and slash totals |
| `(Symbol("DEF_TIME"), borrower: Address)` | ✅ Yes | `auto_slash` / `slash` | Timestamp when the borrower's loan was defaulted |
| `(DataKey::Liens, asset_id: u64)` | ✅ Yes | `record_lien`, `release_lien` | `Vec<LienRecord>` — active lien claims on an asset |
| `Symbol("L_COUNT")` | ❌ **No TTL extension** | `request_loan` | Monotonic loan ID counter. Written on every loan request but never TTL-extended. See [follow-up issues](#follow-up-issues). |
| `(Symbol("L_MAP"), loan_id: u64)` | ❌ **No TTL extension** | `request_loan` | Loan ID → borrower address lookup map. Written on every loan request but never TTL-extended. See [follow-up issues](#follow-up-issues). |

#### Expiry consequences — Lending Contract

| Key | If it expires |
| --- | ------------- |
| `ADMIN` | All admin-gated functions (`pause`, `unpause`, `set_*`, `withdraw_slash`) panic. The contract becomes permanently un-administrable with no recovery path. **Critical.** |
| `TOKEN` | All token transfer calls (repay, slash, withdraw) panic with `NotInitialized`. No loan can be repaid or defaulted. |
| `CONFIG` | All functions that read config panic. The contract becomes inoperable. |
| `PAUSED` | Same hazard as Lifecycle `PAUSED`: a deliberately paused contract silently unpauses. **Critical safety hazard.** |
| `SL_BAL` | Accumulated slash balance is lost. Previously slashed funds appear to not exist; `withdraw_slash` transfers 0 tokens to the admin. Slashed funds are effectively unrecoverable. |
| `SL_BPS` | Slash rate resets to 0 (default). Future defaults slash 0% of voucher stakes — vouchers bear no risk. |
| `LOAN_DUR` | Loan duration resets to 0, making all future loans immediately overdue. |
| `MIN_STK` | Minimum stake requirement resets to 0. Any vouch amount (including 0) is accepted. |
| `YIELD_BPS` | Yield rate resets to 0. Vouchers earn no yield on repaid loans. |
| `LOAN` | The active loan record is lost. The borrower cannot repay (no record to update) and the admin cannot default (no record to read). The vouched stake is effectively frozen. |
| `BORR` | The borrower credit history is lost. Future loan eligibility decisions lose historical context. No direct operational impact on active loans. |
| `VOUCHES` | All voucher records for the borrower are lost. On default or repayment, no vouchers can be settled, slashed, or rewarded. Voucher funds are unrecoverable. |
| `V_HIST` | The voucher's yield and slash history is lost. `get_voucher_history` returns zeroed totals. No impact on active operations, but historical reporting is destroyed. |
| `DEF_TIME` | The default timestamp is lost. Any logic depending on when a loan was defaulted will read 0 (epoch). No direct operational impact since the `LOAN` record already holds `status: Defaulted`. |
| `Liens` | All lien records for the asset are lost. `get_liens` returns empty. Lenders lose their on-chain claim evidence. In-progress loan decisions may treat the asset as unencumbered. **Critical for DeFi integrations.** |
| `L_COUNT` | The loan ID counter is lost. On the next `request_loan` the counter resets to 0, causing new loan IDs to collide with expired historical IDs. **Loan records from the current epoch may overwrite ghost entries.** |
| `L_MAP` | The loan ID → borrower mapping is lost. Any lookup by loan ID (e.g. for lien cross-referencing) returns nothing. Existing `LOAN` records keyed by borrower address are unaffected. |

> **Issue #756 — Pause state TTL**: `PAUSED_KEY` is stored in persistent storage. `pause` and `unpause` must call `extend_ttl(&PAUSED_KEY, TTL_THRESHOLD, TTL_TARGET)` after every write so the pause flag cannot silently expire while the contract is paused during an incident response.

---

## Follow-up Issues

The audit identified the following gaps that require separate issues:

| # | Contract | Key | Finding | Suggested Action |
|---|----------|-----|---------|-----------------|
| A | Lending | `L_COUNT` (`Symbol("L_COUNT")`) | Written on every `request_loan` with no `extend_ttl` call. If this key expires the loan ID counter resets to 0, causing new loan IDs to collide with historical IDs. | Add `extend_persistent_ttl(&env, &symbol_short!("L_COUNT"))` immediately after the `set` call in `request_loan`. |
| B | Lending | `L_MAP` (`(Symbol("L_MAP"), loan_id)`) | Written on every `request_loan` with no `extend_ttl` call. If these entries expire, loan-ID–to–borrower lookups silently return nothing. | Add `extend_persistent_ttl(&env, &(symbol_short!("L_MAP"), new_loan_id))` immediately after the `set` call in `request_loan`. |
| C | Lifecycle | `CollateralValuationHistory(asset_id)` | The `DataKey::CollateralValuationHistory` variant is used in `scoring.rs` and `lib.rs` but is **absent from the `DataKey` enum** in `types.rs`. The code compiles due to Soroban XDR semantics but the missing variant is a correctness and maintainability risk. | Add `CollateralValuationHistory(u64)` to the `DataKey` enum in `contracts/lifecycle/src/types.rs`. |
| D | Asset Registry | `LEND_CTR` (`Symbol("LEND_CTR")`) | Constant defined but never written or read anywhere in the contract. | Remove the dead constant or implement the intended `set_lending_contract` / `get_lending_contract` functions if the integration is planned. |
| E | Engineer Registry | `TRAIN` (`(Symbol("TRAIN"), engineer)`) | `training_key()` helper is defined and `TrainingRecord` struct exists, but no `record_training` or `get_training_history` contract function is implemented. Tests reference `client.record_training(...)` which suggests the feature was started but not completed. | Implement `record_training` and `get_training_history` with `extend_persistent_ttl` on write, or remove the dead key helper and struct if the feature is deferred. |

---

## Extension Logic

### Instance Storage

Instance storage holds the admin address and other critical configuration. If it expires, all admin-gated operations (`pause`, `unpause`, `propose_admin`, `accept_admin`, `upgrade`, `add_trusted_issuer`, `remove_trusted_issuer`) will panic with `NotInitialized`.

To prevent this, **every admin-mutating function** calls `env.storage().instance().extend_ttl(518400, 518400)` after its writes. This ensures the instance TTL is refreshed on every admin interaction, keeping it alive as long as the contract is actively administered.

Functions that extend instance TTL in **AssetRegistry**:
- `initialize_admin`
- `propose_admin`
- `accept_admin`
- `pause`
- `unpause`
- `upgrade`
- `set_lifecycle_contract`

Functions that extend instance TTL in **EngineerRegistry**:
- `initialize_admin`
- `propose_admin`
- `accept_admin`
- `pause`
- `unpause`
- `upgrade`
- `add_trusted_issuer`
- `remove_trusted_issuer`

### Persistent Storage — Pause Flag (Lending Contract)

The Lending Contract stores all data in persistent storage (no instance storage). The `PAUSED` key is extended on every `pause` and `unpause` call:

```rust
env.storage().persistent().extend_ttl(&PAUSED_KEY, TTL_THRESHOLD, TTL_TARGET);
```

Functions that extend `PAUSED_KEY` TTL in **LendingContract**:
- `pause`
- `unpause`

Without this extension, a contract paused during an incident could silently unpause when the persistent entry expires, defeating the safety mechanism.

### Persistent Storage

All `persistent` entries are extended upon every `set` operation using `extend_ttl(518400, 518400)`.

**Exception**: `L_COUNT` and `L_MAP` in the Lending contract are currently missing TTL extension calls (see follow-up issue A and B above).

### Manual Extension

Use the Soroban CLI to extend entries if they are near expiration but no write operations are expected:

```bash
stellar contract storage extend --id <CONTRACT_ID> \
  --key '<KEY_XDR>' \
  --durability persistent \
  --ledgers-to-extend 518400
```

## Why Instance TTL Matters

Instance storage is **not** automatically extended on every contract invocation. If the instance TTL expires:

- `get_admin` panics with `NotInitialized`, locking out all admin operations
- Trusted issuer lookups return empty, blocking engineer registration
- The contract becomes unrecoverable without re-deploying

The fix is to call `env.storage().instance().extend_ttl(518400, 518400)` in every function that writes to instance storage, ensuring the TTL is refreshed on every admin interaction.
