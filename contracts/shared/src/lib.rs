#![no_std]

pub mod error;
pub mod validation;

use soroban_sdk::{Env, IntoVal, Val};

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
