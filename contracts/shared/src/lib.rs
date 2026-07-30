#![no_std]

pub mod error;
pub mod validation;

use soroban_sdk::{Address, Env, IntoVal, Val};

/// Ledger TTL threshold and target for persistent storage entries.
/// 1 ledger ≈ 5 seconds → 518,400 ledgers ≈ 30 days.
pub const TTL_THRESHOLD: u32 = 518_400;
pub const TTL_TARGET: u32 = 518_400;

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
