# Contract Interaction Sequence Diagrams

Detailed sequence and timing diagrams for the flows that span more than one
Mainstay contract. This document complements [architecture.md](architecture.md)
(contract responsibilities and storage) and
[lender-integration-guide.md](lender-integration-guide.md) (integration-facing
API reference) by focusing specifically on **who calls whom, in what order,
and what can go wrong along the way**.

---

## Table of Contents

1. [Contract Map](#contract-map)
2. [Asset Registration](#asset-registration)
3. [Engineer Credentialing](#engineer-credentialing)
4. [Maintenance Submission](#maintenance-submission)
5. [Batch Maintenance Submission](#batch-maintenance-submission)
6. [Ownership Transfer](#ownership-transfer)
7. [Decommissioning](#decommissioning)
8. [Collateral Score Query](#collateral-score-query)
9. [Collateral Usage (Lending Flow)](#collateral-usage-lending-flow)
10. [Error Scenario Reference](#error-scenario-reference)
11. [Contract Call Dependency Graph](#contract-call-dependency-graph)
12. [Timing Diagrams](#timing-diagrams)

---

## Contract Map

| Contract | Owns | Called by |
|---|---|---|
| `AssetRegistry` | Asset records, ownership, dedup, search, collateral locks | `Lifecycle` (read-only queries), the registered `lending_contract` address (`lock_asset_as_collateral` / `unlock_asset_from_collateral`), asset owners directly |
| `EngineerRegistry` | Engineer credentials, trusted issuers, reputation | `Lifecycle` (read-only queries), issuers/engineers directly |
| `Lifecycle` | Maintenance history, collateral score, decay | `AssetRegistry` (via `decommission_notify`, invoked automatically), engineers/owners directly |
| `Lending` | Loans, vouching, liens, slashing | Borrowers/vouchers/admin directly; not cross-called by the other three |

`Lifecycle` is the primary orchestrator — it is the only contract that reads
from *both* `AssetRegistry` and `EngineerRegistry` in the same call. The
reverse direction also exists: `AssetRegistry.search_assets` cross-calls into
`Lifecycle.get_collateral_score` when sorting by `ByCollateralScore`, and
`AssetRegistry.decommission_asset` cross-calls `Lifecycle.decommission_notify`.
`Lending` is intentionally decoupled — it never calls the other three
contracts on-chain; an operator/admin bridges the two systems (see
[Collateral Usage](#collateral-usage-lending-flow)).

---

## Asset Registration

```mermaid
sequenceDiagram
    autonumber
    actor Owner
    participant AssetRegistry

    Owner->>AssetRegistry: register_asset(asset_type, metadata, serial_number, owner)
    Note over AssetRegistry: owner.require_auth()
    Note over AssetRegistry: validate metadata/serial length<br/>validate asset_type against allowlist

    Note over AssetRegistry: sha256(serial_number) → dedup check<br/>panic DuplicateAsset if this machine is already registered

    Note over AssetRegistry: sha256(metadata) + owner + asset_type → secondary dedup check<br/>panic DuplicateAsset if identical

    Note over AssetRegistry: id = ASSET_COUNT + 1<br/>persist Asset { deprecation_status: Active, is_locked: false, … }<br/>update owner index and dedup keys

    AssetRegistry-->>Owner: return asset_id
```

**Preconditions:** contract initialized (`initialize_admin`), `asset_type`
previously allow-listed via `add_asset_type`, contract not paused.

**Notes:**
- Deduplication is two-layered: the **serial number** hash prevents the same
  physical machine from ever being registered twice (by anyone), while the
  **owner + metadata** hash prevents one owner from double-registering the
  same description.
- A newly registered asset always starts `Active`, unlocked, with no lender.

---

## Engineer Credentialing

Maintenance submission depends on a valid engineer credential, so the
credentialing flow is a hard prerequisite for [Maintenance
Submission](#maintenance-submission).

```mermaid
sequenceDiagram
    autonumber
    actor Issuer as Trusted Issuer
    participant EngineerRegistry
    actor Owner as Asset Owner
    participant Lifecycle

    Note over EngineerRegistry: admin.add_trusted_issuer(issuer) — one-time setup

    Issuer->>EngineerRegistry: register_engineer(engineer, credential_hash, issuer, validity_period, notes)
    Note over EngineerRegistry: issuer.require_auth()<br/>panic UntrustedIssuer if issuer is not trusted
    EngineerRegistry-->>Issuer: engineer registered (CredentialStatus starts Valid)

    Issuer->>EngineerRegistry: add_specialization(issuer, engineer, specialization)
    Note over EngineerRegistry: specialization must be on the fixed allow-list<br/>(diesel_ge, wind_turb, solar_pnl, grid_infr, gas_turbn, hydroelec, batteryst, transform)

    Owner->>Lifecycle: authorize_engineer(owner, asset_id, engineer)
    Note over Lifecycle: owner.require_auth()<br/>cross-calls AssetRegistry.get_asset to confirm owner matches<br/>writes ENG_AUTH(asset_id, engineer) = true
```

**Notes:**
- `authorize_engineer` is asset-scoped and separate from the credential
  itself: an engineer can hold a valid credential and still be rejected from
  submitting for a *specific* asset until its owner explicitly authorizes them.
- `CredentialStatus` has 7 states (`Valid`, `GracePeriod`, `HardExpired`,
  `Revoked`, `NotFound`, `Suspended`, `Expired`); `submit_maintenance` only
  accepts `Valid` or `GracePeriod`.
- Revoking an owner's authorization goes through a timelock
  (`propose_revoke_engineer_auth` → `execute_revoke_engineer_auth`) to give an
  engineer a grace period to finish in-progress work — see [Timing
  Diagrams](#timing-diagrams).

---

## Maintenance Submission

The full `submit_maintenance` sequence. Local, gas-free validation always runs
before any cross-contract call, so the transaction fails cheaply on obviously
invalid input.

```mermaid
sequenceDiagram
    autonumber
    actor Engineer
    participant Lifecycle
    participant AssetRegistry
    participant EngineerRegistry

    Engineer->>Lifecycle: submit_maintenance(asset_id, task_type, notes, engineer, cost)
    Note over Lifecycle: engineer.require_auth()
    Note over Lifecycle: (local) validate task_type weight and notes length<br/>(local) prune history if max_history would be exceeded

    Lifecycle->>AssetRegistry: get_asset(asset_id) / asset_status(asset_id)
    AssetRegistry-->>Lifecycle: Asset { owner, asset_type, deprecation_status, … }
    Note over Lifecycle: panic AssetNotFound if unknown<br/>panic AssetDecommissioned if status == Decommissioned

    Lifecycle->>EngineerRegistry: get_credential_status(engineer)
    EngineerRegistry-->>Lifecycle: CredentialStatus
    Note over Lifecycle: panic UnauthorizedEngineer unless Valid or GracePeriod

    Note over Lifecycle: (local) require ENG_AUTH(asset_id, engineer) == true<br/>panic EngineerNotAuthorized otherwise

    Lifecycle->>EngineerRegistry: get_specializations(engineer)
    EngineerRegistry-->>Lifecycle: Vec<Symbol>
    Note over Lifecycle: panic SpecializationMismatch unless asset_type ∈ specializations

    Lifecycle->>EngineerRegistry: get_reputation(engineer)
    EngineerRegistry-->>Lifecycle: reputation (0–1000)

    Note over Lifecycle: weighted_increment = score_increment × (500 + reputation) / 1000<br/>append MaintenanceRecord to HIST, ScoreEntry to SCHIST<br/>write SCORE, LUPD, ENG_HIST

    Lifecycle-->>Engineer: emit (maint, asset_id, engineer, task_type, timestamp)
```

**Cross-contract calls made:** up to 4 (`get_asset`/`asset_status`,
`get_credential_status`, `get_specializations`, `get_reputation`) — all
read-only queries against the other two contracts; `Lifecycle` is the only
contract that writes as a result of this flow.

---

## Batch Maintenance Submission

`batch_submit_maintenance` validates and scores the *entire* batch before
writing anything, so a failure partway through never leaves partial state.

```mermaid
sequenceDiagram
    autonumber
    actor Engineer
    participant Lifecycle
    participant AssetRegistry
    participant EngineerRegistry

    Engineer->>Lifecycle: batch_submit_maintenance(asset_id, records[], engineer, costs[])
    Note over Lifecycle: engineer.require_auth()
    Note over Lifecycle: (local) reject if records is empty<br/>(local) panic BatchTooLarge if records.len() > MAX_BATCH_SIZE (50)
    Note over Lifecycle: (local) validate notes length + task_type per record

    Lifecycle->>AssetRegistry: get_asset(asset_id)
    AssetRegistry-->>Lifecycle: Asset

    Lifecycle->>EngineerRegistry: get_credential_status(engineer)
    EngineerRegistry-->>Lifecycle: CredentialStatus
    Note over Lifecycle: panic UnauthorizedEngineer unless Valid/GracePeriod<br/>(local) require ENG_AUTH — panic EngineerNotAuthorized

    Lifecycle->>EngineerRegistry: get_specializations(engineer)
    EngineerRegistry-->>Lifecycle: Vec<Symbol>
    Note over Lifecycle: panic SpecializationMismatch if asset_type not covered

    Note over Lifecycle: panic HistoryCapReached if history.len() + records.len() > max_history

    Lifecycle->>EngineerRegistry: get_reputation(engineer)
    EngineerRegistry-->>Lifecycle: reputation

    Note over Lifecycle: build every MaintenanceRecord + ScoreEntry in memory first,<br/>panic ScoreOverflow if the running score would overflow —<br/>only once ALL records are valid does it commit to storage

    Lifecycle-->>Engineer: emit (MAINT, asset_id) per record
```

**Why validate-then-commit matters:** because the whole batch is one Soroban
transaction, a `panic!` anywhere aborts and reverts all storage writes anyway
— but building every record up front (rather than writing as you go) keeps
the code's invariants explicit and avoids ever having a half-written batch
*in memory* to reason about, independent of the ledger's own atomicity.

---

## Ownership Transfer

Ownership transfer is a **two-step, two-contract** flow — `AssetRegistry`
moves ownership, then the new owner separately tells `Lifecycle` about it so
the maintenance history gets an `XFER` boundary marker. These are two
independent transactions; nothing forces the second to happen immediately
after the first.

```mermaid
sequenceDiagram
    autonumber
    actor CurrentOwner as Current Owner
    actor NewOwner as New Owner
    participant AssetRegistry
    participant Lifecycle

    CurrentOwner->>AssetRegistry: transfer_asset(asset_id, current_owner, new_owner)
    Note over AssetRegistry: current_owner.require_auth()<br/>panic UnauthorizedOwner if mismatch<br/>panic SameOwner if new_owner == current_owner<br/>panic AssetLocked if is_locked (asset pledged as collateral)
    Note over AssetRegistry: move dedup key + owner index to new_owner<br/>asset.owner = new_owner
    AssetRegistry-->>CurrentOwner: emit (TRANSFER, asset_id)

    Note over NewOwner,Lifecycle: Separate transaction — new_owner must call this themselves

    NewOwner->>Lifecycle: record_transfer(asset_id, previous_owner, new_owner)
    Note over Lifecycle: new_owner.require_auth()
    Lifecycle->>AssetRegistry: get_asset(asset_id)
    AssetRegistry-->>Lifecycle: Asset { owner, … }
    Note over Lifecycle: panic UnauthorizedOwner unless asset.owner == new_owner<br/>(prevents a signature-replay inserting a false transfer sentinel)
    Note over Lifecycle: append XFER sentinel MaintenanceRecord (prune if at cap)<br/>append TransferRecord to XFER_HIST<br/>clear every EngineerAuth the previous owner had granted
    Lifecycle-->>NewOwner: emit (XFER, asset_id, previous_owner, new_owner, timestamp, sentinel_index)
```

**Why the second step matters:** all `EngineerAuth` grants from the previous
owner are revoked as part of `record_transfer`. Until the new owner calls it,
the *previous* owner's authorized engineers can still submit maintenance
records — this is a known integration gotcha worth flagging to new owners.

There is also a **multi-signature variant**
(`initiate_ownership_transfer` → `accept_ownership_transfer`, a 7-day
proposal window) for cases where the current and new owner want to coordinate
the handoff instead of a single unilateral `transfer_asset` call.

---

## Decommissioning

Unlike transfer, decommissioning **is** wired up as an automatic
cross-contract call — the admin only ever calls `AssetRegistry`.

```mermaid
sequenceDiagram
    autonumber
    actor Admin
    participant AssetRegistry
    participant Lifecycle

    Admin->>AssetRegistry: decommission_asset(admin, asset_id)
    Note over AssetRegistry: admin.require_auth(), must match stored admin
    Note over AssetRegistry: asset.deprecation_status = Decommissioned<br/>clear any UnderMaintenance flag
    AssetRegistry-->>Admin: emit (DECOMM, asset_id, ledger_sequence)

    AssetRegistry->>Lifecycle: invoke_contract("decommission_notify", asset_id)
    Note over Lifecycle: asset_registry.require_auth()<br/>(the calling contract's own address self-authorizes<br/>the cross-contract invocation it initiated)
    Note over Lifecycle: frozen_score = compute_decay(asset_id)<br/>persist FROZEN = true, FRZ_SCR = frozen_score
    Lifecycle-->>AssetRegistry: emit (DECOMM, asset_id, 0)
```

**Notes:**
- `AssetRegistry` computes its own `ledger_sequence` for the event payload;
  `Lifecycle`'s own `DECOMM` event always reports a score of exactly `0`
  (issue #794) — even though the *stored* frozen score keeps the real
  decayed value internally, so an already-issued loan's risk models can still
  inspect what the score was at the moment of decommissioning if needed.
- Once `FROZEN` is set, all subsequent `get_collateral_score` calls for that
  asset short-circuit to the frozen value — decay stops entirely.
- `submit_maintenance` also independently checks `asset_status ==
  Decommissioned` and panics `AssetDecommissioned`, so no new maintenance can
  slip in between the two events above.

---

## Collateral Score Query

`get_collateral_score` looks read-only from the caller's side, but it lazily
applies decay and **writes the result back** so repeated calls in the same
ledger stay consistent without redundant recomputation.

```mermaid
sequenceDiagram
    autonumber
    actor Caller
    participant Lifecycle
    participant AssetRegistry

    Caller->>Lifecycle: get_collateral_score(asset_id)

    Lifecycle->>AssetRegistry: get_asset(asset_id)
    AssetRegistry-->>Lifecycle: Asset { deprecation_status, … }
    Note over Lifecycle: return 0 immediately if Deprecated or Decommissioned (and not FROZEN)

    Note over Lifecycle: if FROZEN → return FRZ_SCR immediately (decay stopped at decommission)

    Note over Lifecycle: Model A — recency-weighted history:<br/>Σ over HIST of score_increment × max(0, MAX_AGE_LEDGERS − age) / MAX_AGE_LEDGERS, capped at 100

    Note over Lifecycle: Model B — stored value + lazy config decay:<br/>config_score = max(0, SCORE − (elapsed / decay_interval) × decay_rate)

    Note over Lifecycle: score = min(Model A, Model B)<br/>floor at 1 if HIST is non-empty (MIN_SCORE_WITH_HISTORY)

    Note over Lifecycle: persist score → SCORE, now → LUPD

    Lifecycle-->>Caller: return score (0–100)
```

See [collateral-scoring-formula.md](collateral-scoring-formula.md) and
[scoring-algorithm-deep-dive.md](scoring-algorithm-deep-dive.md) for the full
formula derivation and worked examples.

---

## Collateral Usage (Lending Flow)

This is the flow the issue calls "collateral usage" — a lender discovering
an asset, verifying it, and using it to secure a loan. Unlike the flows
above, **`Lending` and `AssetRegistry`/`Lifecycle` are not wired together by
an automatic cross-contract call** for the loan itself; only the collateral
*lock* is a direct `AssetRegistry` entry point restricted to the registered
`lending_contract` address. Everything else is orchestrated by an
operator/admin bridging both systems (typically off-chain, or by a relayer
acting as the lending contract's identity).

```mermaid
sequenceDiagram
    autonumber
    actor Lender
    participant AssetRegistry
    participant Lifecycle
    participant Lending

    Lender->>AssetRegistry: search_assets(filter: { asset_type, sort: ByCollateralScore, lifecycle_contract })
    AssetRegistry->>Lifecycle: invoke_contract("get_collateral_score", asset_id) — once per candidate, for sorting
    Lifecycle-->>AssetRegistry: score
    AssetRegistry-->>Lender: SearchPage { assets[], total }

    Lender->>Lifecycle: get_collateral_score(asset_id)
    Lifecycle-->>Lender: score
    Note over Lender: reject if score < eligibility threshold (default 50)

    Lender->>AssetRegistry: get_asset(asset_id)
    AssetRegistry-->>Lender: Asset { owner, deprecation_status, is_locked, … }
    Note over Lender: reject if not Active, or owner != borrower

    Lender->>Lending: get_liens(asset_id)
    Lending-->>Lender: Vec<LienRecord>
    Note over Lender: reject if total encumbrance + new loan amount > asset value

    Note over Lender,Lending: Lender/admin decide to proceed

    Lending->>AssetRegistry: lock_asset_as_collateral(lending_contract, asset_id, loan_id)
    Note over AssetRegistry: lending_contract.require_auth()<br/>must match the address set via set_lending_contract<br/>asset.is_locked = true, lender = lending_contract, loan_id = loan_id<br/>(blocks transfer_asset while locked)

    Lender->>Lending: record_lien(admin, asset_id, lender, loan_id, amount)
    Note over Lending: admin.require_auth()<br/>panic LienAlreadyExists for a duplicate (lender, loan_id)

    Note over Lending: borrower.request_loan(amount) / vouchers.vouch(...)

    alt Loan repaid
        Lending->>Lending: repay(borrower) — pays back principal + voucher yield
        Lender->>Lending: release_lien(admin, asset_id, lender, loan_id)
        Lender->>AssetRegistry: unlock_asset_from_collateral(lending_contract, asset_id)
    else Loan defaults
        Lending->>Lending: slash(admin, borrower) — status → Defaulted, voucher stakes slashed
        Note over Lender: verify Defaulted status + matching lien, then coordinate liquidation
        Lender->>Lending: release_lien(admin, asset_id, lender, loan_id)
        Lender->>AssetRegistry: unlock_asset_from_collateral(lending_contract, asset_id)
        Lender->>AssetRegistry: transfer_asset(asset_id, borrower, lender) — lender takes ownership
    end
```

See [lender-integration-guide.md](lender-integration-guide.md) for the full
API reference, TypeScript examples, and a lending-side security checklist.

---

## Error Scenario Reference

Every cross-contract flow above has an unhappy path. The table below lists
the errors a caller is most likely to hit and which contract raises them —
useful when deciding what to catch/retry versus surface to a user.

| Flow | Error | Raised by | Trigger |
|---|---|---|---|
| Registration | `DuplicateAsset` | `AssetRegistry` | Serial number or (owner, metadata) hash already registered |
| Registration | `InvalidAssetType` | `AssetRegistry` | `asset_type` not on the admin allow-list |
| Credentialing | `UntrustedIssuer` | `EngineerRegistry` | Issuer not added via `add_trusted_issuer` |
| Credentialing | `EngineerAlreadyRegistered` | `EngineerRegistry` | Re-registering an existing engineer address |
| Maintenance | `AssetNotFound` | `Lifecycle` (via `AssetRegistry`) | Unknown `asset_id` |
| Maintenance | `AssetDecommissioned` | `Lifecycle` | Asset status is `Decommissioned` |
| Maintenance | `UnauthorizedEngineer` | `Lifecycle` (via `EngineerRegistry`) | Credential not `Valid`/`GracePeriod` |
| Maintenance | `EngineerNotAuthorized` | `Lifecycle` | Owner never called `authorize_engineer` for this asset |
| Maintenance | `SpecializationMismatch` | `Lifecycle` (via `EngineerRegistry`) | Engineer's specializations don't include the asset's type |
| Maintenance | `HistoryCapReached` | `Lifecycle` | Batch would exceed `max_history` (pruning only happens in `submit_maintenance`, not the batch path) |
| Maintenance | `NotesTooLong` | `Lifecycle` | `notes` exceeds `max_notes_length` |
| Batch | `BatchTooLarge` | `Lifecycle` | `records.len() > MAX_BATCH_SIZE` (50) |
| Transfer | `UnauthorizedOwner` | `AssetRegistry` / `Lifecycle` | Caller isn't the asset's current owner |
| Transfer | `AssetLocked` | `AssetRegistry` | Asset is locked as collateral (`is_locked == true`) |
| Transfer | `SameOwner` | `AssetRegistry` | `new_owner == current_owner` |
| Collateral lock | `UnauthorizedAdmin`-class auth failure | `AssetRegistry` | Caller isn't the registered `lending_contract` address |
| Lien | `LienAlreadyExists` | `Lending` | Duplicate `(lender, loan_id)` on the same asset |
| Lien | `LienNotFound` | `Lending` | Releasing a lien that was never recorded (or already released) |
| Loan | `LoanAlreadyActive` | `Lending` | Borrower already has an unresolved loan |
| Loan | `NoActiveLoan` | `Lending` | `repay`/`slash` called with no active loan |
| Admin ops | `TimelockNotExpired` | any contract | Executing a proposal before its delay has elapsed |
| Admin ops | `ProposalNotFound` | any contract | Executing/re-proposing with no pending proposal |

---

## Contract Call Dependency Graph

Initialization order matters: `Lifecycle` stores immutable references to the
other two registries at `initialize` time, so they must exist first.

```mermaid
graph TD
    subgraph "Deployment & Initialization Order"
        A1["1 . AssetRegistry.initialize_admin(admin)"] --> A2["2 . EngineerRegistry.initialize_admin(admin)"]
        A2 --> A3["3 . EngineerRegistry.add_trusted_issuer(admin, issuer)"]
        A3 --> A4["4 . Lifecycle.initialize(asset_registry_addr, engineer_registry_addr, admin, max_history)"]
        A4 --> A5["5 . AssetRegistry.set_lending_contract(admin, lending_addr) — optional, for collateral locking"]
        A5 --> A6["6 . Lending.initialize(admin, token, slash_bps)"]
    end

    subgraph "Runtime Call Directions"
        Lifecycle -->|"get_asset / asset_status<br/>(submit_maintenance, record_transfer, get_collateral_score)"| AssetRegistry
        Lifecycle -->|"get_credential_status / get_specializations / get_reputation<br/>(submit_maintenance, batch_submit_maintenance)"| EngineerRegistry
        AssetRegistry -->|"invoke_contract: decommission_notify<br/>(decommission_asset)"| Lifecycle
        AssetRegistry -->|"invoke_contract: get_collateral_score<br/>(search_assets, sort=ByCollateralScore)"| Lifecycle
        LendingContract["Lending contract's own address"] -->|"lock_asset_as_collateral /<br/>unlock_asset_from_collateral"| AssetRegistry
    end
```

**Key takeaway:** `EngineerRegistry` and `AssetRegistry` never call each
other or `Lending` directly — `Lifecycle` is the hub between the first two,
while `Lending`'s only on-chain touchpoint with the rest of the system is the
collateral lock/unlock pair on `AssetRegistry`.

---

## Timing Diagrams

Several flows are gated by elapsed real time rather than by another
contract's response. The three that matter most operationally:

### Two-step timelocked operations (48-hour delay)

Used by `propose_admin`/`accept_admin`, `propose_pause`/`execute_pause`,
`propose_unpause`/`execute_unpause`, and
`propose_revoke_engineer_auth`/`execute_revoke_engineer_auth` in `Lifecycle`
(and the analogous admin-transfer/upgrade timelocks in the other contracts).

```mermaid
gantt
    title Admin Timelock Window (TIMELOCK_DELAY_SECS = 48h)
    dateFormat X
    axisFormat %H:00
    section Timeline
    propose_* call (starts timer)      :milestone, m1, 0, 0h
    Locked — execute_* reverts with TimelockNotExpired :active, locked, 0, 48h
    execute_* now succeeds              :milestone, m2, 48h, 0h
```

### Ownership transfer proposal window (7 days)

`initiate_ownership_transfer` → `accept_ownership_transfer` (`AssetRegistry`).
Unlike the 48-hour timelocks above, this window is a *deadline*, not a
minimum wait — the new owner may accept at any point up to 7 days, after
which the proposal expires and must be re-initiated.

```mermaid
gantt
    title Ownership Transfer Proposal Window (7 days)
    dateFormat X
    axisFormat %d d
    section Timeline
    initiate_ownership_transfer         :milestone, m1, 0, 0d
    Open — accept_ownership_transfer may succeed :active, open, 0, 7d
    Expired — must re-initiate          :milestone, m2, 7d, 0d
```

### Collateral score decay (30-day intervals)

`decay_rate` (default 5) points are removed from the stored score for every
whole `decay_interval` (default 2,592,000s / 30 days) that elapses since the
asset's last maintenance write — applied lazily on the next
`get_collateral_score`/`decay_score` call, not on a background schedule.

```mermaid
gantt
    title Lazy Score Decay (decay_interval = 30 days, decay_rate = 5 pts)
    dateFormat X
    axisFormat %d d
    section Score
    Last maintenance (score frozen until next read) :milestone, s0, 0, 0d
    0 intervals elapsed — no decay yet   :active, i0, 0, 30d
    1 interval elapsed — −5 pts on next read :active, i1, 30d, 30d
    2 intervals elapsed — −10 pts on next read :active, i2, 60d, 30d
```

Also relevant: **TTL extension** happens on every persistent write (not on a
timer) — see [ttl-strategy.md](ttl-strategy.md) for the 518,400-ledger
(~30-day) threshold/target policy shared by all four contracts.

---

## Further Reading

- [architecture.md](architecture.md) — contract responsibilities and storage layout
- [lender-integration-guide.md](lender-integration-guide.md) — full lending-side API and code examples
- [scoring-algorithm-deep-dive.md](scoring-algorithm-deep-dive.md) — collateral scoring formula and worked examples
- [ttl-strategy.md](ttl-strategy.md) — persistent storage TTL policy
- [asset-lifecycle.md](asset-lifecycle.md) — asset status transitions
- [credentialing.md](credentialing.md) — engineer credential lifecycle
