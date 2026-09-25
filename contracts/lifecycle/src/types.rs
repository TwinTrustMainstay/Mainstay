#![no_std]

use soroban_sdk::{contracttype, Address, Bytes, String, Symbol, Map, Vec};

/// A single ownership-transfer event recorded in the on-chain transfer history.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferRecord {
    pub from: Address,
    pub to: Address,
    pub timestamp: u64,
}

/// Priority level of a maintenance task.
///
/// Used to triage which records are most critical for asset health scoring
/// and DeFi collateral purposes.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Priority level for a maintenance record.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Priority {
    Low = 0,
    Medium = 1,
    High = 2,
    Critical = 3,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaintenanceRecord {
    pub asset_id: u64,
    pub task_type: Symbol,
    /// Priority level of this maintenance task.
    /// Maintenance priority level.
    pub priority: Priority,
    pub notes: String,
    pub engineer: Address,
    pub timestamp: u64,
    /// Maintenance cost in stroops (1 stroop = 10^-7 XLM).
    /// `None` indicates no cost was recorded for this maintenance event.
    pub cost: Option<u64>,
    /// The ledger sequence number at which the current ownership period started.
    ///
    /// Set to `Some(ledger)` on the XFER sentinel written by `record_transfer`
    /// and propagated to all subsequent records in that ownership period.
    /// `None` for records created before any transfer has occurred.
    ///
    /// DeFi lenders can use this field together with
    /// [`LifecycleContract::get_maintenance_history_since_transfer`] to isolate
    /// the maintenance history that belongs to the current owner's tenure.
    pub ownership_start_ledger: Option<u64>,
    /// Sha256 hash of the previous record in this asset's history, forming a
    /// tamper-evident hash chain over the (possibly TTL/cap-pruned) history.
    /// `None` for the oldest record currently visible for this asset.
    pub previous_record_hash: Option<Bytes>,
    /// Whether this record was reconstructed from health snapshots (#1314).
    /// Reconstructed records are synthetic placeholders generated to recover
    /// approximate history after TTL-driven pruning, not actual submissions.
    pub reconstructed: bool,
}

/// A point-in-time snapshot of the collateral score, recorded at each maintenance event.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScoreEntry {
    pub timestamp: u64,
    pub score: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchRecord {
    pub task_type: Symbol,
    /// Priority level of this maintenance task.
    pub priority: Priority,
    pub notes: String,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub admin: Address,
    /// All addresses eligible to co-sign admin operations.
    /// When empty, `admin` alone controls all operations (single-admin mode).
    pub admins: Vec<Address>,
    /// Minimum number of signatures from `admins` required to execute critical operations.
    /// Ignored when `admins` is empty (single-admin mode) or when set to 0 / 1.
    pub admin_threshold: u32,
    pub max_history: u32,
    /// Maximum number of asset IDs retained in an engineer's per-address history.
    /// When the list reaches this cap the oldest entry is dropped before the new
    /// one is appended (sliding-window pruning).  A value of `0` is treated as
    /// "use the contract default" and is replaced with
    /// `DEFAULT_MAX_ENGINEER_HISTORY` at initialisation time.
    pub max_engineer_history: u32,
    pub score_increment: u32,
    pub decay_rate: u32,
    pub decay_interval: u64,
    pub eligibility_threshold: u32,
    /// Minimum collateral score required for an asset to be considered eligible.
    pub min_collateral_score: u32,
    pub max_notes_length: u32,
    pub task_weights: Map<Symbol, u32>,
    /// Maximum maintenance-record submissions a single engineer may make in any
    /// rolling-hour window, across `submit_maintenance` and
    /// `batch_submit_maintenance` (each record in a batch counts individually).
    /// `0` disables rate limiting entirely.
    pub max_submissions_per_hour: u32,
    /// Weight assigned to task types that are not listed in `task_weights` and
    /// do not match any of the built-in hardcoded types.
    ///
    /// A value of `0` uses the contract-level default (`DEFAULT_TASK_WEIGHT`).
    /// Setting this to a non-zero value lets operators gracefully handle new or
    /// custom task types without having to call `set_task_weight` first, and
    /// eliminates the `InvalidTaskType` panic that previously blocked
    /// maintenance submissions for unknown types (see issue #1200).
    pub default_task_weight: u32,
    /// Maximum number of health snapshots retained per asset in
    /// `HealthSnapshots(asset_id)`. When `take_health_snapshot` would push the
    /// list past this cap, the oldest snapshot(s) are evicted first so the
    /// list stays bounded regardless of how often the caller snapshots.
    /// A value of `0` is treated as "use the contract default" and is
    /// replaced with `DEFAULT_MAX_SNAPSHOTS` at initialisation time.
    pub max_snapshots: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelockProposal {
    pub proposed_at: u64,
    pub executed: bool,
}

/// A point-in-time snapshot of an asset's health, persisted independently of
/// maintenance history so lenders can verify condition even after TTL-driven pruning.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthSnapshot {
    pub snapshot_timestamp: u64,
    pub score: u32,
    pub maintenance_count: u32,
    pub last_service_date: u64,
    /// Whether this snapshot was used as an anchor for reconstructed history.
    /// Set to `true` by `anchor_history_to_snapshot` to mark that lost or pruned
    /// maintenance records have been partially recovered via this snapshot.
    pub reconstructed: bool,
}

/// A comprehensive snapshot of an asset's complete state, including all critical
/// on-chain data from both the asset registry and lifecycle contract.
/// Used for off-chain backup and recovery purposes.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetFullSnapshot {
    /// Asset ID (unique identifier)
    pub asset_id: u64,
    /// Asset type classification (e.g., equipment category)
    pub asset_type: Symbol,
    /// Asset metadata description
    pub metadata: String,
    /// Physical serial number of the asset
    pub serial_number: String,
    /// Current owner address
    pub owner: Address,
    /// Timestamp when asset was registered
    pub registered_at: u64,
    /// Timestamp of last metadata update
    pub metadata_updated_at: u64,
    /// Version number of asset metadata
    pub metadata_version: u32,
    /// Asset deprecation status (0=Active, 1=Deprecated, 2=Decommissioned)
    pub deprecation_status: u32,
    /// Whether asset is locked as collateral
    pub is_locked: bool,
    /// Lender address if locked (None if not locked)
    pub lender: Option<Address>,
    /// Loan ID if locked (None if not locked)
    pub loan_id: Option<u64>,
    /// Deprecation timestamp (None if still active)
    pub deprecated_at: Option<u64>,
    /// Current collateral score (0-100)
    pub collateral_score: u32,
    /// Collateral valuation (historical value in stroops)
    pub collateral_valuation: u64,
    /// Timestamp of snapshot (when this data was captured)
    pub snapshot_timestamp: u64,
    /// Total maintenance records for this asset
    pub total_maintenance_records: u32,
    /// Timestamp of last maintenance service
    pub last_service_timestamp: u64,
}

/// Aggregated collateral health for the assets currently owned by an address.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollateralPortfolioHealth {
    pub asset_count: u32,
    pub eligible_asset_count: u32,
    pub locked_asset_count: u32,
    pub total_collateral_score: u64,
    pub average_collateral_score: u32,
}

/// Maintenance activity and cost metrics for one engineer.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineerProductivity {
    pub asset_count: u32,
    pub maintenance_count: u32,
    pub total_cost: u64,
    pub average_cost: u64,
    pub last_activity: Option<u64>,
}

/// Historical maintenance cost analysis and a deterministic forward estimate.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CostAnalytics {
    pub total_cost: u64,
    pub recorded_cost_count: u32,
    pub average_cost: u64,
    pub last_cost: Option<u64>,
    pub forecast_cost: u64,
}

/// An on-chain governance proposal to change a task-type score weight.
///
/// Created by `propose_weight_change`; consumed (executed) by `execute_weight_change`
/// after the `TIMELOCK_DELAY_SECS` delay has elapsed.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeightProposal {
    /// The new weight value proposed for the task type.
    pub new_weight: u32,
    /// Ledger timestamp at which the proposal was created.
    pub proposed_at: u64,
    /// Whether this proposal has already been executed.
    pub executed: bool,
}

/// External score submitted by a provider for cross-contract consensus (Issue #1637).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalScoreEntry {
    pub provider: Address,
    pub score: u32,
    pub timestamp: u64,
    /// Reputation score of the provider (0-100, higher is better).
    pub provider_reputation: u32,
}

/// Score anomaly detection entry (Issue #1639).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScoreAnomaly {
    pub asset_id: u64,
    pub timestamp: u64,
    pub baseline_score: u32,
    pub observed_score: u32,
    pub standard_deviation_multiple: u32, // Multiple of stdev (e.g., 2 for >2 stdev)
    pub investigation_status: Symbol, // "PENDING", "RESOLVED", "FALSE_POSITIVE"
}

/// Degradation curve parameters per asset category (Issue #1638).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DegradationCurve {
    pub category: Symbol,
    pub initial_rate: u32,    // Initial degradation rate per interval
    pub acceleration: u32,     // How much the rate accelerates per interval
    pub floor: u32,           // Minimum score that cannot be degraded further
    pub version: u32,         // Curve version for tracking changes
    pub created_at: u64,      // Timestamp when curve was created
}

/// Peer group statistics for relative scoring (Issue #1640).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerGroup {
    pub group_id: Symbol,
    pub asset_type: Symbol,
    pub age_range_min: u64,   // Minimum age in seconds
    pub age_range_max: u64,   // Maximum age in seconds
    pub member_count: u32,
    pub mean_score: u32,
    pub median_score: u32,
    pub stdev: u32,
}

/// A recurring maintenance task definition.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecurringTask {
    pub task_id: u64,
    pub task_type: Symbol,
    /// Unit for the recurrence interval (e.g., "HOURS", "DAYS", "MONTHS", "CYCLES").
    pub interval_type: Symbol,
    /// Numeric value for the interval (e.g., 500 for "every 500 hours").
    pub interval_value: u64,
    /// Timestamp when the next maintenance is due.
    pub next_due: u64,
    /// Whether this recurring task is active.
    pub is_active: bool,
}

/// Status of an asset retirement process.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetirementStatus {
    Pending = 0,
    Confirmed = 1,
    Cancelled = 2,
    Archived = 3,
}

/// Asset retirement state during the decommissioning workflow.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetirementState {
    pub asset_id: u64,
    pub status: RetirementStatus,
    pub initiated_at: u64,
    pub initiated_by: Address,
    pub reason: Bytes,
    pub review_period_end: u64,
    pub final_score: Option<u32>,
}

/// Certificate issued upon retirement of an asset.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetirementCertificate {
    pub asset_id: u64,
    pub retired_at: u64,
    pub final_score: u32,
    pub total_maintenance_count: u32,
    pub total_cost: u64,
    pub reason: Bytes,
    pub certificate_hash: Bytes,
}

/// Status of a coordinated maintenance task across multiple assets.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoordinationStatus {
    Active = 0,
    Completed = 1,
    Failed = 2,
    Cancelled = 3,
}

/// Coordinated maintenance task spanning multiple assets.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoordinatedTask {
    pub task_id: u64,
    pub asset_ids: Vec<u64>,
    pub task_type: Symbol,
    pub status: CoordinationStatus,
    pub created_at: u64,
    pub created_by: Address,
    pub completion_deadline: u64,
    pub completed_at: Option<u64>,
}

/// Subtask status for a coordinated task on a specific asset.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoordinatedSubtask {
    pub asset_id: u64,
    pub subtask_id: u64,
    pub status: CoordinationStatus,
    pub started_at: Option<u64>,
    pub completed_at: Option<u64>,
    pub notes: Bytes,
}

/// Season enum for seasonal score adjustments.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Season {
    Winter = 0,
    Spring = 1,
    Summer = 2,
    Fall = 3,
}

/// Seasonal adjustment factors for an asset type.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeasonalAdjustment {
    pub asset_type: Symbol,
    pub winter_factor: u32,
    pub spring_factor: u32,
    pub summer_factor: u32,
    pub fall_factor: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub enum DataKey {
    AssetRegistry,
    EngineerRegistry,
    Config,
    Paused,
    PendingAdmin,
    History(u64),
    Score(u64),
    ScoreHistory(u64),
    LastUpdate(u64),
    EngineerHistory(Address),
    EngineerAuth(u64, Address),
    Timelock(Symbol),
    HealthSnapshots(u64),
    TransferHistory(u64),
    /// Stores `Vec<RecurringTask>` for a given asset.
    RecurringTasks(u64),
    /// Stores duplicate maintenance record IDs per asset.
    DuplicateRecords(u64),
    /// Stores `Vec<(timestamp, value)>` collateral valuation snapshots for an asset.
    CollateralValuationHistory(u64),
    /// Stores `Option<u64>` ledger sequence number of the most recent ownership transfer
    /// for an asset. `None` means the asset has never been transferred.  Set by
    /// `record_transfer` and read by `submit_maintenance` / `batch_submit_maintenance`
    /// to stamp the `ownership_start_ledger` field on new records.
    OwnershipStartLedger(u64),
    /// Stores a `WeightProposal` for the given task-type symbol.
    WeightProposal(Symbol),
    /// Stores `Vec<DisputeRecord>` for a given asset (issue #1319).
    Disputes(u64),
}

/// A dispute record for challenging maintenance record authenticity (issue #1319).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisputeRecord {
    pub asset_id: u64,
    pub maintenance_timestamp: u64,
    pub reason: String,
    pub disputed_at: u64,
    pub is_resolved: bool,
    pub admin_decision: Option<Symbol>, // "UPHELD", "REJECTED", etc.
}
