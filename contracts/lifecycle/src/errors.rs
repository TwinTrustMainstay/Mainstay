#![no_std]

use shared::error::SharedContractError;
use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    NoMaintenanceHistory = 1,
    UnauthorizedEngineer = 2,
    UnauthorizedAdmin = 3,
    HistoryCapReached = 4,
    AssetNotFound = 5,
    NotInitialized = 6,
    AlreadyInitialized = 7,
    InvalidConfig = 8,
    Paused = 9,
    InvalidTaskType = 10,
    PendingAdminAlreadyExists = 11,
    ZeroAddress = 12,
    SameRegistryAddress = 13,
    IndexOutOfBounds = 14,
    UnauthorizedOwner = 15,
    EngineerNotAuthorized = 16,
    TimelockNotExpired = 17,
    ProposalNotFound = 18,
    ScoreOverflow = 19,
    /// Notes field exceeds the configured maximum length.
    NotesTooLong = 20,
    /// Asset score is frozen due to decommission; decay and mutation are blocked.
    ScoreFrozen = 21,
    /// Asset is decommissioned and cannot accept maintenance records.
    AssetDecommissioned = 22,
    /// Batch submission exceeds the maximum allowed batch size (DoS / gas-limit guard).
    BatchTooLarge = 23,
    /// Fewer valid signers were provided than the configured admin_threshold requires.
    InsufficientSigners = 24,
    /// Engineer's specialization does not match the asset's type.
    SpecializationMismatch = 25,
    /// No recurring task exists with the given task_id for this asset.
    RecurringTaskNotFound = 26,
    /// The recurring task exists but is not active.
    RecurringTaskInactive = 27,
    /// A recurring task with an equivalent schedule already exists for this asset.
    DuplicateRecurringTask = 28,
    /// Recurring task interval_type/interval_value combination is invalid.
    InvalidRecurringSchedule = 29,
    /// No duplicate maintenance record exists with the given timestamp.
    DuplicateRecordNotFound = 30,
    /// A compliance standard is already registered for this asset type.
    StandardAlreadyRegistered = 31,
    /// Not enough historical data points to compute a prediction.
    InsufficientPredictionData = 32,
    /// Engineer has exceeded `max_submissions_per_hour` maintenance submissions
    /// in the current rolling-hour window.
    RateLimitExceeded = 33,
}

impl From<SharedContractError> for ContractError {
    fn from(e: SharedContractError) -> Self {
        match e {
            SharedContractError::NotInitialized => ContractError::NotInitialized,
            SharedContractError::AlreadyInitialized => ContractError::AlreadyInitialized,
            SharedContractError::UnauthorizedAdmin => ContractError::UnauthorizedAdmin,
            SharedContractError::Paused => ContractError::Paused,
            SharedContractError::TimelockNotExpired => ContractError::TimelockNotExpired,
            SharedContractError::ProposalNotFound => ContractError::ProposalNotFound,
            SharedContractError::PendingAdminAlreadyExists => ContractError::PendingAdminAlreadyExists,
        }
    }
}
