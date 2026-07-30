# Security Audit Report

**Project:** Mainstay — Industrial Asset Collateral & DeFi Integration  
**Platform:** Stellar Soroban (Rust / WASM)  
**Date:** July 30, 2026  
**Status:** Pre-Audit — Firm Selection Phase  

---

## Executive Summary

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

| Role | Name / Firm | Date | Signature |
|------|------------|------|-----------|
| Primary Auditor | *TBD* | | |
| Secondary Auditor | *TBD* | | |
| Project Lead | *TBD* | | |

---

*This report template incorporates guidance from the Stellar Development Foundation's Soroban Security Audit Bank program and follows industry best practices for Rust/WASM smart contract auditing.*
