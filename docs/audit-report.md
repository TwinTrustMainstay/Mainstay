# Security Audit Report

**Project:** Mainstay — Proof of Maintenance for Industrial Assets
**Date:** TBD (post-audit)
**Version:** 0.1.x
**Auditor:** TBD

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
