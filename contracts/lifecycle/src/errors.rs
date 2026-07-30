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
    /// The admins list supplied to set_admin_quorum contains a duplicate address.
    DuplicateAdmin = 25,
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
