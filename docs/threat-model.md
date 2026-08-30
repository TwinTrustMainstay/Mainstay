# Threat Model & Security Analysis

This document provides a structured threat model for the Mainstay smart contract system using the STRIDE methodology. It is intended to serve as input for security auditors and as a reference for integrators and operators.

---

## System Overview

Mainstay is a decentralized physical infrastructure network (DePIN) built on Stellar Soroban. Four smart contracts interact to create a verifiable maintenance audit trail for industrial assets:

1. **Asset Registry** — Canonical registry of industrial assets
2. **Engineer Registry** — Federated credentialing for maintenance engineers
3. **Lifecycle** — Orchestration contract for maintenance records and collateral scoring
4. **Lending** — DeFi lending contract using collateral scores

**Trust Model:** The system assumes the Stellar network validators are honest and the Soroban host environment is secure. Within the application layer, trust is decentralized: asset owners control their assets, trusted issuers verify engineers, and the admin controls configuration. No single party can unilaterally fabricate maintenance history or manipulate collateral scores.

---

## Threat Actors

### TA-1: Malicious Asset Owner
**Motivation:** Inflate collateral score to obtain larger loans or better financing terms.
**Capabilities:**
- Controls their own assets and can submit transactions
- Can authorize engineers for their assets
- Can propose asset transfers and deprecation

### TA-2: Rogue Engineer
**Motivation:** Submit fraudulent maintenance records for assets they are not authorized to service; collude with asset owners to fabricate history.
**Capabilities:**
- Holds a valid credential from a trusted issuer
- Can call `submit_maintenance` (but only for assets they are authorized for)
- May attempt to maintain access after credential revocation

### TA-3: Compromised Issuer
**Motivation:** Issue credentials to unqualified engineers, enabling fraudulent maintenance records at scale.
**Capabilities:**
- Can register engineers with arbitrary validity periods
- Can revoke credentials they issued
- Trusted by the system (added by admin)

### TA-4: Malicious Admin
**Motivation:** Manipulate configuration (scoring weights, decay rates, eligibility thresholds) to favor specific assets or parties.
**Capabilities:**
- Can update all Lifecycle configuration parameters
- Can pause/unpause contracts
- Can transfer admin role (2-step process)
- Cannot directly forge maintenance records or alter individual asset scores

### TA-5: External Attacker
**Motivation:** Exploit contract vulnerabilities to steal funds, destroy data, or manipulate scores for profit.
**Capabilities:**
- No special privileges in the system
- Can call any public function
- May attempt re-entrancy, front-running, or denial-of-service attacks

### TA-6: DeFi Protocol / Lender
**Motivation:** Obtain accurate collateral scores; may attempt to query scores in ways that advantage their lending position.
**Capabilities:**
- Read-only access to all public query functions
- No mutating capabilities

---

## STRIDE Threat Analysis

### Asset Registry

| Threat | Category | Description | Risk | Mitigation |
|--------|----------|-------------|------|------------|
| T-AR-01 | **S**poofing | Attacker registers an asset impersonating another owner | Medium | `owner.require_auth()` on `register_asset` and `transfer_asset` |
| T-AR-02 | **T**ampering | Owner modifies asset metadata post-registration to misrepresent condition | Low | Metadata updates emit events; full history is not overwritten |
| T-AR-03 | **T**ampering | Attacker manipulates dedup hash to bypass uniqueness check | Medium | SHA-256 hash of full metadata; collision is computationally infeasible |
| T-AR-04 | **R**epudiation | Owner denies having registered or transferred an asset | Low | All mutations emit on-chain events with `owner` and `asset_id` |
| T-AR-05 | **I**nformation Disclosure | Asset metadata is public on-chain | Info | By design — transparency is required for DeFi integration |
| T-AR-06 | **D**enial of Service | Attacker registers many assets to exhaust storage or hit limits | Low | Each registration costs transaction fees; no global cap enforced at contract level |
| T-AR-07 | **D**enial of Service | TTL expiry destroys asset records | Critical | Every `put` operation extends TTL by 30 days; documented in ttl-strategy.md |
| T-AR-08 | **E**levation of Privilege | Unauthorized party calls admin functions | Critical | `admin.require_auth()` on all admin-gated functions |
| T-AR-09 | **E**levation of Privilege | Front-run `initialize_admin` to set attacker as admin | Critical | Deployer signature required; same-block initialization mandated in runbook |
| T-AR-10 | **E**levation of Privilege | Attacker bypasses timelock on admin-initiated asset transfer | Medium | 48-hour timelock with `propose`/`execute` pattern |

### Engineer Registry

| Threat | Category | Description | Risk | Mitigation |
|--------|----------|-------------|------|------------|
| T-ER-01 | **S**poofing | Unauthorized issuer registers engineers | High | `add_trusted_issuer` is admin-only; `register_engineer` requires issuer auth |
| T-ER-02 | **T**ampering | Issuer backdates credential issuance timestamp | Low | Timestamp is set to `env.ledger().timestamp()` at registration |
| T-ER-03 | **T**ampering | Engineer credential hash is modified after issuance | Low | Immutable after initial registration; renewal creates a new credential |
| T-ER-04 | **R**epudiation | Issuer denies having issued a credential | Low | `register_engineer` emits event with issuer, engineer, and timestamp |
| T-ER-05 | **D**enial of Service | TTL expiry destroys credential records | Critical | All write operations extend TTL; see ttl-strategy.md |
| T-ER-06 | **D**enial of Service | Attacker exhausts issuer registration slots | Low | Issuer list is admin-managed; cost per transaction |
| T-ER-07 | **E**levation of Privilege | Revoked engineer continues to submit maintenance | High | `verify_engineer` returns `false` for revoked credentials; Lifecycle checks before accepting maintenance |
| T-ER-08 | **E**levation of Privilege | Expired credential used during grace period beyond intended window | Medium | Grace period is time-bounded; hard-expired credentials are rejected on renewal |
| T-ER-09 | **E**levation of Privilege | Compromised issuer registers malicious engineers | High | Admin can remove trusted issuers; existing credentials can be individually revoked |

### Lifecycle

| Threat | Category | Description | Risk | Mitigation |
|--------|----------|-------------|------|------------|
| T-LC-01 | **S**poofing | Attacker submits maintenance for an asset they don't own | High | `engineer.require_auth()` + engineer authorization check per asset |
| T-LC-02 | **T**ampering | Engineer modifies maintenance history after submission | Low | `HIST` is append-only; records cannot be updated or deleted |
| T-LC-03 | **T**ampering | Asset owner colludes with engineer to submit fake maintenance | Medium | Collusion risk is inherent; mitigated by credentialing quality and issuer trust |
| T-LC-04 | **T**ampering | Admin changes scoring weights or decay rates to manipulate scores | Medium | Configuration changes use timelock; events are emitted on every config update |
| T-LC-05 | **T**ampering | History pruning removes valuable records | Low | `max_history` is admin-configured; pruned by age (FIFO), not selectively |
| T-LC-06 | **R**epudiation | Engineer denies having submitted a maintenance record | Low | All records are signed with `engineer.require_auth()` and stored on-chain |
| T-LC-07 | **I**nformation Disclosure | Collateral scores and maintenance history are public | Info | By design for DeFi transparency |
| T-LC-08 | **D**enial of Service | Unbounded maintenance history makes `submit_maintenance` too expensive | Medium | `max_history` capped at 200 (default); paginated queries |
| T-LC-09 | **D**enial of Service | TTL expiry destroys maintenance history and scores | Critical | All write paths extend TTL; `get_collateral_score` applies lazy decay and re-extends |
| T-LC-10 | **D**enial of Service | Pause flag TTL expires, silently unpausing a paused contract | Critical | `pause`/`unpause` extend TTL on `PAUSED` key (issue #756) |
| T-LC-11 | **E**levation of Privilege | Cross-contract call to AssetRegistry returns stale/false data | High | Lifecycle verifies asset existence at call time; `try_get_asset` panics on non-existent assets |
| T-LC-12 | **E**levation of Privilege | Cross-contract call to EngineerRegistry bypassed or replayed | High | `get_credential_status` is called at submission time; fallback `verify_engineer` for non-Valid status |
| T-LC-13 | **E**levation of Privilege | Engineer reputation score is manipulated to inflate collateral weight | Medium | Reputation is read from EngineerRegistry at submission time; not cached in Lifecycle |
| T-LC-14 | **E**levation of Privilege | Same address passed for both registries; cross-contract calls resolve incorrectly (#1254) | High | `initialize` validates `asset_registry != engineer_registry`; returns `SameRegistryAddress` (error 13) if they match |
| T-LC-15 | **E**levation of Privilege | Observer front-runs `initialize` to set attacker as admin (#1255) | Critical | `__constructor` stores deployer at deploy time; `initialize` requires `deployer.require_auth()` and verifies against stored `DEPLOYER_KEY` — non-deployers are rejected |

### Lending

| Threat | Category | Description | Risk | Mitigation |
|--------|----------|-------------|------|------------|
| T-LN-01 | **S**poofing | Attacker requests a loan for another borrower's address | High | `borrower.require_auth()` on `request_loan` |
| T-LN-02 | **T**ampering | Borrower repays less than owed, bypassing interest calculation | High | Yield BPS is applied in contract logic; repayment amount is validated |
| T-LN-03 | **T**ampering | Admin changes yield or slash BPS retroactively | Medium | Config changes only affect future loans, not active ones |
| T-LN-04 | **R**epudiation | Borrower denies taking a loan | Low | On-chain loan record with `borrower` address |
| T-LN-05 | **D**enial of Service | TTL expiry destroys loan records, freezing vouched funds | Critical | All write paths extend TTL |
| T-LN-06 | **E**levation of Privilege | Admin withdraws slash balance without authorization | Medium | `admin.require_auth()` on `withdraw_slash` |
| T-LN-07 | **D**enial of Service | Pause flag TTL expiry silently unpauses contract | Critical | Same mitigation as Lifecycle T-LC-10 |

---

## Cross-Contract Attack Vectors

### CC-01: State Drift Between Contracts
**Description:** Asset is deleted or deprecated in AssetRegistry after Lifecycle has already verified its existence but before the maintenance record is written. This could lead to maintenance records for non-existent assets.

**Risk:** High
**Mitigation:** Cross-contract calls are made atomically within a single transaction. Soroban's transaction model ensures that all contract calls within a transaction succeed or fail together. Note: there are two distinct lifecycle transitions: (1) **Deprecation** (`deprecate_asset`) is owner-only and immediate — this is acceptable because the owner has the right to signal end-of-life and zero out their own collateral score. (2) **Deregistration** (`propose_deregister_asset` → `execute_deregister_asset`) uses a 48-hour timelock, providing a window for detection before permanent removal.

### CC-02: Cross-Contract Re-entrancy
**Description:** Lifecycle calls AssetRegistry and EngineerRegistry. If either were to call back into Lifecycle, state could be manipulated.

**Risk:** Medium
**Mitigation:** Soroban contracts are Wasm-based and isolated. Cross-contract calls execute in the called contract's context. Re-entrancy is limited—the called contract cannot call back into the caller in the same invocation. Lifecycle performs local validation and state writes before external calls.

### CC-03: Lifecycle Registry Binding Immutability
**Description:** If the registry binding addresses (`REGISTRY`, `ENG_REG`) could be changed after initialization, an attacker could redirect Lifecycle to malicious registry contracts.

**Risk:** Critical
**Mitigation:** Registry bindings are set once at `initialize` and are immutable thereafter. The `initialize` function is one-shot (cannot be called twice).

### CC-04: Stale Data from Read-Only Cross-Contract Calls
**Description:** Lifecycle reads asset data from AssetRegistry. If AssetRegistry's TTL has expired and data is stale, Lifecycle could operate on incorrect data.

**Risk:** High
**Mitigation:** Both contracts extend TTL on every write. `get_collateral_score` calls `try_get_asset` which panics if the asset doesn't exist. Soroban's storage model ensures reads return the latest written value or nothing.

---

## Storage & TTL Threats

### ST-01: Silent Persistent Storage Expiry
Persistent storage entries expire silently after TTL. Without explicit extension, any stored data can be lost. This affects all four contracts.

**Affected data:** Asset records, maintenance history, engineer credentials, collateral scores, loan records, configuration.

**Mitigation:** Every `put`/`set` call in all contracts is followed by `extend_ttl(THRESHOLD, TARGET)` where `THRESHOLD = TARGET = 518,400` ledgers (~30 days).

**Verification:** See `docs/ttl-strategy.md` for the complete key-to-extension mapping.

### ST-02: Instance Storage Expiry
Instance storage holds critical configuration: admin address, trusted issuer list, registry bindings. If it expires, admin operations and cross-contract bindings panic with `NotInitialized`.

**Mitigation:** All admin-mutating functions call `env.storage().instance().extend_ttl(518400, 518400)`.

### ST-03: Pause Flag Expiry (Issue #756)
The pause flag is stored in persistent storage. If it expires while the contract is paused, the `unwrap_or(false)` default returns `false`, silently unpausing the contract.

**Mitigation:** `pause()` and `unpause()` explicitly extend TTL on the `PAUSED` key. All four contracts implement this.

---

## Deployment & Initialization Threats

### DI-01: Uninitialized Contract Exploitation
Between deployment and `initialize_admin`/`initialize`, anyone could call uninitialized functions.

**Mitigation:** All functions check initialization state and panic with `NotInitialized` if called before setup. Deploy + initialize must be done in the same transaction block.

### DI-02: Admin Key Compromise During Deployment
The deployer key is the most critical key during deployment. If compromised, the attacker can set themselves as admin.

**Mitigation:** Use a cold wallet for deployment; transfer admin to a multisig account after initialization. Store deployer key in a hardware wallet or secrets manager (HashiCorp Vault).

### DI-03: Front-Running `initialize` on the Lifecycle Contract (#1255)
Between deployment and `initialize`, an observer watching the mempool could attempt to call `initialize` with their own `admin` address before the legitimate deployer does.

**Risk:** Critical

**Mitigation:** The Lifecycle contract uses a `DEPLOYER_KEY` pattern to prevent front-running:
- The `__constructor` (called at deploy time) stores the deployer's address in instance storage under `DEPLOYER_KEY`.
- `initialize` reads `DEPLOYER_KEY` from instance storage and requires `deployer.require_auth()` from that stored address. Any caller presenting a different address — or any call where `DEPLOYER_KEY` is absent — is rejected with `UnauthorizedAdmin` or `NotInitialized` respectively.
- This means only the wallet that deployed the contract can call `initialize`, making front-run initialization impossible.

**Residual risk:** The `__constructor` and `initialize` must still be called in close succession. A well-resourced attacker who can delay ledger inclusion of the deployer's `initialize` transaction cannot front-run initialization, but *could* attempt to prevent the deployer from initializing by spamming the network. The runbook's recommendation to initialize in the same block as deployment (or immediately after) fully eliminates this residual risk.

**Verification:** See `test_initialize_deployer_restriction_non_deployer_rejected` and `test_initialize_deployer_restriction_already_initialized` in `contracts/lifecycle/src/lib.rs`.

### DI-04: Testnet → Mainnet Configuration Drift
Testnet configuration accidentally carried to mainnet (wrong admin, wrong thresholds).

**Mitigation:** `scripts/deploy_testnet.sh` hard-rejects non-testnet networks. Mainnet deployment is manual with `--network mainnet`.

---

## Economic & Game-Theoretic Threats

### EG-01: Score Inflation via Fake Maintenance
**Description:** Asset owner colludes with a credentialed engineer to submit fictitious maintenance records, inflating the collateral score.

**Risk:** Medium
**Mitigation:** Engineer credentialing creates accountability. Issuers vet engineers. Repeated fraud by an engineer leads to credential revocation, destroying their on-chain reputation. The `reputation_score` in EngineerRegistry further weights score increments; low-reputation engineers contribute less to collateral scores.

### EG-02: Decay Avoidance via Minimal Maintenance
**Description:** Asset owner submits minimal maintenance at exactly the decay interval to maintain the score without real upkeep.

**Risk:** Medium
**Mitigation:** Task weights differentiate minor maintenance (2 pts) from major overhauls (10 pts). Small tasks cannot offset decay indefinitely. The recency-weighted dual-model scoring penalizes assets with only old, minor records.

### EG-03: Lending Contract Griefing
**Description:** Attacker vouches for a borrower with a tiny stake, knowing the borrower will default, just to consume storage and compute resources.

**Risk:** Low
**Mitigation:** `min_stake` configuration prevents trivial vouches. Transaction fees make this uneconomical at scale.

### EG-04: Slash Balance Manipulation
**Description:** Admin could set `slash_bps` to 100% (10,000 BPS) to slash the full voucher stake on any default, even for minor defaults.

**Risk:** Low (admin is trusted)
**Mitigation:** Admin is a trusted role. Configuration changes emit events. For mainnet, admin should be a multisig account, making unilateral config changes impossible.

---

## Risk Matrix Summary

| Risk Level | Count | Examples |
|-----------|-------|---------|
| **Critical** | 10 | TTL expiry (all contracts), pause flag expiry, front-run initialization (DI-03 / T-LC-15), registry binding immutability |
| **High** | 10 | Cross-contract state drift, unauthorized engineer maintenance, compromised issuer, loan spoofing, same-registry misconfiguration (T-LC-14) |
| **Medium** | 11 | Score inflation via collusion, decay avoidance, configuration manipulation, dedup bypass |
| **Low** | 11 | Metadata modification, history pruning, backdated credentials, griefing attacks |
| **Informational** | 2 | Public data visibility (by design) |

---

## Audit Focus Areas

Based on this threat model, auditors should prioritize:

1. **TTL extension coverage** — Verify every `put`/`set` has a corresponding `extend_ttl` (see SOR-01, SOR-02 in `audit-report.md`)
2. **Cross-contract authorization** — Verify Lifecycle correctly checks both AssetRegistry and EngineerRegistry before accepting maintenance (see CC-01, T-LC-11, T-LC-12)
3. **Access control completeness** — Verify all admin, owner, issuer, and engineer functions require proper authorization (see T-AR-08, T-ER-01, T-LC-01)
4. **Arithmetic safety** — Verify score calculations, decay computations, and BPS math use checked/saturating operations (see SOR-06)
5. **State machine validation** — Verify credential lifecycle (Valid → GracePeriod → HardExpired) and asset lifecycle (Active → Deprecated → Decommissioned) are correctly enforced (see SOR-10)
6. **Deployment security** — Verify initialization is front-run resistant and deployment runbook is followed (see DI-01, DI-02)
7. **Pause mechanism integrity** — Verify pause flags cannot silently expire (see ST-03, SOR-02)

---

## References

- [Architecture Overview](architecture.md)
- [Security Audit Report](audit-report.md)
- [Deployment Runbook](deployment-runbook.md)
- [TTL Strategy](ttl-strategy.md)
- [Access Control Model](access-control.md)
- [Collateral Scoring Model](collateral-scoring.md)
- [Engineer Credentialing](credentialing.md)
- [Asset Lifecycle](asset-lifecycle.md)
- [Security Policy](../SECURITY.md)
