# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `.github/CODEOWNERS`: required-reviewer rules gate all PRs touching `contracts/`, CI workflows, `SECURITY.md`, and the threat-model doc (closes [#781](https://github.com/TwinTrustMainstay/Mainstay/issues/781))
- `CONTRIBUTING.md`: documented branch-protection requirements (1 approval + passing CI, no force-push) and the CODEOWNERS review expectation for `contracts/` (closes [#781](https://github.com/TwinTrustMainstay/Mainstay/issues/781))
- `test_cross_owner_duplicate_serial_rejected` in `asset-registry`: verifies that the global serial-number dedup key blocks a second owner from registering the same physical machine (closes [#782](https://github.com/TwinTrustMainstay/Mainstay/issues/782))
- `test_decay_score_never_drops_to_zero_with_history` in `lifecycle`: verifies that calling `decay_score` on an asset with maintenance records never stores or returns 0 (closes [#784](https://github.com/TwinTrustMainstay/Mainstay/issues/784))
- `ContractError` (`lifecycle/src/errors.rs`) variants added since 1.0.0: `ScoreOverflow` (19), `NotesTooLong` (20), `ScoreFrozen` (21), `AssetDecommissioned` (22), `BatchTooLarge` (23), `InsufficientSigners` (24), `Reentrancy` (25), `DuplicateAdmin` (26), `SnapshotNotFound` (27), `InsufficientPredictionData` (28), `WeightProposalAlreadyExists` (29), `SpecializationMismatch` (30), `RecurringTaskNotFound` (31), `RecurringTaskInactive` (32), `DuplicateRecurringTask` (33), `InvalidRecurringSchedule` (34), `DuplicateRecordNotFound` (35), `StandardAlreadyRegistered` (36), `RateLimitExceeded` (37), `BatchRevokeTooLarge` (38)
- `Config` (`lifecycle/src/types.rs`) fields added since 1.0.0: `admins` and `admin_threshold` (multisig admin quorum), `max_engineer_history` (per-engineer history cap), `min_collateral_score`, `max_notes_length`, `task_weights`, `max_submissions_per_hour` (engineer rate limiting)
- Storage keys (`lifecycle/src/storage.rs`) added since 1.0.0: `SCHIST` (score history), `LUPD` (last score update timestamp), `FROZEN` / `FRZ_SCR` (decommission freeze flag and frozen score), `HLTH_SNP` (health snapshots), `XFER_HIST` (ownership transfer history), `ENG_HIST` / `ENG_AUTH` (per-engineer history and per-asset authorization), `SUB_WIN` (rolling-hour submission rate window), `RVK_TL` / `TL_PROP` (timelocked engineer-revocation and generic admin proposals), `MSTD` / `SCR_WGT` (per-asset-type maintenance standards and scoring weights)

### Fixed
- `apply_decay` in `lifecycle/src/scoring.rs`: enforce `MIN_SCORE_WITH_HISTORY = 1` in both the main decay path and the early-return path so the stored score is never written as 0 for an asset that has at least one maintenance record (closes [#784](https://github.com/TwinTrustMainstay/Mainstay/issues/784))
- `get_maintenance_history_page` in `lifecycle`: cap the caller-supplied `limit` at `MAX_PAGE_SIZE` (100) so a large `limit` can no longer reintroduce an unbounded, potentially oversized response (closes [#1075](https://github.com/TwinTrustMainstay/Mainstay/issues/1075))
- `HealthSnapshot` in `lifecycle`: renamed the `timestamp` field to `snapshot_timestamp` to make explicit that it records when the snapshot was captured (closes [#1076](https://github.com/TwinTrustMainstay/Mainstay/issues/1076))

### Security
- `initialize_admin` in `asset-registry` already required `deployer.require_auth()`, preventing front-run attacks; the existing `test_initialize_admin_rejects_non_deployer` test and deployment-runbook section 3 now explicitly document this protection (closes [#783](https://github.com/TwinTrustMainstay/Mainstay/issues/783))

## [1.0.0] - 2026-06-02

### Added

- **Asset Registry Contract**: Foundation contract for registering industrial assets with unique on-chain identities
- **Engineer Verification System**: Federated credentialing system for certified maintenance engineers
- **Lifecycle Tracking**: Soroban-based lifecycle contracts that track full asset maintenance history
- **Maintenance Event Signing**: Cryptographic signing and submission of maintenance records by verified engineers
- **Collateral Scoring System**: On-chain health scoring derived from verified maintenance completeness
- **DeFi Integration**: Support for using verified assets as collateral in Stellar-based lending protocols
- **Cross-platform Testing**: Comprehensive test suite with Windows PowerShell and Unix shell support
- **Emergency Pause Mechanism**: Administrative controls for emergency contract pausing
- **Loan Deadline Enforcement**: Time-based loan management with deadline tracking
- **TTL (Time-to-Live) Strategy**: Configurable asset lifecycle management based on time parameters
- **Lending Features**: Core lending functionality including loan issuance and collateral management
- **Voucher History Tracking**: Historical records for voucher-based asset operations
- **Admin Transfer Capabilities**: Privileged operations for administrative asset transfers
- **Full E2E Testing**: End-to-end integration tests covering complete user workflows

### Documentation

- Comprehensive architecture documentation
- Access control model documentation
- Collateral scoring methodology
- TTL (Time-to-Live) strategy guide
- Credentialing system overview
- Deployment runbook for testnet
- Audit report documentation
- Contributing guidelines
- Security policy

### Infrastructure

- CI/CD pipeline with automated testing
- Code quality checks (Clippy, rustfmt)
- Security scanning (cargo audit)
- Build and deployment scripts
- Cross-platform support (Windows PowerShell, Unix)

### Technical Details

- **Language**: Rust
- **Blockchain**: Stellar/Soroban
- **Minimum Rust Version**: 1.70+
- **Smart Contracts**: Multiple specialized contracts (Asset, Asset-Registry, Engineer-Registry, Lending, Lifecycle, Shared)
