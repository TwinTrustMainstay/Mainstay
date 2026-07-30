#![no_std]

pub mod error;
pub mod validation;

use soroban_sdk::{Address, Env, IntoVal, Val};

/// Ledger TTL threshold and target for persistent storage entries.
/// 1 ledger ≈ 5 seconds → 518,400 ledgers ≈ 30 days.
pub const TTL_THRESHOLD: u32 = 518_400;
pub const TTL_TARGET: u32 = 518_400;

// ============================================================================
// TIME & STORAGE TTL CONSTANTS
// ============================================================================

/// 48 hours in seconds — used as the default timelock delay for proposal execution.
/// In governance contexts, this provides a reasonable window for community review
/// before automated execution of critical operations.
pub const TIMELOCK_DELAY_SECS: u64 = 48 * 60 * 60;

/// 24 hours in seconds — minimum validity period for credentials, proposals, and records.
/// Ensures a full day for review and response cycles in administrative operations.
pub const MIN_VALIDITY_PERIOD: u64 = 86_400;

/// 7 days in seconds — grace period allowing engineers to work after credential expiry.
/// Provides a transition window for certification renewal without immediate access loss.
pub const GRACE_PERIOD_SECS: u64 = 7 * 86_400;

/// 30 days in seconds — default decay interval for asset lifecycle scoring.
/// Represents the time period over which maintenance history contributes to asset health scores.
pub const DEFAULT_DECAY_INTERVAL_SECS: u64 = 30 * 86_400;

/// 30 days in Soroban ledger entries (518,400 ledgers ≈ 30 days).
/// One ledger ≈ 5 seconds. Used for persistent storage TTL thresholds and targets
/// to ensure data survives across long periods without being expired by the ledger.
pub const DEFAULT_TTL_LEDGERS: u32 = 518_400;

/// Extend the TTL of a persistent storage entry using the shared threshold/target constants.
pub fn extend_persistent_ttl<K: IntoVal<Env, Val>>(env: &Env, key: K) {
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
}

/// Shared admin authorization helper.
///
/// Calls `admin.require_auth()` and checks that `admin` matches `stored_admin`.
/// Returns `Err(SharedContractError::UnauthorizedAdmin)` on mismatch so each
/// contract can convert to its own `ContractError::UnauthorizedAdmin` variant
/// (preserving per-contract error discriminants).
///
/// # Example
/// ```ignore
/// let stored_admin = Self::get_admin(env.clone());
/// if shared::require_admin(&admin, &stored_admin).is_err() {
///     panic_with_error!(&env, ContractError::UnauthorizedAdmin);
/// }
/// ```
pub fn require_admin(admin: &Address, stored_admin: &Address) -> Result<(), error::SharedContractError> {
    admin.require_auth();
    if *admin != *stored_admin {
        return Err(error::SharedContractError::UnauthorizedAdmin);
    }
    Ok(())
}
