/// Errors that may occur during lifecycle operations.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    /// Maintenance record not found for the given ID.
    NotFound = 1,
    /// Unauthorized attempt to modify lifecycle state.
    Unauthorized = 2,
    /// Score reset attempted on an invalid state.
    InvalidReset = 3,
}
