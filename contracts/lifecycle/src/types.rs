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

/// Urgency/severity of a maintenance task.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Priority {
    Low,
    Medium,
    High,
    Critical,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaintenanceRecord {
    pub asset_id: u64,
    pub task_type: Symbol,
    pub priority: Priority,
    pub notes: String,
    pub engineer: Address,
    pub timestamp: u64,
    /// Maintenance cost in stroops (1 stroop = 10^-7 XLM).
    /// `None` indicates no cost was recorded for this maintenance event.
    pub cost: Option<u64>,
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
    pub score_increment: u32,
    pub decay_rate: u32,
    pub decay_interval: u64,
    pub eligibility_threshold: u32,
    pub max_notes_length: u32,
    pub task_weights: Map<Symbol, u32>,
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
    pub timestamp: u64,
    pub score: u32,
    pub maintenance_count: u32,
    pub last_service_date: u64,
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
}
