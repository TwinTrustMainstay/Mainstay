# Complete Event Reference Guide

> **Last updated:** 2026-08-28 · **Doc version:** 0.1.0

This reference guide provides a comprehensive overview of all smart contract events emitted by the Mainstay protocol on the Stellar network (Soroban). It serves as an authoritative guide for off-chain indexers, analytics engines, subgraphs, and lender integration services.

---

## Table of Contents

1. [Soroban Event Architecture](#soroban-event-architecture)
2. [AssetRegistry Contract Events](#assetregistry-contract-events)
3. [EngineerRegistry Contract Events](#engineerregistry-contract-events)
4. [Lifecycle Contract Events](#lifecycle-contract-events)
5. [Lending Contract Events](#lending-contract-events)
6. [Bridge Contract Events](#bridge-contract-events)
7. [Insurance Pool Contract Events](#insurance-pool-contract-events)
8. [Parsing Event Data](#parsing-event-data)
   - [TypeScript Parsing Example](#typescript-parsing-example)
   - [Rust Event Parsing Example](#rust-event-parsing-example)
9. [Subscription & Indexing Strategies](#subscription--indexing-strategies)

---

## Soroban Event Architecture

Mainstay contracts emit events using Soroban's `env.events().publish(topics, data)` system. 

An event consists of two key components:
- **Topics (`Vec<Val>`)**: Indexed header attributes used by Soroban RPC nodes for quick filtering. Topics contain fixed symbols and key entity identifiers (such as `asset_id`, `engineer` Address, or `borrower` Address).
- **Data (`Val`)**: The event payload, encoded as a primitive value, tuple, or custom Soroban struct containing detailed context.

---

## AssetRegistry Contract Events

The `AssetRegistry` contract manages equipment registration, ownership transfers, deprecation, and deregistration timelocks.

### 1. `REG` — Asset Registered

Emitted when a new physical or industrial asset is successfully registered on-chain.

- **Topics**: `(Symbol("REG"), asset_id: u64)`
- **Data**: `(owner: Address, asset_type: Symbol, serial_number: String, timestamp: u64)`
- **Emission Conditions**: Called inside `register_asset` upon valid input verification and non-duplicate serial number validation.
- **Use Cases**: Off-chain indexers create new asset entities; lenders update collateral availability feeds.

### 2. `DEPR` — Asset Deprecated

Emitted when an asset owner voluntarily marks an asset as deprecated/retired.

- **Topics**: `(Symbol("DEPR"), asset_id: u64)`
- **Data**: `(owner: Address, reason: String, timestamp: u64)`
- **Emission Conditions**: Called inside `deprecate_asset` by the asset owner.
- **Use Cases**: Triggers automatic collateral score zeroing on indexers; notifies active lenders of asset retirement.

### 3. `XFER` — Asset Transferred

Emitted when an asset owner transfers ownership to a new address.

- **Topics**: `(Symbol("XFER"), asset_id: u64)`
- **Data**: `(old_owner: Address, new_owner: Address, timestamp: u64)`
- **Emission Conditions**: Called inside `transfer_asset`.
- **Use Cases**: Updates asset ownership registries; verifies collateral title during loan origination or liquidation.

### 4. `PROP_DEREG` — Deregistration Proposed

Emitted when an admin or owner initiates the 48-hour timelock to deregister an asset.

- **Topics**: `(Symbol("PROP_DEREG"), asset_id: u64)`
- **Data**: `(proposer: Address, execution_timestamp: u64)`
- **Emission Conditions**: Called inside `propose_deregister_asset`.
- **Use Cases**: Alert lenders of pending collateral removal; start timelock monitoring.

### 5. `EXEC_DEREG` — Deregistration Executed

Emitted when the 48-hour timelock expires and deregistration is finalized.

- **Topics**: `(Symbol("EXEC_DEREG"), asset_id: u64)`
- **Data**: `(executor: Address, timestamp: u64)`
- **Emission Conditions**: Called inside `execute_deregister_asset`.
- **Use Cases**: Remove asset from active indexer indices; mark collateral as purged.

### 6. `CAN_DEREG` — Deregistration Cancelled

Emitted when a proposed deregistration is cancelled before execution.

- **Topics**: `(Symbol("CAN_DEREG"), asset_id: u64)`
- **Data**: `(canceller: Address, timestamp: u64)`
- **Emission Conditions**: Called inside `cancel_deregister_asset`.
- **Use Cases**: Reset pending removal alerts; restore standard monitoring.

### 7. Administrative Events (`INIT`, `PAUSED`, `UNPAUSED`, `PROP_ADM`, `ADM_SET`)

- **Topics**: 
  - `(Symbol("INIT"), admin: Address)`
  - `(Symbol("PAUSED"), admin: Address)`
  - `(Symbol("UNPAUSED"), admin: Address)`
  - `(Symbol("PROP_ADM"), current_admin: Address)`
  - `(Symbol("ADM_SET"), new_admin: Address)`
- **Data**: `(timestamp: u64)` or `(pending_admin: Address)`
- **Emission Conditions**: Emitted during system initialization, pause/unpause toggles, and two-step admin transfers.
- **Use Cases**: System security auditing and protocol state monitoring.

---

## EngineerRegistry Contract Events

The `EngineerRegistry` contract manages licensed maintenance engineers, their specializations, and authorized identity issuers.

### 1. `ENG_REG` — Engineer Registered

Emitted when a certified engineer is registered by an authorized issuer.

- **Topics**: `(Symbol("ENG_REG"), engineer: Address)`
- **Data**: `(license_number: String, expiration_date: u64)`
- **Emission Conditions**: Called inside `register_engineer`.
- **Use Cases**: Populate engineer directory; schedule license expiration tracking.

### 2. `ENG_UPD` — Engineer Credentials Updated

Emitted when an engineer's license details or expiration date are updated.

- **Topics**: `(Symbol("ENG_UPD"), engineer: Address)`
- **Data**: `(new_license_number: String, new_expiration_date: u64)`
- **Emission Conditions**: Called inside `update_engineer`.
- **Use Cases**: Maintain up-to-date credential status in off-chain databases.

### 3. `ENG_DEACT` / `ENG_REACT` — Engineer Status Changed

Emitted when an engineer is deactivated or reactivated.

- **Topics**: `(Symbol("ENG_DEACT"), engineer: Address)` / `(Symbol("ENG_REACT"), engineer: Address)`
- **Data**: `(actor: Address, reason: String)`
- **Emission Conditions**: Called inside `deactivate_engineer` / `reactivate_engineer`.
- **Use Cases**: Restrict or re-enable engineer maintenance submission privileges on frontends.

### 4. `SPEC_ADD` / `SPEC_RM` — Specialization Modified

Emitted when a specialization (e.g., `"TURBINE_CERT"`, `"ELECTRICAL"`) is assigned or removed.

- **Topics**: `(Symbol("SPEC_ADD"), engineer: Address)` / `(Symbol("SPEC_RM"), engineer: Address)`
- **Data**: `(specialization: Symbol,)`
- **Emission Conditions**: Called inside `add_specialization` / `remove_specialization`.
- **Use Cases**: Filter engineers by qualifications for scheduled maintenance jobs.

### 5. `ISS_ADD` / `ISS_RM` — Identity Issuer Modified

Emitted when an admin grants or revokes issuer status.

- **Topics**: `(Symbol("ISS_ADD"), admin: Address)` / `(Symbol("ISS_RM"), admin: Address)`
- **Data**: `(issuer: Address,)`
- **Emission Conditions**: Called inside `add_issuer` / `remove_issuer`.
- **Use Cases**: Track protocol access control changes.

---

## Lifecycle Contract Events

The `Lifecycle` contract handles equipment maintenance records, collateral scoring formulas, score decay, and record retention.

### 1. `MAINT_SUB` — Maintenance Record Submitted

Emitted whenever a certified engineer logs a completed maintenance task for an asset.

- **Topics**: `(Symbol("MAINT_SUB"), asset_id: u64)`
- **Data**: `(engineer: Address, task_type: Symbol, score_impact: u32, timestamp: u64)`
- **Emission Conditions**: Called inside `submit_maintenance` after verifying engineer credentials and asset status.
- **Use Cases**: Recompute off-chain collateral score trends; audit maintenance history log.

### 2. `SCORE_UPD` — Collateral Score Recalculated

Emitted when an asset's collateral score changes (either through new maintenance or lazy decay calculation).

- **Topics**: `(Symbol("SCORE_UPD"), asset_id: u64)`
- **Data**: `(old_score: u32, new_score: u32, recalculated_at: u64)`
- **Emission Conditions**: Emitted inside `get_collateral_score` or maintenance submission.
- **Use Cases**: Alert lenders when collateral score drops below minimum thresholds; trigger automated margin calls.

### 3. `CFG_UPD` & `TSK_WT` — Scoring Parameters Updated

Emitted when admin modifies scoring constants or task type weightings.

- **Topics**: `(Symbol("CFG_UPD"), param: Symbol)` / `(Symbol("TSK_WT"), task_type: Symbol)`
- **Data**: `(old_value: u32, new_value: u32)`
- **Emission Conditions**: Called in admin parameters update functions.
- **Use Cases**: Synchronize off-chain valuation models with contract state.

### 4. `PRUNED` — Maintenance Records Pruned

Emitted when expired maintenance records beyond the retention TTL are pruned.

- **Topics**: `(Symbol("PRUNED"), asset_id: u64)`
- **Data**: `(pruned_count: u32, timestamp: u64)`
- **Emission Conditions**: Called during automated or manual storage garbage collection.
- **Use Cases**: Maintain indexer sync with state storage limits.

### 5. `WT_PROP` — Weight Change Proposed

Emitted when an admin proposes a new task-type weight via the governance timelock.

- **Topics**: `(Symbol("WT_PROP"), task_type: Symbol)`
- **Data**: `(admin: Address, new_weight: u32, proposed_at: u64)`
- **Emission Conditions**: Called inside `propose_weight_change` after validation.
- **Use Cases**: Track pending governance proposals; alert off-chain systems of upcoming scoring changes.

### 6. `WT_EXEC` — Weight Change Executed

Emitted when a pending weight-change proposal is executed after the timelock expires.

- **Topics**: `(Symbol("WT_EXEC"), task_type: Symbol)`
- **Data**: `(admin: Address, new_weight: u32, executed_at: u64)`
- **Emission Conditions**: Called inside `execute_weight_change` after timelock verification.
- **Use Cases**: Synchronize off-chain scoring models with new task weights; audit governance execution.

### 7. `RECONSTR` — History Anchored to Snapshot

Emitted when maintenance history is anchored to a previously-recorded health snapshot.

- **Topics**: `(Symbol("RECONSTR"), asset_id: u64)`
- **Data**: `(snapshot_index: u32, score: u32, snapshot_timestamp: u64)`
- **Emission Conditions**: Called inside `anchor_history_to_snapshot` to mark snapshot as anchor point.
- **Use Cases**: Signal off-chain indexers that reconstructed history is available; validate collateral score continuity.

---

## Lending Contract Events

The `Lending` contract manages loan requests, voucher staking, lien attachments, repayment, and default liquidations.

### 1. `LOAN_REQ` — Loan Requested

Emitted when a borrower opens a new loan request.

- **Topics**: `(Symbol("LOAN_REQ"), borrower: Address)`
- **Data**: `(amount: u64, deadline: u64)`
- **Emission Conditions**: Called inside `request_loan`.
- **Use Cases**: Display active loan requests on lender dashboards.

### 2. `VOUCH` — Voucher Staked

Emitted when a voucher stakes collateral tokens to back a loan.

- **Topics**: `(Symbol("VOUCH"), borrower: Address)`
- **Data**: `(voucher: Address, stake_amount: u64)`
- **Emission Conditions**: Called inside `vouch`.
- **Use Cases**: Track total staked backing per loan; build voucher leaderboard.

### 3. `LIEN_REC` — Collateral Lien Recorded

Emitted when a lender's encumbrance/lien is officially attached to an asset.

- **Topics**: `(Symbol("LIEN_REC"), asset_id: u64)`
- **Data**: `(lender: Address, loan_id: u64, amount: u64)`
- **Emission Conditions**: Called inside `record_lien` by contract admin.
- **Use Cases**: Track encumbered asset collateral; prevent double-mortgaging of physical equipment.

### 4. `LIEN_REL` — Collateral Lien Released

Emitted when a lien is removed following loan repayment or agreement settlement.

- **Topics**: `(Symbol("LIEN_REL"), asset_id: u64)`
- **Data**: `(lender: Address, loan_id: u64)`
- **Emission Conditions**: Called inside `release_lien`.
- **Use Cases**: Unencumber asset; update borrower credit capacity.

### 5. `LOAN_REP` — Loan Repaid

Emitted when a loan is fully paid off with interest/yield.

- **Topics**: `(Symbol("LOAN_REP"), borrower: Address)`
- **Data**: `(borrower: Address, total_yield: u64)`
- **Emission Conditions**: Called inside `repay`.
- **Use Cases**: Trigger automatic lien release workflow; distribute voucher rewards.

### 6. `LOAN_SLASH` — Loan Defaulted / Slashed

Emitted when a defaulted loan is slashed by admin after missing deadline.

- **Topics**: `(Symbol("LOAN_SLASH"), borrower: Address)`
- **Data**: `(borrower: Address, slashed_amount: u64)`
- **Emission Conditions**: Called inside `slash`.
- **Use Cases**: Initiate collateral liquidation flow; penalize defaulting borrower rating.

---

## Bridge Contract Events

The Bridge contract emits the events below from `contracts/bridge`. Every event has exactly one topic: its symbol. The remaining values are the event data tuple, in the order shown. Soroban integer values retain their declared widths; token amounts are signed `i128` values in the token's smallest units.

| Event | Data payload (ordered fields and Soroban types) | Emission condition |
|---|---|---|
| `BR_OUT` | `(dest_chain: u32, nonce: u64, sender: Address, token: Address, amount: i128, dest_address: Bytes)` | `bridge_out` succeeds after validating the destination chain, token allowlist, amount and destination address, transferring the tokens into bridge custody, and incrementing that destination chain's nonce. |
| `BR_REL` | `(source_chain: u32, nonce: u64, recipient: Address, token: Address, amount: i128, approvers: Vec<Address>)` | A release receives enough distinct relayer attestations, satisfies the chain's minimum source confirmations and rolling outflow cap, and the bridge transfers the escrowed tokens to the recipient. `approvers` preserves attestation order. |
| `BR_ALERT` | `(source_chain: u32, window_outflow: i128, window_cap: i128)` | A successful inbound release causes the rolling outflow total to cross the configured alert threshold for that source chain. It is emitted only on the below-to-at/above threshold transition within the active window. |
| `BR_SWP_N` | `(swap_id: u64, sender: Address, hashlock: BytesN<32>, expiry: u64, amount: i128)` | `create_swap` succeeds after validating the swap parameters and locking the offered tokens in the bridge. |
| `BR_SWP_C` | `(swap_id: u64, preimage: Bytes)` | `claim_swap` receives a preimage whose SHA-256 hash matches the stored hashlock before expiry, marks the swap claimed, and transfers the tokens to the recipient. |
| `BR_SWP_R` | `(swap_id: u64, sender: Address, token: Address, amount: i128)` | `refund_swap` succeeds after expiry for a still-open swap, marks it refunded, and returns the escrowed tokens to the sender. |
| `BR_PAUSE` | `(admin: Address)` | The authorized admin calls `pause`; new bridge-out, release-attestation, and swap-creation operations are blocked. |
| `BR_RESUME` | `(admin: Address)` | The authorized admin calls `unpause` to resume operations. |
| `BR_RELAYS` | `(admin: Address, threshold: u32, relayers: Vec<Address>)` | The authorized admin successfully replaces the relayer set and required attestation threshold after validating uniqueness, size, and threshold bounds. |
| `BR_CH_ADD` | `(admin: Address, chain_id: u32, config: ChainConfig)` | The authorized admin registers a new non-reserved chain ID with a valid configuration. |
| `BR_CH_UPD` | `(admin: Address, chain_id: u32, config: ChainConfig)` | The authorized admin updates the configuration of an already registered chain. |
| `BR_CH_ENB` | `(admin: Address, chain_id: u32, enabled: bool)` | The authorized admin enables or disables a registered chain. |

### Bridge `ChainConfig` payload type

The `config` field in `BR_CH_ADD` and `BR_CH_UPD` is the contract's `ChainConfig` struct:

| Field | Type | Meaning |
|---|---|---|
| `name` | `Symbol` | Short chain label used by dashboards |
| `enabled` | `bool` | Whether transfers to/from the chain are enabled |
| `min_amount`, `max_amount` | `i128` | Inclusive transfer amount bounds |
| `required_confirmations` | `u32` | Minimum source-chain confirmations required from relayers |
| `window_secs` | `u64` | Rolling outflow window duration in seconds |
| `window_cap`, `alert_threshold` | `i128` | Maximum outflow and alert threshold for the window |
| `attestation_window_secs` | `u64` | Time allowed for a release to reach its attestation threshold |
| `address_len` | `u32` | Required destination-address byte length |

**Indexer guidance:** use the event symbol as the event discriminator and decode the data tuple according to the exact event row above. Do not expect `chain_id`, `nonce`, or `swap_id` to be additional topics; they are data fields for these Bridge events. Treat `BR_ALERT` as an alert signal rather than a release event, and use `BR_REL` as the authoritative successful-release notification.

## Insurance Pool Contract Events

The `InsurancePool` contract manages capital providers, coverage policies, and claim settlement. Each event uses the event name as its sole topic; the payload types below follow the contract's Soroban Rust definitions.

### 1. `ins_pay` — Insurance Claim Paid

Emitted after an admin approves a pending claim and the pool transfers the payout to the claimant. A partial payment still emits this event with the amount actually paid; any unpaid remainder is emitted separately as `ins_shrt`.

- **Topics**: `(Symbol("ins_pay"))`
- **Data**: `(claim_id: u64, payout: i128)`
- **Emission Conditions**: Emitted by `settle_claim` when `approve = true`, after the claim and pool statistics are updated. `payout` is the actual transfer amount and may be less than the requested claim amount.
- **Use Cases**: Reconcile claim payouts, update claimant balances, and calculate insurance loss ratios.

### 2. `ins_rej` — Insurance Claim Rejected

Emitted when an admin rejects a pending claim. The claim reservation is released and the rejected-claim counter is incremented.

- **Topics**: `(Symbol("ins_rej"))`
- **Data**: `claim_id: u64` (a scalar, not a tuple)
- **Emission Conditions**: Emitted by `settle_claim` when `approve = false`, after the claim status and pool statistics are saved.
- **Use Cases**: Track rejected claims, audit settlement decisions, and reconcile released reservations.

### 3. `ins_wdr` — Insurance Capital Withdrawn

Emitted after a provider's shares are burned and the provider receives their pro-rata withdrawal from unreserved pool assets.

- **Topics**: `(Symbol("ins_wdr"))`
- **Data**: `(provider: Address, amount: i128, shares: i128)`
- **Emission Conditions**: Emitted by `withdraw_capital` after the provider's share balance and pool totals are updated and the token transfer succeeds.
- **Use Cases**: Reconcile capital outflows, track provider withdrawals, and update pool liquidity analytics.

## Parsing Event Data

### TypeScript Parsing Example

Using `@stellar/stellar-sdk` and `SorobanRpc`:

```typescript
import {
  SorobanRpc,
  scValToNative,
  xdr
} from "@stellar/stellar-sdk";

const server = new SorobanRpc.Server("https://soroban-testnet.stellar.org");

async function parseContractEvents(contractId: string, startLedger: number) {
  const response = await server.getEvents({
    startLedger,
    filters: [
      {
        type: "contract",
        contractIds: [contractId],
      },
    ],
  });

  for (const event of response.events) {
    const topics = event.topic.map((t) => scValToNative(xdr.ScVal.fromXDR(t, "base64")));
    const data = scValToNative(xdr.ScVal.fromXDR(event.value, "base64"));
    
    const eventType = topics[0]; // e.g., "LIEN_REC" or "MAINT_SUB"

    switch (eventType) {
      case "LIEN_REC": {
        const assetId = topics[1];
        const [lender, loanId, amount] = data;
        console.log(`Lien Recorded -> Asset: ${assetId}, Lender: ${lender}, Loan ID: ${loanId}, Amount: ${amount}`);
        break;
      }
      case "MAINT_SUB": {
        const assetId = topics[1];
        const [engineer, taskType, scoreImpact, timestamp] = data;
        console.log(`Maintenance Logged -> Asset: ${assetId}, Engineer: ${engineer}, Task: ${taskType}, Impact: +${scoreImpact}`);
        break;
      }
      case "LOAN_SLASH": {
        const borrower = topics[1];
        const [borrowerAddr, slashedAmount] = data;
        console.warn(`DEFAULT ALERT -> Borrower ${borrower} defaulted! Slashed amount: ${slashedAmount}`);
        break;
      }
      default:
        console.log(`Event: ${eventType}`, topics, data);
    }
  }
}
```

---

### Rust Event Parsing Example

For Rust indexers or test suites inspecting `env.events().all()`:

```rust
use soroban_sdk::{Env, Symbol, SymbolShort, Val, Vec};

pub fn parse_lien_recorded_event(env: &Env, event_topics: Vec<Val>, event_data: Val) {
    let topic_symbol: Symbol = event_topics.get(0).unwrap().into_val(env);
    
    if topic_symbol == Symbol::new(env, "LIEN_REC") {
        let asset_id: u64 = event_topics.get(1).unwrap().into_val(env);
        
        // Data contains tuple: (Address, u64, u64)
        let (lender, loan_id, amount): (soroban_sdk::Address, u64, u64) = 
            event_data.into_val(env);

        println!("Recorded lien on asset {} for lender {:?}, loan {}, amount {}", 
            asset_id, lender, loan_id, amount);
    }
}
```

---

## Subscription & Indexing Strategies

### 1. RPC Polling (`getEvents`)
- Query `getEvents` every 5 seconds using `startLedger`.
- Maintain a local checkpoint table storing the last processed ledger sequence number.
- Handle re-orgs by indexing 6 ledgers behind current tip (`latestLedger - 6`).

### 2. Multi-Contract Filtering
To monitor the entire Mainstay ecosystem, configure topic-level filters for:
- `AssetRegistry` Contract ID
- `EngineerRegistry` Contract ID
- `Lifecycle` Contract ID
- `Lending` Contract ID

### 3. Event Deduplication
Always key off `(ledger, txHash, eventIndex)` to ensure idempotency in event consumers and relational database writes.
