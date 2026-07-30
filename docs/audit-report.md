# Security Audit Report

**Project:** Mainstay — Proof of Maintenance for Industrial Assets
**Date:** TBD (post-audit)
**Version:** 0.1.x
**Auditor:** TBD
**Project:** Mainstay — Industrial Asset Collateral & DeFi Integration  
**Platform:** Stellar Soroban (Rust / WASM)  
**Date:** July 30, 2026  
**Status:** Pre-Audit — Firm Selection Phase  

---

## Executive Summary

> *To be completed after the audit engagement. This section should summarize the overall security posture of Mainstay, the number and severity of findings, and the auditor's final assessment.*

Mainstay manages real industrial asset records and DeFi collateral scoring on Stellar Soroban. A formal, independent security audit is required before mainnet deployment to ensure:

1. Smart contract logic correctly enforces access control and data integrity.
2. Cross-contract call flows are free of re-entrancy, authorization bypass, and state inconsistency vulnerabilities.
3. Collateral scoring, decay, and maintenance history are tamper-proof.
4. Persistent storage invariants and TTL handling prevent silent data loss.
5. Deployment and initialization procedures are front-run resistant.
6. Dependencies are free of known vulnerabilities.

---

## Auditor Recommendations

The Stellar Development Foundation (SDF) maintains the **Soroban Security Audit Bank**, a curated list of pre-approved audit firms with demonstrated Soroban/Rust expertise. Projects funded through the Stellar Community Fund (SCF) may qualify for subsidized or fully covered audits.

### Pre-Approved Audit Firms (Soroban Security Audit Bank)

| Firm | Specialization | Notable Soroban Audits |
|------|---------------|----------------------|
| **Veridise** | Core competency; audited Soroban Core itself. Proprietary automated analysis (AuditHub). | Soroban Core, Centiiv, RedStone Stellar Connector, Untangled Finance, OrbitCDP, Phoenix DEX |
| **Halborn** | Enterprise-grade security assessments, penetration testing. | ZKCross (Soroban zkCrossDex) |
| **Hacken** | Bridge accounting, trust boundaries, token forwarding. | ROZO Intents V2 (Token Forwarder / Intent Bridge) |
| **Certora** | Formal verification with mathematical proofs of correctness. | Pre-approved Audit Bank partner |
| **Oak Security** | Blinded parallel review process; extensive Rust experience since 2017. | Pre-approved Audit Bank partner |
| **OtterSec** | High-TVL protocol security; exploit prevention. | Pre-approved Audit Bank partner |
| **Spearbit + Cantina** | Decentralized elite researcher network with scalable triage. | Pre-approved Audit Bank partner |
| **Zellic** | Advanced cryptography research; complex Rust architectures. | Pre-approved Audit Bank partner |
| **Runtime Verification** | Formal methods, specification reviews, runtime verification. | Pre-approved Audit Bank partner |
| **ChainSecurity** | High-complexity codebase security since 2017. | Pre-approved Audit Bank partner |
| **Code4rena** | Crowdsourced competitive audits with 100+ researchers. | Pre-approved Audit Bank partner |

### Recommendation

For Mainstay's multi-contract architecture with cross-contract call flows, TTL-dependent storage, and DeFi collateral integration, **Veridise** or **Halborn** are the top recommendations due to their direct Soroban Core auditing experience and proven track record with Soroban DeFi protocols. For teams seeking formal verification of the collateral scoring model, **Certora** provides mathematical correctness guarantees.

> **SCF-funded projects:** Contact the Stellar Community Fund team to access the Soroban Security Audit Bank for subsidized audit services.
Mainstay manages real industrial asset records used as DeFi collateral across four smart contracts: AssetRegistry, EngineerRegistry, Lifecycle, and Lending. A formal, third-party security audit by a Soroban-specialized firm is required before mainnet deployment. This document defines the audit scope, recommends qualified firms, and provides the framework for tracking and resolving findings.

---

## Recommended Audit Firms

The following firms are recognized by the Stellar Development Foundation (SDF) through the **Soroban Security Audit Bank** program and specialize in Rust/WASM smart contract security:

### Tier 1 — Soroban Specialists (Recommended)

| Firm | Specialization | Engagement Model |
|------|---------------|-----------------|
| **[Certora](https://www.certora.com/)** | Formal verification via *Certora Sunbeam* for Soroban WASM bytecode; traditional audits | Direct or via SDF Audit Bank |
| **[OtterSec](https://osec.io/)** | Premier Rust/WASM security; $36B+ TVL secured; deep low-level exploit expertise | Direct engagement |
| **[Veridise](https://veridise.com/)** | Audited Soroban Core itself; advanced static analysis via *AuditHub*; ZK + Rust specialist | Direct or via SDF Audit Bank |

### Tier 2 — Strong Alternatives

| Firm | Specialization | Engagement Model |
|------|---------------|-----------------|
| **[Runtime Verification](https://runtimeverification.com/)** | Rigorous formal methods and mathematical modeling | Direct engagement |
| **[ChainSecurity](https://chainsecurity.com/)** | Complex DeFi infrastructure security | Direct engagement |
| **[Halborn](https://halborn.com/)** | Full-stack (smart contracts + infrastructure + social engineering) | Direct engagement |
| **[Oak Security](https://www.oaksecurity.io/)** | Blinded parallel auditing; Cosmos + Soroban Rust experience | Direct engagement |
| **[Zellic](https://www.zellic.io/)** | Cutting-edge cryptography and protocol security | Direct engagement |

### Funding Options

- **SDF Audit Bank:** Projects in the Stellar Community Fund (SCF) pipeline may qualify for up to 100% audit cost coverage. Apply via the SCF program before engaging a firm.
- **Competitive Platforms:** [Code4rena](https://code4rena.com/), [Cantina](https://cantina.xyz/), and [Spearbit](https://spearbit.com/) offer crowdsourced or researcher-network-based audit models.

**Recommendation:** Engage **two firms** for defense-in-depth — a Tier 1 Soroban specialist (Certora or Veridise) for formal verification / deep Rust analysis, paired with a Tier 2 firm for a complementary manual review.

---

## Audit Scope

### Contract Scope

The audit covers four Soroban smart contracts:

#### 1. Asset Registry (`contracts/asset-registry/`)
- Asset registration with deduplication (SHA-256 based)
- Owner → asset index management
- Asset type allowlist and counting
- Ownership transfer (with timelock for admin-initiated transfers)
- Asset deprecation and decommission lifecycle
- Multisig ownership transfer
- Asset lien recording for DeFi integration
- Asset search with metadata filtering
- Admin 2-step transfer
- Contract pause/unpause mechanism

#### 2. Engineer Registry (`contracts/engineer-registry/`)
- Trusted issuer management (add/remove)
- Engineer credential issuance with validity periods
- Credential verification (including grace period for expired credentials)
- Credential renewal, suspension, and revocation
- Credential expiry state machine (Valid → GracePeriod → HardExpired)
- Engineer specialization tracking
- Admin 2-step transfer
- Contract pause/unpause mechanism

#### 3. Lifecycle (`contracts/lifecycle/`)
- Maintenance submission and batch submission
- Cross-contract verification (AssetRegistry + EngineerRegistry)
- Collateral score computation (task-weighted + engineer reputation)
- Dual-model scoring: recency-weighted history + config decay
- Score history tracking and health snapshots
- Score trend queries and eligibility checks
- Maintenance history pruning (max_history cap)
- Asset transfer sentinel recording
- Engineer authorization (owner-gated per-asset)
- Admin configuration updates with timelock
- Decommission notification and score freezing
- Contract pause/unpause mechanism

#### 4. Lending (`contracts/lending/`)
- Loan request, repayment, and default handling
- Voucher staking with yield and slash mechanics
- Voucher history tracking
- Token transfer integration
- Yield BPS and slash BPS configuration
- Minimum stake and loan duration limits
- Contract pause/unpause mechanism
- Admin 2-step transfer

### Dependency Scope
- `soroban-sdk` version chain
- All transitive dependencies tracked in `Cargo.lock`
- Known vulnerability database (RustSec advisory DB via `cargo audit`)

### Off-Chain Scope
- Build scripts (`scripts/build.sh`)
- Deployment scripts (`scripts/deploy_testnet.sh`)
- CI pipeline security checks (`.github/workflows/ci.yml`)
- Secret scanning configuration (`.gitleaks.toml`)
- Repository access controls (`.github/CODEOWNERS`)

---

## Methodology

The auditor should apply the following methodology:

### 1. Manual Code Review
- Line-by-line review of all contract logic
- Cross-contract call flow analysis
- Access control verification for all admin and user functions
- State machine validation (credential lifecycle, asset lifecycle)

### 2. Automated Analysis
- Static analysis with Soroban-aware tooling
- Fuzz testing of input validation boundaries
- Invariant testing of storage consistency

### 3. Threat Modeling (STRIDE)
- Spoofing, Tampering, Repudiation, Information Disclosure, Denial of Service, Elevation of Privilege
- Reference: `docs/threat-model.md`

### 4. Soroban-Specific Checks
- Storage type correctness (persistent vs. instance vs. temporary)
- TTL extension coverage (all persistent keys extended on write)
- Host function boundary type safety
- Cross-contract call authorization
- Ledger-based timestamp handling

### 5. Dependency Audit
- `cargo audit` against RustSec advisory database
- Manual review of critical dependencies (soroban-sdk)

---

## Soroban-Specific Vulnerability Categories

The audit must specifically evaluate the following Soroban-specific vulnerability surfaces:

### SOR-01: TTL Expiry → Silent Data Loss
**Risk:** Persistent storage entries expire silently after TTL elapses, destroying asset records, maintenance history, engineer credentials, and configuration.

**Checklist:**
- [ ] Every `put`/`set` operation is followed by `extend_ttl`
- [ ] Instance storage TTL is extended on every admin-mutating function
- [ ] `PAUSED` key TTL is extended on `pause` and `unpause` (see issue #756)
- [ ] Read-only paths do not prematurely extend TTL in ways that could mask expiry bugs
- [ ] No critical data lives exclusively in temporary storage

### SOR-02: Pause Flag TTL Expiry
**Risk:** If the pause flag's TTL expires while the contract is paused during an incident, the contract can silently unpause (the `unwrap_or(false)` default returns `false`). This bypasses emergency controls.

**Checklist:**
- [ ] `pause()` calls `extend_ttl` on the `PAUSED` key after writing
- [ ] `unpause()` calls `extend_ttl` on the `PAUSED` key after writing
- [ ] All four contracts (AssetRegistry, EngineerRegistry, Lifecycle, Lending) have this protection

### SOR-03: Front-Running Initialization
**Risk:** Between deployment and initialization, an observer can call uninitialized functions or front-run `initialize`/`initialize_admin` to set themselves as admin.

**Checklist:**
- [ ] `initialize_admin` requires the deployer's authorization
- [ ] `initialize` in Lifecycle requires the deployer's authorization
- [ ] Deployment runbook mandates initialization in the same transaction block

### SOR-04: Cross-Contract Re-entrancy
**Risk:** Lifecycle makes multiple cross-contract calls (AssetRegistry + EngineerRegistry). An attacker could exploit re-entrant calls to manipulate state.

**Checklist:**
- [ ] Cross-contract calls are made after local validation and state writes
- [ ] No state is read from external contracts after writes (read before write)
- [ ] Asset existence is verified before scoring operations

### SOR-05: Storage Collision / Key Namespace
**Risk:** Multiple contracts or storage keys could collide, overwriting each other's data.

**Checklist:**
- [ ] All storage keys use unique, contract-specific prefixes
- [ ] Symbol-based keys are used consistently
- [ ] No overlapping key patterns between contracts

### SOR-06: Arithmetic Overflow/Underflow
**Risk:** Score calculations, decay computations, and BPS math could overflow or underflow.

**Checklist:**
- [ ] All arithmetic operations use checked or saturating operations
- [ ] Score clamping is applied after every computation
- [ ] BPS calculations cannot exceed 10,000 (100%)

### SOR-07: Authorization Bypass
**Risk:** Functions that should be admin-only or owner-only could be called by unauthorized parties.

**Checklist:**
- [ ] Admin functions require admin authorization (not just address comparison)
- [ ] Owner-only functions require the owner's authorization
- [ ] `require_auth()` is called for all mutating operations
- [ ] Engineer authorization is verified per-asset, not globally

### SOR-08: Timestamp Manipulation
**Risk:** Ledger-based timestamps could be manipulated by validators to alter decay calculations.

**Checklist:**
- [ ] Decay calculations have reasonable bounds
- [ ] Timestamp-based checks use ledger numbers where appropriate
- [ ] No economic advantage can be gained from short-term timestamp manipulation

### SOR-09: Unbounded Iteration
**Risk:** Functions that iterate over vectors (maintenance history, engineer lists) could exceed ledger compute limits.

**Checklist:**
- [ ] `max_history` is bounded (default 200) and configurable
- [ ] Pagination is supported for large queries (`get_maintenance_history`, `search_assets`)
- [ ] Batch operations have reasonable size limits
- [ ] No unbounded loop depends on user-controlled input

### SOR-10: Credential Expiry Edge Cases
**Risk:** The credential state machine (Valid → GracePeriod → HardExpired) must handle edge cases correctly, especially around the grace period boundary.

**Checklist:**
- [ ] Grace period enforcement is consistent
- [ ] Hard-expired credentials cannot be renewed (must be re-issued)
- [ ] Suspension and revocation correctly override all other states
- [ ] Credential status is always checked before maintenance submission

---

## Findings Classification

| Severity | Definition | Remediation Timeline |
|----------|-----------|---------------------|
| **Critical** | Direct loss of funds, permanent data destruction, or complete bypass of access control | Must be fixed before mainnet |
| **High** | Potential for loss of funds or data under specific conditions; significant contract state corruption | Must be fixed before mainnet |
| **Medium** | Unexpected behavior affecting contract reliability or user experience; partial bypass of intended constraints | Should be fixed before mainnet; may be deferred with documented justification |
| **Low** | Minor deviations from best practices; cosmetic or documentation issues | Should be addressed; does not block mainnet |
| **Informational** | Suggestions for improvement; no security impact | Optional |

---

## Findings

> *To be populated by the auditor during the engagement.*

| ID | Title | Severity | Contract | Status |
|----|-------|----------|----------|--------|
| — | *No findings yet* | — | — | — |

---

## Remediation Verification

> *To be completed after the audit. Each finding must be independently verified as resolved.*

| Finding ID | Remediation Commit | Verified By | Verification Date |
|------------|-------------------|-------------|-------------------|
| — | — | — | — |

### Remediation Requirements
1. Each fix must include a regression test that reproduces the original vulnerability.
2. Fixes must be reviewed by at least one independent developer (not the original fix author).
3. The auditor must confirm each finding is resolved before sign-off.
4. All critical and high-severity findings must be resolved before mainnet deployment.

---

## Audit Engagement Status

- [ ] Audit firm selected and engaged
- [ ] Kickoff meeting completed
- [ ] Code snapshot provided to auditor (commit hash: `________`)
- [ ] Initial report received
- [ ] Findings triaged and prioritized
- [ ] Remediation implemented for all Critical and High findings
- [ ] Remediation implemented for all Medium findings (or documented justification for deferral)
- [ ] Auditor re-review completed
- [ ] Final report published
- [ ] Sign-off letter received
### Contracts In-Scope

| Contract | Path | Lines (approx.) | Criticality |
|----------|------|-----------------|-------------|
| AssetRegistry | `contracts/asset-registry/src/lib.rs` | ~1,200 | **Critical** — Asset registration, ownership, deduplication, timelock, pause, admin |
| EngineerRegistry | `contracts/engineer-registry/src/lib.rs` | ~700 | **Critical** — Credential issuance, verification, revocation, reputation |
| Lifecycle | `contracts/lifecycle/src/lib.rs` | ~1,400 | **Critical** — Maintenance submission, collateral scoring, decay, cross-contract orchestration |
| Lending | `contracts/lending/src/lib.rs` | ~600 | **High** — Loan lifecycle, voucher staking, slashing, yield distribution |
| Shared | `contracts/shared/src/` | ~150 | **High** — Shared errors, validation helpers, TTL extension utilities |

### Supporting Modules

| Module | Path | Purpose |
|--------|------|---------|
| Lifecycle Scoring | `contracts/lifecycle/src/scoring.rs` | Decay computation, score history management, valuation history |
| Lifecycle Types | `contracts/lifecycle/src/types.rs` | Config, MaintenanceRecord, ScoreEntry, TransferRecord, BatchRecord, HealthSnapshot |
| Lifecycle Errors | `contracts/lifecycle/src/errors.rs` | Contract-specific error enum |
| Shared Validation | `contracts/shared/src/validation.rs` | Input validation helpers (`require_non_empty_vec`, `require_string_length`) |

### Threat Vectors to Assess

#### 1. Authorization & Access Control
- **Admin initialization:** `initialize_admin` is one-shot — verify no reinitialization or front-running is possible.
- **2-step admin transfer:** `propose_admin` → `accept_admin` flow — verify no race conditions, ensure pending admin cannot be overwritten mid-transfer.
- **M-of-N multisig:** `set_admin_quorum` / `require_quorum` — verify threshold enforcement, signer enumeration ordering, edge cases (threshold > signers, empty set, single-to-multi transitions).
- **Owner-gated operations:** Asset registration/deregistration/transfer, metadata updates, engineer authorization/revocation — verify `require_auth()` and ownership checks on every path.
- **Engineer authorization per-asset:** `authorize_engineer` / `require_engineer_authorized` — verify per-asset authorization is enforced on every `submit_maintenance` path, including after ownership transfers.
- **Pause mechanism:** Verify paused contracts reject all state-mutating operations. Verify unpause is admin-only. Verify the PAUSED_KEY TTL extension prevents silent unpause.

#### 2. Cross-Contract Call Safety
- **Lifecycle → AssetRegistry:** `verify_asset_exists`, `get_asset`, `asset_status` calls — verify panics are handled correctly and cannot brick the Lifecycle contract.
- **Lifecycle → EngineerRegistry:** `get_credential_status`, `verify_engineer`, `get_reputation`, `get_specializations` — verify fallback logic between `get_credential_status` and `verify_engineer` is consistent and cannot bypass credential checks.
- **Registry address mutability:** `update_asset_registry` / `update_engineer_registry` are admin-gated with 48-hour timelock — verify the new address cannot be a malicious contract.
- **Re-entrancy:** Cross-contract calls could theoretically re-enter — verify all state mutations happen before or after cross-contract calls, not interleaved.

#### 3. Collateral Scoring Integrity
- **Score accumulation:** `submit_maintenance` adds `weighted_increment` (task weight × engineer reputation multiplier) — verify no overflow, underflow, or wrap-around.
- **Decay computation:** `apply_decay` / `compute_decay` — verify elapsed-time calculation is monotonic, verify the `last_update` timestamp cannot be manipulated.
- **Dual-model scoring:** History-score (Model A) and config-score (Model B) run in parallel; the **lower** value wins — verify this is intentional and correctly implemented for all edge cases.
- **Score floor:** `MIN_SCORE_WITH_HISTORY = 1` — verify it applies only when history is non-empty.
- **Decommissioned/frozen assets:** Score frozen at decommission time; verify `get_collateral_score` returns the frozen score (not a recomputed value).
- **Deprecated assets:** Return 0 immediately; verify no score computation or side effects occur.
- **Dynamic frequency weighting:** `apply_dynamic_frequency_weight` parses JSON-like bytes — verify no panic on malformed input, verify weight ranges are bounded.
- **Score cap:** Verify score is clamped to [0, 100] on every write path.

#### 4. Storage TTL & Data Persistence
- **Instance storage TTL:** Verify every admin-mutating function extends instance TTL (`initialize_admin`, `propose_admin`, `accept_admin`, `pause`, `unpause`, `add_trusted_issuer`, `remove_trusted_issuer`).
- **Persistent storage TTL:** Verify every `set` operation is followed by `extend_persistent_ttl` or `extend_ttl(518400, 518400)`.
- **PAUSED_KEY TTL:** Critical hazard — if TTL expires while paused, the contract silently unpauses. Verify `pause` and `unpause` both extend TTL.
- **CONFIG TTL:** If this key expires, the contract becomes fully inoperable. Verify TTL extension on `initialize` and every `update_*` / `set_*` function.
- **REGISTRY / ENG_REG TTL:** If these expire, all cross-contract calls panic. These are set once at `initialize` — verify TTL is extended periodically or ensure they have a sufficiently long initial TTL.
- **SCORE / HIST / SCHIST / LUPD TTL:** Verify all score-related keys have their TTL extended on every write.

#### 5. Input Validation & Edge Cases
- **Asset registration deduplication:** SHA-256 of serial_number (global) AND SHA-256 of (owner, asset_type, metadata) — verify no hash collision bypass.
- **Batch operations:** `batch_register_assets`, `batch_submit_maintenance` — verify MAX_BATCH_SIZE enforcement, in-batch deduplication, atomicity on failure.
- **Notes length validation:** `validate_notes_length` — verify empty notes are rejected (`notes.is_empty()` panics).
- **Asset type validation:** XDR byte-level symbol validation — verify only `[A-Za-z0-9_]` characters are accepted.
- **Timelock enforcement:** `TIMELOCK_DELAY_SECS = 48 hours` — verify timestamp-based comparison (not ledger sequence), verify proposal cannot be re-proposed while pending.
- **Zero-address checks:** `is_zero_address` — verify admin and registry addresses cannot be the Stellar zero address.
- **Pagination:** `get_assets_by_owner_paginated`, `get_assets_by_type_paginated` — verify overflow protection on `page * page_size`.

#### 6. Lending Contract Specifics
- **Loan lifecycle:** `request_loan` → `repay_loan` / `default_loan` — verify state transitions, double-repay prevention, default-after-deadline enforcement.
- **Voucher staking:** `vouch` / `unvouch` — verify minimum stake, duplicate vouch prevention, unvouch during active loan.
- **Slashing:** Verify slashed funds are correctly credited to the slash balance and can only be withdrawn by admin.
- **Yield distribution:** Verify yield basis points calculation on repayment, verify voucher history updates.
- **Token transfers:** Verify the payment token contract address is immutable after initialization.

#### 7. Event Emission & Audit Trail
- All state-mutating operations must emit events with sufficient context for indexers.
- Verify `ADM_AUD` events are emitted for all admin actions (init, propose, accept, pause, unpause, config changes).
- Verify maintenance, transfer, and deregistration events carry all necessary identifiers.

#### 8. Dependency & Build Security
- Verify `Cargo.toml` dependencies are pinned to specific versions (not wildcards).
- Verify no `unsafe` Rust blocks exist in contract code.
- Verify the `soroban-sdk` version is the latest stable release with no known vulnerabilities.
- Verify the WASM build is optimized (`--release`) and free of debug symbols that could leak internals.

#### 9. Deployment & Initialization Security
- **Front-running prevention:** All three contracts must be initialized in the same transaction block as deployment (see `deployment-runbook.md` §1–§4).
- **Deployer-only initialization:** `initialize_admin` requires `deployer.require_auth()` — verify no other address can initialize.
- **One-shot initialization:** Verify `AlreadyInitialized` / `AdminAlreadyInitialized` errors prevent re-initialization.
- **Registry binding immutability:** After `Lifecycle::initialize`, the asset and engineer registry addresses are stored in persistent storage — verify they cannot be changed without admin timelock.

---

## Security Considerations Specific to Mainstay

### High-Value Target Profile
Mainstay bridges **real-world industrial assets** (generators, turbines, heavy machinery) with **DeFi lending protocols**. A compromise could:
- Allow fraudulent maintenance records to inflate collateral scores.
- Enable unauthorized asset transfers of high-value machinery.
- Let unverified engineers submit maintenance, undermining the credentialing trust model.
- Cause score manipulation that triggers unjustified liquidations or loans.

### Soroban-Specific Concerns
- **Host-boundary type safety:** Raw host values (`Vec`, `Map<K,V>`, `Bytes`) must be validated before use to prevent execution halts.
- **Storage rent economics:** Persistent storage entries incur rent costs. Unbounded growth (e.g., per-asset history without a cap) could make the contract economically unviable.
- **TTL silent expiration:** Unlike EVM storage which is permanent-by-default, Soroban persistent entries expire silently. The PAUSED_KEY TTL hazard is especially critical.
- **Cross-contract call semantics:** Soroban cross-contract calls are synchronous. Panics in the target contract propagate to the caller. Verify the Lifecycle contract handles all possible panic scenarios from registries.

---

## Findings Tracking

Findings should be categorized using the following severity scale:

| Severity | Definition | Examples |
|----------|-----------|----------|
| **Critical** | Direct loss of funds, permanent contract lockup, or complete bypass of access control | Unauthorized admin takeover, score manipulation to 100, PAUSED_KEY TTL expiry unpausing during incident |
| **High** | Significant functionality broken or exploitable with limited preconditions | Bypass engineer verification, cross-contract re-entrancy, storage collision |
| **Medium** | Bug that could cause harm under specific conditions or degrade security posture | Missing event emission, unbounded Vec growth, integer overflow in edge case |
| **Low** | Best-practice deviation with no immediate exploit vector | Missing input validation on non-critical parameter, inconsistent error messages |
| **Informational** | Suggestions for code quality, gas optimization, or documentation improvements | Redundant code, missing doc comments, suboptimal storage layout |

### Findings Log

| ID | Severity | Contract | Description | Status |
|----|----------|----------|-------------|--------|
| — | — | — | *Awaiting audit firm engagement* | — |

---

## Audit Process Checklist

### Phase 1: Pre-Audit Preparation
- [ ] Finalize and freeze the contract codebase (tag a release candidate).
- [ ] Run the full test suite with coverage report: `./scripts/test.sh`.
- [ ] Run `cargo clippy` with all lints enabled and resolve all warnings.
- [ ] Run `cargo audit` to check dependency vulnerabilities.
- [ ] Complete internal threat modeling review (STRIDE framework).
- [ ] Document all security invariants in `docs/architecture.md` and function-level doc comments.
- [ ] Verify all admin functions emit `ADM_AUD` events.
- [ ] Verify the deployment runbook's initialization sequence is correct on testnet.

### Phase 2: Firm Engagement
- [ ] Apply to SDF Audit Bank (if SCF-funded).
- [ ] Select primary audit firm (Tier 1 Soroban specialist).
- [ ] Select secondary audit firm (optional, for defense-in-depth).
- [ ] Negotiate scope, timeline, and deliverables.
- [ ] Provide auditors with:
  - Full source code (tagged release).
  - Architecture documentation (`docs/architecture.md`).
  - TTL strategy documentation (`docs/ttl-strategy.md`).
  - Threat model and known risks.
  - Test suite and coverage report.
  - Deployment runbook.

### Phase 3: Audit Execution
- [ ] Initial report delivered by audit firm(s).
- [ ] Triage all findings by severity.
- [ ] Develop remediation plan for each finding.

### Phase 4: Remediation
- [ ] Fix all **Critical** and **High** severity findings.
- [ ] Fix all **Medium** severity findings or document accepted risk.
- [ ] Address **Low** and **Informational** findings as appropriate.
- [ ] Re-run full test suite after all changes.
- [ ] Request re-review of fixes from audit firm(s).

### Phase 5: Finalization
- [ ] Audit firm(s) sign off on all remediated findings.
- [ ] Final audit report received and published.
- [ ] All findings and resolutions documented in this report's Findings Log.
- [ ] Mainnet deployment checklist in `deployment-runbook.md` completed.
- [ ] Audit report linked from project README.

---

## Sign-Off

> *To be signed by the auditing firm after all critical and high-severity findings are resolved and verified.*

**Auditor:** ______________________________

**Date:** ______________________________

**Statement:** "We have reviewed Mainstay smart contracts at commit `________`. All critical and high-severity findings identified during the audit have been resolved and verified. Based on our assessment, the contracts are ready for mainnet deployment."

---

## References

- [Soroban Security Audit Bank](https://soroban.stellar.org/docs/reference/security-audit-bank)
- [Stellar Community Fund Handbook](https://communityfund.stellar.org/)
- [Mainstay Architecture Overview](architecture.md)
- [Mainstay Threat Model](threat-model.md)
- [Mainstay Deployment Runbook](deployment-runbook.md)
- [Mainstay Security Policy](../SECURITY.md)
- [TTL Strategy](ttl-strategy.md)
- [Access Control Model](access-control.md)
| Role | Name / Firm | Date | Signature |
|------|------------|------|-----------|
| Primary Auditor | *TBD* | | |
| Secondary Auditor | *TBD* | | |
| Project Lead | *TBD* | | |

---

*This report template incorporates guidance from the Stellar Development Foundation's Soroban Security Audit Bank program and follows industry best practices for Rust/WASM smart contract auditing.*
