use soroban_sdk::contracterror;

/// Common error variants shared across all contracts.
///
/// Each contract defines its own `ContractError` enum with contract-specific
/// discriminant values. This enum captures the overlapping variants and provides
/// a canonical mapping via `From<SharedContractError>` impls in each contract.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum SharedContractError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    UnauthorizedAdmin = 3,
    Paused = 4,
    TimelockNotExpired = 5,
    ProposalNotFound = 6,
    PendingAdminAlreadyExists = 7,
}
