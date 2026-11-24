//! Historical asset-state queries (issue #1751, duplicate #1752).
//!
//! Every state-changing path calls [`record_state`], which appends an [`AssetState`] snapshot
//! keyed by the current ledger sequence. Snapshots are kept in ascending ledger order, so
//! [`state_at_ledger`] finds the state at any height with a binary search (O(log n) reads
//! after one storage load).
//!
//! Retention: at most [`MAX_STATE_HISTORY_ENTRIES`] snapshots are kept per asset. Older
//! snapshots are dropped first, so queries below the oldest retained ledger return `None`.
//! Multiple changes within one ledger collapse into a single snapshot holding the final state.

use soroban_sdk::{Env, Vec};

use crate::storage::{frozen_key, history_key, score_key};
use crate::types::{AssetState, DataKey, MaintenanceRecord};
use shared::extend_persistent_ttl;

/// Maximum snapshots retained per asset. Bounds storage growth and keeps the Vec load cheap.
pub const MAX_STATE_HISTORY_ENTRIES: u32 = 500;

/// Append the asset's current state, stamped with the current ledger sequence.
pub(crate) fn record_state(env: &Env, asset_id: u64) {
    let ledger_sequence = env.ledger().sequence();
    let history: Vec<MaintenanceRecord> = env
        .storage()
        .persistent()
        .get(&history_key(asset_id))
        .unwrap_or_else(|| Vec::new(env));
    let state = AssetState {
        asset_id,
        ledger_sequence,
        collateral_score: env
            .storage()
            .persistent()
            .get(&score_key(asset_id))
            .unwrap_or(0),
        maintenance_count: history.len(),
        frozen: env
            .storage()
            .persistent()
            .get::<_, bool>(&frozen_key(asset_id))
            .unwrap_or(false),
        owner_start_ledger: env
            .storage()
            .persistent()
            .get::<_, u64>(&DataKey::OwnershipStartLedger(asset_id)),
    };

    let key = DataKey::StateHistory(asset_id);
    let mut states: Vec<AssetState> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    let same_ledger_index = states
        .len()
        .checked_sub(1)
        .filter(|&i| states.get(i).is_some_and(|last| last.ledger_sequence == ledger_sequence));
    match same_ledger_index {
        Some(i) => states.set(i, state),
        None => states.push_back(state),
    }
    while states.len() > MAX_STATE_HISTORY_ENTRIES {
        states.pop_front();
    }

    env.storage().persistent().set(&key, &states);
    extend_persistent_ttl(env, &key);
}

/// Return the latest state recorded at or before `ledger_height`, or `None` if no snapshot
/// exists that early (or it has been pruned).
pub(crate) fn state_at_ledger(env: &Env, asset_id: u64, ledger_height: u32) -> Option<AssetState> {
    let states: Vec<AssetState> = env
        .storage()
        .persistent()
        .get(&DataKey::StateHistory(asset_id))?;

    // Find the count of snapshots with ledger_sequence <= ledger_height (partition point).
    let mut lo = 0u32;
    let mut hi = states.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if states.get(mid)?.ledger_sequence <= ledger_height {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        None
    } else {
        states.get(lo - 1)
    }
}
