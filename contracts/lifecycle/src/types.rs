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
    /// Stores `Vec<(timestamp: u64, value: u64)>` collateral-valuation history for an asset.
    CollateralValuationHistory(u64),
}
