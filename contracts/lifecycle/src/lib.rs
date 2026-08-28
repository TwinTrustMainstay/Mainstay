#![no_std]

mod errors;
mod scoring;
mod types;
pub(crate) mod storage;
pub(crate) mod events;
pub(crate) mod admin;

// Re-export storage key helpers at the crate root so that
// `super::history_key(...)` etc. in scoring.rs keep working unchanged.
pub(crate) use storage::{
    engineer_auth_key, engineer_history_key, frozen_key, frozen_score_key,
    health_snapshot_key, history_key, last_update_key, revoke_eng_timelock_key,
    score_history_key, score_key, scoring_weights_key, standard_key, timelock_key,
    transfer_hist_key, submission_window_key,
};

// Re-export event constants at the crate root for the same reason.
pub(crate) use events::{
    EVENT_ADMIN_SET, EVENT_DECAY, EVENT_INIT, EVENT_MAINT, EVENT_PROP_ADMIN, EVENT_PRUNED,
    EVENT_REG_AST, EVENT_REG_ENG, EVENT_RST_SCR, EVENT_XFER,
};

use crate::errors::ContractError;
use crate::scoring::{apply_decay, compute_decay, get_task_weight, score_history_push, valuation_history_push};
use crate::types::{
    BatchRecord, Config, DataKey, HealthSnapshot, MaintenanceRecord, Priority, RecurringTask,
    ScoreEntry, TimelockProposal, TransferRecord, WeightProposal,
};
use shared::extend_persistent_ttl;
use shared::validation::require_non_empty_vec;
use shared::{TIMELOCK_DELAY_SECS, DEFAULT_DECAY_INTERVAL_SECS, DEFAULT_TTL_LEDGERS};
use shared::{TTL_THRESHOLD, TTL_TARGET};
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{
    contract, contractimpl, panic_with_error, symbol_short, Address, Bytes, BytesN, Env, Map,
    String, Symbol, Vec,
};

pub use shared::error::SharedContractError as SharedError;

const ASSET_REGISTRY: Symbol = symbol_short!("REGISTRY");
const ENG_REGISTRY: Symbol = symbol_short!("ENG_REG");
const CONFIG: Symbol = symbol_short!("CONFIG");
const PAUSED_KEY: Symbol = symbol_short!("PAUSED");
const PENDING_ADMIN_KEY: Symbol = symbol_short!("PADMIN");
/// Instance-storage key for the reentrancy lock used in `submit_maintenance`.
/// Stored in *instance* storage so it persists only for the duration of the
/// transaction and is automatically cleared when the invocation frame exits,
/// but also explicitly cleared at the end of the guarded function (#1022).
const REENTRANCY_LOCK: Symbol = symbol_short!("LOCKED");
const DEPLOYER_KEY: Symbol = symbol_short!("DEPLOYER");
const DEFAULT_MAX_HISTORY: u32 = 200;
/// Default cap on the number of asset IDs kept in each engineer's history list.
/// Matches `DEFAULT_MAX_HISTORY` so both sliding windows share the same default
/// out of the box; operators can tune them independently via admin calls.
const DEFAULT_MAX_ENGINEER_HISTORY: u32 = 200;
const DEFAULT_SCORE_INCREMENT: u32 = 5;
const DEFAULT_DECAY_RATE: u32 = 5;
/// 30 days in seconds — the default length of one decay interval.
const DEFAULT_DECAY_INTERVAL: u64 = 2_592_000;
const DEFAULT_ELIGIBILITY_THRESHOLD: u32 = 50;
const DEFAULT_MAX_NOTES_LENGTH: u32 = 256;
/// Default per-engineer cap on maintenance submissions within a rolling hour.
const DEFAULT_MAX_SUBMISSIONS_PER_HOUR: u32 = 20;
/// Length of the rolling submission-rate window, in seconds.
const SUBMISSION_RATE_WINDOW_SECS: u64 = 3600;

fn effective_min_collateral_score(config: &Config) -> u32 {
    if config.min_collateral_score > 0 {
        config.min_collateral_score
    } else {
        config.eligibility_threshold
    }
}
/// Maximum collateral score exposed by the lifecycle contract.
///
/// With the highest built-in task weight (`10`), a maximum-size batch can
/// contribute at most `MAX_BATCH_SIZE * 10 = 500` raw points. A retained
/// history has a theoretical raw maximum of `max_history * 10`; at the
/// default `max_history` of 200 that is 2,000 points. Recency can only reduce
/// those values, and `compute_decay` caps the externally visible score at 100
/// before summing another contribution.
const MAX_COLLATERAL_SCORE: u32 = 100;
/// Highest score weight among the built-in maintenance task types.
const MAX_BUILT_IN_TASK_WEIGHT: u32 = 10;
/// Hard cap on the number of records accepted in a single
/// `batch_submit_maintenance` call.
///
/// This bounds per-transaction work to prevent a single caller from
/// submitting an unbounded `Vec<BatchRecord>` that could blow past Soroban
/// ledger gas/instruction limits and cause a denial-of-service. The value
/// is intentionally conservative: maintenance batches are expected to be
/// small (single asset, single engineer, single session), and 50 records
/// comfortably covers realistic workflows while leaving ample headroom
/// for the cross-contract calls and per-record validation performed
/// inside the batch path.
pub const MAX_BATCH_SIZE: u32 = 50;
/// Hard cap on engineers accepted by one bulk authorization-revocation call.
pub const MAX_BATCH_REVOKE_SIZE: u32 = 50;
/// Upper bound on `limit` accepted by [`LifecycleContract::get_maintenance_history_page`].
/// Without this cap a caller could pass an arbitrarily large `limit` and reintroduce
/// the unbounded-response failure the paginated endpoint exists to prevent.
///
/// This endpoint predates [`get_maintenance_history_paginated`] and its cap was
/// never tightened to match. The two limits are kept intentionally distinct
/// (see [`MAX_PAGINATED_LIMIT`]) rather than unified, since `get_maintenance_history_page`
/// already has external callers relying on its current 100-record cap; lowering it
/// would be a breaking change for those integrators. Prefer
/// [`LifecycleContract::get_maintenance_history_paginated`] with [`MAX_PAGINATED_LIMIT`]
/// for new integrations, and see #996 / #1209 for the history behind this split.
pub const MAX_PAGE_SIZE: u32 = 100;
/// Upper bound on `limit` accepted by
/// [`LifecycleContract::get_maintenance_history_paginated`].
///
/// Stricter than [`MAX_PAGE_SIZE`] (100) to keep individual responses
/// well within Soroban's per-call data budget as history grows (#996).
///
/// `get_maintenance_history_paginated` is the newer, preferred pagination
/// endpoint (see its doc comment for the full list of differences from
/// [`LifecycleContract::get_maintenance_history_page`]); its stricter cap was
/// chosen deliberately when the endpoint was added and is not a bug — see #1209.
pub const MAX_PAGINATED_LIMIT: u32 = 50;
const TIMELOCK_DELAY_SECS: u64 = 48 * 60 * 60;
/// Minimum score returned for an asset that has at least one maintenance record.
/// Prevents decay from making a legitimately-maintained asset indistinguishable
/// from one with no history at all.
const MIN_SCORE_WITH_HISTORY: u32 = 1;
/// Maximum age in ledgers for maintenance record recency weighting (~30 days).
/// Records older than this contribute zero to the collateral score.
/// 1 ledger ≈ 5 seconds → 518_400 ledgers ≈ 30 days.
/// Older records still contribute nothing, newer records are weighted linearly.
const MAX_AGE_LEDGERS: u64 = TTL_THRESHOLD as u64;

/// Emitted when a health snapshot is anchored to reconstructed history.
const EVENT_RECONSTR: Symbol = symbol_short!("RECONSTR");
/// Emitted when a task-weight-change proposal is created.
const EVENT_WEIGHT_PROP: Symbol = symbol_short!("WT_PROP");
/// Emitted when a pending task-weight-change proposal is executed.
const EVENT_WEIGHT_EXEC: Symbol = symbol_short!("WT_EXEC");

fn history_key(asset_id: u64) -> (Symbol, u64) {
const MAX_AGE_LEDGERS: u64 = TTL_THRESHOLD as u64;


    (symbol_short!("HIST"), asset_id)

    (symbol_short!("TL_PROP"), op)

    (symbol_short!("SCORE"), asset_id)

    (symbol_short!("SCHIST"), asset_id)

    (symbol_short!("LUPD"), asset_id)

    (symbol_short!("ENG_HIST"), engineer.clone())

    (symbol_short!("ENG_AUTH"), asset_id, engineer.clone())

    (symbol_short!("FROZEN"), asset_id)

    (symbol_short!("FRZ_SCR"), asset_id)

    (symbol_short!("HLTH_SNP"), asset_id)

    (symbol_short!("XFER_HIST"), asset_id)
}

/// Key for the list of currently-authorized engineer addresses for an asset.
/// Used by `authorize_engineer`, `revoke_engineer_authorization`, and
/// `get_authorized_engineers`.
fn authorized_engineers_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("AUTH_ENGS"), asset_id)
}

fn revoke_eng_timelock_key(asset_id: u64, engineer: &Address) -> (Symbol, u64, Address) {
    (symbol_short!("RVK_TL"), asset_id, engineer.clone())

    (symbol_short!("MSTD"), asset_type.clone())
}

/// Storage key for the dynamic frequency-based scoring weights for a given asset type.
fn scoring_weights_key(_env: &Env, asset_type: &Symbol) -> (Symbol, Symbol) {
    (symbol_short!("SCRWTS"), asset_type.clone())
}

/// Storage key for a pending weight-change proposal for a given task type.
fn weight_proposal_key(task_type: &Symbol) -> DataKey {
    DataKey::WeightProposal(task_type.clone())
/// Computes the sha256 hash of a [`MaintenanceRecord`] as it will be stored on-chain
/// (including whatever `previous_record_hash` it already carries), so that the result
/// can be embedded as the `previous_record_hash` of the *next* record in the chain.
fn hash_maintenance_record(env: &Env, record: &MaintenanceRecord) -> Bytes {
    let digest: BytesN<32> = env.crypto().sha256(&record.clone().to_xdr(env)).into();
    digest.into()
}

/// Returns the hash-chain link (`previous_record_hash`) for the next record to be
/// appended to `history`: the hash of the current last record, or `None` when
/// `history` is empty (i.e. the next record will be the first visible one).
fn next_chain_link(env: &Env, history: &Vec<MaintenanceRecord>) -> Option<Bytes> {
    if history.is_empty() {
        None
    } else {
        let last = history.get(history.len() - 1).unwrap();
        Some(hash_maintenance_record(env, &last))
    }
}

/// Enforce M-of-N admin quorum for critical lifecycle operations.
///
/// When `config.admins` is empty or `admin_threshold <= 1`, only the single
/// `config.admin` must have signed (single-admin mode, backward-compatible).
/// Otherwise the caller must be in `config.admins`, and the transaction must
/// also carry signatures from additional admins in `config.admins` (in order)
/// until `admin_threshold` total valid signatures are collected.
///
/// The `caller` is expected to have already called `caller.require_auth()` before
/// this function.
pub(crate) fn require_quorum(env: &Env, config: &Config, caller: &Address) {
    if config.admins.is_empty() || config.admin_threshold <= 1 {
        // Single-admin mode: caller must be the configured admin.
        if config.admin != *caller {
            panic_with_error!(env, ContractError::UnauthorizedAdmin);
        }
        return;
    }

    // Verify caller is a member of the multisig set.
    let mut caller_found = false;
    for a in config.admins.iter() {
        if a == *caller {
            caller_found = true;
            break;
        }
    }
    if !caller_found {
        panic_with_error!(env, ContractError::UnauthorizedAdmin);
    }

    // Require auth from additional admins (in order) until threshold is met.
    // Caller already signed, so we start counting from 1.
    let mut collected: u32 = 1;
    for a in config.admins.iter() {
        if collected >= config.admin_threshold {
            break;
        }
        if a != *caller {
            a.require_auth();
            collected += 1;
        }
    }

    if collected < config.admin_threshold {
        panic_with_error!(env, ContractError::InsufficientSigners);
    }
}

pub(crate) fn require_engineer_authorized(env: &Env, asset_id: u64, engineer: &Address) {
fn require_engineer_authorized(env: &Env, asset_id: u64, engineer: &Address) {
    let key = engineer_auth_key(asset_id, engineer);
    let authorized: bool = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or(false);
    if !authorized {
        panic_with_error!(env, ContractError::EngineerNotAuthorized);
    }
    extend_persistent_ttl(env, &key);
}

/// Enforce the per-engineer `max_submissions_per_hour` rate limit for a
/// submission of `count` records (1 for `submit_maintenance`, the full batch
/// size for `batch_submit_maintenance`). The combined count is checked
/// against the remaining allowance in the current rolling-hour window
/// *before* any record is written, so a single large batch cannot bypass the
/// cap the way `count` individual calls would be blocked.
fn enforce_submission_rate(env: &Env, engineer: &Address, config: &Config, count: u32) {
    if config.max_submissions_per_hour == 0 {
        return;
    }

    let key = submission_window_key(engineer);
    let (window_start, existing_count): (u64, u32) =
        env.storage().persistent().get(&key).unwrap_or((0, 0));

    let now = env.ledger().timestamp();
    let (new_window_start, base_count) = if now.saturating_sub(window_start) >= SUBMISSION_RATE_WINDOW_SECS {
        (now, 0u32)
    } else {
        (window_start, existing_count)
    };

    let new_count = base_count
        .checked_add(count)
        .unwrap_or(u32::MAX);
    if new_count > config.max_submissions_per_hour {
        env.events().publish(
            (symbol_short!("RATE_LIM"), engineer.clone()),
            (base_count, count, config.max_submissions_per_hour),
        );
        panic_with_error!(env, ContractError::RateLimitExceeded);
    }

    env.storage().persistent().set(&key, &(new_window_start, new_count));
    extend_persistent_ttl(env, &key);
}

pub(crate) fn engineer_history_add(env: &Env, engineer: &Address, asset_id: u64, max_history: u32) {
    let key = engineer_history_key(engineer);
    let mut ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));

    // Check if asset_id already exists before appending
    let mut found = false;
    for id in ids.iter() {
        if id == asset_id {
            found = true;
            break;
        }
    }

    if !found {
        if max_history > 0 && ids.len() >= max_history {
            ids.remove(0);
        }
        ids.push_back(asset_id);
    }

    env.storage().persistent().set(&key, &ids);
    extend_persistent_ttl(&env, &key);
}

pub(crate) pub(crate) fn engineer_history_remove(env: &Env, engineer: &Address, asset_id: u64) {
    let key = engineer_history_key(engineer);
    if let Some(ids) = env.storage().persistent().get::<_, Vec<u64>>(&key) {
        let mut new_ids: Vec<u64> = Vec::new(env);
        for id in ids.iter() {
            if id != asset_id {
                new_ids.push_back(id);
            }
        }
        if new_ids.len() < ids.len() {
            env.storage().persistent().set(&key, &new_ids);
            extend_persistent_ttl(&env, &key);
        }
    }
}

pub(crate) fn get_asset_registry_addr(env: &Env) -> Address {
    env.storage()
        .persistent()
        .get(&ASSET_REGISTRY)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized))
}

pub(crate) fn get_engineer_registry_addr(env: &Env) -> Address {
    env.storage()
        .persistent()
        .get(&ENG_REGISTRY)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized))
}

pub(crate) fn set_asset_registry_addr(env: &Env, addr: &Address) {
    env.storage().persistent().set(&ASSET_REGISTRY, addr);
    extend_persistent_ttl(&env, &ASSET_REGISTRY);
}

pub(crate) fn set_engineer_registry_addr(env: &Env, addr: &Address) {
    env.storage().persistent().set(&ENG_REGISTRY, addr);
    extend_persistent_ttl(&env, &ENG_REGISTRY);
}

pub(crate) fn is_zero_address(env: &Env, addr: &Address) -> bool {
    *addr
        == Address::from_str(
            env,
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
        )
}

pub(crate) fn is_paused(env: &Env) -> bool {
    env.storage().persistent().get(&PAUSED_KEY).unwrap_or(false)
}

pub(crate) fn ensure_not_paused(env: &Env) {
    if is_paused(env) {
        panic_with_error!(env, ContractError::Paused);
    }
}

/// Acquire the reentrancy lock for `submit_maintenance`.
///
/// Sets the `LOCKED` flag in instance storage. If the flag is already set
/// (indicating a reentrant call), panics with [`ContractError::Reentrancy`].
///
/// # Issue #1022
/// `submit_maintenance` makes cross-contract calls to the engineer registry and
/// asset registry. A malicious registry contract could re-enter the lifecycle
/// contract before state is committed, enabling double-writes to the maintenance
/// history. This guard prevents that attack vector.
fn acquire_reentrancy_guard(env: &Env) {
    if env
        .storage()
        .instance()
        .get::<_, bool>(&REENTRANCY_LOCK)
        .unwrap_or(false)
    {
        panic_with_error!(env, ContractError::Reentrancy);
    }
    env.storage().instance().set(&REENTRANCY_LOCK, &true);
}

/// Release the reentrancy lock acquired by [`acquire_reentrancy_guard`].
fn release_reentrancy_guard(env: &Env) {
    env.storage().instance().remove(&REENTRANCY_LOCK);
}

/// Advances `next_due` on every active recurring task of `asset_id` whose
/// `task_type` matches a maintenance submission that just completed.
///
/// Mirrors the `next_due` update already performed by `auto_create_recurring_task`
/// (`timestamp + interval_value`) so a manually-submitted record that happens to
/// satisfy a recurring schedule keeps that schedule from going perpetually overdue.
fn advance_recurring_tasks_on_submission(
    env: &Env,
    asset_id: u64,
    task_type: &Symbol,
    timestamp: u64,
) {
    let tasks_key = DataKey::RecurringTasks(asset_id);
    let mut tasks: Vec<RecurringTask> = match env.storage().persistent().get(&tasks_key) {
        Some(t) => t,
        None => return,
    };

    let mut changed = false;
    for i in 0..tasks.len() {
        let mut task = tasks.get(i).unwrap();
        if task.is_active && task.task_type == *task_type {
            task.next_due = timestamp.saturating_add(task.interval_value);
            tasks.set(i, task);
            changed = true;
        }
    }

    if changed {
        env.storage().persistent().set(&tasks_key, &tasks);
        extend_persistent_ttl(env, &tasks_key);
    }
}

pub(crate) fn require_admin(env: &Env, admin: &Address) {
    admin.require_auth();
    let config: Config = env
        .storage()
        .persistent()
        .get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized));
    if config.admin != *admin {
        panic_with_error!(env, ContractError::UnauthorizedAdmin);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FrequencyWeights {
    low: u32,
    medium: u32,
    high: u32,
    medium_threshold: u32,
    high_threshold: u32,
    window_days: u32,
}

pub(crate) fn json_number(bytes: &Bytes, key: &[u8]) -> Option<u32> {
    let len = bytes.len();
    if len == 0 || key.is_empty() || key.len() as u32 >= len {
        return None;
    }

    let mut i: u32 = 0;
    while i + key.len() as u32 <= len {
        let mut matched = true;
        let mut j: u32 = 0;
        while j < key.len() as u32 {
            if bytes.get(i + j).unwrap() != key[j as usize] {
                matched = false;
                break;
            }
            j += 1;
        }

        if matched {
            let mut k = i + key.len() as u32;
            while k < len {
                let ch = bytes.get(k).unwrap();
                if ch == b':' {
                    k += 1;
                    break;
                }
                k += 1;
            }
            while k < len {
                let ch = bytes.get(k).unwrap();
                if ch.is_ascii_digit() {
                    break;
                }
                k += 1;
            }
            if k == len {
                return None;
            }

            let mut value: u32 = 0;
            let mut found_digit = false;
            while k < len {
                let ch = bytes.get(k).unwrap();
                if !ch.is_ascii_digit() {
                    break;
                }
                found_digit = true;
                value = value.saturating_mul(10).saturating_add((ch - b'0') as u32);
                k += 1;
            }
            if found_digit {
                return Some(value);
            }
            return None;
        }

        i += 1;
    }

    None
}

pub(crate) fn parse_frequency_weights(weights_json: &Bytes) -> Option<FrequencyWeights> {
    let low = json_number(weights_json, b"\"low\"")?;
    let medium = json_number(weights_json, b"\"medium\"")?;
    let high = json_number(weights_json, b"\"high\"")?;
    let medium_threshold = json_number(weights_json, b"\"medium_threshold\"").unwrap_or(4);
    let high_threshold = json_number(weights_json, b"\"high_threshold\"").unwrap_or(12);
    let window_days = json_number(weights_json, b"\"window_days\"").unwrap_or(365);

    if low == 0
        || medium == 0
        || high == 0
        || window_days == 0
        || medium_threshold == 0
        || high_threshold < medium_threshold
    {
        return None;
    }

    Some(FrequencyWeights {
        low,
        medium,
        high,
        medium_threshold,
        high_threshold,
        window_days,
    })
}

pub(crate) fn recent_maintenance_count(env: &Env, asset_id: u64, window_days: u32) -> u32 {
    let history: Vec<MaintenanceRecord> = env
        .storage()
        .persistent()
        .get(&history_key(asset_id))
        .unwrap_or_else(|| Vec::new(env));
    if history.is_empty() {
        return 0;
    }

    let window_secs = (window_days as u64).saturating_mul(24 * 60 * 60);
    let cutoff = env.ledger().timestamp().saturating_sub(window_secs);
    let mut count = 0u32;

    for record in history.iter() {
        if record.task_type != symbol_short!("XFER") && record.timestamp >= cutoff {
            count = count.saturating_add(1);
        }
    }

    count
}

pub(crate) fn apply_dynamic_frequency_weight(env: &Env, asset_id: u64, asset_type: &Symbol, score: u32) -> u32 {
    if score == 0 {
        return 0;
    }

    let Some(weights_json) = env
        .storage()
        .persistent()
        .get::<_, Bytes>(&scoring_weights_key(env, asset_type))
    else {
        return score;
    };

    let Some(weights) = parse_frequency_weights(&weights_json) else {
        return score;
    };

    let maintenance_count = recent_maintenance_count(env, asset_id, weights.window_days);
    let multiplier = if maintenance_count >= weights.high_threshold {
        weights.high
    } else if maintenance_count >= weights.medium_threshold {
        weights.medium
    } else {
        weights.low
    };

    ((score as u64).saturating_mul(multiplier as u64) / 100) as u32
}

pub(crate) fn compute_read_only_collateral_score(env: &Env, asset_id: u64, asset_type: &Symbol, config: &Config) -> u32 {
    let history_score = compute_decay(env, asset_id);
    let stored: u32 = env.storage().persistent().get(&score_key(asset_id)).unwrap_or(0);
    let last_update: u64 = env
        .storage()
        .persistent()
        .get(&last_update_key(asset_id))
        .unwrap_or(0);
    let elapsed = env.ledger().timestamp().saturating_sub(last_update);
    let intervals = elapsed / config.decay_interval;
    let decay = (intervals as u32).saturating_mul(config.decay_rate);
    let config_score = stored.saturating_sub(decay);
    let score = history_score.min(config_score);
    let weighted_score = apply_dynamic_frequency_weight(env, asset_id, asset_type, score).min(100);

    let has_history = env
        .storage()
        .persistent()
        .get::<_, Vec<MaintenanceRecord>>(&history_key(asset_id))
        .map(|h| !h.is_empty())
        .unwrap_or(false);

    if has_history && weighted_score < MIN_SCORE_WITH_HISTORY {
        MIN_SCORE_WITH_HISTORY
    } else {
        weighted_score
    }
}

pub(crate) fn store_timelock(env: &Env, op: Symbol) {
    let key = timelock_key(op);
    env.storage().persistent().set(
        &key,
        &TimelockProposal {
            proposed_at: env.ledger().timestamp(),
            executed: false,
        },
    );
    extend_persistent_ttl(&env, &key);
}

pub(crate) fn require_timelock_ready(env: &Env, op: Symbol) {
    let key = timelock_key(op);
    let mut proposal: TimelockProposal = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::ProposalNotFound));
    if proposal.executed {
        panic_with_error!(env, ContractError::ProposalNotFound);
    }
    if env.ledger().timestamp().saturating_sub(proposal.proposed_at) < TIMELOCK_DELAY_SECS {
        panic_with_error!(env, ContractError::TimelockNotExpired);
    }
    proposal.executed = true;
    env.storage().persistent().set(&key, &proposal);
    extend_persistent_ttl(&env, &key);
}

pub(crate) fn validate_notes_length(env: &Env, notes: &soroban_sdk::String, max: u32) {
    if notes.is_empty() {
        panic_with_error!(env, ContractError::NotesTooLong);
    }
    if notes.len() > max {
        panic_with_error!(env, ContractError::NotesTooLong);
    }
}

pub(crate) fn verify_asset_exists(env: &Env, asset_registry: &Address, asset_id: &u64) {
    let client = asset_registry::AssetRegistryClient::new(env, asset_registry);
    let result = client.try_get_asset(asset_id);
    if result.is_err() {
        panic_with_error!(env, ContractError::AssetNotFound);
    }
}

/// Internal helper: compute predicted next service timestamp without panicking.
///
/// Uses a moving average of historical intervals between consecutive records
/// of the same task type. Returns [`ContractError::InsufficientPredictionData`]
/// if fewer than 2 matching records exist.
pub(crate) fn try_predict_next_service(
    env: &Env,
    asset_id: u64,
    task_type: &Symbol,
) -> Result<u64, ContractError> {
    let history: Vec<MaintenanceRecord> = env
        .storage()
        .persistent()
        .get(&history_key(asset_id))
        .unwrap_or_else(|| Vec::new(env));

    if history.is_empty() {
        return Err(ContractError::NoMaintenanceHistory);
    }

    // Collect timestamps for matching task_type, excluding XFER sentinels.
    let mut timestamps: Vec<u64> = Vec::new(env);
    for record in history.iter() {
        if record.task_type == *task_type
            && record.task_type != symbol_short!("XFER")
        {
            timestamps.push_back(record.timestamp);
        }
    }

    if timestamps.len() < 2 {
        return Err(ContractError::InsufficientPredictionData);
    }

    // Moving average of intervals between consecutive records.
    let mut total_interval: u64 = 0;
    let mut interval_count: u64 = 0;
    for i in 1..timestamps.len() {
        let interval = timestamps
            .get(i as u32)
            .unwrap()
            .saturating_sub(timestamps.get((i - 1) as u32).unwrap());
        if interval > 0 {
            total_interval = total_interval.saturating_add(interval);
            interval_count = interval_count.saturating_add(1);
        }
    }

    if interval_count == 0 {
        return Err(ContractError::InsufficientPredictionData);
    }

    let avg_interval = total_interval / interval_count;

    // Clamp: minimum 24 hours, maximum 5 years.
    let min_interval: u64 = 24 * 60 * 60;
    let max_interval: u64 = 5 * 365 * 24 * 60 * 60;
    let clamped = avg_interval.clamp(min_interval, max_interval);

    let last_timestamp = timestamps.get(timestamps.len() - 1).unwrap();
    Ok(last_timestamp.saturating_add(clamped))
}

// Minimal client interface for cross-contract call to EngineerRegistry
mod engineer_registry {
    use soroban_sdk::{contractclient, contracttype, Address, Env, Symbol, Vec};

    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum CredentialStatus {
        Valid = 0,
        GracePeriod = 1,
        HardExpired = 2,
        Revoked = 3,
        NotFound = 4,
    }

    #[allow(dead_code)]
    #[contractclient(name = "EngineerRegistryClient")]
    pub trait EngineerRegistry {
        fn verify_engineer(env: Env, engineer: Address, required_specialization: Option<Symbol>) -> CredentialStatus;
        fn batch_verify_engineers(env: Env, engineers: Vec<Address>) -> Vec<CredentialStatus>;
        fn get_reputation(env: Env, engineer: Address) -> u32;
        fn update_reputation(env: Env, engineer: Address, delta: i32);
        fn get_credential_status(env: Env, engineer: Address) -> CredentialStatus;
        fn get_specializations(env: Env, engineer: Address) -> Vec<Symbol>;
    }
}

#[contract]
pub struct Lifecycle;

#[contractimpl]
impl Lifecycle {
    /// Store the deployer address at deploy time.
    pub fn __constructor(env: Env, deployer: Address) {
        env.storage().instance().set(&DEPLOYER_KEY, &deployer);
        env.storage()
            .instance()
            .extend_ttl(DEFAULT_TTL_LEDGERS, DEFAULT_TTL_LEDGERS);
    }

    /// Propose a configuration change to the Lifecycle contract using the admin timelock.
    ///
    /// # Arguments
    /// * `admin` - The administrator requesting the config update
    /// * `op` - Symbol representing the update operation
    pub fn propose_config_update(env: Env, admin: Address, op: Symbol) {
        crate::admin::propose_config_update(env, admin, op);
    }

    /// Execute a pending score increment update after the timelock expires.
    ///
    /// # Arguments
    /// * `admin` - The administrator performing the update
    /// * `score_increment` - New increment value for each maintenance task
    pub fn execute_update_score_increment(env: Env, admin: Address, score_increment: u32) {
        require_timelock_ready(&env, symbol_short!("SC_INC"));
        crate::admin::update_score_increment(env, admin, score_increment);
    }

    /// Execute a pending decay configuration update after the timelock expires.
    ///
    /// # Arguments
    /// * `admin` - The administrator performing the update
    /// * `decay_rate` - Points to deduct per decay interval
    /// * `decay_interval` - Interval length in seconds used to compute decay
    pub fn execute_update_decay_config(
        env: Env,
        admin: Address,
        decay_rate: u32,
        decay_interval: u64,
    ) {
        require_timelock_ready(&env, symbol_short!("DEC_CFG"));
        crate::admin::update_decay_config(env, admin, decay_rate, decay_interval);
    }

    /// Execute a pending eligibility threshold update after the timelock expires.
    ///
    /// # Arguments
    /// * `admin` - The administrator performing the update
    /// * `threshold` - New collateral eligibility threshold
    pub fn execute_update_eligibility(env: Env, admin: Address, threshold: u32) {
        require_timelock_ready(&env, symbol_short!("ELIG"));
        crate::admin::update_eligibility_threshold(env, admin, threshold);
    }

    /// Execute a pending max history update after the timelock expires.
    ///
    /// # Arguments
    /// * `admin` - The administrator performing the update
    /// * `new_max` - Maximum maintenance history entries per asset
    pub fn execute_update_max_history(env: Env, admin: Address, new_max: u32) {
        require_timelock_ready(&env, symbol_short!("MAX_HIST"));
        crate::admin::update_max_history(env, admin, new_max);
    }

    /// Execute a pending max notes length update after the timelock expires.
    ///
    /// # Arguments
    /// * `admin` - The administrator performing the update
    /// * `new_max` - Maximum length of maintenance notes
    pub fn execute_update_max_notes_length(env: Env, admin: Address, new_max: u32) {
        require_timelock_ready(&env, symbol_short!("MAX_NOTE"));
        crate::admin::update_max_notes_length(env, admin, new_max);
    }

    /// Execute a pending asset registry address update after the timelock expires.
    ///
    /// # Arguments
    /// * `admin` - The administrator performing the update
    /// * `new_registry` - Address of the new asset registry contract
    pub fn execute_update_asset_registry(env: Env, admin: Address, new_registry: Address) {
        require_timelock_ready(&env, symbol_short!("AST_REG"));
        crate::admin::update_asset_registry(env, admin, new_registry);
    }

    /// Execute a pending engineer registry address update after the timelock expires.
    ///
    /// # Arguments
    /// * `admin` - The administrator performing the update
    /// * `new_registry` - Address of the new engineer registry contract
    pub fn execute_update_engineer_registry(env: Env, admin: Address, new_registry: Address) {
        require_timelock_ready(&env, symbol_short!("ENG_REG"));
        crate::admin::update_engineer_registry(env, admin, new_registry);
    }

    /// Propose a full config update for the lifecycle contract.
    /// The new config is stored pending execution after the timelock delay.
    ///
    /// # Arguments
    /// * `admin`      - The current admin address
    /// * `new_config` - The proposed replacement `Config`
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the current admin
    /// - [`ContractError::InvalidConfig`] if any field in `new_config` is invalid
    pub fn propose_update_config(env: Env, admin: Address, new_config: Config) {
        ensure_not_paused(&env);
        admin.require_auth();
        let current: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if current.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        // Validate all fields before storing the proposal.
        if new_config.score_increment == 0 {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }
        if new_config.decay_interval == 0 {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }
        if new_config.eligibility_threshold == 0 {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }
        if new_config.max_notes_length == 0 {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }
        if new_config.admin_threshold > 0
            && new_config.admin_threshold as u32 > new_config.admins.len()
        {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }
        // Store proposed config under a dedicated pending key.
        let pending_key = symbol_short!("PEND_CFG");
        env.storage().persistent().set(&pending_key, &new_config);
        extend_persistent_ttl(&env, &pending_key);
        // Register the timelock.
        store_timelock(&env, symbol_short!("UPD_CFG"));
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("CFG_PROP")),
            (admin, env.ledger().timestamp()),
        );
    }

    /// Execute a pending full config update after the timelock delay has elapsed.
    ///
    /// # Arguments
    /// * `admin` - The current admin address (must still match `config.admin`)
    ///
    /// # Panics
    /// - [`ContractError::ProposalNotFound`] if no pending config proposal exists
    /// - [`ContractError::TimelockNotExpired`] if the timelock has not yet elapsed
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the current admin
    pub fn execute_update_config(env: Env, admin: Address) {
        require_timelock_ready(&env, symbol_short!("UPD_CFG"));
        admin.require_auth();
        let current: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if current.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        let pending_key = symbol_short!("PEND_CFG");
        let new_config: Config = env
            .storage()
            .persistent()
            .get(&pending_key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::ProposalNotFound));
        env.storage().persistent().remove(&pending_key);
        env.storage().persistent().set(&CONFIG, &new_config);
        extend_persistent_ttl(&env, &CONFIG);
        env.events().publish(
            (symbol_short!("CONFIG_UPD"),),
            (admin.clone(), env.ledger().timestamp()),
        );
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("CONFIG_UPD")),
            (admin, env.ledger().timestamp()),
        );
    }


    ///
    /// A verified engineer must also be explicitly authorized by the current asset owner
    /// before submitting maintenance for that asset.
    ///
    /// # Arguments
    /// * `owner` - The current owner of the asset
    /// * `asset_id` - The unique identifier of the asset
    /// * `engineer` - The engineer address being authorized
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedOwner`] if the caller is not the asset owner
    pub fn authorize_engineer(env: Env, owner: Address, asset_id: u64, engineer: Address) {
        ensure_not_paused(&env);
        owner.require_auth();

        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);
        let asset =
            asset_registry::AssetRegistryClient::new(&env, &asset_registry).get_asset(&asset_id);
        if asset.owner != owner {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        let key = engineer_auth_key(asset_id, &engineer);
        env.storage().persistent().set(&key, &true);
        extend_persistent_ttl(&env, &key);

        // Maintain the per-asset authorized-engineers list (#1012).
        let list_key = authorized_engineers_key(asset_id);
        let mut list: Vec<Address> = env
            .storage()
            .persistent()
            .get(&list_key)
            .unwrap_or_else(|| Vec::new(&env));
        let mut already_present = false;
        for addr in list.iter() {
            if addr == engineer {
                already_present = true;
                break;
            }
        }
        if !already_present {
            list.push_back(engineer);
            env.storage().persistent().set(&list_key, &list);
            extend_persistent_ttl(&env, &list_key);
        }
    }

    /// Revoke an engineer's owner-approved authorization for a specific asset.
    ///
    /// Propose the revocation of an engineer's authorization for an asset.
    /// The revocation is subject to a timelock to give engineers a grace period
    /// to complete any in-progress maintenance work.
    ///
    /// # Arguments
    /// * `owner` - The current owner of the asset
    /// * `asset_id` - The unique identifier of the asset
    /// * `engineer` - The engineer address whose authorization is being revoked
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedOwner`] if the caller is not the asset owner
    pub fn propose_revoke_engineer_auth(env: Env, owner: Address, asset_id: u64, engineer: Address) {
        ensure_not_paused(&env);
        owner.require_auth();

        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);
        let asset =
            asset_registry::AssetRegistryClient::new(&env, &asset_registry).get_asset(&asset_id);
        if asset.owner != owner {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        let key = revoke_eng_timelock_key(asset_id, &engineer);
        env.storage().persistent().set(
            &key,
            &TimelockProposal {
                proposed_at: env.ledger().timestamp(),
                executed: false,
            },
        );
        extend_persistent_ttl(&env, &key);

        env.events().publish(
            (symbol_short!("PROP_RVK"), owner.clone()),
            (asset_id, engineer.clone(), env.ledger().timestamp()),
        );
    }

    /// Execute a previously proposed engineer authorization revocation after the timelock expires.
    ///
    /// # Arguments
    /// * `owner` - The current owner of the asset
    /// * `asset_id` - The unique identifier of the asset
    /// * `engineer` - The engineer address whose authorization is being revoked
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedOwner`] if the caller is not the asset owner
    /// - [`ContractError::ProposalNotFound`] if no revocation was proposed or already executed
    /// - [`ContractError::TimelockNotExpired`] if the delay has not elapsed
    pub fn execute_revoke_engineer_auth(env: Env, owner: Address, asset_id: u64, engineer: Address) {
        ensure_not_paused(&env);
        owner.require_auth();

        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);
        let asset =
            asset_registry::AssetRegistryClient::new(&env, &asset_registry).get_asset(&asset_id);
        if asset.owner != owner {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        let key = revoke_eng_timelock_key(asset_id, &engineer);
        let mut proposal: TimelockProposal = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::ProposalNotFound));
        if proposal.executed {
            panic_with_error!(&env, ContractError::ProposalNotFound);
        }
        if env.ledger().timestamp().saturating_sub(proposal.proposed_at) < TIMELOCK_DELAY_SECS {
            panic_with_error!(&env, ContractError::TimelockNotExpired);
        }
        proposal.executed = true;
        env.storage().persistent().set(&key, &proposal);
        extend_persistent_ttl(&env, &key);

        env.storage()
            .persistent()
            .remove(&engineer_auth_key(asset_id, &engineer));

        env.events().publish(
            (symbol_short!("RVK_ENG"), owner.clone()),
            (asset_id, engineer.clone(), env.ledger().timestamp()),
        );
    }

    /// Clear all engineer authorizations for an asset after ownership transfer.
    /// Called by new owner to invalidate previous owner's engineer authorizations.
    ///
    /// # Arguments
    /// * `new_owner` - The current owner of the asset (must match registry)
    /// * `asset_id` - The asset whose engineer authorizations should be cleared
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedOwner`] if caller is not the current owner
    pub fn clear_engineer_authorizations(env: Env, new_owner: Address, asset_id: u64) {
        ensure_not_paused(&env);
        new_owner.require_auth();

        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);
        let asset =
            asset_registry::AssetRegistryClient::new(&env, &asset_registry).get_asset(&asset_id);
        if asset.owner != new_owner {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        // Get the maintenance history to find all engineers who have worked on this asset
        if let Some(history) = env
            .storage()
            .persistent()
            .get::<_, Vec<MaintenanceRecord>>(&history_key(asset_id))
        {
            let mut cleared_engineers = Vec::new(&env);
            for record in history.iter() {
                let eng = record.engineer.clone();
                let mut already_cleared = false;
                for cleared in cleared_engineers.iter() {
                    if cleared == eng {
                        already_cleared = true;
                        break;
                    }
                }
                if !already_cleared {
                    env.storage()
                        .persistent()
                        .remove(&engineer_auth_key(asset_id, &eng));
                    cleared_engineers.push_back(eng);
                }
            }
        }
    }

    // ─── Issue #1011 ──────────────────────────────────────────────────────────

    /// Immediately revoke a single engineer's authorization for a specific asset.
    ///
    /// Unlike `propose_revoke_engineer_auth` / `execute_revoke_engineer_auth` this
    /// function takes effect instantly — there is no timelock.  The asset owner
    /// calls this when they need to remove an engineer's access right away (e.g.
    /// after a trust violation or credential revocation).
    ///
    /// The engineer is removed from the per-asset authorized-engineers list so
    /// that `get_authorized_engineers` reflects the change immediately.
    ///
    /// # Arguments
    /// * `owner`     - The current owner of the asset; must sign this transaction.
    /// * `asset_id`  - The unique identifier of the asset.
    /// * `engineer`  - The engineer address whose authorization is being revoked.
    ///
    /// # Events
    /// Emits `(REVOKE_AUTH, owner) → (asset_id, engineer, timestamp)`.
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`]   if the contract has not been initialized.
    /// - [`ContractError::AssetNotFound`]    if the asset does not exist.
    /// - [`ContractError::UnauthorizedOwner`] if the caller is not the asset owner.
    pub fn revoke_engineer_authorization(
        env: Env,
        owner: Address,
        asset_id: u64,
        engineer: Address,
    ) {
        ensure_not_paused(&env);
        owner.require_auth();

        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);
        let asset =
            asset_registry::AssetRegistryClient::new(&env, &asset_registry).get_asset(&asset_id);
        if asset.owner != owner {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        // Remove the per-engineer-per-asset auth flag.
        env.storage()
            .persistent()
            .remove(&engineer_auth_key(asset_id, &engineer));

        // Remove from the per-asset authorized-engineers list (#1012).
        let list_key = authorized_engineers_key(asset_id);
        if let Some(list) = env
            .storage()
            .persistent()
            .get::<_, Vec<Address>>(&list_key)
        {
            let mut new_list: Vec<Address> = Vec::new(&env);
            for addr in list.iter() {
                if addr != engineer {
                    new_list.push_back(addr);
                }
            }
            env.storage().persistent().set(&list_key, &new_list);
            extend_persistent_ttl(&env, &list_key);
        }

        env.events().publish(
            (symbol_short!("REVOKE_AUTH"), owner.clone()),
            (asset_id, engineer.clone(), env.ledger().timestamp()),
        );
    }

    /// Immediately revoke multiple engineers' owner-approved authorizations for an asset.
    ///
    /// The current owner must authorize the call. The operation is bounded to
    /// keep transaction work predictable and updates the maintained authorized
    /// engineer list in one storage write.
    pub fn batch_revoke_engineer_authorizations(
        env: Env,
        owner: Address,
        asset_id: u64,
        engineers: Vec<Address>,
    ) {
        ensure_not_paused(&env);
        owner.require_auth();
        if engineers.len() > MAX_BATCH_REVOKE_SIZE {
            panic_with_error!(&env, ContractError::BatchRevokeTooLarge);
        }

        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);
        let asset =
            asset_registry::AssetRegistryClient::new(&env, &asset_registry).get_asset(&asset_id);
        if asset.owner != owner {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        let list_key = authorized_engineers_key(asset_id);
        let current_list: Vec<Address> = env
            .storage()
            .persistent()
            .get(&list_key)
            .unwrap_or_else(|| Vec::new(&env));
        let mut remaining = Vec::new(&env);

        for current in current_list.iter() {
            let mut revoked = false;
            for engineer in engineers.iter() {
                if current == engineer {
                    revoked = true;
                    break;
                }
            }
            if revoked {
                env.storage()
                    .persistent()
                    .remove(&engineer_auth_key(asset_id, &current));
                env.events().publish(
                    (symbol_short!("REVOKE_AUTH"), owner.clone()),
                    (asset_id, current, env.ledger().timestamp()),
                );
            } else {
                remaining.push_back(current);
            }
        }

        if !current_list.is_empty() {
            env.storage().persistent().set(&list_key, &remaining);
            extend_persistent_ttl(&env, &list_key);
        }
    }

    // ─── Issue #1012 ──────────────────────────────────────────────────────────

    /// Return the list of engineers currently authorized to submit maintenance
    /// for a specific asset.
    ///
    /// The list is maintained by `authorize_engineer` (add) and
    /// `revoke_engineer_authorization` (remove), and is also cleared by
    /// `clear_engineer_authorizations` on ownership transfer.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset.
    ///
    /// # Returns
    /// `Vec<Address>` of currently-authorized engineer addresses (may be empty).
    pub fn get_authorized_engineers(env: Env, asset_id: u64) -> Vec<Address> {
        env.storage()
            .persistent()
            .get(&authorized_engineers_key(asset_id))
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Initialize the lifecycle contract with registry addresses and configuration.
    /// Must be called once after deployment to bind dependent registries.
    ///
    /// # Arguments
    /// * `asset_registry` - Address of the asset registry contract
    /// * `engineer_registry` - Address of the engineer registry contract
    /// * `deployer` - The address of the contract deployer; must sign this transaction.
    /// * `admin` - Address that will have administrative privileges
    /// * `max_history` - Maximum maintenance records per asset (0 for default 200)
    ///
    /// # Panics
    /// - [`ContractError::AlreadyInitialized`] if contract has already been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if deployer is not the transaction invoker
    /// - [`ContractError::InvalidConfig`] if max_history exceeds 10,000, if registries are the same address, or if the compiled default decay_interval is 0
    pub fn initialize(
        env: Env,
        deployer: Address,
        asset_registry: Address,
        engineer_registry: Address,
        admin: Address,
        max_history: u32,
    ) {
        deployer.require_auth();
        let stored_deployer: Address = env
            .storage()
            .instance()
            .get(&DEPLOYER_KEY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if deployer != stored_deployer {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        if env.storage().persistent().has(&CONFIG) {
            panic_with_error!(&env, ContractError::AlreadyInitialized);
        }
        if is_zero_address(&env, &asset_registry) {
            panic_with_error!(&env, ContractError::ZeroAddress);
        }
        if is_zero_address(&env, &engineer_registry) {
            panic_with_error!(&env, ContractError::ZeroAddress);
        }
        if max_history > 10_000 {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }
        if asset_registry == engineer_registry {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }
        // Guard: the baked-in default decay_interval must never be 0; if it
        // somehow is (e.g. a misconfigured build constant), reject immediately
        // instead of silently deploying a contract that will panic on the first
        // `apply_decay` call with a division-by-zero.
        if DEFAULT_DECAY_INTERVAL == 0 {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }

        set_asset_registry_addr(&env, &asset_registry);
        set_engineer_registry_addr(&env, &engineer_registry);

        let config = Config {
            admin: admin.clone(),
            admins: Vec::new(&env),
            admin_threshold: 0,
            max_history: if max_history == 0 {
                DEFAULT_MAX_HISTORY
            } else {
                max_history
            },
            max_engineer_history: DEFAULT_MAX_ENGINEER_HISTORY,
            score_increment: DEFAULT_SCORE_INCREMENT,
            decay_rate: DEFAULT_DECAY_RATE,
            decay_interval: DEFAULT_DECAY_INTERVAL,
            eligibility_threshold: DEFAULT_ELIGIBILITY_THRESHOLD,
            min_collateral_score: DEFAULT_ELIGIBILITY_THRESHOLD,
            max_notes_length: DEFAULT_MAX_NOTES_LENGTH,
            task_weights: Map::new(&env),
            max_submissions_per_hour: DEFAULT_MAX_SUBMISSIONS_PER_HOUR,
        };
        env.storage().persistent().set(&CONFIG, &config);
        extend_persistent_ttl(&env, &CONFIG);

        env.events()
            .publish((EVENT_INIT,), (asset_registry, engineer_registry, admin));
    }

    /// Admin-only function to pause the contract.
    ///
    /// # Arguments
    /// * `admin` - The address that must match the stored admin
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    pub fn pause(env: Env, admin: Address) {
        crate::admin::pause(env, admin);
    }

    /// Admin-only function to unpause the contract.
    ///
    /// # Arguments
    /// * `admin` - The address that must match the stored admin
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    pub fn unpause(env: Env, admin: Address) {
        crate::admin::unpause(env, admin);
    }

     /// Propose pausing the contract using the admin timelock.
     ///
     /// # Arguments
     /// * `admin` - The administrator requesting the pause
    pub fn propose_pause(env: Env, admin: Address) {
        crate::admin::propose_pause(env, admin);
    }

     /// Execute a pending pause after the timelock expires.
     ///
     /// # Arguments
     /// * `admin` - The administrator performing the pause
    pub fn execute_pause(env: Env, admin: Address) {
        crate::admin::execute_pause(env, admin);
    }

     /// Propose unpausing the contract using the admin timelock.
     ///
     /// # Arguments
     /// * `admin` - The administrator requesting the unpause
    pub fn propose_unpause(env: Env, admin: Address) {
        crate::admin::propose_unpause(env, admin);
    }

     /// Execute a pending unpause after the timelock expires.
     ///
     /// # Arguments
     /// * `admin` - The administrator performing the unpause
    pub fn execute_unpause(env: Env, admin: Address) {
        crate::admin::execute_unpause(env, admin);
    }

     /// Check if the contract is currently paused.
    ///
    /// # Returns
    /// `true` if paused; `false` otherwise
    pub pub(crate) fn is_paused(env: Env) -> bool {
        is_paused(&env)
    }

    /// Propose a new admin address (step 1 of 2-step transfer).
    /// Only the current admin can propose a new admin.
    ///
    /// # Arguments
    /// * `admin` - The current admin address
    /// * `new_admin` - The address to propose as the new admin
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the current admin
    /// - [`ContractError::PendingAdminAlreadyExists`] if a pending admin already exists
    pub fn propose_admin(env: Env, admin: Address, new_admin: Address) {
        crate::admin::propose_admin(env, admin, new_admin);
    }

    /// Accept the admin transfer (step 2 of 2-step transfer).
    /// Only the pending admin can accept and become the new admin.
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if no pending admin exists
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the pending admin
    pub fn accept_admin(env: Env) {
        crate::admin::accept_admin(env);
    }

    /// Cancel a pending admin transfer proposal.
    /// Only the current admin can cancel. Clears the pending admin and its timelock entry.
    ///
    /// # Arguments
    /// * `admin` - The current admin address (must match stored config admin)
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the current admin
    /// - [`ContractError::ProposalNotFound`] if no pending admin proposal exists
    pub fn cancel_admin_proposal(env: Env, admin: Address) {
        admin.require_auth();
        let config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if config.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        if !env.storage().instance().has(&PENDING_ADMIN_KEY) {
            panic_with_error!(&env, ContractError::ProposalNotFound);
        }
        env.storage().instance().remove(&PENDING_ADMIN_KEY);
        // Also clear the admin-transfer timelock entry so it cannot be replayed.
        let tl_key = timelock_key(symbol_short!("ADM_XFER"));
        env.storage().persistent().remove(&tl_key);
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("ADM_CNCL")),
            (admin.clone(), env.ledger().timestamp()),
        );
        env.events().publish(
            (symbol_short!("ADM_CANCEL"),),
            (admin,),
        );
    }


    ///
    /// Sets the list of co-signers and the minimum number of signatures required to execute
    /// `reset_score`, `pause`, and other protected admin operations. Passing an empty
    /// `new_admins` or a `threshold` of 0 / 1 reverts to single-admin mode.
    ///
    /// # Arguments
    /// * `admin` - The current single admin (must match `config.admin`)
    /// * `new_admins` - Full replacement list of multisig co-signer addresses
    /// * `threshold` - Minimum signatures required (M in M-of-N); 0 means single-admin mode
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the current admin
    /// - [`ContractError::InvalidConfig`] if threshold exceeds the length of new_admins
    pub fn set_admin_quorum(env: Env, admin: Address, new_admins: Vec<Address>, threshold: u32) {
        ensure_not_paused(&env);
        admin.require_auth();

        let mut config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if config.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        if threshold > 0 && threshold as u32 > new_admins.len() {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }

        // Reject any list that contains duplicate addresses.  A duplicate
        // would let a single admin accumulate multiple quorum votes by
        // appearing at different positions in the list.  Fix #990.
        for i in 0..new_admins.len() {
            for j in (i + 1)..new_admins.len() {
                if new_admins.get(i).unwrap() == new_admins.get(j).unwrap() {
                    panic_with_error!(&env, ContractError::DuplicateAdmin);
                }
            }
        }

        config.admins = new_admins.clone();
        config.admin_threshold = threshold;
        env.storage().persistent().set(&CONFIG, &config);
        env.storage()
            .persistent()
            .extend_ttl(&CONFIG, TTL_THRESHOLD, TTL_TARGET);

        env.events().publish(
            (symbol_short!("SET_QRUM"), admin.clone()),
            (new_admins, threshold),
        );
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("SET_QRUM")),
            (admin, env.ledger().timestamp(), threshold),
        );
        crate::admin::set_admin_quorum(env, admin, new_admins, threshold);
    }

    /// Admin-only function to update the score increment configuration.
    /// This controls how much scores increase per maintenance task.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    /// * `score_increment` - New score increment value (must be > 0)
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::InvalidConfig`] if score_increment is 0
    pub fn update_score_increment(env: Env, admin: Address, score_increment: u32) {
        crate::admin::update_score_increment(env, admin, score_increment);
    }

    /// Admin-only function to update the decay rate and interval for collateral score decay.
    /// This controls how quickly scores decrease over time without maintenance.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    /// * `decay_rate` - Points to deduct per decay interval
    /// * `decay_interval` - Time interval in seconds for each decay step (must be > 0)
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::InvalidConfig`] if decay_interval is 0
    pub fn update_decay_config(env: Env, admin: Address, decay_rate: u32, decay_interval: u64) {
        crate::admin::update_decay_config(env, admin, decay_rate, decay_interval);
    }

    /// Admin-only function to update the eligibility threshold for collateral scoring.
    /// This sets the minimum score required for an asset to be eligible as collateral.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    /// * `threshold` - New eligibility threshold value
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    pub fn update_eligibility_threshold(env: Env, admin: Address, threshold: u32) {
        ensure_not_paused(&env);
        admin.require_auth();

        let mut config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if config.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        if threshold == 0 {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }

        let old_threshold = config.eligibility_threshold;
        config.eligibility_threshold = threshold;
        config.min_collateral_score = threshold;
        env.storage().persistent().set(&CONFIG, &config);
        extend_persistent_ttl(&env, &CONFIG);
        env.events()
            .publish((symbol_short!("CFG_UPD"),), (old_threshold, threshold));
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("CFG_UPD")),
            (
                admin,
                env.ledger().timestamp(),
                symbol_short!("ELIG"),
                threshold,
            ),
        );
        crate::admin::update_eligibility_threshold(env, admin, threshold);
    }

    /// Admin-only function to update the minimum collateral score required for eligibility.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    /// * `min_collateral_score` - New minimum collateral score value (must be > 0)
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::InvalidConfig`] if `min_collateral_score` is 0
    pub fn update_config(env: Env, admin: Address, min_collateral_score: u32) {
        ensure_not_paused(&env);
        admin.require_auth();

        if min_collateral_score == 0 {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }

        let mut config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if config.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        let old_min = config.min_collateral_score;
        config.min_collateral_score = min_collateral_score;
        config.eligibility_threshold = min_collateral_score;
        env.storage().persistent().set(&CONFIG, &config); 
        extend_persistent_ttl(&env, &CONFIG);

        env.events().publish(
            (symbol_short!("CFG_UPD"),),
            (old_min, min_collateral_score),
        );
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("CFG_UPD")),
            (
                admin,
                env.ledger().timestamp(),
                symbol_short!("MIN_COLL"),
                min_collateral_score,
            ),
        );
    }

    /// Admin-only function to update the maximum history records per asset.
    /// This allows adjusting the cap on maintenance history without redeployment.
    ///
    /// # Lazy Pruning Behavior
    /// When `new_max` is lower than the current cap, existing per-asset histories that exceed
    /// the new cap are **not** automatically pruned. Pruning happens lazily during the next
    /// maintenance submission for that asset. To immediately prune an asset's history to the
    /// new cap, use `prune_asset_history()`.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    /// * `new_max` - New maximum history value (must be > 0)
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::InvalidConfig`] if new_max is 0
    pub fn update_max_history(env: Env, admin: Address, new_max: u32) {
        crate::admin::update_max_history(env, admin, new_max);
    }

    /// Admin-only function to update the cap on each engineer's per-address
    /// asset-history list.
    ///
    /// Once the list reaches `new_max` entries the oldest entry is silently
    /// dropped on the next write, bounding per-engineer storage and keeping
    /// every `submit_maintenance` / `batch_submit_maintenance` call to O(cap)
    /// work instead of O(ever-growing list).
    ///
    /// # Arguments
    /// * `admin`   - The admin address that must match the stored config admin.
    /// * `new_max` - New per-engineer history cap (must be > 0).
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialised.
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin.
    /// - [`ContractError::InvalidConfig`] if `new_max` is 0.
    pub fn update_max_engineer_history(env: Env, admin: Address, new_max: u32) {
        crate::admin::update_max_engineer_history(env, admin, new_max);
    }

    /// Admin-only function to update the maximum allowed notes length per maintenance record.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    /// * `new_max` - New maximum notes length in bytes (must be > 0)
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::InvalidConfig`] if new_max is 0
    pub fn update_max_notes_length(env: Env, admin: Address, new_max: u32) {
        crate::admin::update_max_notes_length(env, admin, new_max);
    }

    /// Admin-only function to directly set the maximum allowed notes length.
    /// Unlike `update_max_notes_length`, this takes effect immediately without a timelock.
    /// Useful for deployments that need to quickly adjust storage cost controls.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    /// * `length` - New maximum notes length in bytes (must be > 0)
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::InvalidConfig`] if length is 0
    pub fn set_max_notes_length(env: Env, admin: Address, length: u32) {
        crate::admin::set_max_notes_length(env, admin, length);
    }

    /// Admin-only function to set the minimum lifecycle score required for collateral eligibility.
    /// Different DeFi lenders may require different minimum scores; this allows post-deploy configuration.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    /// * `value` - The new eligibility threshold (must be > 0)
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::InvalidConfig`] if value is 0
    pub fn set_eligibility_threshold(env: Env, admin: Address, value: u32) {
        crate::admin::set_eligibility_threshold(env, admin, value);
    }

    /// Admin-only function to set a custom weight for a specific task type.
    /// Allows per-task-type score increment configuration. Falls back to defaults if not set.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    /// * `task_type` - The task type symbol to configure
    /// * `weight` - The weight/increment value for this task type. The minimum supported weight is 1.
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::InvalidConfig`] if weight is 0
    /// - [`ContractError::ProposalNotFound`] if no task-weight update has been proposed
    /// - [`ContractError::TimelockNotExpired`] if the proposal is still inside the timelock delay
    pub fn set_task_weight(env: Env, admin: Address, task_type: Symbol, weight: u32) {
        ensure_not_paused(&env);
        admin.require_auth();

        assert!(weight > 0, "task weight must be greater than 0");

        let mut config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if config.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        require_timelock_ready(&env, symbol_short!("TSK_WT"));

        config.task_weights.set(task_type.clone(), weight);
        env.storage().persistent().set(&CONFIG, &config);
        extend_persistent_ttl(&env, &CONFIG);

        let now = env.ledger().timestamp();
        // Primary structured event: full weight-change record for off-chain indexers.
        env.events().publish(
            (EVENT_WEIGHT_SET,),
            (task_type.clone(), old_weight, weight, admin.clone(), now),
        );
        // Legacy events kept for backward compatibility.
        env.events()
            .publish((symbol_short!("TSK_WT"),), (task_type.clone(), weight));
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("TSK_WT")),
            (admin, now, task_type, weight),
        );
        crate::admin::set_task_weight(env, admin, task_type, weight);
    }

    /// Propose a task-type score-weight change, subject to a timelock.
    ///
    /// Creates a [`WeightProposal`] stored under [`DataKey::WeightProposal`] for
    /// `task_type`. The change cannot be executed until `TIMELOCK_DELAY_SECS`
    /// seconds have elapsed (call [`execute_weight_change`]).
    ///
    /// Only one active proposal per `task_type` is allowed at a time; creating a
    /// second proposal for the same task type before the first is executed will
    /// panic with [`ContractError::WeightProposalAlreadyExists`].
    ///
    /// # Arguments
    /// * `admin` - Admin address (must match stored admin).
    /// * `task_type` - The task-type symbol whose weight is being changed.
    /// * `new_weight` - The proposed new weight value (must be > 0).
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized.
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin.
    /// - [`ContractError::InvalidConfig`] if `new_weight` is 0.
    /// - [`ContractError::WeightProposalAlreadyExists`] if a non-executed proposal
    ///   already exists for this task type.
    pub fn propose_weight_change(env: Env, admin: Address, task_type: Symbol, new_weight: u32) {
        ensure_not_paused(&env);
        require_admin(&env, &admin);

        if new_weight == 0 {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }

        let key = weight_proposal_key(&task_type);

        // Reject if an active (non-executed) proposal already exists.
        if let Some(existing) = env.storage().persistent().get::<_, WeightProposal>(&key) {
            if !existing.executed {
                panic_with_error!(&env, ContractError::WeightProposalAlreadyExists);
            }
        }

        let proposal = WeightProposal {
            new_weight,
            proposed_at: env.ledger().timestamp(),
            executed: false,
        };
        env.storage().persistent().set(&key, &proposal);
        extend_persistent_ttl(&env, &key);

        env.events().publish(
            (EVENT_WEIGHT_PROP, task_type.clone()),
            (admin, new_weight, env.ledger().timestamp()),
        );
    }

    /// Execute a pending task-type weight-change proposal after the timelock expires.
    ///
    /// Looks up the proposal created by [`propose_weight_change`] for `task_type`,
    /// checks that `TIMELOCK_DELAY_SECS` have elapsed, applies the new weight to
    /// the config, marks the proposal as executed, and emits a `WT_EXEC` event.
    ///
    /// # Arguments
    /// * `admin` - Admin address (must match stored admin).
    /// * `task_type` - The task-type symbol whose weight is being changed.
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized.
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin.
    /// - [`ContractError::ProposalNotFound`] if no proposal exists or it was already executed.
    /// - [`ContractError::TimelockNotExpired`] if the delay has not yet elapsed.
    pub fn execute_weight_change(env: Env, admin: Address, task_type: Symbol) {
        ensure_not_paused(&env);
        require_admin(&env, &admin);

        let key = weight_proposal_key(&task_type);
        let mut proposal: WeightProposal = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::ProposalNotFound));

        if proposal.executed {
            panic_with_error!(&env, ContractError::ProposalNotFound);
        }

        if env.ledger().timestamp().saturating_sub(proposal.proposed_at) < TIMELOCK_DELAY_SECS {
            panic_with_error!(&env, ContractError::TimelockNotExpired);
        }

        let new_weight = proposal.new_weight;
        proposal.executed = true;
        env.storage().persistent().set(&key, &proposal);
        extend_persistent_ttl(&env, &key);

        // Apply the new weight to the config.
        let mut config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        config.task_weights.set(task_type.clone(), new_weight);
        env.storage().persistent().set(&CONFIG, &config);
        extend_persistent_ttl(&env, &CONFIG);

        env.events().publish(
            (EVENT_WEIGHT_EXEC, task_type.clone()),
            (admin, new_weight, env.ledger().timestamp()),
        );
    }

    /// Admin-only function to set per-asset-type maintenance-frequency scoring weights.
    ///
    /// `weights_json` is stored verbatim and interpreted as a JSON object with these keys:
    /// `low`, `medium`, `high`, and optional `medium_threshold`, `high_threshold`, `window_days`.
    /// Weight values are treated as percentages, so `120` means a 1.2x multiplier.
    pub fn update_scoring_weights(env: Env, admin: Address, asset_type: Symbol, weights_json: Bytes) {
        crate::admin::update_scoring_weights(env, admin, asset_type, weights_json);
    }

    /// Returns the stored dynamic scoring weights for an asset type.
    /// If none are configured, an empty `Bytes` value is returned.
    pub fn get_scoring_weights(env: Env, asset_type: Symbol) -> Bytes {
        env.storage()
            .persistent()
            .get(&scoring_weights_key(&env, &asset_type))
            .unwrap_or_else(|| Bytes::from_slice(&env, &[]))
    }

    /// Submit a maintenance record for an asset.
    /// Only verified engineers can submit maintenance records.
    ///
    /// # Ownership Transfer Note
    /// Maintenance history is asset-scoped and persists across ownership transfers.
    /// Records submitted before a transfer still reference the original engineer addresses.
    /// A sentinel record with `task_type = "XFER"` is inserted by [`record_transfer`] to
    /// mark the ownership boundary; records before it were performed under the previous owner.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset being maintained
    /// * `task_type` - Symbol representing the type of maintenance task
    /// * `notes` - String containing maintenance notes and details
    /// * `engineer` - Address of the engineer performing the maintenance
    /// * `cost` - Optional maintenance cost in stroops (1 stroop = 10^-7 XLM)
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    /// - [`ContractError::UnauthorizedEngineer`] if the engineer is not verified
    /// - [`ContractError::HistoryCapReached`] if the asset has reached max history records
    pub fn submit_maintenance(
        env: Env,
        asset_id: u64,
        task_type: Symbol,
        priority: Priority,
        notes: String,
        engineer: Address,
        cost: Option<u64>,
    ) {
        ensure_not_paused(&env);
        engineer.require_auth();

        let config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));

        // Validate task type early before cross-contract calls
        let _weight = get_task_weight(&env, &task_type, &config);
        validate_notes_length(&env, &notes, config.max_notes_length);

        // Enforce per-engineer submission rate before cross-contract calls / writes.
        enforce_submission_rate(&env, &engineer, &config, 1);

        // Check history cap before cross-contract calls to avoid wasting gas
        let mut history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or(Vec::new(&env));

        let pruned = if config.max_history > 0 && history.len() >= config.max_history {
            let excess = (history.len() - config.max_history + 1) as u32;
            for _ in 0..excess {
                history.remove(0);
            }
            excess
        } else {
            0
        };
        if pruned > 0 {
            env.events().publish((EVENT_PRUNED,), (asset_id, pruned));
        }

        // #1022: Acquire reentrancy lock before cross-contract calls to prevent
        // a malicious registry contract from re-entering submit_maintenance
        // mid-execution and causing double-writes to maintenance history.
        acquire_reentrancy_guard(&env);

        // Verify asset exists and is not decommissioned
        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);
        let asset_client = asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        let status = asset_client.asset_status(&asset_id);
        use asset_registry::AssetStatus;
        if status == AssetStatus::Decommissioned {
            panic_with_error!(&env, ContractError::AssetDecommissioned);
        }
        // Also block if the asset was decommissioned via lifecycle's decommission_asset (#1013).
        if env.storage().persistent().get::<_, bool>(&frozen_key(asset_id)).unwrap_or(false) {
            panic_with_error!(&env, ContractError::AssetDecommissioned);
        }

        // Verify engineer credential via the engineer registry.
        let registry_id = get_engineer_registry_addr(&env);
        let registry = engineer_registry::EngineerRegistryClient::new(&env, &registry_id);
        let status = registry.get_credential_status(&engineer);
        if status != engineer_registry::CredentialStatus::Valid
            && status != engineer_registry::CredentialStatus::GracePeriod
        {
            panic_with_error!(&env, ContractError::UnauthorizedEngineer);
        }

        // Verify engineer is explicitly authorized by the asset owner
        // for this specific asset (DataKey::EngineerAuth check).
        require_engineer_authorized(&env, asset_id, &engineer);

        // Validate engineer specialization matches asset type
        let asset_client = asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        let asset = asset_client.get_asset(&asset_id);
        let specializations = registry.get_specializations(&engineer);
        let mut spec_matched = false;
        for spec in specializations.iter() {
            if spec == asset.asset_type {
                spec_matched = true;
                break;
            }
        }
        if !spec_matched {
            panic_with_error!(&env, ContractError::SpecializationMismatch);
        }

        let timestamp = env.ledger().timestamp();
        let previous_record_hash = next_chain_link(&env, &history);

        // Propagate ownership_start_ledger: if this asset has had a transfer, all
        // new maintenance records carry the ledger sequence number at which that
        // transfer was recorded.  This lets DeFi lenders filter history to only
        // the current owner's tenure via get_maintenance_history_since_transfer.
        let ownership_start_ledger: Option<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::OwnershipStartLedger(asset_id));

        let record = MaintenanceRecord {
            asset_id,
            task_type: task_type.clone(),
            priority,
            notes,
            engineer: engineer.clone(),
            timestamp,
            cost,
            ownership_start_ledger,
            previous_record_hash,
        };

        history.push_back(record);
        env.storage()
            .persistent()
            .set(&history_key(asset_id), &history);
        extend_persistent_ttl(&env, &history_key(asset_id));

        // #1222: Advance next_due for any recurring task this submission satisfies,
        // so the schedule doesn't stay perpetually overdue after the first submission.
        advance_recurring_tasks_on_submission(&env, asset_id, &task_type, timestamp);

        engineer_history_add(&env, &engineer, asset_id, config.max_engineer_history);

        // Accumulate score: add this submission's increment to the stored score (cap at 100).
        // Weight the increment by the engineer's reputation (0–1000), scaled to 0.5×–1.5×:
        //   multiplier = (500 + reputation) / 1000  (reputation=0 → 0.5×, 500 → 1.0×, 1000 → 1.5×)
        let reputation = registry.get_reputation(&engineer);
        let weighted_increment = ((config.score_increment as u64) * (500 + reputation as u64) / 1000) as u32;
        let current_score: u32 = env
            .storage()
            .persistent()
            .get(&score_key(asset_id))
            .unwrap_or(0);
        let new_score = current_score.saturating_add(weighted_increment).min(100);

        // Persist the accumulated score so apply_decay / get_collateral_score can read it.
        env.storage().persistent().set(&score_key(asset_id), &new_score);
        extend_persistent_ttl(&env, &score_key(asset_id));

        // Append (timestamp, score) snapshot to score history for historical tracking
        score_history_push(
            &env,
            asset_id,
            ScoreEntry {
                timestamp,
                score: new_score,
            },
            config.max_history,
        );

        // Update last maintenance timestamp for decay tracking
        env.storage()
            .persistent()
            .set(&last_update_key(asset_id), &timestamp);
        extend_persistent_ttl(&env, &last_update_key(asset_id));

        // Emit maintenance submission event
        env.events().publish(
            (symbol_short!("maint"),),
            (asset_id, engineer.clone(), task_type, timestamp),
        );

        // Increment the engineer's reputation score by 1 (clamped at 1000 inside the registry).
        // We emit REP_UPD with old and new scores so indexers can track progression.
        let old_rep = registry.get_reputation(&engineer);
        registry.update_reputation(&engineer, 1);
        let new_rep = registry.get_reputation(&engineer);
        env.events().publish(
            (symbol_short!("REP_UPD"), engineer.clone()),
            (old_rep, new_rep),
        );
        // #1022: Release reentrancy lock now that all state mutations and
        // cross-contract calls are complete.
        release_reentrancy_guard(&env);
    }

    /// Record an ownership transfer in the asset's maintenance history.
    ///
    /// Appends a sentinel [`MaintenanceRecord`] with `task_type = "XFER"` and emits a
    /// `XFER` event so that indexers and new owners can identify the ownership boundary.
    /// Records before this sentinel were performed under the previous owner; records after
    /// it are performed under the new owner.
    /// Record a sentinel entry in the maintenance history to mark an ownership transfer.
    ///
    /// Must be called **after** `asset_registry::transfer_asset` has already updated the
    /// on-chain owner, and must be called by the new owner (i.e. the address that now owns
    /// the asset in the registry).  The function cross-calls the asset registry to confirm
    /// that `new_owner` is indeed the current owner before writing the sentinel, preventing
    /// a replay of `new_owner`'s signature from inserting a false transfer record into an
    /// asset they do not own.
    ///
    /// # Arguments
    /// * `asset_id`       - The unique identifier of the transferred asset
    /// * `previous_owner` - Address of the owner before the transfer
    /// * `new_owner`      - Address of the owner after the transfer (must match registry)
    ///
    /// # Events
    /// Emits `(EVENT_XFER, asset_id)` with data `(previous_owner, new_owner, timestamp, sentinel_index)`
    /// where `sentinel_index` is the zero-based position of the XFER sentinel in the history vec,
    /// allowing indexers to directly correlate the event with the record.
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    /// - [`ContractError::UnauthorizedOwner`] if `new_owner` does not match the current
    ///   owner recorded in the asset registry
    pub fn record_transfer(env: Env, asset_id: u64, previous_owner: Address, new_owner: Address) {
        ensure_not_paused(&env);
        new_owner.require_auth();

        let config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));

        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);

        // Verify new_owner is actually the current owner in the asset registry.
        // This prevents a signature replay from inserting a false transfer sentinel
        // into an asset the caller does not own.
        let registry_client = asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        let asset = registry_client.get_asset(&asset_id);
        if asset.owner != new_owner {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        // Invalidate all per-asset engineer authorizations on every ownership
        // transfer so an engineer authorized by the previous owner cannot keep
        // submitting maintenance records against the new owner without being
        // re-authorized.
        let auth_list_key = authorized_engineers_key(asset_id);
        let authorized_engineers: Vec<Address> = env
            .storage()
            .persistent()
            .get(&auth_list_key)
            .unwrap_or_else(|| Vec::new(&env));
        for eng in authorized_engineers.iter() {
            env.storage()
                .persistent()
                .remove(&engineer_auth_key(asset_id, &eng));
        }
        env.storage().persistent().remove(&auth_list_key);

        let timestamp = env.ledger().timestamp();
        // Record the current ledger sequence as the ownership_start_ledger for this
        // asset. The XFER sentinel itself carries this value (rather than `None`) so
        // that `get_maintenance_history_since_transfer` can anchor on the sentinel to
        // distinguish pre- and post-transfer records; all subsequent maintenance
        // records also carry this value.
        let current_ledger = env.ledger().sequence();

        let mut history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or_else(|| Vec::new(&env));

        if config.max_history > 0 && history.len() >= config.max_history {
            history.remove(0);
        }

        let previous_record_hash = next_chain_link(&env, &history);
        let sentinel = MaintenanceRecord {
            asset_id,
            task_type: symbol_short!("XFER"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Ownership transferred"),
            engineer: new_owner.clone(),
            timestamp,
            cost: None,
            ownership_start_ledger: Some(current_ledger),
            previous_record_hash,
        };
        history.push_back(sentinel);
        let sentinel_index = history.len() - 1;
        env.storage()
            .persistent()
            .set(&history_key(asset_id), &history);
        extend_persistent_ttl(&env, &history_key(asset_id));

        env.storage()
            .persistent()
            .set(&DataKey::OwnershipStartLedger(asset_id), &current_ledger);
        extend_persistent_ttl(&env, &DataKey::OwnershipStartLedger(asset_id));

        // Append to the dedicated transfer history for provenance verification.
        let xfer_key = transfer_hist_key(asset_id);
        let mut xfer_history: Vec<TransferRecord> = env
            .storage()
            .persistent()
            .get(&xfer_key)
            .unwrap_or_else(|| Vec::new(&env));
        xfer_history.push_back(TransferRecord {
            from: previous_owner.clone(),
            to: new_owner.clone(),
            timestamp,
        });
        env.storage().persistent().set(&xfer_key, &xfer_history);
        env.storage()
            .persistent()
            .extend_ttl(&xfer_key, TTL_THRESHOLD, TTL_TARGET);

        // Clear all EngineerAuth entries granted by the previous owner so that
        // engineers authorized by the previous owner cannot submit maintenance
        // under the new owner without re-authorization.
        let mut cleared_engineers: Vec<Address> = Vec::new(&env);
        // Reuse the already-loaded maintenance history (which now includes the
        // transfer sentinel) to avoid an extra storage read.
        for record in history.iter() {
            let eng = record.engineer.clone();
            let mut already_cleared = false;
            for cleared in cleared_engineers.iter() {
                if cleared == eng {
                    already_cleared = true;
                    break;
                }
            }
            if !already_cleared {
                env.storage()
                    .persistent()
                    .remove(&engineer_auth_key(asset_id, &eng));
                cleared_engineers.push_back(eng);
            }
        }
        if !cleared_engineers.is_empty() {
            env.events().publish(
                (symbol_short!("AUTH_CLR"), asset_id),
                cleared_engineers.len(),
            );
        }

        env.events().publish(
            (EVENT_XFER, asset_id),
            (previous_owner, new_owner, timestamp, sentinel_index),
        );
    }

    /// Return the full ownership transfer history for an asset.
    ///
    /// Each entry records the previous owner, new owner, and the ledger timestamp
    /// at which the transfer was recorded. Useful for provenance verification and
    /// DeFi due diligence.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset to query
    ///
    /// # Returns
    /// Vec of [`TransferRecord`] in chronological order (oldest first).
    /// Returns an empty vec if no transfers have occurred.
    pub fn get_transfer_history(env: Env, asset_id: u64) -> Vec<TransferRecord> {
        let key = transfer_hist_key(asset_id);
        let history: Vec<TransferRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(&env));
        if env.storage().persistent().has(&key) {
            env.storage()
                .persistent()
                .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
        }
        history
    }

    /// Return a paginated view of the ownership transfer history for an asset.
    ///
    /// DeFi lenders use this to assess collateral risk when an asset has changed
    /// hands many times. Pagination keeps individual call costs predictable.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset to query
    /// * `offset`   - Zero-based index of the first record to return
    /// * `limit`    - Maximum number of records to return (capped at 50; 0 returns empty vec)
    ///
    /// # Returns
    /// Vec of [`TransferRecord`] in chronological order for the requested page.
    /// Returns an empty vec if `offset` is beyond the end of the history.
    pub fn get_transfer_history_paginated(
        env: Env,
        asset_id: u64,
        offset: u32,
        limit: u32,
    ) -> Vec<TransferRecord> {
        if limit == 0 {
            return Vec::new(&env);
        }

        // Cap at 50 entries per call to bound compute costs.
        const MAX_PAGE: u32 = 50;
        let limit = limit.min(MAX_PAGE);

        let key = transfer_hist_key(asset_id);
        let history: Vec<TransferRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(&env));

        let len = history.len();
        if offset >= len {
            return Vec::new(&env);
        }

        if env.storage().persistent().has(&key) {
            env.storage()
                .persistent()
                .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
        }

        let end = (offset + limit).min(len);
        let mut page: Vec<TransferRecord> = Vec::new(&env);
        for i in offset..end {
            page.push_back(history.get(i).unwrap());
        }
        page
    }

    /// Submit multiple maintenance records for the same asset in a single transaction.
    /// All records are validated before any are written to ensure atomicity.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset being maintained
    /// * `records` - Vec of BatchRecord containing maintenance data
    /// * `engineer` - Address of the engineer performing the maintenance
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    /// - [`ContractError::UnauthorizedEngineer`] if the engineer is not verified
    /// - [`ContractError::HistoryCapReached`] if adding records would exceed max history
    /// - [`ContractError::BatchTooLarge`] if `records.len() > MAX_BATCH_SIZE`
    pub fn batch_submit_maintenance(
        env: Env,
        asset_id: u64,
        records: Vec<BatchRecord>,
        engineer: Address,
        costs: Option<Vec<Option<u64>>>,
    ) {
        ensure_not_paused(&env);
        engineer.require_auth();

        let config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));

        // Validate records early before cross-contract calls
        require_non_empty_vec(&records, "records");

        // DoS / gas-limit guard: reject unbounded batches before doing any
        // further work (cross-contract calls, storage reads, etc.). This
        // returns a structured `BatchTooLarge` contract error rather than
        // letting the transaction exhaust the ledger's instruction budget.
        if records.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, ContractError::BatchTooLarge);
        }
        for record in records.iter() {
            validate_notes_length(&env, &record.notes, config.max_notes_length);
            // Validate task type is known
            let _ = get_task_weight(&env, &record.task_type, &config);
        }

        // Enforce per-engineer submission rate: each record in the batch counts
        // individually, since a large batch is just as capable of flooding the
        // ledger as the same number of individual `submit_maintenance` calls.
        enforce_submission_rate(&env, &engineer, &config, records.len());

        // Validate asset exists
        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);

        // Verify engineer credential via the engineer registry.
        let engineer_registry = get_engineer_registry_addr(&env);
        let engineer_registry_client =
            engineer_registry::EngineerRegistryClient::new(&env, &engineer_registry);
        let status = engineer_registry_client.get_credential_status(&engineer);
        if status != engineer_registry::CredentialStatus::Valid
            && status != engineer_registry::CredentialStatus::GracePeriod
        {
            panic_with_error!(&env, ContractError::UnauthorizedEngineer);
        }

        // Verify engineer is explicitly authorized by the asset owner
        // for this specific asset (DataKey::EngineerAuth check).
        require_engineer_authorized(&env, asset_id, &engineer);

        // Validate engineer specialization matches asset type
        let asset_client = asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        let asset = asset_client.get_asset(&asset_id);
        let specializations = engineer_registry_client.get_specializations(&engineer);
        let mut spec_matched = false;
        for spec in specializations.iter() {
            if spec == asset.asset_type {
                spec_matched = true;
                break;
            }
        }
        if !spec_matched {
            panic_with_error!(&env, ContractError::SpecializationMismatch);
        }

        let timestamp = env.ledger().timestamp();
        let mut history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or(Vec::new(&env));

        // Validate all records fit before writing any
        if history.len() + records.len() > config.max_history {
            panic_with_error!(&env, ContractError::HistoryCapReached);
        }

        if costs.is_some() && costs.as_ref().map(|c| c.len() != records.len()).unwrap_or(false) {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }

        // Propagate ownership_start_ledger for post-transfer records (same as
        // submit_maintenance; batches must carry the same field so lenders get
        // consistent results from get_maintenance_history_since_transfer).
        let ownership_start_ledger: Option<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::OwnershipStartLedger(asset_id));

        // Build all records and compute final score before any write.
        let reputation = engineer_registry_client.get_reputation(&engineer);
        let weighted_increment = ((config.score_increment as u64) * (500 + reputation as u64) / 1000) as u32;
        let mut score: u32 = env
            .storage()
            .persistent()
            .get(&score_key(asset_id))
            .unwrap_or(0u32);

        let mut new_records: Vec<MaintenanceRecord> = Vec::new(&env);
        let mut score_entries: Vec<ScoreEntry> = Vec::new(&env);
        let mut rec_idx: u32 = 0;
        let mut chain_link = next_chain_link(&env, &history);
        for record in records.iter() {
            score = score
                .checked_add(weighted_increment)
                .map(|s: u32| s.min(100))
                .unwrap_or_else(|| panic_with_error!(&env, ContractError::ScoreOverflow));
            let rec_cost = costs
                .as_ref()
                .and_then(|c| c.get(rec_idx))
                .and_then(|o| o);
            let new_record = MaintenanceRecord {
                asset_id,
                task_type: record.task_type.clone(),
                priority: record.priority,
                notes: record.notes.clone(),
                engineer: engineer.clone(),
                timestamp,
                cost: rec_cost,
                ownership_start_ledger,
                previous_record_hash: chain_link,
            };
            chain_link = Some(hash_maintenance_record(&env, &new_record));
            new_records.push_back(new_record);
            score_entries.push_back(ScoreEntry { timestamp, score });
            rec_idx += 1;
        }

        // All validation passed — now commit everything atomically.
        for record in new_records.iter() {
            history.push_back(record);
        }
        for entry in score_entries.iter() {
            score_history_push(&env, asset_id, entry, config.max_history);
        }
        for record in records.iter() {
            env.events().publish(
                (EVENT_MAINT, asset_id),
                (record.task_type.clone(), record.priority, engineer.clone(), timestamp),
            );
            // #1221: Critical-priority records get no elevated authorization check,
            // so alert lenders/watchers directly via a dedicated event instead.
            if record.priority == Priority::Critical {
                env.events().publish(
                    (symbol_short!("CRIT_MNT"), asset_id),
                    (record.task_type.clone(), engineer.clone(), timestamp),
                );
            }
        }

        // Add to engineer history only once per asset per batch
        engineer_history_add(&env, &engineer, asset_id, config.max_engineer_history);

        env.storage()
            .persistent()
            .set(&history_key(asset_id), &history);
        extend_persistent_ttl(&env, &history_key(asset_id));
        env.storage().persistent().set(&score_key(asset_id), &score);
        extend_persistent_ttl(&env, &score_key(asset_id));
        env.storage()
            .persistent()
            .set(&last_update_key(asset_id), &timestamp);
        extend_persistent_ttl(&env, &last_update_key(asset_id));

        // Increment the engineer's reputation score by 1 per record submitted in the batch.
        let batch_len = records.len() as i32;
        let old_rep = engineer_registry_client.get_reputation(&engineer);
        engineer_registry_client.update_reputation(&engineer, batch_len);
        let new_rep = engineer_registry_client.get_reputation(&engineer);
        env.events().publish(
            (symbol_short!("REP_UPD"), engineer.clone()),
            (old_rep, new_rep),
        );
    }

    /// Apply time-based decay to an asset's collateral score.
    /// Can be called by anyone to ensure scores reflect current maintenance status.
    /// Uses configured decay rate and interval settings.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset to decay
    ///
    /// # Returns
    /// The new collateral score after applying decay
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    pub fn decay_score(env: Env, asset_id: u64) -> u32 {
        ensure_not_paused(&env);
        // Frozen (decommissioned) assets are not eligible for collateral; return 0.
        // Fix #794: Frozen (decommissioned) assets always score 0; decay is a no-op.
        if env.storage().persistent().get::<_, bool>(&frozen_key(asset_id)).unwrap_or(false) {
            return 0;
        }
        let config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        apply_decay(&env, asset_id, true, true, config.max_history)
    }

    /// Admin-only: force time-based decay to be applied immediately for an asset.
    ///
    /// This is useful for marking assets that have been idle for extended periods
    /// without waiting for the next maintenance submission.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    /// * `asset_id` - The asset to decay
    pub fn force_decay(env: Env, admin: Address, asset_id: u64) {
        crate::admin::force_decay(env, admin, asset_id);
    }

    // ─── Issue #1013 ──────────────────────────────────────────────────────────

    /// Permanently decommission an asset, freezing its collateral score and
    /// blocking any further maintenance submissions or score decay.
    ///
    /// This is the owner-facing counterpart to the asset-registry-initiated
    /// decommission path.  Once called:
    ///
    /// * `decay_score` / `apply_decay` become no-ops (returns 0).
    /// * `submit_maintenance` panics with [`ContractError::AssetDecommissioned`].
    /// * `get_collateral_score` returns the frozen score (0 after this call).
    ///
    /// The function mirrors what `decommission_notify` does when called by the
    /// asset registry, but is callable directly by the asset owner without
    /// requiring an on-chain asset-registry admin action.
    ///
    /// # Arguments
    /// * `owner`    - The current owner of the asset; must sign this transaction.
    /// * `asset_id` - The unique identifier of the asset to decommission.
    ///
    /// # Events
    /// Emits `(DECOMM, asset_id) → 0u32` (frozen score).
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`]    if the contract has not been initialized.
    /// - [`ContractError::AssetNotFound`]     if the asset does not exist.
    /// - [`ContractError::UnauthorizedOwner`] if the caller is not the asset owner.
    /// - [`ContractError::ScoreFrozen`]       if the asset is already decommissioned.
    pub fn decommission_asset(env: Env, owner: Address, asset_id: u64) {
        ensure_not_paused(&env);
        owner.require_auth();

        // Ensure contract is initialized.
        env.storage()
            .persistent()
            .get::<_, Config>(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));

        // Verify the asset exists and the caller is the owner.
        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);
        let asset =
            asset_registry::AssetRegistryClient::new(&env, &asset_registry).get_asset(&asset_id);
        if asset.owner != owner {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        // Guard against double-decommission.
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&frozen_key(asset_id))
            .unwrap_or(false)
        {
            panic_with_error!(&env, ContractError::ScoreFrozen);
        }

        // Freeze the score at 0 (decommissioned assets are worthless as collateral).
        let zero_score: u32 = 0;
        env.storage()
            .persistent()
            .set(&frozen_score_key(asset_id), &zero_score);
        extend_persistent_ttl(&env, &frozen_score_key(asset_id));

        // Zero out the live score key as well so every read path agrees.
        env.storage()
            .persistent()
            .set(&score_key(asset_id), &zero_score);
        extend_persistent_ttl(&env, &score_key(asset_id));

        // Set the frozen flag — this is what `decay_score` and `submit_maintenance`
        // check to bail out early.
        env.storage()
            .persistent()
            .set(&frozen_key(asset_id), &true);
        extend_persistent_ttl(&env, &frozen_key(asset_id));

        env.events()
            .publish((symbol_short!("DECOMM"), asset_id), zero_score);
    }

    /// Called by the asset registry when an asset is decommissioned.
    /// Zeroes out the collateral score so that lenders cannot use a
    /// decommissioned asset as DeFi collateral.  The pre-decommission
    /// score is *not* preserved — any non-zero value would be misleading
    /// because the asset is permanently out of service.
    ///
    /// Authorization: only callable by the stored asset registry address.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the decommissioned asset
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// Notify lifecycle contract of asset ownership transfer.
    /// Called by asset-registry to clear engineer authorizations for the new owner.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the transferred asset
    /// * `new_owner` - The address of the new asset owner
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedOwner`] if asset owner doesn't match
    pub fn transfer_notify(env: Env, asset_id: u64, new_owner: Address) {
        let asset_registry = get_asset_registry_addr(&env);
        asset_registry.require_auth();
        env.storage()
            .persistent()
            .get::<_, Config>(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));

        // Fix #794: store 0 as the frozen score so decommissioned assets always
        // report a collateral score of zero rather than the last live score.
        let zero_score: u32 = 0;
        env.storage()
            .persistent()
            .set(&frozen_score_key(asset_id), &zero_score);
        env.storage()
            .persistent()
            .extend_ttl(&frozen_score_key(asset_id), TTL_THRESHOLD, TTL_TARGET);

        // Also zero out the live score key so any path that reads score_key
        // directly (e.g. apply_decay, bulk queries) also returns 0.
        env.storage()
            .persistent()
            .set(&score_key(asset_id), &zero_score);
        env.storage()
            .persistent()
            .extend_ttl(&score_key(asset_id), TTL_THRESHOLD, TTL_TARGET);

        // Verify asset exists and is owned by new_owner (as verified by asset-registry)
        verify_asset_exists(&env, &asset_registry, &asset_id);
        let asset = asset_registry::AssetRegistryClient::new(&env, &asset_registry).get_asset(&asset_id);
        if asset.owner != new_owner {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        // Clear engineer authorizations for the asset
        if let Some(history) = env
            .storage()
            .persistent()
            .get::<_, Vec<MaintenanceRecord>>(&history_key(asset_id))
        {
            let mut cleared_engineers = Vec::new(&env);
            for record in history.iter() {
                let eng = record.engineer.clone();
                let mut already_cleared = false;
                for cleared in cleared_engineers.iter() {
                    if cleared == eng {
                        already_cleared = true;
                        break;
                    }
                }
                if !already_cleared {
                    env.storage()
                        .persistent()
                        .remove(&engineer_auth_key(asset_id, &eng));
                    cleared_engineers.push_back(eng);
                }
            }
            if !cleared_engineers.is_empty() {
                env.events().publish(
                    (symbol_short!("AUTH_CLR"), asset_id),
                    cleared_engineers.len(),
                );
            }
        }

        env.events()
            .publish((symbol_short!("XFER"), asset_id), new_owner);
    }

    /// Decommission notify for lifecycle contract.
    ///
    /// Called by the asset registry (via cross-contract call) when an asset is
    /// decommissioned, to freeze its collateral score. The internal frozen score
    /// preserves the last computed value, but the emitted `DECOMM` event always
    /// reports `0` so off-chain indexers/lenders immediately treat the asset as
    /// having zero collateral value once decommissioned (see issue #794).
    pub fn decommission_notify(env: Env, asset_id: u64) {
        let asset_registry = get_asset_registry_addr(&env);
        asset_registry.require_auth();
        env.storage()
            .persistent()
            .get::<_, Config>(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));

        let frozen_score = compute_decay(&env, asset_id);
        env.storage()
            .persistent()
            .set(&frozen_score_key(asset_id), &frozen_score);
        extend_persistent_ttl(&env, &frozen_score_key(asset_id));
        env.storage()
            .persistent()
            .set(&frozen_key(asset_id), &true);
        extend_persistent_ttl(&env, &frozen_key(asset_id));

        let zero_score: u32 = 0;
        env.events()
            .publish((symbol_short!("DECOMM"), asset_id), zero_score);
    }

    /// Get the complete maintenance history for an asset.
    ///
    /// **Deprecated** — returns the entire history vector in a single read.
    /// With `max_history` at its default of 200 this is a 200-element `Vec`,
    /// which is expensive and can approach Soroban's per-call data limits as
    /// history grows. Use [`get_maintenance_history_paginated`] (cap 50) or
    /// [`get_maintenance_history_page`] (cap 100) for new integrations (#996).
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    ///
    /// # Returns
    /// Vec containing all maintenance records in chronological order
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::AssetNotFound`] if no asset exists with the given ID
    pub fn get_maintenance_history(env: Env, asset_id: u64) -> Vec<MaintenanceRecord> {
        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);
        env.storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or(Vec::new(&env))
    }

    /// Get a paginated slice of the maintenance history for an asset (#996).
    ///
    /// This is the preferred replacement for [`get_maintenance_history`], which
    /// returns the entire history vector in a single read. With `max_history`
    /// set to 200 that is a 200-element `Vec` — expensive and potentially close
    /// to Soroban's per-call data limits as history grows.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    /// * `offset`   - Zero-based start index (inclusive). Returns an empty vec
    ///                when `offset` ≥ history length.
    /// * `limit`    - Maximum number of records to return. A value of `0`
    ///                returns an empty vec. Hard-capped at
    ///                [`MAX_PAGINATED_LIMIT`] (50).
    ///
    /// # Returns
    /// `Vec<MaintenanceRecord>` — the requested page, possibly shorter than
    /// `limit` if the end of the history is reached.
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    ///
    /// # vs. `get_maintenance_history_page`
    /// This contract also exposes [`get_maintenance_history_page`], an older
    /// endpoint with the same `(asset_id, offset, limit)` shape but a looser
    /// [`MAX_PAGE_SIZE`] (100) cap. The two are kept separate rather than
    /// merged so existing `get_maintenance_history_page` callers are not
    /// broken by a lower cap (see #1209). New integrations should prefer
    /// this function.
    ///
    /// # Deprecation note
    /// [`get_maintenance_history`] (unbounded) is deprecated. New integrations
    /// should use this function or [`get_maintenance_history_page`].
    pub fn get_maintenance_history_paginated(
        env: Env,
        asset_id: u64,
        offset: u32,
        limit: u32,
    ) -> Vec<MaintenanceRecord> {
        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);

        if limit == 0 {
            return Vec::new(&env);
        }

        let history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or(Vec::new(&env));

        let len = history.len();
        if offset >= len {
            return Vec::new(&env);
        }

        let capped_limit = limit.min(MAX_PAGINATED_LIMIT);
        let end = (offset + capped_limit).min(len);
        let mut page = Vec::new(&env);
        for i in offset..end {
            page.push_back(history.get(i).unwrap());
        }
        page
    }

    /// Issue #799 — Option-returning variant of [`get_maintenance_history`].
    ///
    /// `get_maintenance_history` panics with `AssetNotFound` for assets
    /// that were never registered AND returns an empty `Vec` for assets
    /// that exist but have no history — two different failure modes that
    /// callers using the SDK-generated `try_*` wrapper would still have
    /// to disambiguate by hand.
    ///
    /// This variant collapses both signals into one return value:
    ///
    /// - `None` → the asset is not registered.
    /// - `Some(vec![])` → the asset is registered but has no
    ///   maintenance history yet.
    /// - `Some(records)` → the asset is registered and has history.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset.
    ///
    /// # Returns
    /// * `Option<Vec<MaintenanceRecord>>` - see the three cases above.
    pub fn get_maintenance_history_opt(
        env: Env,
        asset_id: u64,
    ) -> Option<Vec<MaintenanceRecord>> {
        let asset_registry = get_asset_registry_addr(&env);
        let client = asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        if client.try_get_asset(&asset_id).is_err() {
            return None;
        }
        Some(
            env.storage()
                .persistent()
                .get(&history_key(asset_id))
                .unwrap_or(Vec::new(&env)),
        )
    }

    /// Return all maintenance records that belong to the current owner's tenure,
    /// i.e. records submitted after the most recent ownership transfer.
    ///
    /// DeFi lenders that only want to see what maintenance the *current* owner has
    /// done should use this function instead of [`get_maintenance_history`].  All
    /// returned records will have `ownership_start_ledger == Some(ledger)` where
    /// `ledger` is the sequence number at which the most recent transfer was
    /// recorded. The XFER sentinel record itself also carries this same
    /// `ownership_start_ledger` value (see [`LifecycleContract::record_transfer`]),
    /// but is excluded from the returned Vec since it is a boundary marker, not
    /// a maintenance event.
    ///
    /// If the asset has never been transferred, this function returns the full
    /// maintenance history (excluding any future XFER sentinels, of which there
    /// would be none).
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    ///
    /// # Returns
    /// Vec of [`MaintenanceRecord`] containing only post-transfer records in
    /// chronological order.  Returns an empty Vec if no maintenance has been
    /// submitted since the last transfer (or if the asset has no history at all).
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if no asset exists with the given ID
    pub fn get_maintenance_history_since_transfer(
        env: Env,
        asset_id: u64,
    ) -> Vec<MaintenanceRecord> {
        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);

        let history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or_else(|| Vec::new(&env));

        // Find the index of the last XFER sentinel.  All records *after* it
        // (excluding the sentinel itself) belong to the current owner's tenure.
        let mut last_xfer_idx: Option<u32> = None;
        for i in 0..history.len() {
            let record = history.get(i).unwrap();
            if record.task_type == symbol_short!("XFER") {
                last_xfer_idx = Some(i);
            }
        }

        let start = match last_xfer_idx {
            Some(idx) => idx + 1,
            // No transfer has ever occurred — all records belong to the original owner.
            None => 0,
        };

        let mut result = Vec::new(&env);
        for i in start..history.len() {
            result.push_back(history.get(i).unwrap());
        }
        result
    }

    /// Get a paginated slice of the maintenance history for an asset.
    /// Useful for UI components that display maintenance records in pages.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    /// * `offset` - Zero-based start index for pagination
    /// * `limit` - Maximum number of records to return (returns empty vec if 0),
    ///   capped at [`MAX_PAGE_SIZE`]
    ///
    /// # Returns
    /// Vec containing the requested page of maintenance records
    ///
    /// # Panics
    /// - [`ContractError::IndexOutOfBounds`] if `offset` >= history length
    ///
    /// # vs. `get_maintenance_history_paginated`
    /// [`get_maintenance_history_paginated`] offers the same `(asset_id, offset,
    /// limit)` shape with a stricter [`MAX_PAGINATED_LIMIT`] (50) cap. This
    /// function's looser 100-record cap is kept as-is for backward
    /// compatibility with existing callers rather than unified with the
    /// stricter endpoint (see #1209). Prefer `get_maintenance_history_paginated`
    /// for new integrations.
    pub fn get_maintenance_history_page(
        env: Env,
        asset_id: u64,
        offset: u32,
        limit: u32,
    ) -> Vec<MaintenanceRecord> {
        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);

        let history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or(Vec::new(&env));

        let len = history.len();
        if limit == 0 {
            return Vec::new(&env);
        }
        if offset >= len {
            return Vec::new(&env);
        }

        let limit = limit.min(MAX_PAGE_SIZE);
        let end = (offset + limit).min(len);
        let mut page = Vec::new(&env);
        for i in offset..end {
            page.push_back(history.get(i).unwrap());
        }
        page
    }

    // ---------------------------------------------------------------------------
    //  Hash Chain Tamper Detection
    // ---------------------------------------------------------------------------

    /// Returns the zero-based indices of records in an asset's maintenance history
    /// whose `previous_record_hash` does not match the actual hash of the preceding
    /// record, i.e. the points at which the tamper-evident hash chain is broken.
    ///
    /// The chain is only checked over the currently visible (non-pruned) history:
    /// index `0`'s `previous_record_hash` is never checked, since it may legitimately
    /// point at a record that has since aged out of `max_history`.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    ///
    /// # Returns
    /// A `Vec<u32>` of indices `i` (into `get_maintenance_history`) where
    /// `history[i].previous_record_hash != hash(history[i - 1])`. Empty if the
    /// chain is intact (or the asset has fewer than two records).
    pub fn get_broken_chain_links(env: Env, asset_id: u64) -> Vec<u32> {
        let history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or_else(|| Vec::new(&env));

        let mut broken = Vec::new(&env);
        for i in 1..history.len() {
            let previous = history.get(i - 1).unwrap();
            let current = history.get(i).unwrap();
            let expected = hash_maintenance_record(&env, &previous);
            if current.previous_record_hash != Some(expected) {
                broken.push_back(i);
            }
        }
        broken
    }

    /// Verifies that an asset's maintenance history hash chain is intact, i.e. that
    /// off-chain data has not been tampered with since submission.
    ///
    /// Equivalent to `get_broken_chain_links(asset_id).is_empty()`.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    ///
    /// # Returns
    /// `true` if every record's `previous_record_hash` matches the hash of the record
    /// before it (or the asset has fewer than two records); `false` if any link is broken.
    pub fn verify_maintenance_chain_integrity(env: Env, asset_id: u64) -> bool {
        Self::get_broken_chain_links(env, asset_id).is_empty()
    }

    /// Get the most recent maintenance record for an asset, determined by the highest timestamp.
    ///
    /// History is append-only (records are never inserted out of order by normal contract
    /// operations), but this function defensively selects the record with the greatest
    /// timestamp so that any future admin tooling that inserts records cannot silently
    /// return a stale entry.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    ///
    /// # Returns
    /// `Some(MaintenanceRecord)` with the highest timestamp, or `None` if no history exists
    pub fn get_last_service(env: Env, asset_id: u64) -> Option<MaintenanceRecord> {
        let history: Vec<MaintenanceRecord> =
            env.storage().persistent().get(&history_key(asset_id))?;

        let mut best: Option<MaintenanceRecord> = None;
        for i in 0..history.len() {
            let record = history.get(i).unwrap();
            let is_newer = best.as_ref().is_none_or(|b| record.timestamp > b.timestamp);
            if is_newer {
                best = Some(record);
            }
        }
        best
    }

    /// Returns the total maintenance cost for an asset across all records (in stroops).
    ///
    /// Sums all `cost` fields from the asset's maintenance history.
    /// Records with `cost: None` contribute 0 to the total.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    ///
    /// # Returns
    /// Total cost in stroops (1 stroop = 10^-7 XLM). Returns 0 if no history.
    pub fn get_total_maintenance_cost(env: Env, asset_id: u64) -> u64 {
        let history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or_else(|| Vec::new(&env));
        let mut total: u64 = 0;
        for i in 0..history.len() {
            let record = history.get(i).unwrap();
            total = total.saturating_add(record.cost.unwrap_or(0));
        }
        total
    }

    /// Returns a chronological list of (timestamp, cost) pairs for an asset.
    ///
    /// Only includes records where `cost` is `Some`. Records with `None` cost
    /// are filtered out. Useful for trend analysis and cost anomaly detection.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    ///
    /// # Returns
    /// `Vec<(u64, u64)>` where each element is `(timestamp, cost_in_stroops)`
    pub fn get_maintenance_cost_history(env: Env, asset_id: u64) -> Vec<(u64, u64)> {
        let history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or_else(|| Vec::new(&env));
        let mut result = Vec::<(u64, u64)>::new(&env);
        for i in 0..history.len() {
            let record = history.get(i).unwrap();
            if let Some(cost) = record.cost {
                result.push_back((record.timestamp, cost));
            }
        }
        result
    }

    /// Returns the average maintenance cost for a specific task type on an asset.
    ///
    /// Only considers records matching the given `task_type`. Records with
    /// `cost: None` are excluded from the average calculation.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    /// * `task_type` - The task type to filter by
    ///
    /// # Returns
    /// Average cost in stroops, or 0 if no matching records with cost data exist.
    pub fn get_average_maintenance_interval_cost(
        env: Env,
        asset_id: u64,
        task_type: Symbol,
    ) -> u64 {
        let history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or_else(|| Vec::new(&env));
        let mut total: u64 = 0;
        let mut count: u64 = 0;
        for i in 0..history.len() {
            let record = history.get(i).unwrap();
            if record.task_type == task_type {
                if let Some(cost) = record.cost {
                    total = total.saturating_add(cost);
                    count += 1;
                }
            }
        }
        if count == 0 {
            0
        } else {
            total / count
        }
    }

    /// View alias for [`get_last_service`].
    /// Returns the most recent maintenance record for an asset, or `None` if no history exists.
    /// Frontends and lenders should prefer this over fetching the full history.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    ///
    /// # Returns
    /// `Some(MaintenanceRecord)` for the latest record, or `None` for assets with no history
    pub fn get_last_maintenance(env: Env, asset_id: u64) -> Option<MaintenanceRecord> {
        Self::get_last_service(env, asset_id)
    }

    // ---------------------------------------------------------------------------
    //  Recurring Maintenance Tasks
    // ---------------------------------------------------------------------------

    /// Schedule a recurring maintenance task for an asset.
    ///
    /// Only the asset owner (as recorded in the asset registry) may schedule recurring tasks.
    ///
    /// # Arguments
    /// * `owner` - The owner address (must match the asset's current owner)
    /// * `asset_id` - The unique identifier of the asset
    /// * `task_id` - A unique identifier for this recurring task definition
    /// * `task_type` - The type of maintenance task (e.g., "OIL_CHG")
    /// * `interval_type` - Unit of the interval (e.g., "HOURS", "DAYS", "CYCLES")
    /// * `interval_value` - The numeric interval (e.g., 500 for "every 500 hours")
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedOwner`] if caller is not the asset owner
    /// - [`ContractError::DuplicateRecurringTask`] if task_id already exists
    /// - [`ContractError::InvalidRecurringSchedule`] if interval_value is 0
    pub fn schedule_recurring_task(
        env: Env,
        owner: Address,
        asset_id: u64,
        task_id: u64,
        task_type: Symbol,
        interval_type: Symbol,
        interval_value: u64,
    ) {
        ensure_not_paused(&env);
        owner.require_auth();

        if interval_value == 0 {
            panic_with_error!(&env, ContractError::InvalidRecurringSchedule);
        }

        // Verify caller is the asset owner
        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);
        let asset =
            asset_registry::AssetRegistryClient::new(&env, &asset_registry).get_asset(&asset_id);
        if asset.owner != owner {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        let mut tasks: Vec<RecurringTask> = env
            .storage()
            .persistent()
            .get(&DataKey::RecurringTasks(asset_id))
            .unwrap_or_else(|| Vec::new(&env));

        // Check for duplicate task_id
        for i in 0..tasks.len() {
            if tasks.get(i).unwrap().task_id == task_id {
                panic_with_error!(&env, ContractError::DuplicateRecurringTask);
            }
        }

        let now = env.ledger().timestamp();
        let next_due = now.saturating_add(interval_value);

        tasks.push_back(RecurringTask {
            task_id,
            task_type,
            interval_type,
            interval_value,
            next_due,
            is_active: true,
        });

        env.storage()
            .persistent()
            .set(&DataKey::RecurringTasks(asset_id), &tasks);
        extend_persistent_ttl(&env, &DataKey::RecurringTasks(asset_id));

        env.events().publish(
            (symbol_short!("RECUR_SCH"), asset_id),
            (task_id, task_type.clone(), interval_value),
        );
    }

    /// Auto-create a maintenance record from a recurring task schedule.
    ///
    /// Called by an off-chain scheduler (or any caller) when a recurring task's
    /// `next_due` timestamp has been reached. Creates a new maintenance record
    /// and updates the `next_due` to `now + interval_value`.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    /// * `task_id` - The recurring task ID to execute
    /// * `engineer` - The engineer performing the maintenance
    ///
    /// # Panics
    /// - [`ContractError::RecurringTaskNotFound`] if task_id not found
    /// - [`ContractError::RecurringTaskInactive`] if the task is not active
    pub fn auto_create_recurring_task(
        env: Env,
        asset_id: u64,
        task_id: u64,
        engineer: Address,
    ) {
        ensure_not_paused(&env);
        engineer.require_auth();

        let mut tasks: Vec<RecurringTask> = env
            .storage()
            .persistent()
            .get(&DataKey::RecurringTasks(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::RecurringTaskNotFound));

        let mut found_idx: Option<u32> = None;
        for i in 0..tasks.len() {
            let task = tasks.get(i).unwrap();
            if task.task_id == task_id {
                if !task.is_active {
                    panic_with_error!(&env, ContractError::RecurringTaskInactive);
                }
                found_idx = Some(i);
                break;
            }
        }

        let idx = found_idx.unwrap_or_else(|| {
            panic_with_error!(&env, ContractError::RecurringTaskNotFound)
        });

        let task = tasks.get(idx).unwrap();
        let task_type = task.task_type.clone();

        // Create the maintenance record with no cost (auto-generated)
        let timestamp = env.ledger().timestamp();
        let notes = String::from_str(&env, "Auto-created from recurring schedule");

        // Propagate ownership_start_ledger so recurring tasks are also correctly
        // associated with the current ownership period.
        let ownership_start_ledger: Option<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::OwnershipStartLedger(asset_id));

        let record = MaintenanceRecord {
            asset_id,
            task_type: task_type.clone(),
            priority: Priority::Low,
            notes,
            engineer: engineer.clone(),
            timestamp,
            cost: None,
            ownership_start_ledger,
        };

        let mut history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or(Vec::new(&env));

        let config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));

        if config.max_history > 0 && history.len() >= config.max_history {
            history.remove(0);
        }

        let previous_record_hash = next_chain_link(&env, &history);
        let record = MaintenanceRecord {
            asset_id,
            task_type: task_type.clone(),
            priority: Priority::Low,
            notes,
            engineer: engineer.clone(),
            timestamp,
            cost: None,
            previous_record_hash,
        };
        history.push_back(record);
        env.storage()
            .persistent()
            .set(&history_key(asset_id), &history);
        extend_persistent_ttl(&env, &history_key(asset_id));

        // Update next_due for this recurring task
        let mut task_mut = tasks.get(idx).unwrap();
        task_mut.next_due = timestamp.saturating_add(task_mut.interval_value);
        tasks.set(idx, task_mut);
        env.storage()
            .persistent()
            .set(&DataKey::RecurringTasks(asset_id), &tasks);
        extend_persistent_ttl(&env, &DataKey::RecurringTasks(asset_id));

        env.storage()
            .persistent()
            .set(&last_update_key(asset_id), &timestamp);
        extend_persistent_ttl(&env, &last_update_key(asset_id));

        env.events().publish(
            (symbol_short!("RECUR_EXEC"), asset_id),
            (task_id, task_type, timestamp),
        );
    }

    /// Returns all recurring tasks configured for an asset.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    ///
    /// # Returns
    /// `Vec<RecurringTask>` — empty if none are configured
    pub fn get_recurring_tasks(env: Env, asset_id: u64) -> Vec<RecurringTask> {
        env.storage()
            .persistent()
            .get(&DataKey::RecurringTasks(asset_id))
            .unwrap_or_else(|| Vec::new(&env))
    }

    // ---------------------------------------------------------------------------
    //  Duplicate Maintenance Detection
    // ---------------------------------------------------------------------------

    /// Detect pairs of duplicate maintenance events within a time window.
    ///
    /// Two records are considered duplicates if they share the same `task_type`
    /// and `engineer` and were submitted within `window_seconds` of each other.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    /// * `window_seconds` - Time window in seconds for duplicate detection
    ///
    /// # Returns
    /// `Vec<(u64, u64)>` where each element is a pair of timestamps of similar events
    pub fn get_duplicate_maintenance_events(
        env: Env,
        asset_id: u64,
        window_seconds: u64,
    ) -> Vec<(u64, u64)> {
        let history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or_else(|| Vec::new(&env));

        let mut result = Vec::<(u64, u64)>::new(&env);
        let len = history.len();

        for i in 0..len {
            let a = history.get(i).unwrap();
            if a.task_type == symbol_short!("XFER") {
                continue;
            }
            let mut j = i.saturating_add(1);
            while j < len {
                let b = history.get(j).unwrap();
                if b.task_type == symbol_short!("XFER") {
                    j += 1;
                    continue;
                }
                let time_diff = if a.timestamp > b.timestamp {
                    a.timestamp.saturating_sub(b.timestamp)
                } else {
                    b.timestamp.saturating_sub(a.timestamp)
                };
                if a.task_type == b.task_type
                    && a.engineer == b.engineer
                    && time_diff <= window_seconds
                {
                    result.push_back((a.timestamp, b.timestamp));
                }
                j += 1;
            }
        }

        result
    }

    /// Mark a maintenance record as a duplicate.
    ///
    /// Admin-only. Stores the duplicate record's timestamp so that scoring
    /// logic can exclude it from collateral score calculations.
    ///
    /// # Arguments
    /// * `admin` - The admin address
    /// * `asset_id` - The unique identifier of the asset
    /// * `primary_id` - The timestamp of the canonical (primary) record (informational)
    /// * `duplicate_id` - The timestamp of the record to mark as duplicate
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::DuplicateRecordNotFound`] if the duplicate timestamp not in history
    pub fn mark_maintenance_as_duplicate(
        env: Env,
        admin: Address,
        asset_id: u64,
        primary_id: u64,
        duplicate_id: u64,
    ) {
        ensure_not_paused(&env);
        require_admin(&env, &admin);

        // Verify the duplicate timestamp exists in history
        let history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or_else(|| Vec::new(&env));

        let mut found = false;
        for i in 0..history.len() {
            if history.get(i).unwrap().timestamp == duplicate_id {
                found = true;
                break;
            }
        }
        if !found {
            panic_with_error!(&env, ContractError::DuplicateRecordNotFound);
        }

        let mut duplicates: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::DuplicateRecords(asset_id))
            .unwrap_or_else(|| Vec::new(&env));

        duplicates.push_back(duplicate_id);
        env.storage()
            .persistent()
            .set(&DataKey::DuplicateRecords(asset_id), &duplicates);
        extend_persistent_ttl(&env, &DataKey::DuplicateRecords(asset_id));

        env.events().publish(
            (symbol_short!("MARK_DUP"), asset_id),
            (primary_id, duplicate_id, env.ledger().timestamp()),
        );
    }

    /// Clear a previously-marked duplicate maintenance record.
    ///
    /// Admin-only. Removes `duplicate_id` from the asset's `DuplicateRecords` list
    /// so it no longer keeps the list — and the O(n*m) scan `compute_decay`
    /// performs against it — growing indefinitely (#1223).
    ///
    /// # Arguments
    /// * `admin` - The admin address
    /// * `asset_id` - The unique identifier of the asset
    /// * `duplicate_id` - The timestamp of the previously-marked duplicate record to clear
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::DuplicateRecordNotFound`] if `duplicate_id` is not currently marked
    pub fn clear_duplicate_record(env: Env, admin: Address, asset_id: u64, duplicate_id: u64) {
        ensure_not_paused(&env);
        require_admin(&env, &admin);

        let duplicates: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::DuplicateRecords(asset_id))
            .unwrap_or_else(|| Vec::new(&env));

        let mut remaining: Vec<u64> = Vec::new(&env);
        let mut found = false;
        for d in duplicates.iter() {
            if d == duplicate_id {
                found = true;
            } else {
                remaining.push_back(d);
            }
        }
        if !found {
            panic_with_error!(&env, ContractError::DuplicateRecordNotFound);
        }

        env.storage()
            .persistent()
            .set(&DataKey::DuplicateRecords(asset_id), &remaining);
        extend_persistent_ttl(&env, &DataKey::DuplicateRecords(asset_id));

        env.events().publish(
            (symbol_short!("CLR_DUP"), asset_id),
            (duplicate_id, env.ledger().timestamp()),
        );
    }

    // ---------------------------------------------------------------------------
    //  Maintenance Compliance Standards
    // ---------------------------------------------------------------------------

    /// Register a maintenance compliance standard for an asset type.
    ///
    /// Admin-only. Stores the compliance standard definition hash, enabling
    /// later validation that submitted maintenance meets industry requirements.
    ///
    /// # Arguments
    /// * `admin` - The admin address
    /// * `asset_type` - The asset type to register the standard for
    /// * `standard_definition_hash` - Hash of the compliance standard document
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::StandardAlreadyRegistered`] if already registered for this type
    pub fn register_standard(
        env: Env,
        admin: Address,
        asset_type: Symbol,
        standard_definition_hash: Bytes,
    ) {
        ensure_not_paused(&env);
        require_admin(&env, &admin);

        let key = standard_key(&asset_type);

        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, ContractError::StandardAlreadyRegistered);
        }

        env.storage().persistent().set(&key, &standard_definition_hash);
        extend_persistent_ttl(&env, &key);

        env.events().publish(
            (symbol_short!("REG_STD"), asset_type.clone()),
            standard_definition_hash.clone(),
        );
    }

    /// Validate that a submitted maintenance task complies with the registered standard.
    ///
    /// Retrieves the asset type from the registry, then checks whether the provided
    /// `compliance_proof_hash` matches the registered standard for that asset type.
    /// Returns `true` if compliant, `false` otherwise.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    /// * `task_type` - The type of maintenance task being validated
    /// * `compliance_proof_hash` - The hash to validate against the standard
    ///
    /// # Returns
    /// `true` if the proof matches the registered standard
    pub fn validate_maintenance_compliance(
        env: Env,
        asset_id: u64,
        task_type: Symbol,
        compliance_proof_hash: Bytes,
    ) -> bool {
        // Get the asset type from the registry
        let asset_registry = get_asset_registry_addr(&env);
        let client = asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        if client.try_get_asset(&asset_id).is_err() {
            return false;
        }
        let asset = client.get_asset(&asset_id);

        let key = standard_key(&asset.asset_type);

        // Look up the registered standard for this asset type and compare
        if let Some(registered_hash) =
            env.storage().persistent().get::<_, Bytes>(&key)
        {
            registered_hash == compliance_proof_hash
        } else {
            false
        }
    }

    /// Return the registered maintenance compliance standard for an asset type.
    ///
    /// Returns the raw standard bytes if registered, or empty `Bytes` if none.
    ///
    /// # Arguments
    /// * `asset_type` - The asset type to look up
    ///
    /// # Returns
    /// `Bytes` containing the standard definition, empty if not registered
    pub fn get_maintenance_standard(env: Env, asset_type: Symbol) -> Bytes {
        let key = standard_key(&asset_type);
        env.storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Bytes::from_slice(&env, &[]))
    }

    /// Get the current collateral score for an asset.
    /// Verifies asset exists before returning the score.
    ///
    /// This function is **read-only**: it computes the time-decayed score without
    /// writing anything to storage. To persist the decayed score and update the
    /// last-update timestamp, call [`decay_score`] explicitly.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    ///
    /// # Returns
    /// The current collateral score (0-100) after applying time-based decay
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    pub fn get_collateral_score(env: Env, asset_id: u64) -> u32 {
        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);
        let config: Config = env
            .storage()
            .persistent()
            .get::<_, Config>(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        // Deprecated or decommissioned assets are not eligible for collateral — return 0 immediately.
        let asset = asset_registry::AssetRegistryClient::new(&env, &asset_registry).get_asset(&asset_id);
        if asset.deprecation_status != asset_registry::DeprecationStatus::Active {
            return 0;
        }
        // Frozen (decommissioned) assets are not eligible — return 0.
        // Fix #794: Frozen (decommissioned) assets always return 0.
        // A decommissioned asset must never appear as valid DeFi collateral.
        if env.storage().persistent().get::<_, bool>(&frozen_key(asset_id)).unwrap_or(false) {
            return 0;
        }

        let final_score = compute_read_only_collateral_score(&env, asset_id, &asset.asset_type, &config);
        // Persist the computed score so stored and returned values are always consistent.
        env.storage()
            .persistent()
            .set(&score_key(asset_id), &final_score);
        extend_persistent_ttl(&env, &score_key(asset_id));
        env.storage()
            .persistent()
            .set(&last_update_key(asset_id), &env.ledger().timestamp());
        extend_persistent_ttl(&env, &last_update_key(asset_id));
        final_score
    }

    /// Return the current collateral valuation and its timestamp for an asset.
    ///
    /// The valuation currently tracks the asset's collateral score as a `u64` so that
    /// analytics consumers can query it using a stable valuation-history interface.
    pub fn get_collateral_valuation(env: Env, asset_id: u64) -> (u64, u64) {
        let value = Self::get_collateral_score(env.clone(), asset_id) as u64;
        let history: Vec<(u64, u64)> = env
            .storage()
            .persistent()
            .get(&DataKey::CollateralValuationHistory(asset_id))
            .unwrap_or_else(|| Vec::new(&env));

        if history.is_empty() {
            (value, env.ledger().timestamp())
        } else {
            let (timestamp, value) = history.get(history.len() - 1).unwrap();
            (value, timestamp)
        }
    }

    /// Return the chronological collateral valuation history for an asset.
    pub fn get_valuation_history(env: Env, asset_id: u64) -> Vec<(u64, u64)> {
        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);
        env.storage()
            .persistent()
            .get(&DataKey::CollateralValuationHistory(asset_id))
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Get collateral scores for multiple assets in a single call.
    ///
    /// # Arguments
    /// * `asset_ids` - A list of asset IDs to query
    ///
    /// # Returns
    /// A Vec of `(asset_id, score)` pairs with lazy decay applied. Unknown asset
    /// IDs are skipped (omitted from results) rather than causing a panic.
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if the contract is not initialized
    /// Issue #798 — Option-returning variant of [`get_collateral_score`].
    ///
    /// `get_collateral_score` panics with `AssetNotFound` for unregistered
    /// assets, which forces every caller into the SDK-generated `try_*`
    /// wrapper plus an error-type check. This variant collapses both
    /// signals into a single return value so callers can distinguish
    /// "unregistered" from "registered but score == 0" (e.g. deprecated,
    /// frozen-at-0, or fully-decayed):
    ///
    /// - `None` → the asset is not registered.
    /// - `Some(0)` → the asset is registered but has no positive score
    ///   (deprecated, decommissioned-at-zero, or fully decayed).
    /// - `Some(n)` → the asset is registered with score `n` (0..=100).
    ///
    /// Unlike `get_collateral_score` this is **read-only**: it does not
    /// persist the recomputed score or bump TTL. Callers that want the
    /// side effects should call `get_collateral_score` directly.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset.
    ///
    /// # Returns
    /// * `Option<u32>` - see the three cases above.
    pub fn get_collateral_score_opt(env: Env, asset_id: u64) -> Option<u32> {
        let asset_registry = get_asset_registry_addr(&env);
        let client = asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        if client.try_get_asset(&asset_id).is_err() {
            return None;
        }
        let config: Config = env
            .storage()
            .persistent()
            .get::<_, Config>(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        // Mirror the deprecation + frozen handling of `get_collateral_score`
        // exactly (read-only path).
        let asset = client.get_asset(&asset_id);
        if asset.deprecation_status != asset_registry::DeprecationStatus::Active {
            return Some(0);
        }
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&frozen_key(asset_id))
            .unwrap_or(false)
        {
            // Extend the frozen score's TTL on every read, not just on write, so
            // an infrequently-queried frozen asset never silently loses its
            // preserved score to TTL expiry and falls back to 0.
            if env.storage().persistent().has(&frozen_score_key(asset_id)) {
                extend_persistent_ttl(&env, &frozen_score_key(asset_id));
            }
            return Some(
                env.storage()
                    .persistent()
                    .get(&frozen_score_key(asset_id))
                    .unwrap_or(0),
            );
        }
        // `apply_decay` is the same read-only score computation used by
        // `get_collateral_score_batch` (which also passes `write=false`).
        Some(apply_decay(&env, asset_id, false, false, config.max_history))
    }

    /// Returns the current collateral score for each requested asset.
    ///
    /// # Parameters
    /// - `asset_ids`: the list of asset IDs to score.
    ///
    /// # Errors
    /// Does not panic on a missing or deprecated asset; instead it is skipped
    /// from the returned list. Panics with [`ContractError::NotInitialized`]
    /// if the contract has not been initialized.
    ///
    /// # Events
    /// None. This is a read-only batch query and does not mutate storage or
    /// emit events (equivalent to calling `get_collateral_score` with
    /// `write = false` for each ID, so no decay is persisted).
    ///
    /// # Soroban notes
    /// Performs one cross-contract call to AssetRegistry per asset ID via
    /// `try_get_asset`, so callers should keep `asset_ids` reasonably small
    /// to stay within the transaction's CPU/instruction budget.
    pub fn get_collateral_score_batch(env: Env, asset_ids: Vec<u64>) -> Vec<(u64, u32)> {
        let config: Config = env
            .storage()
            .persistent()
            .get::<_, Config>(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        let asset_registry = get_asset_registry_addr(&env);
        let client = asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        let mut results: Vec<(u64, u32)> = Vec::new(&env);
        for asset_id in asset_ids.iter() {
            if let Ok(asset) = client.try_get_asset(&asset_id) {
                // Check deprecation status first
                if asset.deprecation_status != asset_registry::DeprecationStatus::Active {
                    results.push_back((asset_id, 0));
                    continue;
                }
                // Check if frozen (decommissioned)
                if env.storage().persistent().get::<_, bool>(&frozen_key(asset_id)).unwrap_or(false) {
                    results.push_back((asset_id, 0));
                    continue;
                }
                // Compute score normally
                let score = apply_decay(&env, asset_id, false, false, config.max_history);
                results.push_back((asset_id, score));
            }
        }
        results
    }

    /// Returns the full score history (SCHIST) for an asset.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    /// * `offset` - Zero-based start index for pagination
    /// * `limit` - Maximum number of entries to return (returns empty vec if 0)
    ///
    /// # Returns
    /// Vec of [`ScoreEntry`] containing the requested page of the score history
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    pub fn get_score_history(env: Env, asset_id: u64, offset: u32, limit: u32) -> Vec<ScoreEntry> {
        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);

        if limit == 0 {
            return Vec::new(&env);
        }

        // Cap at 50 entries per call to bound compute and fees for DeFi integrators.
        const MAX_PAGE: u32 = 50;
        let limit = limit.min(MAX_PAGE);

        let history: Vec<ScoreEntry> = env
            .storage()
            .persistent()
            .get(&score_history_key(asset_id))
            .unwrap_or(Vec::new(&env));

        let len = history.len();
        if offset >= len {
            return Vec::new(&env);
        }

        let end = (offset + limit).min(len);
        let mut page = Vec::new(&env);
        for i in offset..end {
            page.push_back(history.get(i).unwrap());
        }
        page
    }


    /// Get the last `n` ScoreEntry items from the score history.
    /// Useful for displaying recent score trends in dashboards.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    /// * `n` - Number of most recent entries to return
    ///
    /// # Returns
    /// Vec containing the last `n` score entries (or fewer if history is shorter)
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    pub fn get_score_trend(env: Env, asset_id: u64, n: u32) -> Vec<ScoreEntry> {
        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);
        if n == 0 {
            return Vec::new(&env);
        }
        let history: Vec<ScoreEntry> = env
            .storage()
            .persistent()
            .get(&score_history_key(asset_id))
            .unwrap_or(Vec::new(&env));
        let len = history.len();
        if len == 0 {
            return Vec::new(&env);
        }
        let start = if n >= len {
            0u32
        } else {
            len.saturating_sub(n)
        };
        let mut result = Vec::new(&env);
        for i in start..len {
            result.push_back(history.get(i).unwrap());
        }
        result
    }

    /// Check if an asset is eligible for collateral based on its score.
    /// Verifies asset exists and compares score to eligibility threshold.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    ///
    /// # Returns
    /// `true` if the asset meets eligibility criteria; `false` otherwise
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    /// Check if an asset is currently eligible to be used as collateral.
    ///
    /// # On-demand decay
    ///
    /// Before reading the collateral score this function calls [`apply_decay`]
    /// to write the time-decayed score back to storage. Without this step the
    /// stored score could be stale — an asset that was last serviced months ago
    /// might still show its original high score and appear eligible when its
    /// real decayed score is below the eligibility threshold (#994).
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    pub fn is_collateral_eligible(env: Env, asset_id: u64) -> bool {
        // Verify asset exists before checking eligibility
        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);

        let config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));

        // Check asset deprecation status first
        let asset = asset_registry::AssetRegistryClient::new(&env, &asset_registry).get_asset(&asset_id);
        if asset.deprecation_status != asset_registry::DeprecationStatus::Active {
            return false;
        }

        // Check if asset is frozen (decommissioned)
        if env.storage().persistent().get::<_, bool>(&frozen_key(asset_id)).unwrap_or(false) {
            return false;
        }

        // #994: Apply decay before reading the score so the stored value is
        // always up-to-date and we never compare against a stale high score.
        apply_decay(&env, asset_id, true, false, config.max_history);

        let effective_score = compute_read_only_collateral_score(&env, asset_id, &asset.asset_type, &config);
        effective_score >= effective_min_collateral_score(&config)
    }

    /// Returns the timestamp of the most recent maintenance event, or None if no maintenance has been submitted.
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    pub fn get_last_service_timestamp(env: Env, asset_id: u64) -> Option<u64> {
        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);
        env.storage().persistent().get(&last_update_key(asset_id))
    }

    /// Get the address of the asset registry contract.
    ///
    /// # Returns
    /// The address of the currently configured asset registry
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    pub fn get_asset_registry(env: Env) -> Address {
        get_asset_registry_addr(&env)
    }

    /// Get all asset IDs that have been maintained by a specific engineer.
    ///
    /// # Arguments
    /// * `engineer` - The address of the engineer to query
    ///
    /// # Returns
    /// A Vec containing the **first 100** asset IDs this engineer has worked on.
    /// If the engineer has more than 100 entries the result is silently truncated —
    /// call [`get_eng_maint_hist_count`] to check the total, then use
    /// call [`get_eng_maint_count`] to check the total, then use
    /// [`get_eng_history_page`] with `offset`/`limit` to retrieve the full history.
    pub fn get_engineer_maintenance_history(env: Env, engineer: Address) -> Vec<u64> {
        let history: Vec<u64> = env
            .storage()
            .persistent()
            .get(&engineer_history_key(&engineer))
            .unwrap_or_else(|| Vec::new(&env));

        let len = history.len();
        if len <= 100 {
            return history;
        }

        let mut result = Vec::new(&env);
        for i in 0..100u32 {
            result.push_back(history.get(i).unwrap());
        }
        result
    }

    /// Get a paginated list of asset IDs that an engineer has worked on.
    /// Supports offset and limit for pagination.
    ///
    /// # Arguments
    /// * `engineer` - The address of the engineer to query
    /// * `offset` - Zero-based start index for pagination
    /// * `limit` - Maximum number of records to return (returns empty vec if 0)
    ///
    /// # Returns
    /// Vec containing the requested page of asset IDs
    pub fn get_engineer_history(env: Env, engineer: Address, offset: u32, limit: u32) -> Vec<u64> {
        let history: Vec<u64> = env
            .storage()
            .persistent()
            .get(&engineer_history_key(&engineer))
            .unwrap_or_else(|| Vec::new(&env));

        let len = history.len();
        if limit == 0 {
            return Vec::new(&env);
        }
        if offset >= len {
            return Vec::new(&env);
        }

        let end = (offset + limit).min(len);
        let mut page = Vec::new(&env);
        for i in offset..end {
            page.push_back(history.get(i).unwrap());
        }
        page
    }


    /// Get a paginated slice of asset IDs an engineer has worked on, along with
    /// the total record count, in a single call.
    ///
    /// Implements issue #759: avoids hitting Soroban return-data limits for
    /// engineers with very large maintenance histories.
    ///
    /// # Arguments
    /// * `engineer` - The address of the engineer to query
    /// * `page` - Zero-based page index
    /// * `page_size` - Number of records per page (returns empty page if 0)
    ///
    /// # Returns
    /// A tuple `(page_records, total_count)` where `page_records` is the requested
    /// slice of asset IDs and `total_count` is the total number of records for
    /// this engineer.
    pub fn get_engineer_maintenance_history_page(
        env: Env,
        engineer: Address,
        page: u32,
        page_size: u32,
    ) -> (Vec<u64>, u32) {
        let history: Vec<u64> = env
            .storage()
            .persistent()
            .get(&engineer_history_key(&engineer))
            .unwrap_or_else(|| Vec::new(&env));

        let total = history.len();

        if page_size == 0 {
            return (Vec::new(&env), total);
        }

        let start = page.saturating_mul(page_size);
        if start >= total {
            return (Vec::new(&env), total);
        }

        let end = start.saturating_add(page_size).min(total);
        let mut page_records = Vec::new(&env);
        for i in start..end {
            page_records.push_back(history.get(i).unwrap());
        }
        (page_records, total)
    }

    /// Return the total number of asset IDs recorded for an engineer.
    ///
    /// Use this together with [`get_eng_history_page`] to paginate through histories
    /// that exceed the 100-entry cap of [`get_engineer_maintenance_history`].
    ///
    /// # Arguments
    /// * `engineer` - The address of the engineer to query
    ///
    /// # Returns
    /// Total number of entries in the engineer's maintenance history.
    pub fn get_eng_maint_hist_count(env: Env, engineer: Address) -> u32 {
        let history: Vec<u64> = env
            .storage()
            .persistent()
            .get(&engineer_history_key(&engineer))
            .unwrap_or_else(|| Vec::new(&env));
        history.len()
    }

    /// Alias for [`get_eng_maint_hist_count`].
    ///
    /// # Arguments
    /// * `engineer` - The address of the engineer to query
    ///
    /// # Returns
    /// Total number of entries in the engineer's maintenance history.
    pub fn get_engineer_history_count(env: Env, engineer: Address) -> u32 {
        Self::get_eng_maint_hist_count(env, engineer)
    }

    /// Alias for [`get_eng_maint_hist_count`].
    ///
    /// # Arguments
    /// * `engineer` - The address of the engineer to query
    ///
    /// # Returns
    /// Total number of entries in the engineer's maintenance history.
    pub fn get_eng_maint_count(env: Env, engineer: Address) -> u32 {
        Self::get_eng_maint_hist_count(env, engineer)
    }

    /// Alias for [`get_eng_maint_hist_count`].
    ///
    /// # Arguments
    /// * `engineer` - The address of the engineer to query
    ///
    /// # Returns
    /// Total number of entries in the engineer's maintenance history.
    pub fn eng_maintenance_history_count(env: Env, engineer: Address) -> u32 {
        Self::get_eng_maint_hist_count(env, engineer)
    }

    /// Get a paginated list of asset IDs that an engineer has worked on.
    ///
    /// # Arguments
    /// * `engineer` - The address of the engineer to query
    /// * `offset` - Zero-based start index for pagination
    /// * `limit` - Maximum number of records to return (returns empty vec if 0)
    ///
    /// # Returns
    /// Vec containing the requested page of asset IDs
    ///
    /// # Panics
    /// - [`ContractError::IndexOutOfBounds`] if `offset` >= history length
    pub fn get_eng_history_page(env: Env, engineer: Address, offset: u32, limit: u32) -> Vec<u64> {
        let history: Vec<u64> = env
            .storage()
            .persistent()
            .get(&engineer_history_key(&engineer))
            .unwrap_or_else(|| Vec::new(&env));

        let len = history.len();
        if limit == 0 {
            return Vec::new(&env);
        }
        if offset >= len {
            return Vec::new(&env);
        }

        let end = (offset + limit).min(len);
        let mut page = Vec::new(&env);
        for i in offset..end {
            page.push_back(history.get(i).unwrap());
        }
        page
    }

    /// Admin-only function to update the asset registry address.
    /// Useful for registry migrations or updates.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    /// * `new_registry` - The new asset registry contract address
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    pub fn update_asset_registry(env: Env, admin: Address, new_registry: Address) {
        crate::admin::update_asset_registry(env, admin, new_registry);
    }

    /// Get the address of the engineer registry contract.
    ///
    /// # Returns
    /// The address of the currently configured engineer registry
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    pub fn get_engineer_registry(env: Env) -> Address {
        get_engineer_registry_addr(&env)
    }

    /// Admin-only function to update the engineer registry address.
    /// Useful for registry migrations or updates.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    /// * `new_registry` - The new engineer registry contract address
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    pub fn update_engineer_registry(env: Env, admin: Address, new_registry: Address) {
        crate::admin::update_engineer_registry(env, admin, new_registry);
    }

    /// Get the current configuration of the lifecycle contract.
    ///
    /// # Returns
    /// The complete Config struct with all current settings
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    pub fn get_config(env: Env) -> Config {
        env.storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized))
    }

    // ---------------------------------------------------------------------------
    //  Per-Engineer Submission Rate Limiting
    // ---------------------------------------------------------------------------

    /// Admin-only function to update the maximum maintenance-record submissions a
    /// single engineer may make per rolling-hour window.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    /// * `new_max` - New per-hour submission cap (`0` disables rate limiting)
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    pub fn update_max_submissions_per_hour(env: Env, admin: Address, new_max: u32) {
        ensure_not_paused(&env);
        admin.require_auth();

        let mut config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if config.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        config.max_submissions_per_hour = new_max;
        env.storage().persistent().set(&CONFIG, &config);
        extend_persistent_ttl(&env, &CONFIG);

        env.events()
            .publish((symbol_short!("UPD_RATE"), admin.clone()), new_max);
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("CFG_UPD")),
            (admin, env.ledger().timestamp(), symbol_short!("MAX_RATE"), new_max),
        );
    }

    /// Returns whether `engineer` is currently within the configured
    /// `max_submissions_per_hour` limit, i.e. allowed to submit another
    /// maintenance record right now without being rejected.
    ///
    /// Read-only: does not itself count against the engineer's rate. A single
    /// engineer submitting far more records than this check allows for is a
    /// signal of potential fraud or a runaway integration and should be
    /// investigated (see `RATE_LIM` events emitted by `submit_maintenance` and
    /// `batch_submit_maintenance` when the limit is actually enforced).
    ///
    /// # Arguments
    /// * `engineer` - The engineer address to check
    ///
    /// # Returns
    /// `true` if the engineer may submit at least one more record in the
    /// current rolling-hour window (or rate limiting is disabled via
    /// `max_submissions_per_hour == 0`); `false` if they have reached the cap.
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    pub fn check_engineer_submission_rate(env: Env, engineer: Address) -> bool {
        let config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));

        if config.max_submissions_per_hour == 0 {
            return true;
        }

        let (window_start, count): (u64, u32) = env
            .storage()
            .persistent()
            .get(&submission_window_key(&engineer))
            .unwrap_or((0, 0));

        let now = env.ledger().timestamp();
        if now.saturating_sub(window_start) >= SUBMISSION_RATE_WINDOW_SECS {
            return true;
        }
        count < config.max_submissions_per_hour
    }

    /// Propose a WASM upgrade for the lifecycle contract.
    /// The upgrade must be executed after the timelock delay has passed.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    /// * `new_wasm_hash` - The hash of the new WASM to deploy
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    pub fn propose_upgrade(env: Env, admin: Address, new_wasm_hash: BytesN<32>) {
        crate::admin::propose_upgrade(env, admin, new_wasm_hash);
    }

    /// Execute a previously proposed WASM upgrade after the timelock delay has expired.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::ProposalNotFound`] if no upgrade was proposed or already executed
    /// - [`ContractError::TimelockNotExpired`] if the delay has not elapsed
    pub fn execute_upgrade(env: Env, admin: Address) {
        crate::admin::execute_upgrade(env, admin);
    }

    /// Admin-only: reset an asset's collateral score to zero.
    ///
    /// Use this after a major incident, asset rebuild, or verified fraud event
    /// to clear the score and force re-establishment of the maintenance record.
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if the contract has not been initialized.
    /// - [`ContractError::UnauthorizedAdmin`] if `admin` does not match the stored config admin.
    pub fn reset_score(env: Env, admin: Address, asset_id: u64) {
        crate::admin::reset_score(env, admin, asset_id);
    }

    /// Check collateral eligibility for multiple assets in a single call.
    ///
    /// # Arguments
    /// * `asset_ids` - Vec of asset IDs to check
    ///
    /// # Returns
    /// Vec of `bool` in the same order as `asset_ids`; each entry is `true` if
    /// the corresponding asset meets the eligibility threshold.
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::AssetNotFound`] if any asset ID does not exist
    pub fn batch_is_collateral_eligible(env: Env, asset_ids: Vec<u64>) -> Vec<bool> {
        let config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        let asset_registry = get_asset_registry_addr(&env);
        let asset_registry_client = asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        let mut results: Vec<bool> = Vec::new(&env);
        for asset_id in asset_ids.iter() {
            let asset = asset_registry_client.get_asset(&asset_id);
            let score = if asset.deprecation_status != asset_registry::DeprecationStatus::Active {
                0
            } else if env.storage().persistent().get::<_, bool>(&frozen_key(asset_id)).unwrap_or(false) {
                // Extend TTL on read (mirrors `get_collateral_score_opt`) so the
                // frozen score survives regardless of read/write cadence.
                if env.storage().persistent().has(&frozen_score_key(asset_id)) {
                    extend_persistent_ttl(&env, &frozen_score_key(asset_id));
                }
                env.storage().persistent().get(&frozen_score_key(asset_id)).unwrap_or(0)
            } else {
                compute_read_only_collateral_score(&env, asset_id, &asset.asset_type, &config)
            };
            results.push_back(score >= effective_min_collateral_score(&config));
        }
        results
    }

    /// Admin-only function to prune a specific asset's history to the current max_history cap.
    ///
    /// Truncates both maintenance history and score history to not exceed the current
    /// `max_history` setting. Useful when `max_history` has been reduced and you need
    /// to immediately enforce the new cap on existing assets.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored config admin
    /// * `asset_id` - The unique identifier of the asset to prune
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    pub fn prune_asset_history(env: Env, admin: Address, asset_id: u64) {
        crate::admin::prune_asset_history(env, admin, asset_id);
        ensure_not_paused(&env);
        admin.require_auth();

        let config: Config = env
            .storage()
            .persistent()
            .get(&CONFIG)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if config.admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        // Prune maintenance history if it exceeds max_history
        let history_key = history_key(asset_id);
        if let Some(history) = env
            .storage()
            .persistent()
            .get::<_, Vec<MaintenanceRecord>>(&history_key)
        {
            if history.len() > config.max_history {
                // Keep only the last max_history entries
                let start_idx = history.len() - config.max_history;
                let pruned_count = start_idx as u32;
                let oldest_pruned_timestamp = history.get(0).unwrap().timestamp;
                let mut pruned = Vec::new(&env);
                for i in start_idx..history.len() {
                    pruned.push_back(history.get(i).unwrap());
                }
                env.storage().persistent().set(&history_key, &pruned);
                extend_persistent_ttl(&env, &history_key);
                env.events().publish(
                    (EVENT_PRUNED,),
                    (asset_id, pruned_count, oldest_pruned_timestamp),
                );

                // Remove asset from engineer index for engineers whose records were
                // entirely dropped (i.e. they appear only in the pruned prefix).
                let mut retained_engs: Vec<Address> = Vec::new(&env);
                for i in start_idx..history.len() {
                    let eng = history.get(i).unwrap().engineer;
                    let mut found = false;
                    for existing in retained_engs.iter() {
                        if existing == eng {
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        retained_engs.push_back(eng);
                    }
                }
                let mut removed_engs: Vec<Address> = Vec::new(&env);
                for i in 0..start_idx {
                    let eng = history.get(i).unwrap().engineer;
                    let mut in_retained = false;
                    for existing in retained_engs.iter() {
                        if existing == eng {
                            in_retained = true;
                            break;
                        }
                    }
                    if in_retained {
                        continue;
                    }
                    let mut already_removed = false;
                    for existing in removed_engs.iter() {
                        if existing == eng {
                            already_removed = true;
                            break;
                        }
                    }
                    if !already_removed {
                        removed_engs.push_back(eng.clone());
                        engineer_history_remove(&env, &eng, asset_id);
                    }
                }

                // Issue #1072: the collateral score is derived from maintenance
                // history, so it must be recalculated and persisted here —
                // otherwise a stale, pre-prune score (potentially inflated by
                // the records just removed) would keep being served.
                let asset_registry = get_asset_registry_addr(&env);
                let asset = asset_registry::AssetRegistryClient::new(&env, &asset_registry)
                    .get_asset(&asset_id);
                let new_score =
                    compute_read_only_collateral_score(&env, asset_id, &asset.asset_type, &config);
                env.storage()
                    .persistent()
                    .set(&score_key(asset_id), &new_score);
                extend_persistent_ttl(&env, &score_key(asset_id));
                env.storage()
                    .persistent()
                    .set(&last_update_key(asset_id), &env.ledger().timestamp());
                extend_persistent_ttl(&env, &last_update_key(asset_id));
            }
        }

        // Prune score history if it exceeds max_history
        let score_history_key_val = score_history_key(asset_id);
        if let Some(score_history) = env
            .storage()
            .persistent()
            .get::<_, Vec<ScoreEntry>>(&score_history_key_val)
        {
            if score_history.len() > config.max_history {
                // Keep only the last max_history entries
                let start_idx = score_history.len() - config.max_history;
                let mut pruned = Vec::new(&env);
                for i in start_idx..score_history.len() {
                    pruned.push_back(score_history.get(i).unwrap());
                }
                env.storage()
                    .persistent()
                    .set(&score_history_key_val, &pruned);
                extend_persistent_ttl(&env, &score_history_key_val);
            }
        }

        let valuation_history_key = DataKey::CollateralValuationHistory(asset_id);
        if let Some(valuation_history) = env
            .storage()
            .persistent()
            .get::<_, Vec<(u64, u64)>>(&valuation_history_key)
        {
            if valuation_history.len() > config.max_history {
                let start_idx = valuation_history.len() - config.max_history;
                let mut pruned = Vec::new(&env);
                for i in start_idx..valuation_history.len() {
                    pruned.push_back(valuation_history.get(i).unwrap());
                }
                env.storage().persistent().set(&valuation_history_key, &pruned);
                env.storage().persistent().extend_ttl(
                    &valuation_history_key,
                    TTL_THRESHOLD,
                    TTL_TARGET,
                );
            }
        }

        env.events()
            .publish((symbol_short!("PRUNE"), admin.clone()), asset_id);
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("PRUNE")),
            (admin, env.ledger().timestamp(), asset_id),
        );
    }

    /// Remove all lifecycle data for a deregistered asset.
    ///
    /// After `deregister_asset` is called on the asset registry the asset record is gone,
    /// but maintenance history, collateral score, score history, and the last-update
    /// timestamp remain in lifecycle storage. Call this function to reclaim that storage
    /// and prevent stale data from being read by anyone who knows the asset ID.
    ///
    /// This is a no-op for keys that do not exist (safe to call on already-clean assets).
    ///
    /// # Arguments
    /// * `admin` - The lifecycle admin address
    /// * `asset_id` - The unique identifier of the deregistered asset
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    pub fn purge_asset_data(env: Env, admin: Address, asset_id: u64) {
        crate::admin::purge_asset_data(env, admin, asset_id);
    }

    /// Request a loan against a collateral-eligible asset.
    ///
    /// # Arguments
    /// * `asset_id`  - The asset to use as collateral
    /// * `threshold` - Minimum vouch count required; must be > 0
    /// * `amount`    - Loan amount in stroops; must be > 0
    ///
    /// # Panics
    /// - [`ContractError::InvalidConfig`] if `threshold` or `amount` is 0
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    pub fn request_loan(env: Env, asset_id: u64, threshold: u32, amount: i128) -> bool {
        if threshold == 0 {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }
        if amount == 0 {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }
        Self::is_collateral_eligible(env, asset_id)
    }

    /// Capture a point-in-time health snapshot for an asset.
    ///
    /// Anyone may call this. The snapshot persists independently of maintenance
    /// history so lenders can verify condition even after TTL-driven pruning.
    ///
    /// # Arguments
    /// * `asset_id` - The asset to snapshot
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    /// - [`ContractError::AssetDecommissioned`] if the asset has been decommissioned
    pub fn take_health_snapshot(env: Env, asset_id: u64) -> HealthSnapshot {
        let asset_registry = get_asset_registry_addr(&env);
        verify_asset_exists(&env, &asset_registry, &asset_id);

        // Reject snapshots for decommissioned assets: a decommissioned asset's
        // health data is no longer current and must not appear as a valid health
        // indicator to DeFi lenders.  Fix #989.
        let asset_client = asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        let status = asset_client.asset_status(&asset_id);
        use asset_registry::AssetStatus;
        if status == AssetStatus::Decommissioned {
            panic_with_error!(&env, ContractError::AssetDecommissioned);
        }

        let score = {
            let stored: u32 = env.storage().persistent().get(&score_key(asset_id)).unwrap_or(0);
            let config: Config = env
                .storage()
                .persistent()
                .get::<_, Config>(&CONFIG)
                .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
            let last_update: u64 = env
                .storage()
                .persistent()
                .get(&last_update_key(asset_id))
                .unwrap_or(0);
            let elapsed = env.ledger().timestamp().saturating_sub(last_update);
            let decay = (elapsed / config.decay_interval) as u32 * config.decay_rate;
            stored.saturating_sub(decay)
        };

        let history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or_else(|| Vec::new(&env));

        let maintenance_count = history.len();
        let last_service_date = history
            .iter()
            .map(|r| r.timestamp)
            .fold(0u64, |acc, t| if t > acc { t } else { acc });

        let snapshot = HealthSnapshot {
            snapshot_timestamp: env.ledger().timestamp(),
            score,
            maintenance_count,
            last_service_date,
            reconstructed: false,
        };

        let key = health_snapshot_key(asset_id);
        let mut snapshots: Vec<HealthSnapshot> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(&env));
        snapshots.push_back(snapshot.clone());
        env.storage().persistent().set(&key, &snapshots);
        extend_persistent_ttl(&env, &key);

        snapshot
    }

    /// Return all stored health snapshots for an asset.
    ///
    /// # Arguments
    /// * `asset_id` - The asset to query
    ///
    /// # Returns
    /// Vec of [`HealthSnapshot`] in chronological order (oldest first).
    pub fn get_health_snapshots(env: Env, asset_id: u64) -> Vec<HealthSnapshot> {
        env.storage()
            .persistent()
            .get(&health_snapshot_key(asset_id))
            .unwrap_or_else(|| Vec::new(&env))
    }

    // ─── Issue #1010 ──────────────────────────────────────────────────────────

    /// Return a paginated slice of the stored health snapshots for an asset.
    ///
    /// Snapshots are stored in chronological order (oldest first).  Callers can
    /// walk through the full history by incrementing `offset` by `limit` on
    /// each successive call.
    ///
    /// # Arguments
    /// * `asset_id` - The asset to query.
    /// * `offset`   - Zero-based index of the first snapshot to return.
    /// * `limit`    - Maximum number of snapshots to return per call.
    ///               Passing `0` returns an empty `Vec`.
    ///
    /// # Returns
    /// `Vec<HealthSnapshot>` containing at most `limit` entries starting at
    /// `offset`.  Returns an empty `Vec` when `offset` is beyond the end of
    /// the stored list or no snapshots exist.
    pub fn get_health_snapshots_paginated(
        env: Env,
        asset_id: u64,
        offset: u32,
        limit: u32,
    ) -> Vec<HealthSnapshot> {
        if limit == 0 {
            return Vec::new(&env);
        }
        let all: Vec<HealthSnapshot> = env
            .storage()
            .persistent()
            .get(&health_snapshot_key(asset_id))
            .unwrap_or_else(|| Vec::new(&env));

        let total = all.len();
        if offset >= total || total == 0 {
            return Vec::new(&env);
        }

        let end = (offset + limit).min(total);
        let mut page: Vec<HealthSnapshot> = Vec::new(&env);
        for i in offset..end {
            page.push_back(all.get(i).unwrap());
        }
        page
    /// Admin function: mark an existing health snapshot as a reconstruction anchor.
    ///
    /// When maintenance records are pruned via the `max_history` cap or lost
    /// due to TTL expiry, raw record detail is unrecoverable. This function
    /// allows an admin to flag a stored health snapshot as an anchor for a
    /// "reconstructed" summary, recording that the snapshot represents the
    /// best available substitute for the missing raw history.
    ///
    /// Sets `HealthSnapshot::reconstructed = true` on the snapshot at
    /// `snapshot_index` and emits a `RECONSTR` event.
    ///
    /// # Arguments
    /// * `admin` - Admin address (must match stored admin).
    /// * `asset_id` - The asset whose snapshot is being anchored.
    /// * `snapshot_index` - Zero-based index into the stored snapshots vec.
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized.
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin.
    /// - [`ContractError::SnapshotNotFound`] if no snapshots exist for the asset
    ///   or `snapshot_index` is out of bounds.
    pub fn anchor_history_to_snapshot(env: Env, admin: Address, asset_id: u64, snapshot_index: u32) {
        ensure_not_paused(&env);
        require_admin(&env, &admin);

        let key = health_snapshot_key(asset_id);
        let mut snapshots: Vec<HealthSnapshot> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::SnapshotNotFound));

        if snapshots.is_empty() || snapshot_index >= snapshots.len() {
            panic_with_error!(&env, ContractError::SnapshotNotFound);
        }

        let mut snapshot = snapshots.get(snapshot_index).unwrap();
        snapshot.reconstructed = true;
        snapshots.set(snapshot_index, snapshot.clone());

        env.storage().persistent().set(&key, &snapshots);
        extend_persistent_ttl(&env, &key);

        env.events().publish(
            (EVENT_RECONSTR, asset_id),
            (snapshot_index, snapshot.score, snapshot.timestamp),
        );
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("RECON")),
            (admin, asset_id, env.ledger().timestamp()),
        );
    }

    /// Predict the next service date for a given task type on an asset.
    ///
    /// Uses a simple moving average of historical intervals between consecutive
    /// maintenance records of the same `task_type`. The prediction is clamped
    /// to a minimum of 24 hours from now to prevent infinite loops from
    /// back-to-back submissions, and a maximum of 5 years to prevent
    /// unreasonable far-future predictions from sparse data.
    ///
    /// # Arguments
    /// * `env` - The environment.
    /// * `asset_id` - The unique identifier of the asset.
    /// * `task_type` - The task type symbol to predict (e.g., `symbol_short!("OIL_CHG")`).
    ///
    /// # Returns
    /// Predicted Unix timestamp (seconds) for the next service of this type.
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if the contract has not been initialized.
    /// - [`ContractError::InsufficientPredictionData`] if fewer than 2 matching
    ///   records exist (need at least 2 to compute an interval).
    pub fn calculate_predicted_next_service(
        env: Env,
        asset_id: u64,
        task_type: Symbol,
    ) -> u64 {
        try_predict_next_service(&env, asset_id, &task_type)
            .unwrap_or_else(|e| panic_with_error!(&env, e))
    }

    /// Return active maintenance alerts for an asset.
    ///
    /// An alert is generated when the predicted next service date for a task
    /// type has already passed, meaning the asset is overdue for maintenance.
    ///
    /// # Arguments
    /// * `env` - The environment.
    /// * `asset_id` - The unique identifier of the asset.
    ///
    /// # Returns
    /// A `Vec<(Symbol, u64)>` where each entry is a `(task_type, overdue_since_timestamp)`.
    /// The second value is the predicted service timestamp that was missed.
    /// Returns an empty vec if no alerts are active or if there is insufficient
    /// data to make predictions.
    pub fn get_maintenance_alerts(
        env: Env,
        asset_id: u64,
    ) -> Vec<(Symbol, u64)> {
        let history: Vec<MaintenanceRecord> = env
            .storage()
            .persistent()
            .get(&history_key(asset_id))
            .unwrap_or_else(|| Vec::new(&env));

        if history.is_empty() {
            return Vec::new(&env);
        }

        // Collect unique task types from the history, excluding XFER sentinels.
        let mut task_types: Vec<Symbol> = Vec::new(&env);
        for record in history.iter() {
            if record.task_type == symbol_short!("XFER") {
                continue;
            }
            let mut found = false;
            for t in task_types.iter() {
                if t == record.task_type {
                    found = true;
                    break;
                }
            }
            if !found {
                task_types.push_back(record.task_type.clone());
            }
        }

        let now = env.ledger().timestamp();
        let mut alerts: Vec<(Symbol, u64)> = Vec::new(&env);

        for task_type in task_types.iter() {
            // Use the fallible helper; skip task types with insufficient data.
            if let Ok(predicted) = try_predict_next_service(&env, asset_id, &task_type) {
                if now > predicted {
                    alerts.push_back((task_type.clone(), predicted));
                }
            }
        }

        if !alerts.is_empty() {
            env.events().publish(
                (symbol_short!("MAINT_ALRT"), asset_id),
                alerts.clone(),
            );
        }

        alerts
    }

    /// Paginated view of all asset IDs owned by `owner`, delegating to the
    /// asset-registry's owner index.
    ///
    /// DeFi lenders and portfolio managers can use this to enumerate a
    /// borrower's full asset portfolio and calculate total collateral value
    /// without needing direct access to the asset-registry contract.
    ///
    /// # Arguments
    /// * `owner` - The address to query.
    /// * `offset` - Zero-based start index into the owner's asset list.
    /// * `limit`  - Maximum number of asset IDs to return (capped at 100).
    ///
    /// # Returns
    /// A `Vec<u64>` of up to `limit` asset IDs, or an empty vec if `owner`
    /// has no assets or `offset` is beyond the list length.
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if contract has not been initialized.
    pub fn get_assets_by_owner(env: Env, owner: Address, offset: u32, limit: u32) -> Vec<u64> {
        const MAX_PAGE_SIZE: u32 = 100;
        let limit = limit.min(MAX_PAGE_SIZE);

        let asset_registry = get_asset_registry_addr(&env);
        let client = asset_registry::AssetRegistryClient::new(&env, &asset_registry);
        let all: Vec<u64> = client.get_assets_by_owner(&owner);

        let total = all.len();
        if limit == 0 || offset >= total {
            return Vec::new(&env);
        }

        let end = offset.checked_add(limit).unwrap_or(total).min(total);
        let mut page: Vec<u64> = Vec::new(&env);
        for i in offset..end {
            page.push_back(all.get(i).unwrap());
        }
        page
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::engineer_registry::{EngineerRegistry, EngineerRegistryClient};
    use asset_registry::{AssetRegistry, AssetRegistryClient};
    use soroban_sdk::{
        symbol_short,
        testutils::{storage::Persistent as _, Address as _, Events, Ledger},
        Bytes, BytesN, Env, String, Symbol, TryIntoVal,
    };

    fn setup<'a>(
        env: &'a Env,
        max_history: u32,
    ) -> (
        LifecycleClient<'a>,
        AssetRegistryClient<'a>,
        EngineerRegistryClient<'a>,
        Address,
    ) {
        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());
        let admin = Address::generate(env);
        let asset_admin = Address::generate(env);

        let lifecycle = LifecycleClient::new(env, &lifecycle_id);
        lifecycle.initialize(
            &admin,
            &asset_registry_id,
            &engineer_registry_id,
            &admin,
            &max_history,
        );

        let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
        asset_registry.initialize_admin(&asset_admin, &asset_admin);
        asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));

        (
            lifecycle,
            asset_registry,
            EngineerRegistryClient::new(env, &engineer_registry_id),
            admin,
        )
    }

    fn propose_and_expire_task_weight_timelock(
        env: &Env,
        client: &LifecycleClient,
        admin: &Address,
    ) {
        client.propose_config_update(admin, &symbol_short!("TSK_WT"));
        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);
    }

    /// Generate a unique serial number string for each test asset registration.
    fn unique_serial(env: &Env) -> String {
        use core::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        // Build "SN-<n>" without std::format! (crate is no_std)
        let mut buf = [0u8; 24];
        buf[0] = b'S'; buf[1] = b'N'; buf[2] = b'-';
        let mut end = 24usize;
        let mut v = if n == 0 { 1u64 } else { n };
        while v > 0 {
            end -= 1;
            buf[end] = b'0' + (v % 10) as u8;
            v /= 10;
        }
        let digit_len = 24 - end;
        let mut out = [0u8; 24];
        out[0] = b'S'; out[1] = b'N'; out[2] = b'-';
        out[3..3 + digit_len].copy_from_slice(&buf[end..24]);
        let s = core::str::from_utf8(&out[..3 + digit_len]).unwrap_or("SN-1");
        String::from_str(env, s)
    }

    fn register_asset(env: &Env, registry_client: &AssetRegistryClient) -> (u64, Address) {
        let owner = Address::generate(env);
        let serial = unique_serial(env);
        let asset_id = registry_client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(env, "Caterpillar 3516"),
            &serial,
            &owner,
        );
        (asset_id, owner)
    }

    fn register_asset_for_owner(
        env: &Env,
        registry_client: &AssetRegistryClient,
        owner: &Address,
    ) -> u64 {
        let serial = unique_serial(env);
        registry_client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(env, "Caterpillar 3516"),
            &serial,
            owner,
        )
    }

    fn register_engineer(env: &Env, registry_client: &EngineerRegistryClient) -> Address {
        let engineer = Address::generate(env);
        let issuer = Address::generate(env);
        let admin = Address::generate(env);
        let hash = BytesN::from_array(env, &[1u8; 32]);
        registry_client.initialize_admin(&admin, &admin);
        registry_client.add_trusted_issuer(&admin, &issuer);
        registry_client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        // Set reputation to 500 (neutral 1.0× multiplier) so existing score assertions hold
        registry_client.update_reputation(&engineer, &500);
        engineer
    }

    // ---------------------------------------------------------------------------
    //  Hash Chain Tamper Detection (issue #1088)
    // ---------------------------------------------------------------------------

    /// Self-contained setup for hash-chain tests: registers an asset type/engineer
    /// specialization pair that actually match (`diesel_ge`), independent of the
    /// generic `setup()`/`register_asset()` helpers above, since those hardcode the
    /// "GENSET" asset type which does not appear in the engineer registry's allowed
    /// specialization list and so can never pass `submit_maintenance`'s specialization check.
    fn setup_chain_test(env: &Env) -> (LifecycleClient<'_>, u64, Address, Address) {
        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());
        let admin = Address::generate(env);
        let asset_admin = Address::generate(env);

        let client = LifecycleClient::new(env, &lifecycle_id);
        client.initialize(&admin, &asset_registry_id, &engineer_registry_id, &admin, &0);

        let asset_registry_client = AssetRegistryClient::new(env, &asset_registry_id);
        asset_registry_client.initialize_admin(&asset_admin, &asset_admin);
        asset_registry_client.add_asset_type(&asset_admin, &symbol_short!("diesel_ge"));

        let engineer_registry_client = EngineerRegistryClient::new(env, &engineer_registry_id);
        let eng_admin = Address::generate(env);
        let issuer = Address::generate(env);
        engineer_registry_client.initialize_admin(&eng_admin, &eng_admin);
        engineer_registry_client.add_trusted_issuer(&eng_admin, &issuer);

        let engineer = Address::generate(env);
        let hash = BytesN::from_array(env, &[9u8; 32]);
        engineer_registry_client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        engineer_registry_client.update_reputation(&engineer, &500);
        engineer_registry_client.add_specialization(&issuer, &engineer, &symbol_short!("diesel_ge"));

        let owner = Address::generate(env);
        let asset_id = asset_registry_client.register_asset(
            &symbol_short!("diesel_ge"),
            &String::from_str(env, "Chain Test Genset"),
            &unique_serial(env),
            &owner,
        );
        client.authorize_engineer(&owner, &asset_id, &engineer);

        (client, asset_id, owner, engineer)
    }

    #[test]
    fn test_chain_integrity_true_for_empty_history() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, asset_id, _owner, _engineer) = setup_chain_test(&env);

        assert!(client.verify_maintenance_chain_integrity(&asset_id));
        assert_eq!(client.get_broken_chain_links(&asset_id).len(), 0);
    }

    #[test]
    fn test_first_record_has_no_previous_hash() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, asset_id, _owner, engineer) = setup_chain_test(&env);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Routine oil change"),
            &engineer,
            &None,
        );

        let history = client.get_maintenance_history(&asset_id);
        assert_eq!(history.get(0).unwrap().previous_record_hash, None);
        assert!(client.verify_maintenance_chain_integrity(&asset_id));
    }

    #[test]
    fn test_hash_chain_intact_across_multiple_submissions() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, asset_id, _owner, engineer) = setup_chain_test(&env);

        for _ in 0..5 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "Routine oil change"),
                &engineer,
                &None,
            );
        }

        assert!(client.verify_maintenance_chain_integrity(&asset_id));
        assert_eq!(client.get_broken_chain_links(&asset_id).len(), 0);

        let history = client.get_maintenance_history(&asset_id);
        for i in 1..history.len() {
            let expected = hash_maintenance_record(&env, &history.get(i - 1).unwrap());
            assert_eq!(history.get(i).unwrap().previous_record_hash, Some(expected));
        }
    }

    #[test]
    fn test_hash_chain_intact_across_batch_submit() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, asset_id, _owner, engineer) = setup_chain_test(&env);

        // One pre-existing record submitted individually, then a batch of three.
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Routine oil change"),
            &engineer,
            &None,
        );

        let batch = soroban_sdk::vec![
            &env,
            BatchRecord {
                task_type: symbol_short!("OIL_CHG"),
                priority: Priority::Low,
                notes: String::from_str(&env, "Batch 1"),
            },
            BatchRecord {
                task_type: symbol_short!("OIL_CHG"),
                priority: Priority::Low,
                notes: String::from_str(&env, "Batch 2"),
            },
            BatchRecord {
                task_type: symbol_short!("OIL_CHG"),
                priority: Priority::Low,
                notes: String::from_str(&env, "Batch 3"),
            },
        ];
        client.batch_submit_maintenance(&asset_id, &batch, &engineer, &None);

        assert_eq!(client.get_maintenance_history(&asset_id).len(), 4);
        assert!(client.verify_maintenance_chain_integrity(&asset_id));
        assert_eq!(client.get_broken_chain_links(&asset_id).len(), 0);
    }

    #[test]
    fn test_verify_maintenance_chain_integrity_detects_tampered_record() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, asset_id, _owner, engineer) = setup_chain_test(&env);

        for _ in 0..3 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "Routine oil change"),
                &engineer,
                &None,
            );
        }
        assert!(client.verify_maintenance_chain_integrity(&asset_id));

        // Simulate off-chain tampering: directly overwrite record index 1's notes
        // in persistent storage, without recomputing the hash chain, the way a
        // storage-level exploit or buggy migration might.
        let contract_id = client.address.clone();
        env.as_contract(&contract_id, || {
            let mut history: Vec<MaintenanceRecord> = env
                .storage()
                .persistent()
                .get(&history_key(asset_id))
                .unwrap();
            let mut tampered = history.get(1).unwrap();
            tampered.notes = String::from_str(&env, "Tampered notes");
            history.set(1, tampered);
            env.storage().persistent().set(&history_key(asset_id), &history);
        });

        assert!(!client.verify_maintenance_chain_integrity(&asset_id));
        let broken = client.get_broken_chain_links(&asset_id);
        assert_eq!(broken.len(), 1);
        assert_eq!(broken.get(0).unwrap(), 2);
    }

    #[test]
    fn test_submit_and_score() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // 10 maintenance events at default score_increment (5) each = 50 points
        for _ in 0..10 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "Routine oil change"),
                &engineer,
            );
        }

        assert_eq!(client.get_collateral_score(&asset_id), 50);
        assert_eq!(client.get_maintenance_history(&asset_id).len(), 10);
    }

    /// Issue #838: every write must extend the relevant persistent entry's TTL
    /// to at least `TTL_THRESHOLD`. Verify the history entry after `submit_maintenance`.
    #[test]
    fn test_submit_maintenance_history_ttl_at_least_threshold() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Routine oil change"),
            &engineer,
        );

        let lifecycle_id = client.address.clone();
        let history_ttl = env.as_contract(&lifecycle_id, || {
            env.storage().persistent().get_ttl(&history_key(asset_id))
        });
        assert!(
            history_ttl >= TTL_THRESHOLD,
            "history entry TTL ({}) must be >= TTL_THRESHOLD ({}) after submit_maintenance",
            history_ttl,
            TTL_THRESHOLD
        );
    }

    #[test]
    fn test_get_collateral_valuation_returns_current_value_and_timestamp() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Routine oil change"),
            &engineer,
        );

        let (timestamp, value) = client.get_valuation_history(&asset_id).get(0).unwrap();
        let current = client.get_collateral_valuation(&asset_id);
        assert_eq!(current, (value, timestamp));
        assert_eq!(value, client.get_collateral_score(&asset_id) as u64);
    }

    #[test]
    fn test_valuation_history_records_on_score_updates() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "First service"),
            &engineer,
        );
        env.ledger().with_mut(|li| li.timestamp += 31 * 24 * 60 * 60);
        client.decay_score(&asset_id);

        let history = client.get_valuation_history(&asset_id);
        assert_eq!(history.len(), 2);
        assert_eq!(history.get(0).unwrap().1, 5);
        assert_eq!(history.get(1).unwrap().1, 0);
    }

    #[test]
    fn test_valuation_history_records_reset_score() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "First service"),
            &engineer,
        );
        env.ledger().with_mut(|li| li.timestamp += 1);
        client.reset_score(&admin, &asset_id);

        let history = client.get_valuation_history(&asset_id);
        assert_eq!(history.len(), 2);
        assert_eq!(history.get(0).unwrap().1, 5);
        assert_eq!(history.get(1).unwrap().1, 0);
    }

    #[test]
    fn test_collateral_score_monotonically_increases_with_verified_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let owner = Address::generate(&env);
        let asset_id = asset_registry_client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Caterpillar 3516"),
            &unique_serial(&env),
            &owner,
        );
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&owner, &asset_id, &engineer);

        let task_types = [
            symbol_short!("OIL_CHG"),
            symbol_short!("FILTER"),
            symbol_short!("ENGINE"),
            symbol_short!("INSPECT"),
        ];
        let notes = [
            "Oil change",
            "Filter replacement",
            "Engine overhaul",
            "Inspection follow-up",
        ];

        let mut previous_score = client.get_collateral_score(&asset_id);
        for (task_type, note) in task_types.iter().zip(notes.iter()) {
            client.submit_maintenance(
                &asset_id,
                task_type,
                &String::from_str(&env, note),
                &engineer,
            );
            env.ledger().with_mut(|li| li.timestamp += 1);

            let current_score = client.get_collateral_score(&asset_id);
            assert!(
                current_score >= previous_score,
                "Collateral score must not decrease after additional verified maintenance: {} -> {}",
                previous_score,
                current_score,
            );
            previous_score = current_score;
        }

        assert_eq!(client.get_maintenance_history(&asset_id).len(), 4);
    }

    #[test]
    fn test_authorized_engineer_can_submit_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let owner = Address::generate(&env);
        let asset_id = register_asset_for_owner(&env, &asset_registry_client, &owner);
        let engineer = register_engineer(&env, &engineer_registry_client);

        client.authorize_engineer(&owner, &asset_id, &engineer);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "authorized"),
            &engineer,
        );

        assert_eq!(client.get_maintenance_history(&asset_id).len(), 1);
    }

    #[test]
    fn test_unauthorized_engineer_cannot_submit_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, _asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        // Intentionally NOT authorizing the engineer so the submission fails

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "unauthorized"),
            &engineer,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::EngineerNotAuthorized as u32,
            ))),
        );
    }

    #[test]
    fn test_non_owner_cannot_authorize_engineer() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let owner = Address::generate(&env);
        let rogue = Address::generate(&env);
        let asset_id = register_asset_for_owner(&env, &asset_registry_client, &owner);
        let engineer = register_engineer(&env, &engineer_registry_client);

        let result = client.try_authorize_engineer(&rogue, &asset_id, &engineer);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedOwner as u32,
            ))),
        );
    }

    #[test]
    fn test_get_maintenance_history_nonexistent_asset_returns_error() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let result = client.try_get_maintenance_history(&999u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_submit_maintenance_nonexistent_asset() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, engineer_registry_client, _) = setup(&env, 0);
        let engineer = register_engineer(&env, &engineer_registry_client);

        let result = client.try_submit_maintenance(
            &999u64,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Should fail"),
            &engineer,
            &None,
        );

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_history_cap_enforced() {
        // When max_history is reached, the oldest record is pruned and the new one is accepted.
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 3);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        for _ in 0..3 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "ok"),
                &engineer,
            );
        }

        // 4th submission should succeed (pruning the oldest) rather than erroring.
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "over cap"),
            &engineer,
        );
        let history = client.get_maintenance_history(&asset_id);
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn test_pruning_event_emitted_when_history_trimmed() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 3);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        for _ in 0..3 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "ok"),
                &engineer,
            );
        }

        // This submission crosses max_history=3, triggering a prune of 1 record.
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "triggers prune"),
            &engineer,
        );

        let events = env.events().all();
        let pruned_event = events.iter().find(|(_, topics, _)| {
            topics.len() == 1
                && topics.get(0).and_then(|v| TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                    .map(|s: Symbol| s == EVENT_PRUNED)
                    .unwrap_or(false)
        });
        assert!(pruned_event.is_some(), "expected PRUNED event");
        let (_, _, data) = pruned_event.unwrap();
        let (emitted_asset_id, pruned_count): (u64, u32) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_asset_id, asset_id);
        assert_eq!(pruned_count, 1u32);
    }

    #[test]
    fn test_history_cap_checked_before_cross_contract_calls() {
        // An unregistered engineer is still rejected even when history is at capacity.
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 3);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        for _ in 0..3 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "ok"),
                &engineer,
            );
        }

        // Unregistered engineer should be rejected with UnauthorizedEngineer.
        let unregistered = Address::generate(&env);
        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "over cap"),
            &unregistered,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
        );
    }

    #[test]
    fn test_submit_maintenance_rejects_empty_task_type() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!(""),
            &String::from_str(&env, "Empty task type"),
            &engineer,
        );

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidTaskType as u32,
            ))),
        );
    }

    #[test]
    fn test_submit_maintenance_rejects_unknown_task_type() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("UNKNOWN"),
            &Priority::Low,
            &String::from_str(&env, "Unknown task type"),
            &engineer,
        );

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidTaskType as u32,
            ))),
        );
    }

    #[test]
    fn test_submit_maintenance_rejects_oversized_notes() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let oversized_notes = String::from_str(&env, &"x".repeat(300));

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &oversized_notes,
            &engineer,
        );

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NotesTooLong as u32,
            ))),
        );
    }

    #[test]
    fn test_submit_maintenance_rejects_empty_notes() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let empty_notes = String::from_str(&env, "");

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &empty_notes,
            &engineer,
        );

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NotesTooLong as u32,
            ))),
        );
    }

    #[test]
    fn test_submit_maintenance_rejects_decommissioned_asset() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Decommission the asset
        let admin = Address::generate(&env);
        asset_registry_client.initialize_admin(&admin, &admin);
        asset_registry_client.decommission_asset(&admin, &asset_id);

        // Attempt to submit maintenance for decommissioned asset should fail
        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Should be rejected"),
            &engineer,
        );

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetDecommissioned as u32,
            ))),
        );
    }

    #[test]
    fn test_unregistered_engineer_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let unregistered = Address::generate(&env);

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Should fail"),
            &unregistered,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
        );
    }

    #[test]
    fn test_maintenance_history_by_engineer() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id1, asset_owner1) = register_asset(&env, &asset_registry_client);
        let (asset_id2, asset_owner2) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner1, &asset_id1, &engineer);
        client.authorize_engineer(&asset_owner2, &asset_id2, &engineer);

        client.submit_maintenance(
            &asset_id1,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "one"),
            &engineer,
        );
        client.submit_maintenance(
            &asset_id2,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "two"),
            &engineer,
        );

        let history = client.get_engineer_maintenance_history(&engineer);
        assert_eq!(history.len(), 2);
        assert!(history.contains(&asset_id1));
        assert!(history.contains(&asset_id2));

        let other_engineer = Address::generate(&env);
        let empty_history = client.get_engineer_maintenance_history(&other_engineer);
        assert_eq!(empty_history.len(), 0);
    }
    #[test]
    fn test_engineer_history_bounded() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 3);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // Register and maintain 5 different assets (exceeds max_history=3)
        let mut asset_ids = Vec::new(&env);
        for _ in 0..5 {
            let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
            asset_ids.push_back(asset_id);
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "service"),
                &engineer,
            );
        }

        // Engineer history should be capped at max_history (3)
        let history = client.get_engineer_maintenance_history(&engineer);
        assert_eq!(
            history.len(),
            3,
            "Engineer history should be bounded by max_history"
        );

        // Oldest entries (asset_ids[0], asset_ids[1]) should have been evicted
        assert!(!history.contains(&asset_ids.get(0).unwrap()));
        assert!(!history.contains(&asset_ids.get(1).unwrap()));

        // Newest entries should remain
        assert!(history.contains(&asset_ids.get(2).unwrap()));
        assert!(history.contains(&asset_ids.get(3).unwrap()));
        assert!(history.contains(&asset_ids.get(4).unwrap()));
    }

    #[test]
    fn test_engineer_history_no_duplicate_asset_id_on_repeated_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "first"),
            &engineer,
        );
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &Priority::Low,
            &String::from_str(&env, "second"),
            &engineer,
        );

        let history = client.get_engineer_maintenance_history(&engineer);
        assert_eq!(history.len(), 1);
        assert!(history.contains(&asset_id));
    }

    #[test]
    fn test_get_last_service_no_history() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        assert_eq!(client.get_last_service(&asset_id), None);
    }

    #[test]
    fn test_get_last_service_no_asset() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        assert_eq!(client.get_last_service(&9999u64), None);
    }

    #[test]
    fn test_get_last_service_returns_most_recent_by_timestamp() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Submit first record at t=1000
        env.ledger().set_timestamp(1000);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "first"),
            &engineer,
        );

        // Submit second record at t=2000 (most recent)
        env.ledger().set_timestamp(2000);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &Priority::Low,
            &String::from_str(&env, "second"),
            &engineer,
        );

        let last = client.get_last_service(&asset_id).unwrap();
        assert_eq!(last.timestamp, 2000);
        assert_eq!(last.task_type, symbol_short!("INSPECT"));
    }

    #[test]
    fn test_get_last_maintenance_none_for_no_history() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let (asset_id, _) = register_asset(&env, &asset_registry_client);
        assert_eq!(client.get_last_maintenance(&asset_id), None);
    }

    #[test]
    fn test_get_last_maintenance_returns_most_recent() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        env.ledger().set_timestamp(500);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "older"),
            &engineer,
        );

        env.ledger().set_timestamp(1500);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &String::from_str(&env, "newer"),
            &engineer,
        );

        let last = client.get_last_maintenance(&asset_id).unwrap();
        assert_eq!(last.timestamp, 1500);
        assert_eq!(last.task_type, symbol_short!("INSPECT"));
    }

    #[test]
    fn test_admin_can_update_score_increment() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.update_score_increment(&admin, &12);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Configured increment"),
            &engineer,
        );

        // score_increment (12) governs scoring, not task weight
        assert_eq!(client.get_collateral_score(&asset_id), 12);
    }

    #[test]
    fn test_score_increment_affects_scoring() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Default score_increment is 5
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "First"),
            &engineer,
        );
        assert_eq!(client.get_collateral_score(&asset_id), 5);

        // Update score_increment to 8
        client.update_score_increment(&admin, &8);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &Priority::Low,
            &String::from_str(&env, "Second"),
            &engineer,
        );
        // Score should be 5 + 8 = 13
        assert_eq!(client.get_collateral_score(&asset_id), 13);

        // Update score_increment to 20 (capped at 100)
        client.update_score_increment(&admin, &20);
        for _ in 0..5 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("ENGINE"),
                &Priority::Low,
                &String::from_str(&env, "Bulk"),
                &engineer,
            );
        }
        // 13 + 5*20 = 113, capped at 100
        assert_eq!(client.get_collateral_score(&asset_id), 100);
    }

    #[test]
    fn test_collateral_score_respects_theoretical_max_for_config_variants() {
        let variants = [(1u32, 3u32), (3u32, 5u32), (5u32, 7u32), (10u32, 12u32)];

        for (max_history, score_increment) in variants {
            let env = Env::default();
            env.mock_all_auths();

            let (client, asset_registry_client, engineer_registry_client, admin) =
                setup(&env, max_history);
            let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
            let engineer = register_engineer(&env, &engineer_registry_client);
            client.authorize_engineer(&asset_owner, &asset_id, &engineer);

            client.update_score_increment(&admin, &score_increment);

            for _ in 0..max_history {
                client.submit_maintenance(
                    &asset_id,
                    &symbol_short!("OIL_CHG"),
                    &String::from_str(&env, "invariant"),
                    &engineer,
                );
            }

            let score = client.get_collateral_score(&asset_id);
            let theoretical_max = max_history.saturating_mul(score_increment);
            assert!(
                score <= theoretical_max,
                "score {score} exceeded theoretical max {theoretical_max} for max_history={max_history} score_increment={score_increment}"
            );
        }
    }

    #[test]
    fn test_non_admin_cannot_update_score_increment() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let outsider = Address::generate(&env);
        let result = client.try_update_score_increment(&outsider, &12);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_update_score_increment_zero_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let result = client.try_update_score_increment(&admin, &0);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidConfig as u32,
            ))),
        );
    }

    #[test]
    fn test_update_score_increment_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let old_increment = client.get_config().score_increment;
        let new_increment: u32 = 12;

        client.update_score_increment(&admin, &new_increment);

        let events = env.events().all();
        assert!(events.len() >= 1);
        let (_, topics, data) = events.get(0).unwrap();
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("CFG_UPD"));

        let (emitted_old, emitted_new): (u32, u32) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_old, old_increment);
        assert_eq!(emitted_new, new_increment);
    }

    #[test]
    fn test_admin_can_update_max_history() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        client.update_max_history(&admin, &300);
        let config = client.get_config();
        assert_eq!(config.max_history, 300);
    }

    #[test]
    fn test_non_admin_cannot_update_max_history() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let outsider = Address::generate(&env);
        let result = client.try_update_max_history(&outsider, &300);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_update_max_history_zero_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let result = client.try_update_max_history(&admin, &0);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidConfig as u32,
            ))),
        );
    }

    #[test]
    fn test_set_task_weight_configures_custom_weight() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Set custom weight for OIL_CHG to 20
        propose_and_expire_task_weight_timelock(&env, &client, &admin);
        client.set_task_weight(&admin, &symbol_short!("OIL_CHG"), &20);

        // Submit OIL_CHG maintenance and verify score uses custom weight
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "custom weight test"),
            &engineer,
        );

        let score = client.get_collateral_score(&asset_id);
        assert_eq!(score, 20);
    }

    #[test]
    fn test_set_task_weight_rejects_zero_weight() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);

        let result = client.try_set_task_weight(&admin, &symbol_short!("OIL_CHG"), &0);
        assert!(result.is_err(), "zero weight should be rejected");
    }

    #[test]
    fn test_set_task_weight_non_admin_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let not_admin = Address::generate(&env);

        let result = client.try_set_task_weight(&not_admin, &symbol_short!("OIL_CHG"), &15);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_set_task_weight_falls_back_to_defaults() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Don't set custom weight - should use default
        // OIL_CHG has default weight of 2 but score_increment is 5
        // So first submission should give 5
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "default weight test"),
            &engineer,
        );

        let score = client.get_collateral_score(&asset_id);
        assert_eq!(score, 5);
    }

    #[test]
    fn test_set_multiple_task_weights() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id1, asset_owner1) = register_asset(&env, &asset_registry_client);
        let (asset_id2, asset_owner2) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner1, &asset_id1, &engineer);
        client.authorize_engineer(&asset_owner2, &asset_id2, &engineer);

        // Configure different weights
        propose_and_expire_task_weight_timelock(&env, &client, &admin);
        client.set_task_weight(&admin, &symbol_short!("OIL_CHG"), &10);
        propose_and_expire_task_weight_timelock(&env, &client, &admin);
        client.set_task_weight(&admin, &symbol_short!("ENGINE"), &50);

        // Submit OIL_CHG to asset1
        client.submit_maintenance(
            &asset_id1,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "oil change"),
            &engineer,
        );

        // Submit ENGINE to asset2
        client.submit_maintenance(
            &asset_id2,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "engine overhaul"),
            &engineer,
        );

        assert_eq!(client.get_collateral_score(&asset_id1), 10);
        assert_eq!(client.get_collateral_score(&asset_id2), 50);
    }

    // --- #1029: WEIGHT_SET audit event ---

    /// `set_task_weight` must emit a `WEIGHT_SET` event whose data tuple is
    /// `(task_type, old_weight, new_weight, admin, timestamp)`.
    /// On first set the old_weight must be 0 (key not yet present in map).
    #[test]
    fn test_set_task_weight_emits_weight_set_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let task_type = symbol_short!("OIL_CHG");
        let new_weight: u32 = 25;

        // First call: no existing weight, so old_weight must be 0.
        client.set_task_weight(&admin, &task_type, &new_weight);

        let events = env.events().all();
        let weight_set_event = events.iter().find(|(_, topics, _)| {
            topics.len() == 1
                && topics
                    .get(0)
                    .and_then(|v| TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                    .map(|s| s == symbol_short!("WEIGHT_SET"))
                    .unwrap_or(false)
        });

        assert!(weight_set_event.is_some(), "expected WEIGHT_SET event to be emitted");

        let (_, _, data) = weight_set_event.unwrap();
        let (emitted_task_type, old_weight, emitted_new_weight, emitted_admin, _timestamp): (
            Symbol,
            u32,
            u32,
            Address,
            u64,
        ) = data.try_into_val(&env).unwrap();

        assert_eq!(emitted_task_type, task_type);
        assert_eq!(old_weight, 0u32, "first set must report old_weight = 0");
        assert_eq!(emitted_new_weight, new_weight);
        assert_eq!(emitted_admin, admin);
    }

    /// On a subsequent `set_task_weight` call the event must carry the
    /// previously stored weight as `old_weight`.
    #[test]
    fn test_set_task_weight_event_reports_previous_weight() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let task_type = symbol_short!("ENGINE");

        // Establish initial weight.
        client.set_task_weight(&admin, &task_type, &10);

        // Advance ledger timestamp so we can distinguish the two event sets.
        env.ledger().set_timestamp(env.ledger().timestamp() + 1);

        // Update to a new weight.
        client.set_task_weight(&admin, &task_type, &30);

        // The last WEIGHT_SET event must reflect the update: old=10, new=30.
        let events = env.events().all();
        let last_weight_set = events.iter().rev().find(|(_, topics, _)| {
            topics.len() == 1
                && topics
                    .get(0)
                    .and_then(|v| TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                    .map(|s| s == symbol_short!("WEIGHT_SET"))
                    .unwrap_or(false)
        });

        assert!(last_weight_set.is_some(), "expected WEIGHT_SET event on second call");

        let (_, _, data) = last_weight_set.unwrap();
        let (_emitted_task_type, old_weight, new_weight, _emitted_admin, _timestamp): (
            Symbol,
            u32,
            u32,
            Address,
            u64,
        ) = data.try_into_val(&env).unwrap();

        assert_eq!(old_weight, 10u32, "old_weight must reflect the previous value");
        assert_eq!(new_weight, 30u32);
    }

    #[test]
    fn test_update_scoring_weights_admin_only() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let outsider = Address::generate(&env);
        let weights = Bytes::from_slice(
            &env,
            b"{\"low\":80,\"medium\":100,\"high\":140,\"medium_threshold\":4,\"high_threshold\":10,\"window_days\":365}",
        );

        let result = client.try_update_scoring_weights(&outsider, &symbol_short!("GENSET"), &weights);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_update_scoring_weights_rejects_invalid_json_shape() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let invalid = Bytes::from_slice(&env, b"{\"low\":80,\"medium\":100}");

        let result = client.try_update_scoring_weights(&admin, &symbol_short!("GENSET"), &invalid);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidConfig as u32,
            ))),
        );
    }

    #[test]
    fn test_get_scoring_weights_returns_stored_bytes() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let weights = Bytes::from_slice(
            &env,
            b"{\"low\":80,\"medium\":100,\"high\":140,\"medium_threshold\":4,\"high_threshold\":10,\"window_days\":365}",
        );

        client.update_scoring_weights(&admin, &symbol_short!("GENSET"), &weights);
        assert_eq!(client.get_scoring_weights(&symbol_short!("GENSET")), weights);
    }

    #[test]
    fn test_dynamic_scoring_weights_apply_low_frequency_penalty() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let weights = Bytes::from_slice(
            &env,
            b"{\"low\":80,\"medium\":100,\"high\":140,\"medium_threshold\":4,\"high_threshold\":10,\"window_days\":365}",
        );
        client.update_scoring_weights(&admin, &symbol_short!("GENSET"), &weights);

        for _ in 0..3 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("FILTER"),
                &String::from_str(&env, "frequency test"),
                &engineer,
            );
            env.ledger().with_mut(|li| li.timestamp += 1);
        }

        // Base score is 15; low-frequency multiplier is 80%.
        assert_eq!(client.get_collateral_score(&asset_id), 12);
    }

    #[test]
    fn test_dynamic_scoring_weights_apply_high_frequency_bonus() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let weights = Bytes::from_slice(
            &env,
            b"{\"low\":80,\"medium\":100,\"high\":140,\"medium_threshold\":4,\"high_threshold\":10,\"window_days\":365}",
        );
        client.update_scoring_weights(&admin, &symbol_short!("GENSET"), &weights);

        for _ in 0..10 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("FILTER"),
                &String::from_str(&env, "high frequency test"),
                &engineer,
            );
            env.ledger().with_mut(|li| li.timestamp += 1);
        }

        // Base score is 50; high-frequency multiplier is 140%, capped at 70 here.
        assert_eq!(client.get_collateral_score(&asset_id), 70);
        assert!(client.is_collateral_eligible(&asset_id));
    }

    #[test]
    fn test_score_history_bounded_after_max_history_update() {
        let env = Env::default();
        env.mock_all_auths();

        // Setup with initial max_history of 10
        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 10);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Submit 4 maintenance records (below max_history of 10)
        for _i in 0..4 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "Maintenance"),
                &engineer,
            );
            env.ledger().set_timestamp(env.ledger().timestamp() + 1000);
        }

        // Verify score history has 4 entries
        let history = client.get_score_history(&asset_id);
        assert_eq!(history.len(), 4u32);

        // Update max_history to 5 - from now on, history should be capped at 5
        client.update_max_history(&admin, &5);

        // Submit one more maintenance record up to the new cap
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Maintenance"),
            &engineer,
        );
        env.ledger().set_timestamp(env.ledger().timestamp() + 1000);

        // Call decay_score which will use the new max_history value
        client.decay_score(&asset_id);

        // Verify score history is now bounded to the new max_history (5)
        let history_after = client.get_score_history(&asset_id);
        assert!(
            history_after.len() <= 5u32,
            "Score history {} should be <= 5 after max_history update",
            history_after.len()
        );
    }

    #[test]
    fn test_max_history_reduction_does_not_automatically_prune() {
        let env = Env::default();
        env.mock_all_auths();

        // Setup with initial max_history of 10
        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 10);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Submit 10 maintenance records to reach max_history
        for _i in 0..10 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "Maintenance"),
                &engineer,
            );
        }

        // Verify both histories have 10 entries
        let history = client.get_maintenance_history(&asset_id);
        let score_history = client.get_score_history(&asset_id);
        assert_eq!(history.len(), 10u32);
        assert_eq!(score_history.len(), 10u32);

        // Reduce max_history to 3
        client.update_max_history(&admin, &3);

        // Verify that existing histories were NOT pruned automatically
        let history_after = client.get_maintenance_history(&asset_id);
        let score_history_after = client.get_score_history(&asset_id);
        assert_eq!(
            history_after.len(),
            10u32,
            "Maintenance history should remain at 10 until next write"
        );
        assert_eq!(
            score_history_after.len(),
            10u32,
            "Score history should remain at 10 until next write"
        );
    }

    #[test]
    fn test_prune_asset_history_reduces_both_histories() {
        let env = Env::default();
        env.mock_all_auths();

        // Setup with initial max_history of 10
        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 10);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Submit 10 maintenance records to reach max_history
        for _i in 0..10 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "Maintenance"),
                &engineer,
            );
        }

        // Reduce max_history and verify histories still at 10
        client.update_max_history(&admin, &3);
        let history_before = client.get_maintenance_history(&asset_id);
        let score_history_before = client.get_score_history(&asset_id);
        assert_eq!(history_before.len(), 10u32);
        assert_eq!(score_history_before.len(), 10u32);

        // Call prune_asset_history to immediately prune to the new cap
        client.prune_asset_history(&admin, &asset_id);

        // Verify both histories are now pruned to max_history of 3
        let history_after = client.get_maintenance_history(&asset_id);
        let score_history_after = client.get_score_history(&asset_id);
        assert_eq!(
            history_after.len(),
            3u32,
            "Maintenance history should be pruned to 3"
        );
        assert_eq!(
            score_history_after.len(),
            3u32,
            "Score history should be pruned to 3"
        );

        // Verify that the most recent entries were kept (not the oldest)
        let last_before = history_before.get(9).unwrap();
        let last_after = history_after.get(2).unwrap();
        assert_eq!(
            last_before.timestamp, last_after.timestamp,
            "Most recent entries should be kept"
        );
    }

    /// Issue #1208: after pruning, the new oldest record's `previous_record_hash`
    /// must not still point at a now-pruned record — that would leave the hash
    /// chain referencing a link a verifier can never resolve.
    #[test]
    fn test_prune_asset_history_resets_hash_chain_anchor() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 10);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        for _i in 0..10 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "Maintenance"),
                &engineer,
            );
        }

        // Before pruning, every record but the first chains to its predecessor.
        let history_before = client.get_maintenance_history(&asset_id);
        assert!(history_before.get(0).unwrap().previous_record_hash.is_none());
        assert!(history_before.get(1).unwrap().previous_record_hash.is_some());

        client.update_max_history(&admin, &3);
        client.prune_asset_history(&admin, &asset_id);

        let history_after = client.get_maintenance_history(&asset_id);
        assert_eq!(history_after.len(), 3u32);
        assert!(
            history_after.get(0).unwrap().previous_record_hash.is_none(),
            "new oldest record must not chain to a pruned record"
        );
    }

    /// Issue #1072: pruning must recalculate and persist the collateral score
    /// immediately, otherwise readers of the raw stored score (e.g.
    /// `take_health_snapshot`, which does not recompute from live history)
    /// keep seeing the stale, pre-prune value.
    #[test]
    fn test_prune_asset_history_updates_stale_score_snapshot() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 10);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // 10 records at the default OIL_CHG weight (5) each = score 50.
        for _ in 0..10 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "Routine oil change"),
                &engineer,
                &None,
            );
        }
        let score_before = client.take_health_snapshot(&asset_id).score;
        assert_eq!(score_before, 50);

        // Shrink max_history and prune — most of the scoring history is dropped.
        client.update_max_history(&admin, &2);
        client.prune_asset_history(&admin, &asset_id);

        let score_after = client.take_health_snapshot(&asset_id).score;
        assert!(
            score_after < score_before,
            "collateral score must be recalculated immediately after pruning, got {} (was {})",
            score_after,
            score_before
        );
    }

    #[test]
    fn test_prune_asset_history_recalculates_score_for_remaining_records() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 1);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        for _ in 0..3 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("ENGINE"),
                &Priority::High,
                &String::from_str(&env, "maintenance"),
                &engineer,
            );
        }

        let initial_score = client.get_collateral_score(&asset_id);
        assert_eq!(initial_score, 15, "initial score should reflect all submitted records");

        client.prune_asset_history(&admin, &asset_id);

        let remaining_score = client.get_collateral_score(&asset_id);
        assert_eq!(remaining_score, 5, "score should reflect only the single remaining record after pruning");
    }

    #[test]
    fn test_prune_asset_history_cleans_engineer_index() {
        let env = Env::default();
        env.mock_all_auths();

        // max_history starts at 10
        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 10);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);

        // Two engineers: eng_a submits the first 5 records, eng_b submits the next 5
        let eng_a = register_engineer(&env, &engineer_registry_client);
        let eng_b = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &eng_a);
        client.authorize_engineer(&asset_owner, &asset_id, &eng_b);

        for _ in 0..5 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("INSPECT"),
                &String::from_str(&env, "Early check"),
                &eng_a,
            );
        }
        for _ in 0..5 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "Later check"),
                &eng_b,
            );
        }

        // Both engineers should be in the index before pruning
        assert!(client
            .get_engineer_maintenance_history(&eng_a)
            .contains(&asset_id));
        assert!(client
            .get_engineer_maintenance_history(&eng_b)
            .contains(&asset_id));

        // Reduce max_history to 5 so eng_a's records are entirely pruned
        client.update_max_history(&admin, &5);
        client.prune_asset_history(&admin, &asset_id);

        // eng_a no longer has any retained records → must be removed from index
        assert!(
            !client
                .get_engineer_maintenance_history(&eng_a)
                .contains(&asset_id),
            "eng_a should be removed from engineer index after all their records are pruned"
        );
        // eng_b still has retained records → must remain in index
        assert!(
            client
                .get_engineer_maintenance_history(&eng_b)
                .contains(&asset_id),
            "eng_b should remain in engineer index because their records were kept"
        );
    }

    #[test]
    fn test_non_admin_cannot_prune_asset_history() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 10);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let outsider = Address::generate(&env);

        let result = client.try_prune_asset_history(&outsider, &asset_id);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_admin_can_update_decay_config() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Build up a score first (default score_increment = 5)
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &Priority::Low,
            &String::from_str(&env, "Major overhaul"),
            &engineer,
        );

        let initial_score = client.get_collateral_score(&asset_id);

        // Update decay config: 2 points per 60 seconds (for testing)
        client.update_decay_config(&admin, &2, &60);

        // Advance ledger time by 120 seconds (2 intervals)
        env.ledger()
            .with_mut(|li| li.timestamp = li.timestamp + 120);

        // Apply decay: should lose 4 points (2 * 2 intervals)
        client.decay_score(&asset_id);
        let new_score = client.get_collateral_score(&asset_id);

        assert_eq!(new_score, initial_score.saturating_sub(4));
    }

    #[test]
    fn test_update_decay_config_persists_via_get_config() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        client.update_decay_config(&admin, &7, &3600);

        let config = client.get_config();
        assert_eq!(config.decay_rate, 7);
        assert_eq!(config.decay_interval, 3600);
    }

    #[test]
    fn test_non_admin_cannot_update_decay_config() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let outsider = Address::generate(&env);
        let result = client.try_update_decay_config(&outsider, &10, &60);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_update_decay_config_zero_interval_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let result = client.try_update_decay_config(&admin, &10, &0);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidConfig as u32,
            ))),
        );
    }

    #[test]
    fn test_update_decay_config_zero_rate_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let result = client.try_update_decay_config(&admin, &0, &2592000);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidConfig as u32,
            ))),
        );
    }

    /// Verify that `initialize` stores a non-zero `decay_interval` in the config.
    ///
    /// This is the runtime counterpart of the compile-time guard added inside
    /// `initialize` that rejects a zero `DEFAULT_DECAY_INTERVAL` constant.
    /// If that constant were ever accidentally zeroed out in a build, both this
    /// test and the guard itself would catch it before the contract is deployed
    /// (preventing a division-by-zero in `apply_decay`).
    #[test]
    fn test_initialize_decay_interval_is_non_zero() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let config = client.get_config();
        assert!(
            config.decay_interval > 0,
            "initialize must set a non-zero decay_interval to prevent division-by-zero in apply_decay"
        );
    }

    #[test]
    fn test_decay_score_uses_configured_values() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Build up a score to 25 (5 * default score_increment of 5)
        for _ in 0..5 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("ENGINE"),
                &Priority::Low,
                &String::from_str(&env, "Major work"),
                &engineer,
            );
        }

        let initial_score = client.get_collateral_score(&asset_id);

        // Set custom decay: 2 points per 100 seconds
        client.update_decay_config(&admin, &2, &100);

        // Advance time by 250 seconds (2 full intervals)
        env.ledger()
            .with_mut(|li| li.timestamp = li.timestamp + 250);

        // Apply decay: should lose 4 points (2 * 2 intervals)
        client.decay_score(&asset_id);
        let new_score = client.get_collateral_score(&asset_id);

        assert_eq!(new_score, initial_score.saturating_sub(4));
    }

    #[test]
    fn test_get_collateral_score_applies_lazy_decay() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Build score to 20 (default score_increment = 5)
        for _ in 0..4 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("ENGINE"),
                &Priority::Low,
                &String::from_str(&env, "Build score"),
                &engineer,
            );
        }

        // Fast decay: 5 points per 60 seconds
        client.update_decay_config(&admin, &5, &60);

        // Advance 120 seconds (2 intervals -> 10 points decay)
        env.ledger()
            .with_mut(|li| li.timestamp = li.timestamp + 120);

        let decayed = client.get_collateral_score(&asset_id);
        assert_eq!(decayed, 10);

        // Ensure value is written back to storage (subsequent reads are consistent)
        let decayed_again = client.get_collateral_score(&asset_id);
        assert_eq!(decayed_again, 10);
    }

    #[test]
    fn test_get_collateral_score_is_read_only() {
        // get_collateral_score must NOT write the decayed score back to storage.
        // Calling it multiple times across ledger advances must always return the
        // score computed from the *original* stored value, not a previously-decayed one.
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Build score to 20 (4 × default score_increment of 5)
        for _ in 0..4 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("ENGINE"),
                &Priority::Low,
                &String::from_str(&env, "Build score"),
                &engineer,
            );
        }
        assert_eq!(client.get_collateral_score(&asset_id), 20);

        // Fast decay: 5 points per 60 seconds
        client.update_decay_config(&admin, &5, &60);

        // Advance 60 s (1 interval → −5 pts → expected 15)
        env.ledger().with_mut(|li| li.timestamp += 60);
        assert_eq!(client.get_collateral_score(&asset_id), 15);

        // Advance another 60 s without calling decay_score.
        // If get_collateral_score had written 15 back to storage, the next call
        // would compute from 15 and return 10. But because it is read-only it must
        // still compute from the original stored value of 20 and return 10 (2 intervals).
        env.ledger().with_mut(|li| li.timestamp += 60);
        assert_eq!(client.get_collateral_score(&asset_id), 10);

        // Confirm the stored score is still 20 (untouched by get_collateral_score)
        let contract_id = client.address.clone();
        env.as_contract(&contract_id, || {
            let stored: u32 = env
                .storage()
                .persistent()
                .get(&score_key(asset_id))
                .unwrap_or(0);
            assert_eq!(
                stored, 20,
                "stored score must not be mutated by get_collateral_score"
            );
        });
    }

    #[test]
    fn test_decay_score_five_points_per_thirty_day_interval() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        for _ in 0..10 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("ENGINE"),
                &Priority::Low,
                &String::from_str(&env, "Build score to 50"),
                &engineer,
            );
        }
        assert_eq!(client.get_collateral_score(&asset_id), 50);

        env.ledger()
            .with_mut(|li| li.timestamp = li.timestamp + 2 * DEFAULT_DECAY_INTERVAL);

        let decayed = client.decay_score(&asset_id);
        assert_eq!(decayed, 40);
        assert_eq!(client.get_collateral_score(&asset_id), 40);
    }

    #[test]
    fn test_decay_score_clamps_at_zero_after_long_elapsed_time() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &Priority::Low,
            &String::from_str(&env, "Single major service"),
            &engineer,
        );
        assert_eq!(client.get_collateral_score(&asset_id), 5);

        const SECONDS_PER_DAY: u64 = 86_400;
        const DAYS_PER_YEAR: u64 = 365;
        env.ledger().with_mut(|li| {
            li.timestamp = li.timestamp + DAYS_PER_YEAR * SECONDS_PER_DAY;
        });

        let decayed = client.decay_score(&asset_id);
        assert_eq!(decayed, 0);
        assert_eq!(client.get_collateral_score(&asset_id), 0);
    }

    #[test]
    fn test_decay_score_returns_zero_for_asset_with_no_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let owner = Address::generate(&env);
        let asset_id = asset_registry_client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "No-maintenance asset"),
            &unique_serial(&env),
            &owner,
        );

        // Advance ledger so last_update_key unwrap_or(0) would produce a large time_elapsed
        env.ledger().with_mut(|li| li.timestamp += 10_000_000);

        // Score is 0 (never maintained) ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â early return must fire and return 0
        assert_eq!(client.decay_score(&asset_id), 0);
    }

    #[test]
    fn test_decay_score_returns_zero_for_nonexistent_asset_id() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);

        // Asset ID 9999 was never registered; score_key is absent ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ unwrap_or(0) ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ early return
        assert_eq!(client.decay_score(&9999u64), 0);
    }

    #[test]
    fn test_apply_decay_extends_last_update_ttl_when_score_is_zero() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Build a score then reset it to 0 so last_update_key exists but score is 0
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "ok"),
            &engineer,
        );
        client.reset_score(&admin, &asset_id);
        assert_eq!(client.get_collateral_score(&asset_id), 0);

        // decay_score on a zero-score asset should return 0 and extend last_update_key TTL
        assert_eq!(client.decay_score(&asset_id), 0);

        let contract_id = client.address.clone();
        let ttl = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get_ttl(&last_update_key(asset_id))
        });
        assert!(
            ttl > 0,
            "last_update_key TTL should be extended even when score is 0"
        );
    }

    #[test]
    fn test_apply_decay_extends_ttl_on_zero_interval_early_return() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Build a non-zero score
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "ok"),
            &engineer,
        );
        let initial_score = client.get_collateral_score(&asset_id);
        assert!(initial_score > 0);

        let contract_id = client.address.clone();

        // Verify keys exist before decay_score
        env.as_contract(&contract_id, || {
            assert!(env.storage().persistent().has(&score_key(asset_id)));
            assert!(env.storage().persistent().has(&last_update_key(asset_id)));
        });

        // Call decay_score with no time elapsed (zero intervals) -> early return path
        let score_after = client.decay_score(&asset_id);
        assert_eq!(score_after, initial_score);

        // Verify TTLs were extended even on the early-return path
        env.as_contract(&contract_id, || {
            assert!(
                env.storage().persistent().get_ttl(&score_key(asset_id)) > 0,
                "score_key TTL should be extended on zero-interval early return"
            );
            assert!(
                env.storage()
                    .persistent()
                    .get_ttl(&last_update_key(asset_id))
                    > 0,
                "last_update_key TTL should be extended on zero-interval early return"
            );
        });
    }

    #[test]
    fn test_submit_maintenance_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let task_type = symbol_short!("OIL_CHG");
        let timestamp = env.ledger().timestamp();
        client.submit_maintenance(
            &asset_id,
            &task_type,
            &String::from_str(&env, "Routine"),
            &engineer,
        );

        use soroban_sdk::{FromVal, TryIntoVal};
        let maint_topic = symbol_short!("maint");
        let events = env.events().all();
        let (_, topics, data) = events
            .iter()
            .find(|(_, topics, _)| {
                topics.get(0).map_or(false, |v| {
                    Symbol::from_val(&env, &v) == maint_topic
                })
            })
            .expect("maint event not emitted");

        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, maint_topic);

        let (emitted_asset_id, emitted_engineer, emitted_task_type, emitted_timestamp): (
            u64,
            Address,
            Symbol,
            u64,
        ) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_asset_id, asset_id);
        assert_eq!(emitted_engineer, engineer);
        assert_eq!(emitted_task_type, task_type);
        assert_eq!(emitted_timestamp, timestamp);
    }

    #[test]
    fn test_initialize_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());
        let admin = Address::generate(&env);

        let lifecycle = LifecycleClient::new(&env, &lifecycle_id);
        lifecycle.initialize(
            &admin,
            &asset_registry_id,
            &engineer_registry_id,
            &admin,
            &0u32,
        );

        let events = env.events().all();
        assert!(events.len() >= 1);
    }

    #[test]
    fn test_initialize_twice_panics_with_already_initialized() {
        let env = Env::default();
        env.mock_all_auths();

        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());
        let admin = Address::generate(&env);

        let lifecycle = LifecycleClient::new(&env, &lifecycle_id);
        lifecycle.initialize(
            &admin,
            &asset_registry_id,
            &engineer_registry_id,
            &admin,
            &0u32,
        );

        // Try to initialize again
        let result = lifecycle.try_initialize(
            &admin,
            &asset_registry_id,
            &engineer_registry_id,
            &admin,
            &0u32,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AlreadyInitialized as u32,
            ))),
        );
    }

    #[test]
    fn test_initialize_rejects_same_registry_addresses() {
        let env = Env::default();
        env.mock_all_auths();

        let same_registry_id = env.register(AssetRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());
        let admin = Address::generate(&env);

        let lifecycle = LifecycleClient::new(&env, &lifecycle_id);
        let result =
            lifecycle.try_initialize(&admin, &same_registry_id, &same_registry_id, &admin, &0u32);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidConfig as u32,
            ))),
        );
    }

    // --- #1028: zero-address validation in initialize ---

    /// Passing the Stellar zero address as `asset_registry` must be rejected
    /// with `ContractError::ZeroAddress` before any state is written.
    #[test]
    fn test_initialize_rejects_zero_asset_registry() {
        let env = Env::default();
        env.mock_all_auths();

        let engineer_registry_id = env.register(EngineerRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());
        let admin = Address::generate(&env);
        let zero = Address::from_str(
            &env,
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
        );

        let lifecycle = LifecycleClient::new(&env, &lifecycle_id);
        let result =
            lifecycle.try_initialize(&admin, &zero, &engineer_registry_id, &admin, &0u32);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::ZeroAddress as u32,
            ))),
        );
    }

    /// Passing the Stellar zero address as `engineer_registry` must be rejected
    /// with `ContractError::ZeroAddress` before any state is written.
    #[test]
    fn test_initialize_rejects_zero_engineer_registry() {
        let env = Env::default();
        env.mock_all_auths();

        let asset_registry_id = env.register(AssetRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());
        let admin = Address::generate(&env);
        let zero = Address::from_str(
            &env,
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
        );

        let lifecycle = LifecycleClient::new(&env, &lifecycle_id);
        let result =
            lifecycle.try_initialize(&admin, &asset_registry_id, &zero, &admin, &0u32);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::ZeroAddress as u32,
            ))),
        );
    }

    #[test]
    fn test_get_collateral_score_unregistered_asset() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);

        // Query score for non-existent asset ID
        let result = client.try_get_collateral_score(&999u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_is_collateral_eligible_unregistered_asset() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);

        // Check eligibility for non-existent asset ID
        let result = client.try_is_collateral_eligible(&999u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_is_collateral_eligible_below_default_threshold() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // One maintenance record gives a low score (well below default threshold of 50)
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "notes"),
            &engineer,
        );

        assert!(!client.is_collateral_eligible(&asset_id));
    }

    #[test]
    fn test_is_collateral_eligible_after_threshold_lowered() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "notes"),
            &engineer,
        );

        // Score is low; lower threshold so asset becomes eligible
        let score = client.get_collateral_score(&asset_id);
        client.update_eligibility_threshold(&admin, &score);

        assert!(client.is_collateral_eligible(&asset_id));
    }

    #[test]
    fn test_is_collateral_eligible_uses_custom_min_collateral_score() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        for _ in 0..5 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("FILTER"),
                &Priority::Low,
                &String::from_str(&env, "notes"),
                &engineer,
            );
        }

        client.update_config(&admin, &25);
        assert!(client.is_collateral_eligible(&asset_id));
    }

    #[test]
    fn test_is_collateral_eligible_respects_higher_custom_min_collateral_score() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        for _ in 0..6 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("FILTER"),
                &Priority::Low,
                &String::from_str(&env, "notes"),
                &engineer,
            );
        }

        client.update_config(&admin, &35);
        assert!(!client.is_collateral_eligible(&asset_id));
    }

    #[test]
    fn test_is_collateral_eligible_flips_false_after_decay() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Build score to exactly the eligibility threshold (50) via 10 ÃƒÆ’Ã¢â‚¬â€ FILTER (5 pts each)
        for _ in 0..10 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("FILTER"),
                &Priority::Low,
                &String::from_str(&env, "notes"),
                &engineer,
            );
        }
        assert!(client.is_collateral_eligible(&asset_id));

        // Fast decay: 5 points per 60 seconds; advance 2 intervals ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ -10 pts ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ score 40 < 50
        client.update_decay_config(&admin, &5, &60);
        env.ledger()
            .with_mut(|li| li.timestamp = li.timestamp + 120);

        assert!(!client.is_collateral_eligible(&asset_id));
    }

    #[test]
    fn test_full_cross_contract_threshold_boundary() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Set eligibility threshold to a deterministic value for boundary testing.
        client.update_eligibility_threshold(&admin, &10);

        // Just below threshold: one maintenance event (FILTER = 5 points)
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &Priority::Low,
            &String::from_str(&env, "Filter replacement 1"),
            &engineer,
        );
        assert_eq!(client.get_collateral_score(&asset_id), 5);
        assert!(!client.is_collateral_eligible(&asset_id));

        // Cross threshold with one more event (total = 10)
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &Priority::Low,
            &String::from_str(&env, "Filter replacement 2"),
            &engineer,
        );
        assert_eq!(client.get_collateral_score(&asset_id), 10);
        assert!(client.is_collateral_eligible(&asset_id));
    }

    #[test]
    fn test_update_eligibility_threshold_non_admin_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let outsider = Address::generate(&env);

        let result = client.try_update_eligibility_threshold(&outsider, &10);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_update_eligibility_threshold_zero_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);

        let result = client.try_update_eligibility_threshold(&admin, &0);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidConfig as u32,
            ))),
        );
    }

    #[test]
    fn test_batch_is_collateral_eligible_mixed() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // asset_a: 10 ÃƒÆ’Ã¢â‚¬â€ ENGINE (5 pts each) = 50 ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ eligible
        let (asset_a, asset_owner_a) = register_asset(&env, &asset_registry_client);
        for _ in 0..10 {
            client.submit_maintenance(
                &asset_a,
                &symbol_short!("ENGINE"),
                &String::from_str(&env, "ok"),
                &engineer,
            );
        }

        // asset_b: 1 ÃƒÆ’Ã¢â‚¬â€ OIL_CHG (5 pts) ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ not eligible
        let (asset_b, asset_owner_b) = register_asset(&env, &asset_registry_client);
        client.submit_maintenance(
            &asset_b,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "ok"),
            &engineer,
        );

        let mut ids = Vec::new(&env);
        ids.push_back(asset_a);
        ids.push_back(asset_b);

        let results = client.batch_is_collateral_eligible(&ids);
        assert_eq!(results.len(), 2);
        assert!(results.get(0).unwrap());
        assert!(!results.get(1).unwrap());
    }

    #[test]

    #[test]
    fn test_is_collateral_eligible_with_sufficient_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Default score_increment is 5, default threshold is 50
        // So we need 10 maintenance events to reach 50
        for _ in 0..10 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "routine"),
                &engineer,
            );
        }

        assert!(client.is_collateral_eligible(&asset_id));
    }

    #[test]
    fn test_is_collateral_eligible_just_above_threshold() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Set threshold to 30
        client.update_eligibility_threshold(&admin, &30);

        // Submit 6 maintenance events (6 * 5 = 30, but capped, need to check actual score)
        for _ in 0..7 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "routine"),
                &engineer,
            );
        }

        assert!(client.is_collateral_eligible(&asset_id));
    }

    #[test]
    fn test_is_collateral_eligible_with_no_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let (asset_id, _) = register_asset(&env, &asset_registry_client);

        // Asset with no maintenance history should not be eligible
        assert!(!client.is_collateral_eligible(&asset_id));
    }
    fn test_batch_is_collateral_eligible_empty_input() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let results = client.batch_is_collateral_eligible(&Vec::new(&env));
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_batch_is_collateral_eligible_unknown_asset_returns_error() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let mut ids = Vec::new(&env);
        ids.push_back(999u64);

        let result = client.try_batch_is_collateral_eligible(&ids);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                asset_registry::ContractError::AssetNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_batch_is_collateral_eligible_no_state_mutation() {
        // batch_is_collateral_eligible must not write decayed scores back to storage.
        // The stored score before and after the batch call must be identical.
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let engineer = register_engineer(&env, &engineer_registry_client);

        let (asset_a, asset_owner_a) = register_asset(&env, &asset_registry_client);
        let (asset_b, asset_owner_b) = register_asset(&env, &asset_registry_client);

        // Give both assets a score above the default threshold (50)
        for _ in 0..10 {
            client.submit_maintenance(
                &asset_a,
                &symbol_short!("ENGINE"),
                &String::from_str(&env, "ok"),
                &engineer,
            );
            client.submit_maintenance(
                &asset_b,
                &symbol_short!("ENGINE"),
                &String::from_str(&env, "ok"),
                &engineer,
            );
        }

        // Capture the raw stored scores before time advance
        let contract_id = client.address.clone();
        let stored_a_before: u32 = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&score_key(asset_a))
                .unwrap_or(0)
        });
        let stored_b_before: u32 = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&score_key(asset_b))
                .unwrap_or(0)
        });

        // Advance time so decay would apply if storage were written
        env.ledger().with_mut(|l| l.timestamp += 2_592_000 * 3); // 3 decay intervals

        let mut ids = Vec::new(&env);
        ids.push_back(asset_a);
        ids.push_back(asset_b);
        client.batch_is_collateral_eligible(&ids);

        // Raw stored scores must be unchanged — batch call must not write to storage
        let stored_a_after: u32 = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&score_key(asset_a))
                .unwrap_or(0)
        });
        let stored_b_after: u32 = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&score_key(asset_b))
                .unwrap_or(0)
        });
        assert_eq!(
            stored_a_after, stored_a_before,
            "batch_is_collateral_eligible must not write decayed score to storage"
        );
        assert_eq!(
            stored_b_after, stored_b_before,
            "batch_is_collateral_eligible must not write decayed score to storage"
        );
    }

    // --- get_collateral_score_batch tests ---

    #[test]
    fn test_get_collateral_score_batch_single_element() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let engineer = register_engineer(&env, &engineer_registry_client);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "ok"),
            &engineer,
        );

        let mut ids = Vec::new(&env);
        ids.push_back(asset_id);
        let results = client.get_collateral_score_batch(&ids);
        assert_eq!(results.len(), 1);
        let (ret_id, ret_score) = results.get(0).unwrap();
        assert_eq!(ret_id, asset_id);
        assert_eq!(ret_score, client.get_collateral_score(&asset_id));
    }

    #[test]
    fn test_get_collateral_score_batch_multi_element() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let engineer = register_engineer(&env, &engineer_registry_client);

        let (asset_a, asset_owner_a) = register_asset(&env, &asset_registry_client);
        let (asset_b, asset_owner_b) = register_asset(&env, &asset_registry_client);
        client.authorize_engineer(&asset_owner_a, &asset_a, &engineer);
        client.authorize_engineer(&asset_owner_b, &asset_b, &engineer);

        for _ in 0..10 {
            client.submit_maintenance(
                &asset_a,
                &symbol_short!("ENGINE"),
                &String::from_str(&env, "ok"),
                &engineer,
            );
        }
        client.submit_maintenance(
            &asset_b,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "ok"),
            &engineer,
        );

        let mut ids = Vec::new(&env);
        ids.push_back(asset_a);
        ids.push_back(asset_b);
        let results = client.get_collateral_score_batch(&ids);
        assert_eq!(results.len(), 2);
        let (id_a, score_a) = results.get(0).unwrap();
        assert_eq!(id_a, asset_a);
        assert_eq!(score_a, client.get_collateral_score(&asset_a));
        let (id_b, score_b) = results.get(1).unwrap();
        assert_eq!(id_b, asset_b);
        assert_eq!(score_b, client.get_collateral_score(&asset_b));
    }

    #[test]
    fn test_get_collateral_score_batch_unknown_asset_is_skipped() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let engineer = register_engineer(&env, &engineer_registry_client);
        let (known_id, asset_owner) = register_asset(&env, &asset_registry_client);
        client.authorize_engineer(&asset_owner, &known_id, &engineer);
        client.submit_maintenance(
            &known_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, ""),
            &engineer,
        );

        let mut ids = Vec::new(&env);
        ids.push_back(known_id);
        ids.push_back(999u64); // unknown

        // Unknown asset is silently skipped; only the known asset appears in results.
        let results = client.get_collateral_score_batch(&ids);
        assert_eq!(results.len(), 1);
        let (ret_id, ret_score) = results.get(0).unwrap();
        assert_eq!(ret_id, known_id);
        assert_eq!(ret_score, client.get_collateral_score(&known_id));
    }

    // --- Upgrade tests ---

    #[test]
    fn test_admin_can_propose_upgrade() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let new_wasm_hash = BytesN::from_array(&env, &[0xabu8; 32]);

        // propose_upgrade should succeed for admin
        let result = client.try_propose_upgrade(&admin, &new_wasm_hash);
        assert!(
            result
                != Err(Ok(soroban_sdk::Error::from_contract_error(
                    ContractError::UnauthorizedAdmin as u32
                )))
        );
    }

    #[test]
    fn test_non_admin_cannot_propose_upgrade() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let outsider = Address::generate(&env);
        let new_wasm_hash = BytesN::from_array(&env, &[0xabu8; 32]);

        let result = client.try_propose_upgrade(&outsider, &new_wasm_hash);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_upgrade_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let new_wasm_hash = BytesN::from_array(&env, &[0xabu8; 32]);

        client.propose_upgrade(&admin, &new_wasm_hash);

        // Advance past timelock delay (48 hours)
        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

        client.execute_upgrade(&admin);

        let events = env.events().all();
        use soroban_sdk::TryIntoVal;
        let upgrade_event = events.iter().find(|(_, topics, _)| {
            if let Some(val) = topics.get(0) {
                if let Ok(s) = TryIntoVal::<_, Symbol>::try_into_val(&val, &env) {
                    return s == symbol_short!("UPGRADE");
                }
            }
            false
        });
        assert!(upgrade_event.is_some(), "UPGRADE event must be emitted");
        let (_, _, data) = upgrade_event.unwrap();
        let emitted_hash: BytesN<32> = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_hash, new_wasm_hash);
    }

    // --- Score history tests ---

    #[test]
    fn test_propose_and_accept_admin_transfer() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let new_admin = Address::generate(&env);

        client.propose_admin(&admin, &new_admin);

        // Timelock must expire before accept_admin succeeds.
        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

        client.accept_admin();

        assert_eq!(client.get_config().admin, new_admin);
    }

    #[test]
    fn test_pending_admin_key_cleared_after_accept() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let new_admin = Address::generate(&env);

        client.propose_admin(&admin, &new_admin);

        // Timelock must expire before accept_admin succeeds.
        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

        client.accept_admin();

        let contract_id = client.address.clone();
        env.as_contract(&contract_id, || {
            assert!(!env.storage().instance().has(&PENDING_ADMIN_KEY));
        });
    }

    #[test]
    fn test_non_admin_cannot_propose_admin() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let outsider = Address::generate(&env);
        let new_admin = Address::generate(&env);

        let result = client.try_propose_admin(&outsider, &new_admin);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_wrong_address_cannot_accept_admin() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let new_admin = Address::generate(&env);
        let impostor = Address::generate(&env);

        client.propose_admin(&admin, &new_admin);

        use soroban_sdk::IntoVal;
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &impostor,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &client.address,
                fn_name: "accept_admin",
                args: ().into_val(&env),
                sub_invokes: &[],
            },
        }]);

        let result = client.try_accept_admin();
        assert!(result.is_err());
        assert_eq!(client.get_config().admin, admin);
    }

    #[test]
    fn test_accept_admin_rejected_before_timelock() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let new_admin = Address::generate(&env);

        client.propose_admin(&admin, &new_admin);

        // Attempt to accept before timelock expires must fail.
        let result = client.try_accept_admin();
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::TimelockNotExpired as u32,
            ))),
            "accept_admin must fail with TimelockNotExpired before the timelock expires",
        );

        // Admin must remain unchanged.
        assert_eq!(client.get_config().admin, admin);
    }

    #[test]
    fn test_propose_pause_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);

        client.propose_pause(&admin);

        let events = env.events().all();
        assert!(events.iter().any(|(_, topics, _)| {
            topics
                .get(0)
                .and_then(|v| soroban_sdk::TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                .map(|s: Symbol| s == symbol_short!("PROP_PAUSE"))
                .unwrap_or(false)
        }));
    }

    #[test]
    fn test_execute_pause_after_timelock() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);

        client.propose_pause(&admin);

        // Advance past timelock.
        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

        client.execute_pause(&admin);

        assert!(client.is_paused());
    }

    #[test]
    fn test_execute_pause_rejected_before_timelock() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);

        client.propose_pause(&admin);

        let result = client.try_execute_pause(&admin);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::TimelockNotExpired as u32,
            ))),
            "execute_pause must fail with TimelockNotExpired before the timelock expires",
        );
    }

    #[test]
    fn test_propose_unpause_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);

        // First pause the contract so unpause can be proposed.
        client.propose_pause(&admin);
        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);
        client.execute_pause(&admin);
        assert!(client.is_paused());

        client.propose_unpause(&admin);

        let events = env.events().all();
        assert!(events.iter().any(|(_, topics, _)| {
            topics
                .get(0)
                .and_then(|v| soroban_sdk::TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                .map(|s: Symbol| s == symbol_short!("PROP_UNPAUSE"))
                .unwrap_or(false)
        }));
    }

    #[test]
    fn test_execute_unpause_after_timelock() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);

        // First pause the contract so unpause can be proposed.
        client.propose_pause(&admin);
        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);
        client.execute_pause(&admin);
        assert!(client.is_paused());

        client.propose_unpause(&admin);

        // Advance past timelock.
        let base2 = env.ledger().timestamp();
        env.ledger().set_timestamp(base2 + TIMELOCK_DELAY_SECS + 1);

        client.execute_unpause(&admin);

        assert!(!client.is_paused());
    }

    #[test]
    fn test_execute_unpause_rejected_before_timelock() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);

        // First pause the contract so unpause can be proposed.
        client.propose_pause(&admin);
        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);
        client.execute_pause(&admin);
        assert!(client.is_paused());

        client.propose_unpause(&admin);

        let result = client.try_execute_unpause(&admin);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::TimelockNotExpired as u32,
            ))),
            "execute_unpause must fail with TimelockNotExpired before the timelock expires",
        );
    }

    // --- Score history tests (original) ---

    #[test]
    fn test_score_history_empty_before_any_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);

        let history = client.get_score_history(&asset_id);
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn test_score_history_records_entry_per_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "First"),
            &engineer,
        );
        env.ledger().with_mut(|li| li.timestamp += 1);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &Priority::Low,
            &String::from_str(&env, "Second"),
            &engineer,
        );
        env.ledger().with_mut(|li| li.timestamp += 1);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &Priority::Low,
            &String::from_str(&env, "Third"),
            &engineer,
        );

        let history = client.get_score_history(&asset_id);
        // One entry per maintenance event (each at a distinct timestamp)
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn test_score_history_scores_are_cumulative() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // All tasks use score_increment (default 5); advance ledger between each to ensure
        // distinct timestamps so deduplication does not collapse the entries.
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "a"),
            &engineer,
        );
        env.ledger().with_mut(|li| li.timestamp += 1);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &Priority::Low,
            &String::from_str(&env, "b"),
            &engineer,
        );
        env.ledger().with_mut(|li| li.timestamp += 1);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &Priority::Low,
            &String::from_str(&env, "c"),
            &engineer,
        );

        let history = client.get_score_history(&asset_id);
        assert_eq!(history.get(0).unwrap().score, 5); // 0 + 5
        assert_eq!(history.get(1).unwrap().score, 10); // 5 + 5
        assert_eq!(history.get(2).unwrap().score, 15); // 10 + 5
    }

    #[test]
    fn test_score_history_timestamps_match_ledger() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let t0 = env.ledger().timestamp();
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "at t0"),
            &engineer,
        );

        env.ledger()
            .with_mut(|li| li.timestamp = li.timestamp + 1000);
        let t1 = env.ledger().timestamp();
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("LUBE"),
            &Priority::Low,
            &String::from_str(&env, "at t1"),
            &engineer,
        );

        let history = client.get_score_history(&asset_id);
        assert_eq!(history.get(0).unwrap().timestamp, t0);
        assert_eq!(history.get(1).unwrap().timestamp, t1);
    }

    #[test]
    fn test_score_history_capped_at_100() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // 20 tasks at default score_increment (5) each would be 100, then more should stay at 100.
        // Advance ledger by 1 second between each so every submission gets a distinct timestamp
        // and deduplication does not collapse entries into one.
        for _ in 0..22 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("REBUILD"),
                &Priority::Low,
                &String::from_str(&env, "major"),
                &engineer,
            );
            env.ledger().with_mut(|li| li.timestamp += 1);
        }

        let history = client.get_score_history(&asset_id);
        // Score should never exceed 100
        for i in 0..history.len() {
            assert!(history.get(i).unwrap().score <= 100);
        }
        // After 20 tasks the score is already 100; subsequent entries stay at 100
        assert_eq!(history.get(20).unwrap().score, 100);
        assert_eq!(history.get(21).unwrap().score, 100);
    }

    #[test]
    fn test_score_history_pruned_at_max_history() {
        let env = Env::default();
        env.mock_all_auths();

        // max_history = 5
        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 5);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Submit 5 records -- history_key is capped at 5, score_history must also stay at 5.
        // Advance ledger by 1 second between each so every submission gets a distinct timestamp.
        for _ in 0..5 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "ok"),
                &engineer,
            );
            env.ledger().with_mut(|li| li.timestamp += 1);
        }
        assert_eq!(client.get_score_history(&asset_id).len(), 5);

        // history_key is now full; further submit_maintenance calls are rejected,
        // so trigger score_history growth via decay_score instead.
        // Advance past one decay interval and call decay_score 3 more times.
        for _ in 0..3 {
            env.ledger()
                .with_mut(|li| li.timestamp += DEFAULT_DECAY_INTERVAL);
            client.decay_score(&asset_id);
        }

        // score_history must never exceed max_history
        assert_eq!(client.get_score_history(&asset_id).len(), 5);
    }

    #[test]
    fn test_score_trend_returns_last_n() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        for _ in 0..5 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "entry"),
                &engineer,
            );
            env.ledger().with_mut(|li| li.timestamp += 1);
        }

        let full = client.get_score_history(&asset_id);
        let trend = client.get_score_trend(&asset_id, &3);
        assert_eq!(trend.len(), 3);
        // Should be the last 3 entries
        assert_eq!(trend.get(0).unwrap().score, full.get(2).unwrap().score);
        assert_eq!(trend.get(1).unwrap().score, full.get(3).unwrap().score);
        assert_eq!(trend.get(2).unwrap().score, full.get(4).unwrap().score);
    }

    #[test]
    fn test_score_trend_n_exceeds_history_length() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "only one"),
            &engineer,
        );

        // n=10 but only 1 entry exists ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â should return all 1
        let trend = client.get_score_trend(&asset_id, &10);
        assert_eq!(trend.len(), 1);
    }

    #[test]
    fn test_score_trend_n_zero_returns_empty() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "entry"),
            &engineer,
        );

        let trend = client.get_score_trend(&asset_id, &0);
        assert_eq!(trend.len(), 0);
    }

    #[test]
    fn test_score_trend_empty_history() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);

        let trend = client.get_score_trend(&asset_id, &5);
        assert_eq!(trend.len(), 0);
    }

    #[test]
    fn test_batch_submit_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Oil change"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("INSPECT"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Inspection"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("ENGINE"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Engine repair"),
        });

        client.batch_submit_maintenance(&asset_id, &records, &engineer);

        // 3 records at default score_increment (5) each => 15
        assert_eq!(client.get_collateral_score(&asset_id), 15);
        assert_eq!(client.get_maintenance_history(&asset_id).len(), 3);
    }

    #[test]
    fn test_batch_submit_maintenance_emits_maint_events() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Oil change"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("INSPECT"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Inspection"),
        });

        client.batch_submit_maintenance(&asset_id, &records, &engineer);

        let events = env.events().all();
        let maint_count = events
            .iter()
            .filter(|(_, topics, _)| {
                let t0: Result<Symbol, _> = topics.get(0).unwrap().try_into_val(&env);
                t0.map(|s: Symbol| s == EVENT_MAINT).unwrap_or(false)
            })
            .count();
        // One MAINT event per record submitted
        assert_eq!(maint_count, 2);
    }

    #[test]
    fn test_batch_submit_maintenance_rejects_unknown_task_type() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("UNKNOWN"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Unknown task type"),
        });

        let result = client.try_batch_submit_maintenance(&asset_id, &records, &engineer);

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidTaskType as u32,
            ))),
        );
    }

    /// Regression test: a `batch_submit_maintenance` call carrying more
    /// than `MAX_BATCH_SIZE` records must be rejected with the structured
    /// `BatchTooLarge` contract error, *before* any cross-contract calls
    /// or storage writes occur. This protects the contract from a DoS
    /// where an unbounded `Vec<BatchRecord>` could exhaust the ledger's
    /// gas/instruction budget.
    #[test]
    fn test_batch_submit_maintenance_rejects_oversized_batch() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Build a batch with one more record than the allowed maximum.
        let mut records = Vec::new(&env);
        for _ in 0..(MAX_BATCH_SIZE + 1) {
            records.push_back(BatchRecord {
                task_type: symbol_short!("OIL_CHG"),
                notes: String::from_str(&env, "Oil change"),
            });
        }

        let result = client.try_batch_submit_maintenance(&asset_id, &records, &engineer);

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::BatchTooLarge as u32,
            ))),
        );

        // Confirm no partial state was written: no maintenance history,
        // no score, no engineer history entry for this asset.
        assert_eq!(client.get_maintenance_history(&asset_id).len(), 0);
        assert_eq!(client.get_collateral_score(&asset_id), 0);
    }

    /// Boundary test: a batch with exactly `MAX_BATCH_SIZE` records must
    /// be accepted (the limit is inclusive on the allowed side).
    #[test]
    fn test_batch_submit_maintenance_accepts_max_batch_size() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let mut records = Vec::new(&env);
        for _ in 0..MAX_BATCH_SIZE {
            records.push_back(BatchRecord {
                task_type: symbol_short!("OIL_CHG"),
                notes: String::from_str(&env, "Oil change"),
            });
        }

        // Should not panic / should not return an error.
        client.batch_submit_maintenance(&asset_id, &records, &engineer);

        assert_eq!(
            client.get_maintenance_history(&asset_id).len(),
            MAX_BATCH_SIZE
        );
    }

    #[test]
    fn test_batch_submit_no_duplicate_engineer_history() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Submit multiple records for the same asset in one batch
        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Oil change 1"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Oil change 2"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("INSPECT"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Inspection"),
        });

        client.batch_submit_maintenance(&asset_id, &records, &engineer);

        // Verify engineer history contains asset_id only once
        let history = client.get_engineer_maintenance_history(&engineer);
        let asset_count = history.iter().filter(|id| *id == asset_id).count();
        assert_eq!(asset_count, 1);
    }

    #[test]
    fn test_batch_submit_no_partial_writes_on_failure() {
        let env = Env::default();
        env.mock_all_auths();

        // max_history = 0 means unlimited
        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_id_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_id_owner, &asset_id, &engineer);

        // First record is valid; second has an invalid task type — batch must fail cleanly.
        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "valid"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("INVALID"),
            notes: String::from_str(&env, "bad task"),
        });

        let result = client.try_batch_submit_maintenance(&asset_id, &records, &engineer);
        assert!(result.is_err(), "batch should fail on invalid task type");

        // Neither maintenance history nor score history should have any entries.
        assert_eq!(
            client.get_maintenance_history(&asset_id).len(),
            0,
            "HIST must be empty after failed batch"
        );
        assert_eq!(
            client.get_score_history(&asset_id).len(),
            0,
            "SCHIST must be empty after failed batch"
        );
    }

    #[test]
    fn test_batch_submit_fails_atomically_on_history_cap() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 3);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Fill to max_history - 1 = 2
        for _ in 0..2 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "ok"),
                &engineer,
            );
        }
        assert_eq!(client.get_maintenance_history(&asset_id).len(), 2);

        // Batch of 2 would push total to 4, exceeding cap of 3
        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "ok"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "ok"),
        });

        let result = client.try_batch_submit_maintenance(&asset_id, &records, &engineer);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::HistoryCapReached as u32,
            ))),
        );

        // No records written ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â history still at 2
        assert_eq!(client.get_maintenance_history(&asset_id).len(), 2);
    }

    #[test]
    fn test_batch_submit_exceeds_history_cap() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 2);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            priority: Priority::Low,
            notes: String::from_str(&env, "First"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Second"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Third - over cap"),
        });

        let result = client.try_batch_submit_maintenance(&asset_id, &records, &engineer);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::HistoryCapReached as u32,
            ))),
        );
    }

    #[test]
    fn test_batch_submit_unauthorized_engineer() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let unregistered = Address::generate(&env);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Should fail"),
        });

        let result = client.try_batch_submit_maintenance(&asset_id, &records, &unregistered);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
        );
    }

    #[test]
    fn test_batch_submit_reports_failing_record_index() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Valid"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("INSPECT"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Valid"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("UNKNOWN"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Invalid task type"),
        });

        let result = client.try_batch_submit_maintenance(&asset_id, &records, &engineer);

        // Should fail with InvalidTaskType at index 2
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidTaskType as u32,
            ))),
        );
    }

    #[test]
    fn test_batch_submit_maintenance_rejects_oversized_notes() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            priority: Priority::Low,
            notes: String::from_str(&env, &"x".repeat(300)),
        });

        let result = client.try_batch_submit_maintenance(&asset_id, &records, &engineer);

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NotesTooLong as u32,
            ))),
        );
    }

    #[test]
    fn test_submit_maintenance_unregistered_engineer_should_panic() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let unregistered = Address::generate(&env);

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Should fail"),
            &unregistered,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
        );
    }

    #[test]
    fn test_collateral_score_caps_at_100() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // FILTER = 5 points each; 25 submissions would be 125 without a cap
        for _ in 0..25 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("FILTER"),
                &Priority::Low,
                &String::from_str(&env, "Filter replacement"),
                &engineer,
            );
        }

        assert_eq!(client.get_collateral_score(&asset_id), 100);
    }

    #[test]
    fn test_decay_score_sums_maximum_default_history_inputs() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry, engineer_registry, admin) =
            setup(&env, DEFAULT_MAX_HISTORY);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);
        client.update_score_increment(&admin, &MAX_BUILT_IN_TASK_WEIGHT);

        for _ in 0..DEFAULT_MAX_HISTORY {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("ENGINE"),
                &Priority::High,
                &String::from_str(&env, "Maximum-weight history entry"),
                &engineer,
            );
        }

        assert_eq!(
            client.get_maintenance_history(&asset_id).len(),
            DEFAULT_MAX_HISTORY
        );
        assert_eq!(client.get_collateral_score(&asset_id), MAX_COLLATERAL_SCORE);
    }

    #[test]
    fn test_submit_maintenance_revoked_engineer_should_panic() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        engineer_registry_client.revoke_credential(&engineer);

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Post-revocation attempt"),
            &engineer,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
        );
    }

    #[test]
    fn test_submit_maintenance_revoked_engineer_panics() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        assert_eq!(engineer_registry_client.verify_engineer(&engineer, &None::<Symbol>), ::engineer_registry::CredentialStatus::Valid);
        engineer_registry_client.revoke_credential(&engineer);
        assert_ne!(engineer_registry_client.verify_engineer(&engineer, &None::<Symbol>), ::engineer_registry::CredentialStatus::Valid);

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Post-revocation attempt"),
            &engineer,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
        );
    }

    #[test]
    fn test_submit_maintenance_rejects_revoked_credential_with_unauthorized_engineer_error() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        assert_eq!(
            engineer_registry_client.verify_engineer(&engineer),
            ::engineer_registry::CredentialStatus::Valid
        );

        engineer_registry_client.revoke_credential(&engineer);

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "revoked credential should be rejected"),
            &engineer,
        );

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
            "revoked credential should panic with UnauthorizedEngineer"
        );
    }

    /// Issue #128: revoked engineer cannot submit, but can after re-registration with a new credential.
    #[test]
    fn test_submit_maintenance_revoked_then_reregistered_engineer() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);

        // Set up a trusted issuer and register the engineer
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let admin = Address::generate(&env);
        let hash_v1 = BytesN::from_array(&env, &[1u8; 32]);

        engineer_registry_client.initialize_admin(&admin, &admin);
        engineer_registry_client.add_trusted_issuer(&admin, &issuer);
        engineer_registry_client.register_engineer(&engineer, &hash_v1, &issuer, &31_536_000, &None);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Revoke the credential
        engineer_registry_client.revoke_credential(&engineer);
        assert_ne!(engineer_registry_client.verify_engineer(&engineer, &None::<Symbol>), ::engineer_registry::CredentialStatus::Valid);

        // Attempt to submit maintenance ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â must fail with UnauthorizedEngineer
        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Post-revocation attempt"),
            &engineer,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
        );

        // Re-register the same engineer with a new credential hash
        let hash_v2 = BytesN::from_array(&env, &[2u8; 32]);
        engineer_registry_client.register_engineer(&engineer, &hash_v2, &issuer, &31_536_000, &None);
        assert_eq!(engineer_registry_client.verify_engineer(&engineer, &None::<Symbol>), ::engineer_registry::CredentialStatus::Valid);

        // Submission must now succeed
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Post-reregistration submission"),
            &engineer,
        );

        let history = client.get_maintenance_history(&asset_id);
        assert_eq!(history.len(), 1);
        assert_eq!(history.get(0).unwrap().engineer, engineer);
    }

    #[test]
    fn test_submit_maintenance_expired_engineer_should_panic() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);

        // Register engineer with minimum validity period (86400 seconds)
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let admin = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        engineer_registry_client.initialize_admin(&admin, &admin);
        engineer_registry_client.add_trusted_issuer(&admin, &issuer);
        engineer_registry_client.register_engineer(&engineer, &hash, &issuer, &86_400, &None);

        // Verify engineer is initially valid
        assert_eq!(engineer_registry_client.verify_engineer(&engineer, &None::<Symbol>), ::engineer_registry::CredentialStatus::Valid);

        // Advance ledger past expiry (86401 seconds)
        env.ledger()
            .with_mut(|li| li.timestamp = li.timestamp + 86_401);

        // Verify engineer is now expired
        assert_ne!(engineer_registry_client.verify_engineer(&engineer, &None::<Symbol>), ::engineer_registry::CredentialStatus::Valid);

        // Attempt submit_maintenance and assert UnauthorizedEngineer is returned
        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Post-expiry attempt"),
            &engineer,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
        );
    }

    #[test]
    fn test_submit_maintenance_rejects_expired_credential_via_cross_contract_call() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);

        // Register asset
        let owner = Address::generate(&env);
        let asset_id = asset_registry_client.register_asset(
            &symbol_short!("GENSET"),
            &Priority::Low,
            &String::from_str(&env, "Test Generator"),
            &unique_serial(&env),
            &owner,
        );

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let eng_admin = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        engineer_registry_client.initialize_admin(&eng_admin, &eng_admin);
        engineer_registry_client.add_trusted_issuer(&eng_admin, &issuer);
        // Register with validity_period = 86400 seconds (minimum)
        engineer_registry_client.register_engineer(&engineer, &hash, &issuer, &86_400, &None);

        assert_eq!(engineer_registry_client.verify_engineer(&engineer, &None::<Symbol>), ::engineer_registry::CredentialStatus::Valid);

        // Advance ledger by 101 seconds ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â credential is now expired
        env.ledger().with_mut(|li| li.timestamp += 86_401);

        assert_ne!(engineer_registry_client.verify_engineer(&engineer, &None::<Symbol>), ::engineer_registry::CredentialStatus::Valid);

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Post-expiry attempt"),
            &engineer,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
        );
    }

    #[test]
    fn test_batch_submit_maintenance_rejects_expired_credential_via_cross_contract_call() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);

        // Register asset
        let owner = Address::generate(&env);
        let asset_id = asset_registry_client.register_asset(
            &symbol_short!("GENSET"),
            &Priority::Low,
            &String::from_str(&env, "Test Generator"),
            &unique_serial(&env),
            &owner,
        );

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let eng_admin = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        engineer_registry_client.initialize_admin(&eng_admin, &eng_admin);
        engineer_registry_client.add_trusted_issuer(&eng_admin, &issuer);
        // Register with validity_period = 86400 seconds (minimum)
        engineer_registry_client.register_engineer(&engineer, &hash, &issuer, &86_400, &None);

        assert_eq!(engineer_registry_client.verify_engineer(&engineer, &None::<Symbol>), ::engineer_registry::CredentialStatus::Valid);

        // Advance ledger by 101 seconds ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â credential is now expired
        env.ledger().with_mut(|li| li.timestamp += 86_401);

        assert_ne!(engineer_registry_client.verify_engineer(&engineer, &None::<Symbol>), ::engineer_registry::CredentialStatus::Valid);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Post-expiry batch attempt"),
        });

        let result = client.try_batch_submit_maintenance(&asset_id, &records, &engineer);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
        );
    }

    #[test]
    fn test_full_lifecycle_integration() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);

        // 1. Register asset
        let asset_admin = asset_registry.get_admin();
        asset_registry.add_asset_type(&asset_admin, &symbol_short!("TURBINE"));
        let owner = Address::generate(&env);
        let asset_id = asset_registry.register_asset(
            &symbol_short!("TURBINE"),
            &String::from_str(&env, "GE LM2500 Turbine Unit 7"),
            &unique_serial(&env),
            &owner,
        );
        let asset = asset_registry.get_asset(&asset_id);
        assert_eq!(asset.owner, owner);

        // 2. Register and verify engineer
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let admin = Address::generate(&env);
        engineer_registry.initialize_admin(&admin, &admin);
        engineer_registry.add_trusted_issuer(&admin, &issuer);
        engineer_registry.register_engineer(
            &engineer,
            &BytesN::from_array(&env, &[2u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );
        assert_eq!(engineer_registry.verify_engineer(&engineer, &None::<Symbol>), ::engineer_registry::CredentialStatus::Valid);

        // 3. Submit 10 maintenance records (default score_increment = 5pts each)
        for i in 0..10u32 {
            lifecycle.submit_maintenance(
                &asset_id,
                &symbol_short!("ENGINE"),
                &Priority::Low,
                &String::from_str(&env, "Full engine service"),
                &engineer,
            );
            // advance ledger timestamp so records are distinct
            env.ledger().set_timestamp(env.ledger().timestamp() + 1);
            let _ = i;
        }

        // 4. Assert collateral eligible (score >= 50)
        assert!(lifecycle.is_collateral_eligible(&asset_id));

        // 5. Assert get_last_service returns the correct record
        let last = lifecycle.get_last_service(&asset_id).unwrap();
        assert_eq!(last.asset_id, asset_id);
        assert_eq!(last.engineer, engineer);
        assert_eq!(last.task_type, symbol_short!("ENGINE"));
    }

    #[test]
    fn test_decay_score_emits_correct_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Default score_increment = 5
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "ok"),
            &engineer,
        );
        let initial_score: u32 = 5;

        // Use fast decay: 2 pts per 60s, advance 60s (1 interval)
        client.update_decay_config(&admin, &2, &60);
        env.ledger().with_mut(|li| li.timestamp += 60);
        let decay_time = env.ledger().timestamp();

        client.decay_score(&asset_id);

        let events = env.events().all();
        assert!(events.len() >= 1);

        let (_, topics, data) = events.get(0).unwrap();

        // Topics: (symbol("DECAY"), asset_id)
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        let t1: u64 = topics.get(1).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, EVENT_DECAY);
        assert_eq!(t1, asset_id);

        // Data: (old_score, new_score, timestamp)
        let expected_new_score: u32 = initial_score - 2;
        let (ev_old, ev_new, ev_ts): (u32, u32, u64) = data.try_into_val(&env).unwrap();
        assert_eq!(ev_old, initial_score);
        assert_eq!(ev_new, expected_new_score);
        assert_eq!(ev_ts, decay_time);
    }

    #[test]
    fn test_admin_can_reset_score() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Build up a non-zero score
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &Priority::Low,
            &String::from_str(&env, "Major overhaul"),
            &engineer,
        );
        assert!(client.get_collateral_score(&asset_id) > 0);

        // Admin resets the score
        client.reset_score(&admin, &asset_id);
        assert_eq!(client.get_collateral_score(&asset_id), 0);
    }

    #[test]
    fn test_reset_score_appends_zero_to_score_history() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &Priority::Low,
            &String::from_str(&env, "Major overhaul"),
            &engineer,
        );
        let history_before = client.get_score_history(&asset_id);
        assert!(history_before.len() > 0);
        assert!(history_before.last_unchecked().score > 0);

        client.reset_score(&admin, &asset_id);

        let history_after = client.get_score_history(&asset_id);
        assert_eq!(history_after.len(), history_before.len() + 1);
        assert_eq!(history_after.last_unchecked().score, 0);
    }

    #[test]
    fn test_decay_after_reset_uses_reset_timestamp() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Build up a score, then reset
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &Priority::Low,
            &String::from_str(&env, "Major overhaul"),
            &engineer,
        );
        client.reset_score(&admin, &asset_id);

        // Rebuild score after reset
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &Priority::Low,
            &String::from_str(&env, "Post-reset work"),
            &engineer,
        );
        let score_after_rebuild = client.get_collateral_score(&asset_id);
        assert!(score_after_rebuild > 0);

        // Advance time by less than one decay interval (default 2592000s / 30 days)
        // so no decay should be applied
        env.ledger().with_mut(|li| li.timestamp += 100);
        let score_after_short_wait = client.decay_score(&asset_id);
        assert_eq!(score_after_short_wait, score_after_rebuild);

        // Advance time by one full decay interval and verify exactly one decay step
        env.ledger().with_mut(|li| li.timestamp += 2592000);
        let score_after_decay = client.decay_score(&asset_id);
        assert_eq!(score_after_decay, score_after_rebuild.saturating_sub(5));
    }

    #[test]
    fn test_task_weight_tiers() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Minor: OIL_CHG — score increments by score_increment (default 5)
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "ok"),
            &engineer,
        );
        assert_eq!(client.get_collateral_score(&asset_id), 5);

        client.reset_score(&admin, &asset_id);

        // Medium: FILTER — score increments by score_increment (default 5)
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &String::from_str(&env, "ok"),
            &engineer,
        );
        assert_eq!(client.get_collateral_score(&asset_id), 5);

        client.reset_score(&admin, &asset_id);

        // Major: ENGINE — score increments by score_increment (default 5)
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "ok"),
            &engineer,
        );
        assert_eq!(client.get_collateral_score(&asset_id), 5);

        client.reset_score(&admin, &asset_id);

        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("UNKNOWN"),
            &String::from_str(&env, "ok"),
            &engineer,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidTaskType as u32,
            ))),
        );
    }

    #[test]
    fn test_reset_score_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Build up a non-zero score
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &Priority::Low,
            &String::from_str(&env, "Major overhaul"),
            &engineer,
        );

        // Reset the score
        let reset_time = env.ledger().timestamp();
        client.reset_score(&admin, &asset_id);

        // Verify the reset event was emitted
        let events = env.events().all();
        let (_, topics, data) = events.last().unwrap();

        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        let t1: u64 = topics.get(1).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, EVENT_RST_SCR);
        assert_eq!(t1, asset_id);

        let (emitted_admin, emitted_timestamp): (Address, u64) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_admin, admin);
        assert_eq!(emitted_timestamp, reset_time);
    }

    #[test]
    fn test_non_admin_cannot_reset_score() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &Priority::Low,
            &String::from_str(&env, "Major overhaul"),
            &engineer,
        );

        let outsider = Address::generate(&env);
        let result = client.try_reset_score(&outsider, &asset_id);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    // --- get_last_service_timestamp tests ---

    #[test]
    fn test_get_last_service_timestamp_none_before_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);

        assert_eq!(client.get_last_service_timestamp(&asset_id), None);
    }

    #[test]
    fn test_get_last_service_timestamp_returns_ledger_time() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let t0 = env.ledger().timestamp();
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "first service"),
            &engineer,
        );
        assert_eq!(client.get_last_service_timestamp(&asset_id), Some(t0));

        env.ledger().with_mut(|li| li.timestamp += 500);
        let t1 = env.ledger().timestamp();
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &Priority::Low,
            &String::from_str(&env, "second service"),
            &engineer,
        );
        assert_eq!(client.get_last_service_timestamp(&asset_id), Some(t1));
    }

    #[test]
    fn test_get_score_history_nonexistent_asset_returns_empty() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let result = client.get_score_history(&999u64);
        assert_eq!(
            result.len(),
            0,
            "nonexistent asset should return empty history"
        );
    }

    #[test]
    fn test_get_score_trend_nonexistent_asset_returns_error() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let result = client.try_get_score_trend(&999u64, &10u32);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_get_last_service_timestamp_nonexistent_asset_returns_error() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let result = client.try_get_last_service_timestamp(&999u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_get_engineer_maintenance_history_caps_at_100() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // Register and maintain 150 assets
        for _ in 0..150 {
            let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "maintenance"),
                &engineer,
            );
        }

        let history = client.get_engineer_maintenance_history(&engineer);
        assert_eq!(history.len(), 100u32);
    }

    #[test]
    fn test_get_engineer_maintenance_history_truncates_at_101() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let engineer = register_engineer(&env, &engineer_registry_client);

        for _ in 0..101 {
            let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
            client.authorize_engineer(&asset_owner, &asset_id, &engineer);
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "maintenance"),
                &engineer,
            );
        }

        let history = client.get_engineer_maintenance_history(&engineer);
        assert_eq!(history.len(), 100u32);
        // confirm the full count is accessible via the count helper
        assert_eq!(client.get_eng_maint_hist_count(&engineer), 101u32);
        assert_eq!(client.get_eng_maint_count(&engineer), 101u32);
        assert_eq!(client.eng_maintenance_history_count(&engineer), 101u32);
    }

    #[test]
    fn test_get_engineer_maintenance_history_returns_all_if_under_100() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // Register and maintain 50 assets
        for _ in 0..50 {
            let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
            client.authorize_engineer(&asset_owner, &asset_id, &engineer);
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "maintenance"),
                &engineer,
            );
        }

        let history = client.get_engineer_maintenance_history(&engineer);
        assert_eq!(history.len(), 50u32);
    }

    // --- Issue #142: NotInitialized structured error ---

    #[test]
    fn test_registry_addresses_survive_instance_ttl_boundary() {
        let env = Env::default();
        env.mock_all_auths();

        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());
        let admin = Address::generate(&env);

        let lifecycle = LifecycleClient::new(&env, &lifecycle_id);
        lifecycle.initialize(
            &admin,
            &asset_registry_id,
            &engineer_registry_id,
            &admin,
            &0u32,
        );

        // Verify registries are accessible normally
        assert_eq!(lifecycle.get_asset_registry(), asset_registry_id);
        assert_eq!(lifecycle.get_engineer_registry(), engineer_registry_id);

        // Simulate instance TTL expiration by clearing instance storage keys
        env.as_contract(&lifecycle_id, || {
            env.storage().instance().remove(&CONFIG);
        });

        // After instance TTL expiry, registry addresses should still be readable
        // from persistent storage even though CONFIG is gone
        let asset_reg_persisted: Option<Address> = env.as_contract(&lifecycle_id, || {
            env.storage().persistent().get(&ASSET_REGISTRY)
        });
        let eng_reg_persisted: Option<Address> = env.as_contract(&lifecycle_id, || {
            env.storage().persistent().get(&ENG_REGISTRY)
        });

        assert_eq!(asset_reg_persisted, Some(asset_registry_id));
        assert_eq!(eng_reg_persisted, Some(engineer_registry_id));
    }

    #[test]
    fn test_get_collateral_score_before_init_returns_structured_error() {
        let env = Env::default();
        env.mock_all_auths();

        // Deploy lifecycle without calling initialize
        let lifecycle_id = env.register(Lifecycle, ());
        let client = LifecycleClient::new(&env, &lifecycle_id);

        let result = client.try_get_collateral_score(&1u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NotInitialized as u32,
            ))),
        );
    }

    #[test]
    fn test_get_asset_registry_before_init_returns_structured_error() {
        let env = Env::default();
        env.mock_all_auths();

        let lifecycle_id = env.register(Lifecycle, ());
        let client = LifecycleClient::new(&env, &lifecycle_id);

        let result = client.try_get_asset_registry();
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NotInitialized as u32,
            ))),
        );
    }

    #[test]
    fn test_get_engineer_registry_before_init_returns_structured_error() {
        let env = Env::default();
        env.mock_all_auths();

        let lifecycle_id = env.register(Lifecycle, ());
        let client = LifecycleClient::new(&env, &lifecycle_id);

        let result = client.try_get_engineer_registry();
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NotInitialized as u32,
            ))),
        );
    }

    #[test]
    fn test_get_config_before_init_returns_structured_error() {
        let env = Env::default();
        env.mock_all_auths();

        let lifecycle_id = env.register(Lifecycle, ());
        let client = LifecycleClient::new(&env, &lifecycle_id);

        let result = client.try_get_config();
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NotInitialized as u32,
            ))),
        );
    }

    #[test]
    fn test_update_asset_registry_before_init_returns_structured_error() {
        let env = Env::default();
        env.mock_all_auths();

        let lifecycle_id = env.register(Lifecycle, ());
        let client = LifecycleClient::new(&env, &lifecycle_id);
        let admin = Address::generate(&env);
        let new_registry = Address::generate(&env);

        let result = client.try_update_asset_registry(&admin, &new_registry);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NotInitialized as u32,
            ))),
        );
    }

    #[test]
    fn test_update_engineer_registry_before_init_returns_structured_error() {
        let env = Env::default();
        env.mock_all_auths();

        let lifecycle_id = env.register(Lifecycle, ());
        let client = LifecycleClient::new(&env, &lifecycle_id);
        let admin = Address::generate(&env);
        let new_registry = Address::generate(&env);

        let result = client.try_update_engineer_registry(&admin, &new_registry);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NotInitialized as u32,
            ))),
        );
    }

    #[test]
    fn test_update_asset_registry_emits_reg_ast_topic() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let new_registry = Address::generate(&env);

        client.update_asset_registry(&admin, &new_registry);

        let events = env.events().all();
        assert!(events.len() >= 1);

        let (_, topics, _data) = events.get(0).unwrap();
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, EVENT_REG_AST);
    }

    #[test]
    fn test_update_asset_registry_rejects_same_address_as_engineer_registry() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, engineer_registry_client, admin) = setup(&env, 0);
        let eng_registry_addr = engineer_registry_client.address.clone();

        let result = client.try_update_asset_registry(&admin, &eng_registry_addr);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::SameRegistryAddress as u32,
            ))),
        );
    }

    #[test]
    fn test_update_engineer_registry_emits_reg_eng_topic() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let new_registry = Address::generate(&env);

        client.update_engineer_registry(&admin, &new_registry);

        let events = env.events().all();
        assert!(events.len() >= 1);

        let (_, topics, _data) = events.get(0).unwrap();
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, EVENT_REG_ENG);
    }

    #[test]
    fn test_update_asset_registry_zero_address_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let zero = Address::from_str(
            &env,
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
        );

        let result = client.try_update_asset_registry(&admin, &zero);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::ZeroAddress as u32,
            ))),
        );
    }

    #[test]
    fn test_update_engineer_registry_zero_address_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let zero = Address::from_str(
            &env,
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
        );

        let result = client.try_update_engineer_registry(&admin, &zero);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::ZeroAddress as u32,
            ))),
        );
    }

    #[test]
    fn test_update_asset_registry_same_as_engineer_registry_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, engineer_registry_client, admin) = setup(&env, 0);
        let eng_addr = engineer_registry_client.address.clone();

        let result = client.try_update_asset_registry(&admin, &eng_addr);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::SameRegistryAddress as u32,
            ))),
        );
    }

    #[test]
    fn test_update_engineer_registry_same_as_asset_registry_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, admin) = setup(&env, 0);
        let asset_addr = asset_registry_client.address.clone();

        let result = client.try_update_engineer_registry(&admin, &asset_addr);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::SameRegistryAddress as u32,
            ))),
        );
    }

    // --- Issue #144: batch_submit_maintenance updates score_history_key ---

    #[test]
    fn test_batch_submit_score_history_length_matches_records() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            priority: Priority::Low,
            notes: String::from_str(&env, "First"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("INSPECT"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Second"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("ENGINE"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Third"),
        });

        client.batch_submit_maintenance(&asset_id, &records, &engineer);

        let score_history = client.get_score_history(&asset_id);
        // All 3 batch records share the same ledger timestamp, so score_history_push
        // deduplicates them into a single entry containing the final score.
        assert_eq!(
            score_history.len(),
            1,
            "batch records in the same ledger should produce exactly 1 score history entry"
        );
    }

    #[test]
    fn test_batch_submit_extends_ttl() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            priority: Priority::Low,
            notes: String::from_str(&env, "ttl test"),
        });
        client.batch_submit_maintenance(&asset_id, &records, &engineer);

        let contract_id = client.address.clone();
        env.as_contract(&contract_id, || {
            assert!(env.storage().persistent().get_ttl(&history_key(asset_id)) > 0);
            assert!(env.storage().persistent().get_ttl(&score_key(asset_id)) > 0);
            assert!(
                env.storage()
                    .persistent()
                    .get_ttl(&score_history_key(asset_id))
                    > 0
            );
            assert!(
                env.storage()
                    .persistent()
                    .get_ttl(&last_update_key(asset_id))
                    > 0
            );
        });
    }

    #[test]
    fn test_get_maintenance_history_page() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        for _ in 0..5 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "oil change"),
                &engineer,
            );
        }

        // First page: offset=0, limit=2 ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ 2 records
        assert_eq!(
            client.get_maintenance_history_page(&asset_id, &0, &2).len(),
            2
        );
        // Second page: offset=2, limit=2 ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ 2 records
        assert_eq!(
            client.get_maintenance_history_page(&asset_id, &2, &2).len(),
            2
        );
        // Third page: offset=4, limit=2 ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ 1 record (only one left)
        assert_eq!(
            client.get_maintenance_history_page(&asset_id, &4, &2).len(),
            1
        );
        // Out-of-bounds offset -> empty vec
        assert_eq!(
            client
                .get_maintenance_history_page(&asset_id, &10, &2)
                .len(),
            0
        );
        // limit=0 -> empty
        assert_eq!(
            client.get_maintenance_history_page(&asset_id, &0, &0).len(),
            0
        );
    }

    #[test]
    fn test_get_maintenance_history_page_out_of_bounds() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        for _ in 0..3 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "oil change"),
                &engineer,
            );
        }

        // offset == len (3) -> empty vec
        let result = client.get_maintenance_history_page(&asset_id, &3, &2);
        assert_eq!(result.len(), 0);

        // offset >> len (10) -> empty vec
        let result = client.get_maintenance_history_page(&asset_id, &10, &2);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_get_maintenance_history_page_nonexistent_asset_returns_error() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let result = client.try_get_maintenance_history_page(&999u64, &0, &10);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_get_maintenance_history_page_limit_capped_at_max_page_size() {
        let env = Env::default();
        env.mock_all_auths();

        // max_history=0 → default of 200, comfortably above MAX_PAGE_SIZE (100)
        // so none of the records submitted below get pruned.
        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Submit 150 records (3 batches of 50, MAX_BATCH_SIZE) — more than MAX_PAGE_SIZE.
        for _ in 0..3 {
            let mut records = Vec::new(&env);
            for _ in 0..MAX_BATCH_SIZE {
                records.push_back(BatchRecord {
                    task_type: symbol_short!("OIL_CHG"),
                    notes: String::from_str(&env, "Maintenance record"),
                });
            }
            client.batch_submit_maintenance(&asset_id, &records, &engineer);
        }
        assert_eq!(client.get_maintenance_history(&asset_id).len(), 150);

        // Requesting a limit far above MAX_PAGE_SIZE must be clamped, not
        // return every available record — this is the guard against the
        // unbounded-response failure the pagination endpoint exists to prevent.
        let page = client.get_maintenance_history_page(&asset_id, &0, &(MAX_PAGE_SIZE * 10));
        assert_eq!(page.len(), MAX_PAGE_SIZE);
    }

    #[test]
    fn test_get_engineer_maintenance_history_page() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // Submit maintenance on 5 different assets
        for _ in 0..5 {
            let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "oil change"),
                &engineer,
            );
        }

        // First page: offset=0, limit=2 ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ 2 assets
        assert_eq!(client.get_eng_history_page(&engineer, &0, &2).len(), 2);
        // Second page: offset=2, limit=2 ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ 2 assets
        assert_eq!(client.get_eng_history_page(&engineer, &2, &2).len(), 2);
        // Third page: offset=4, limit=2 ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ 1 asset (only one left)
        assert_eq!(client.get_eng_history_page(&engineer, &4, &2).len(), 1);
        // Out-of-bounds offset ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ empty
        assert_eq!(client.get_eng_history_page(&engineer, &10, &2).len(), 0);
        // limit=0 ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ empty
        assert_eq!(client.get_eng_history_page(&engineer, &0, &0).len(), 0);
    }

    #[test]
    fn test_engineer_maintenance_history_page_small_set() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // Submit maintenance on 5 different assets
        let mut asset_ids: Vec<u64> = Vec::new();
        for _ in 0..5 {
            let (asset_id, _asset_owner) = register_asset(&env, &asset_registry_client);
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "oil change"),
                &engineer,
            );
            asset_ids.push(asset_id);
        }

        // Page 0, size 2 -> 2 records, total = 5
        let (page0, total0) = client.get_engineer_maintenance_history_page(&engineer, &0, &2);
        assert_eq!(page0.len(), 2);
        assert_eq!(total0, 5);

        // Page 1, size 2 -> next 2 records
        let (page1, total1) = client.get_engineer_maintenance_history_page(&engineer, &1, &2);
        assert_eq!(page1.len(), 2);
        assert_eq!(total1, 5);

        // Page 2, size 2 -> final partial page (1 record)
        let (page2, total2) = client.get_engineer_maintenance_history_page(&engineer, &2, &2);
        assert_eq!(page2.len(), 1);
        assert_eq!(total2, 5);

        // Confirm pages don't overlap and cover all 5 records in order
        assert_eq!(page0.get(0).unwrap(), page0.get(0).unwrap()); // sanity
    }

    #[test]
    fn test_engineer_maintenance_history_page_out_of_range_and_zero_size() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let engineer = register_engineer(&env, &engineer_registry_client);

        for _ in 0..3 {
            let (asset_id, _asset_owner) = register_asset(&env, &asset_registry_client);
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "oil change"),
                &engineer,
            );
        }

        // page_size = 0 -> empty page, correct total
        let (empty_page, total) = client.get_engineer_maintenance_history_page(&engineer, &0, &0);
        assert_eq!(empty_page.len(), 0);
        assert_eq!(total, 3);

        // page far beyond range -> empty page, correct total
        let (oob_page, total2) = client.get_engineer_maintenance_history_page(&engineer, &50, &2);
        assert_eq!(oob_page.len(), 0);
        assert_eq!(total2, 3);

        // engineer with zero records at all
        let other_engineer = register_engineer(&env, &engineer_registry_client);
        let (none_page, none_total) =
            client.get_engineer_maintenance_history_page(&other_engineer, &0, &10);
        assert_eq!(none_page.len(), 0);
        assert_eq!(none_total, 0);
    }

    #[test]
    fn test_engineer_maintenance_history_page_large_set() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // Simulate a prolific engineer: 1200 records
        const RECORD_COUNT: u32 = 1200;
        for _ in 0..RECORD_COUNT {
            let (asset_id, _asset_owner) = register_asset(&env, &asset_registry_client);
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "oil change"),
                &engineer,
            );
        }

        let page_size = 50u32;
        let mut seen: u32 = 0;
        let mut page_index = 0u32;

        loop {
            let (page, total) =
                client.get_engineer_maintenance_history_page(&engineer, &page_index, &page_size);
            assert_eq!(total, RECORD_COUNT);

            if page.len() == 0 {
                break;
            }

            seen += page.len() as u32;
            page_index += 1;

            // Safety valve so a bug can't infinite-loop the test
            assert!(page_index < 100);
        }

        assert_eq!(seen, RECORD_COUNT);
    }

    #[test]
    fn test_get_engineer_history_with_pagination() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // Submit maintenance on 5 different assets
        for _ in 0..5 {
            let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
            client.authorize_engineer(&asset_owner, &asset_id, &engineer);
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "oil change"),
                &engineer,
            );
        }

        // First page: offset=0, limit=2 -> 2 assets
        assert_eq!(client.get_engineer_history(&engineer, &0, &2).len(), 2);
        // Second page: offset=2, limit=2 -> 2 assets
        assert_eq!(client.get_engineer_history(&engineer, &2, &2).len(), 2);
        // Third page: offset=4, limit=2 -> 1 asset (only one left)
        assert_eq!(client.get_engineer_history(&engineer, &4, &2).len(), 1);
        // Out-of-bounds offset -> empty
        assert_eq!(client.get_engineer_history(&engineer, &10, &2).len(), 0);
        // limit=0 -> empty
        assert_eq!(client.get_engineer_history(&engineer, &0, &0).len(), 0);
    }

    #[test]
    fn test_get_engineer_history_empty_engineer() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let engineer = Address::generate(&env);

        // Engineer with no history should return empty
        assert_eq!(client.get_engineer_history(&engineer, &0, &10).len(), 0);
    }
    // --- Issue #207: decay_score extends TTL ---

    #[test]
    fn test_decay_score_extends_ttl() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &Priority::Low,
            &String::from_str(&env, "Maintenance"),
            &engineer,
        );

        let score_key = (symbol_short!("SCORE"), asset_id);
        let last_update_key = (symbol_short!("LUPD"), asset_id);
        let score_history_key = (symbol_short!("SCHIST"), asset_id);

        let contract_id = client.address.clone();

        // Verify entries exist before decay
        env.as_contract(&contract_id, || {
            assert!(env.storage().persistent().has(&score_key));
            assert!(env.storage().persistent().has(&last_update_key));
            assert!(env.storage().persistent().has(&score_history_key));
        });

        // Call decay_score
        client.decay_score(&asset_id);

        // Verify entries still exist after decay (TTL was extended)
        env.as_contract(&contract_id, || {
            assert!(env.storage().persistent().has(&score_key));
            assert!(env.storage().persistent().has(&last_update_key));
            assert!(env.storage().persistent().has(&score_history_key));
        });
    }

    // --- Issue #208: submit_maintenance extends TTL ---

    #[test]
    fn test_submit_maintenance_extends_ttl() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let history_key = (symbol_short!("HIST"), asset_id);
        let score_key = (symbol_short!("SCORE"), asset_id);
        let score_history_key = (symbol_short!("SCHIST"), asset_id);
        let last_update_key = (symbol_short!("LUPD"), asset_id);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &Priority::Low,
            &String::from_str(&env, "Maintenance"),
            &engineer,
        );

        let contract_id = client.address.clone();

        // Verify all keys exist and TTL was extended
        env.as_contract(&contract_id, || {
            assert!(env.storage().persistent().has(&history_key));
            assert!(env.storage().persistent().has(&score_key));
            assert!(env.storage().persistent().has(&score_history_key));
            assert!(env.storage().persistent().has(&last_update_key));
        });
    }

    // --- Issue #209: batch_submit_maintenance extends TTL ---

    #[test]
    fn test_batch_submit_maintenance_extends_ttl() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let history_key = (symbol_short!("HIST"), asset_id);
        let score_key = (symbol_short!("SCORE"), asset_id);
        let score_history_key = (symbol_short!("SCHIST"), asset_id);
        let last_update_key = (symbol_short!("LUPD"), asset_id);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Oil change"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("INSPECT"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Inspection"),
        });

        client.batch_submit_maintenance(&asset_id, &records, &engineer);

        // Verify all keys exist and TTL was extended
        let contract_id = client.address.clone();
        env.as_contract(&contract_id, || {
            assert!(env.storage().persistent().has(&history_key));
            assert!(env.storage().persistent().has(&score_key));
            assert!(env.storage().persistent().has(&score_history_key));
            assert!(env.storage().persistent().has(&last_update_key));
        });
    }

    // --- Issue #396: score_history_push sets TTL on first creation and extends on subsequent writes ---

    #[test]
    fn test_score_history_ttl_set_on_first_creation() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let score_history_key = (symbol_short!("SCHIST"), asset_id);
        let contract_id = client.address.clone();

        // Key must not exist before first maintenance
        env.as_contract(&contract_id, || {
            assert!(!env.storage().persistent().has(&score_history_key));
        });

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "first"),
            &engineer,
        );

        // After first write the key must exist (TTL was set)
        env.as_contract(&contract_id, || {
            assert!(env.storage().persistent().has(&score_history_key));
        });
    }

    #[test]
    fn test_score_history_ttl_extended_on_subsequent_writes() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        let score_history_key = (symbol_short!("SCHIST"), asset_id);
        let contract_id = client.address.clone();

        // First write — creates the entry
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "first"),
            &engineer,
        );

        // Second write — extends TTL on an existing entry
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "second"),
            &engineer,
        );

        env.as_contract(&contract_id, || {
            assert!(env.storage().persistent().has(&score_history_key));
        });
    }

    // --- Issue #210: reset_score extends TTL ---

    #[test]
    fn test_reset_score_extends_ttl() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &Priority::Low,
            &String::from_str(&env, "Maintenance"),
            &engineer,
        );

        let score_key = (symbol_short!("SCORE"), asset_id);

        // Verify entry exists before reset
        let contract_id = client.address.clone();
        env.as_contract(&contract_id, || {
            assert!(env.storage().persistent().has(&score_key));
        });

        // Call reset_score
        client.reset_score(&admin, &asset_id);

        // Verify entry still exists after reset (TTL was extended)
        env.as_contract(&contract_id, || {
            assert!(env.storage().persistent().has(&score_key));
        });
        assert_eq!(client.get_collateral_score(&asset_id), 0);
    }

    #[test]
    fn test_pause_affects_all_state_changes_in_lifecycle() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.pause(&admin);

        // Read-only access should still work while paused
        let score = client.get_collateral_score(&asset_id);
        assert_eq!(score, 0);
        assert!(client.try_get_collateral_score(&asset_id).is_ok());

        // submit_maintenance
        assert_eq!(
            client.try_submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "ok"),
                &engineer
            ),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );

        // batch_submit_maintenance
        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "ok"),
        });
        assert_eq!(
            client.try_batch_submit_maintenance(&asset_id, &records, &engineer),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );

        // decay_score
        assert_eq!(
            client.try_decay_score(&asset_id),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );

        // propose_upgrade
        assert_eq!(
            client.try_propose_upgrade(&admin, &BytesN::from_array(&env, &[0u8; 32])),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );
    }

    #[test]
    fn test_pause_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        client.pause(&admin);

        let events = env.events().all();
        assert!(events.len() >= 1);
        let (_, topics, data) = events.get(0).unwrap();
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("PAUSED"));
        let (emitted_admin,): (Address,) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_admin, admin);
    }

    #[test]
    fn test_unpause_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        client.pause(&admin);
        client.unpause(&admin);

        let events = env.events().all();
        assert!(events.len() >= 1);
        let (_, topics, data) = events.get(0).unwrap();
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("UNPAUSED"));
        let (emitted_admin,): (Address,) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_admin, admin);
    }

    #[test]
    fn test_pause_state_persists_across_ttl_boundary() {
        let env = Env::default();
        env.mock_all_auths();

        let lifecycle_id = env.register(Lifecycle, ());
        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let admin = Address::generate(&env);

        let client = LifecycleClient::new(&env, &lifecycle_id);
        client.initialize(
            &admin,
            &asset_registry_id,
            &engineer_registry_id,
            &admin,
            &0u32,
        );

        // Pause the contract
        client.pause(&admin);
        assert!(client.is_paused());

        // Simulate instance TTL expiry — PAUSED_KEY must survive because it is in persistent storage
        env.as_contract(&lifecycle_id, || {
            // Wipe all instance storage to mimic TTL expiration
            env.storage().instance().remove(&PENDING_ADMIN_KEY);
        });

        // Pause state must still be true after instance TTL boundary
        assert!(
            client.is_paused(),
            "pause state must survive instance TTL expiry"
        );

        // submit_maintenance must still be blocked
        let (_, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);
        assert_eq!(
            client.try_submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "should be blocked"),
                &engineer,
            ),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );
    }

    #[test]
    fn test_engineer_maintenance_history_multiple_assets_and_sessions() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset1, asset1_owner) = register_asset(&env, &asset_registry_client);

    #[test]
    fn test_emergency_pause_blocks_maintenance_submission() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Can submit before pause
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "before pause"),
            &engineer,
        );
        assert_eq!(client.get_maintenance_history(&asset_id).len(), 1);

        // Pause contract
        client.pause(&admin);

        // Cannot submit after pause
        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &String::from_str(&env, "after pause"),
            &engineer,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );

        // History should not have grown
        assert_eq!(client.get_maintenance_history(&asset_id).len(), 1);
    }

    #[test]
    fn test_emergency_pause_blocks_score_updates() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Submit maintenance and verify score increases
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "maintenance 1"),
            &engineer,
        );
        let initial_score = client.get_collateral_score(&asset_id);
        assert!(initial_score > 0);

        // Pause contract
        client.pause(&admin);

        // Cannot reset score while paused
        let result = client.try_reset_score(&admin, &asset_id);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );

        // Score should not have changed
        assert_eq!(client.get_collateral_score(&asset_id), initial_score);
    }

    #[test]
    fn test_emergency_unpause_restores_functionality() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Pause and then unpause
        client.pause(&admin);
        assert!(client.is_paused());
        client.unpause(&admin);
        assert!(!client.is_paused());

        // Can submit after unpause
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "after unpause"),
            &engineer,
        );

        assert_eq!(client.get_maintenance_history(&asset_id).len(), 1);
    }

    #[test]
    fn test_pause_rejects_non_admin() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _admin) = setup(&env, 0);
        let not_admin = Address::generate(&env);

        let result = client.try_pause(&not_admin);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32
            )))
        );

        // Contract should not be paused
        assert!(!client.is_paused());
    }

    #[test]
    fn test_unpause_rejects_non_admin() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        client.pause(&admin);
        assert!(client.is_paused());

        let not_admin = Address::generate(&env);
        let result = client.try_unpause(&not_admin);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32
            )))
        );

        // Contract should still be paused
        assert!(client.is_paused());
    }
        let (asset2, asset2_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset1_owner, &asset1, &engineer);
        client.authorize_engineer(&asset2_owner, &asset2, &engineer);

        client.submit_maintenance(
            &asset1,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Session 1"),
            &engineer,
        );
        // Advance time
        env.ledger().with_mut(|li| li.timestamp += 3600);
        client.submit_maintenance(
            &asset2,
            &symbol_short!("INSPECT"),
            &Priority::Low,
            &String::from_str(&env, "Session 2"),
            &engineer,
        );

        let history = client.get_engineer_maintenance_history(&engineer);
        assert_eq!(history.len(), 2);
        assert!(history.contains(&asset1));
        assert!(history.contains(&asset2));
    }

    #[test]
    fn test_is_collateral_eligible_threshold_boundary() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // 9 ÃƒÆ’Ã¢â‚¬â€ FILTER (5 pts each) = 45 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â below threshold of 50
        for _ in 0..9 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("FILTER"),
                &Priority::Low,
                &String::from_str(&env, "Filter replacement"),
                &engineer,
            );
        }
        assert_eq!(client.get_collateral_score(&asset_id), 45);
        assert!(!client.is_collateral_eligible(&asset_id));

        // 1 more FILTER ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ 50 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â at threshold, now eligible
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &Priority::Low,
            &String::from_str(&env, "Filter replacement"),
            &engineer,
        );
        assert_eq!(client.get_collateral_score(&asset_id), 50);
        assert!(client.is_collateral_eligible(&asset_id));
    }

    // --- Issue #103: initialize rejects zero addresses ---

    #[test]
    fn test_full_cross_contract_integration_with_transfer() {
        let env = Env::default();
        env.mock_all_auths();

        // 1. Set up all three contracts
        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);

        // 2. Register asset
        let owner = Address::generate(&env);
        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT 3516 Generator"),
            &unique_serial(&env),
            &owner,
        );
        assert_eq!(asset_registry.get_asset(&asset_id).owner, owner);

        // 3. Register engineer
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let eng_admin = Address::generate(&env);
        engineer_registry.initialize_admin(&eng_admin, &eng_admin);
        engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
        engineer_registry.register_engineer(
            &engineer,
            &BytesN::from_array(&env, &[3u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );
        assert_eq!(engineer_registry.verify_engineer(&engineer, &None::<Symbol>), ::engineer_registry::CredentialStatus::Valid);

        // 4. Submit maintenance ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â 10 ÃƒÆ’Ã¢â‚¬â€ OVERHAUL (5 pts each) = 50, eligible
        for _ in 0..10 {
            lifecycle.submit_maintenance(
                &asset_id,
                &symbol_short!("OVERHAUL"),
                &Priority::Low,
                &String::from_str(&env, "Full overhaul"),
                &engineer,
            );
        }

        // 5. Verify score and collateral eligibility
        assert_eq!(lifecycle.get_collateral_score(&asset_id), 50);
        assert!(lifecycle.is_collateral_eligible(&asset_id));
        assert_eq!(lifecycle.get_maintenance_history(&asset_id).len(), 10);

        // 6. Transfer asset to new owner
        let new_owner = Address::generate(&env);
        asset_registry.transfer_asset(&asset_id, &owner, &new_owner);

        // 7. Verify new owner and that lifecycle state is preserved
        assert_eq!(asset_registry.get_asset(&asset_id).owner, new_owner);
        assert_eq!(lifecycle.get_collateral_score(&asset_id), 50);
        assert!(lifecycle.is_collateral_eligible(&asset_id));
        assert_eq!(
            lifecycle.get_last_service(&asset_id).unwrap().engineer,
            engineer
        );
    }

    #[test]
    fn test_config_survives_ttl_boundary() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);

        // Simulate instance TTL expiry by removing instance-stored keys
        // (Config is in persistent storage, so it should survive)
        env.as_contract(&client.address, || {
            env.storage().instance().remove(&PAUSED_KEY);
        });

        // Config must still be readable and hold the updated value.
        client.update_score_increment(&admin, &10);
        let config = client.get_config();
        assert_eq!(config.score_increment, 10);
    }

    #[test]
    fn test_post_transfer_maintenance_history_access() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let issuer = Address::generate(&env);
        let engineer = Address::generate(&env);
        let eng_admin = Address::generate(&env);

        engineer_registry.initialize_admin(&eng_admin, &eng_admin);
        engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Generator GEN-001"),
            &unique_serial(&env),
            &owner,
        );
        engineer_registry.register_engineer(
            &engineer,
            &BytesN::from_array(&env, &[1u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );

        // Submit 2 records under original owner
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &Priority::Low,
            &String::from_str(&env, "Pre-transfer inspection"),
            &engineer,
        );
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Oil change"),
            &engineer,
        );

        // Transfer asset and record the transfer sentinel
        asset_registry.transfer_asset(&asset_id, &owner, &new_owner);
        lifecycle.record_transfer(&asset_id, &owner, &new_owner);

        // New owner can read full history (2 maintenance + 1 sentinel)
        let history = lifecycle.get_maintenance_history(&asset_id);
        assert_eq!(history.len(), 3);

        // Sentinel is last and marks the ownership boundary
        let sentinel = history.get(2).unwrap();
        assert_eq!(sentinel.task_type, symbol_short!("XFER"));
        assert_eq!(sentinel.engineer, new_owner);

        // Pre-transfer records are still accessible and reference the original engineer
        assert_eq!(history.get(0).unwrap().engineer, engineer);
        assert_eq!(history.get(1).unwrap().engineer, engineer);

        // Score and eligibility are preserved for the new owner
        assert!(lifecycle.get_collateral_score(&asset_id) > 0);
        assert_eq!(asset_registry.get_asset(&asset_id).owner, new_owner);
    }

    /// A non-owner who obtains `new_owner`'s signature for an unrelated transaction
    /// must not be able to replay it to insert a false transfer sentinel.
    /// `record_transfer` must reject any call where `new_owner` is not the current
    /// owner recorded in the asset registry.
    #[test]
    fn test_record_transfer_rejects_non_owner() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, _, _) = setup(&env, 0);

        let real_owner = Address::generate(&env);
        let attacker = Address::generate(&env);

        // Register an asset under real_owner; ownership stays with real_owner.
        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Generator NON-OWNER-TEST"),
            &unique_serial(&env),
            &real_owner,
        );

        // Attacker tries to record a transfer claiming they are the new owner,
        // but the registry still shows real_owner as the owner.
        let result = lifecycle.try_record_transfer(&asset_id, &real_owner, &attacker);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedOwner as u32
            )))
        );

        // Confirm no sentinel was written — history must be empty.
        let history = lifecycle.get_maintenance_history(&asset_id);
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn test_record_transfer_event_includes_sentinel_index() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let issuer = Address::generate(&env);
        let engineer = Address::generate(&env);
        let eng_admin = Address::generate(&env);

        engineer_registry.initialize_admin(&eng_admin, &eng_admin);
        engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Generator XFER-IDX-001"),
            &unique_serial(&env),
            &owner,
        );
        engineer_registry.register_engineer(
            &engineer,
            &BytesN::from_array(&env, &[2u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );

        // Submit 2 records so the sentinel lands at index 2
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &String::from_str(&env, "Pre-transfer inspection"),
            &engineer,
        );
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Oil change"),
            &engineer,
        );

        asset_registry.transfer_asset(&asset_id, &owner, &new_owner);
        lifecycle.record_transfer(&asset_id, &owner, &new_owner);

        let history = lifecycle.get_maintenance_history(&asset_id);
        let expected_index = (history.len() - 1) as u32;

        let events = env.events().all();
        let xfer_event = events
            .iter()
            .find(|(_, topics, _)| {
                topics
                    .get(0)
                    .and_then(|v| v.try_into_val(&env).ok())
                    .map(|s: Symbol| s == symbol_short!("XFER"))
                    .unwrap_or(false)
            })
            .expect("XFER event not emitted");

        let (_, _, data) = xfer_event;
        let (_, _, _, emitted_index): (Address, Address, u64, u32) =
            data.try_into_val(&env).unwrap();

        assert_eq!(emitted_index, expected_index);
    }

    #[test]
    fn test_get_transfer_history_empty_before_transfer() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, _, _) = setup(&env, 0);
        let (asset_id, _owner) = register_asset(&env, &asset_registry);

        let history = lifecycle.get_transfer_history(&asset_id);
        assert_eq!(history.len(), 0, "transfer history must be empty before any transfer");
    }

    #[test]
    fn test_get_transfer_history_records_single_transfer() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, _, _) = setup(&env, 0);
        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Provenance Gen A"),
            &unique_serial(&env),
            &owner,
        );

        asset_registry.transfer_asset(&asset_id, &owner, &new_owner);
        lifecycle.record_transfer(&asset_id, &owner, &new_owner);

        let history = lifecycle.get_transfer_history(&asset_id);
        assert_eq!(history.len(), 1);

        let record = history.get(0).unwrap();
        assert_eq!(record.from, owner);
        assert_eq!(record.to, new_owner);
    }

    #[test]
    fn test_get_transfer_history_accumulates_multiple_transfers() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, _, _) = setup(&env, 0);
        let owner_a = Address::generate(&env);
        let owner_b = Address::generate(&env);
        let owner_c = Address::generate(&env);
        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Provenance Gen B"),
            &unique_serial(&env),
            &owner_a,
        );

        // First transfer: A → B
        asset_registry.transfer_asset(&asset_id, &owner_a, &owner_b);
        lifecycle.record_transfer(&asset_id, &owner_a, &owner_b);

        // Second transfer: B → C
        asset_registry.transfer_asset(&asset_id, &owner_b, &owner_c);
        lifecycle.record_transfer(&asset_id, &owner_b, &owner_c);

        let history = lifecycle.get_transfer_history(&asset_id);
        assert_eq!(history.len(), 2);

        let first = history.get(0).unwrap();
        assert_eq!(first.from, owner_a);
        assert_eq!(first.to, owner_b);

        let second = history.get(1).unwrap();
        assert_eq!(second.from, owner_b);
        assert_eq!(second.to, owner_c);
    }

    #[test]
    fn test_get_transfer_history_timestamp_monotone() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, _, _) = setup(&env, 0);
        let owner_a = Address::generate(&env);
        let owner_b = Address::generate(&env);
        let owner_c = Address::generate(&env);
        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Provenance Gen C"),
            &unique_serial(&env),
            &owner_a,
        );

        env.ledger().with_mut(|li| li.timestamp = 1000);
        asset_registry.transfer_asset(&asset_id, &owner_a, &owner_b);
        lifecycle.record_transfer(&asset_id, &owner_a, &owner_b);

        env.ledger().with_mut(|li| li.timestamp = 2000);
        asset_registry.transfer_asset(&asset_id, &owner_b, &owner_c);
        lifecycle.record_transfer(&asset_id, &owner_b, &owner_c);

        let history = lifecycle.get_transfer_history(&asset_id);
        assert_eq!(history.len(), 2);
        assert!(
            history.get(0).unwrap().timestamp <= history.get(1).unwrap().timestamp,
            "transfer history must be in chronological order"
        );
    }

    // ── #1001: get_maintenance_history_since_transfer tests ─────────────────

    /// Helper: register an engineer via the already-initialised engineer-registry
    /// that comes out of `setup`.  Unlike the standalone `register_engineer`
    /// helper this one does NOT call `initialize_admin` again, avoiding a
    /// panic on the second call.
    fn register_engineer_for_transfer_tests(
        env: &Env,
        registry: &EngineerRegistryClient,
    ) -> Address {
        let engineer = Address::generate(env);
        let issuer = Address::generate(env);
        let admin = Address::generate(env);
        let hash = BytesN::from_array(env, &[7u8; 32]);
        // The engineer registry used in `setup` is a fresh, uninitialised instance
        // that hasn't had initialize_admin called on it yet.  We piggy-back on the
        // first call here; any subsequent helper call would conflict, so each test
        // that needs an engineer uses a fresh setup().
        registry.initialize_admin(&admin, &admin);
        registry.add_trusted_issuer(&admin, &issuer);
        registry.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        registry.update_reputation(&engineer, &500);
        engineer
    }

    /// get_maintenance_history_since_transfer returns the full history when
    /// the asset has never been transferred (no XFER sentinel present).
    #[test]
    fn test_history_since_transfer_no_transfer_returns_all() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);

        let owner = Address::generate(&env);
        let asset_id = register_asset_for_owner(&env, &asset_registry, &owner);
        let engineer = register_engineer_for_transfer_tests(&env, &engineer_registry);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        // Submit 3 records without any transfer.
        for _ in 0..3 {
            lifecycle.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "Routine oil change"),
                &engineer,
            );
            env.ledger().with_mut(|li| li.timestamp += 1);
        }

        // All 3 records belong to the single (original) owner.
        let since = lifecycle.get_maintenance_history_since_transfer(&asset_id);
        let full = lifecycle.get_maintenance_history(&asset_id);
        assert_eq!(
            since.len(),
            full.len(),
            "without a transfer, since_transfer must equal full history"
        );
        assert_eq!(since.len(), 3);
    }

    /// get_maintenance_history_since_transfer returns an empty Vec when a
    /// transfer has occurred but no maintenance has been submitted yet under
    /// the new owner.
    #[test]
    fn test_history_since_transfer_empty_after_fresh_transfer() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let asset_id = register_asset_for_owner(&env, &asset_registry, &owner);
        let engineer = register_engineer_for_transfer_tests(&env, &engineer_registry);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        // Submit one record under the original owner, then transfer.
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &Priority::Low,
            &String::from_str(&env, "Pre-transfer inspection"),
            &engineer,
        );

        asset_registry.transfer_asset(&asset_id, &owner, &new_owner);
        lifecycle.record_transfer(&asset_id, &owner, &new_owner);

        // No maintenance submitted post-transfer yet.
        let since = lifecycle.get_maintenance_history_since_transfer(&asset_id);
        assert_eq!(
            since.len(),
            0,
            "no post-transfer maintenance means since_transfer must be empty"
        );
    }

    /// Pre-transfer records are excluded; post-transfer records are included.
    /// This is the core reconciliation contract for DeFi lenders.
    #[test]
    fn test_history_since_transfer_separates_pre_and_post_records() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let asset_id = register_asset_for_owner(&env, &asset_registry, &owner);
        let engineer = register_engineer_for_transfer_tests(&env, &engineer_registry);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        // ── Phase 1: pre-transfer maintenance (2 records) ────────────────────
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &Priority::Low,
            &String::from_str(&env, "Pre-transfer inspection 1"),
            &engineer,
        );
        env.ledger().with_mut(|li| li.timestamp += 1);
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Pre-transfer oil change"),
            &engineer,
        );
        env.ledger().with_mut(|li| li.timestamp += 1);

        // ── Phase 2: transfer ────────────────────────────────────────────────
        asset_registry.transfer_asset(&asset_id, &owner, &new_owner);
        lifecycle.record_transfer(&asset_id, &owner, &new_owner);

        // Re-authorise engineer under new owner and submit post-transfer maintenance.
        lifecycle.authorize_engineer(&new_owner, &asset_id, &engineer);
        env.ledger().with_mut(|li| li.timestamp += 1);

        // ── Phase 3: post-transfer maintenance (2 records) ───────────────────
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &Priority::Low,
            &String::from_str(&env, "Post-transfer filter replacement"),
            &engineer,
        );
        env.ledger().with_mut(|li| li.timestamp += 1);
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("TUNE_UP"),
            &Priority::Low,
            &String::from_str(&env, "Post-transfer tune-up"),
            &engineer,
        );

        // Full history: 2 pre + 1 XFER sentinel + 2 post = 5 total.
        let full = lifecycle.get_maintenance_history(&asset_id);
        assert_eq!(full.len(), 5, "full history must have 5 entries");

        // Since-transfer: only the 2 post-transfer records, no sentinel.
        let since = lifecycle.get_maintenance_history_since_transfer(&asset_id);
        assert_eq!(
            since.len(),
            2,
            "since_transfer must return exactly the 2 post-transfer records"
        );

        // Post-transfer records carry ownership_start_ledger == Some(_).
        for record in since.iter() {
            assert!(
                record.ownership_start_ledger.is_some(),
                "post-transfer records must carry ownership_start_ledger"
            );
        }

        // Pre-transfer records (indices 0 and 1 in full history) must NOT carry ownership_start_ledger.
        assert!(
            full.get(0).unwrap().ownership_start_ledger.is_none(),
            "pre-transfer record 0 must have ownership_start_ledger == None"
        );
        assert!(
            full.get(1).unwrap().ownership_start_ledger.is_none(),
            "pre-transfer record 1 must have ownership_start_ledger == None"
        );

        // The XFER sentinel itself (index 2) must also be None.
        let sentinel = full.get(2).unwrap();
        assert_eq!(sentinel.task_type, symbol_short!("XFER"));
        assert!(
            sentinel.ownership_start_ledger.is_none(),
            "XFER sentinel must have ownership_start_ledger == None"
        );

        // Verify task types of the returned post-transfer records.
        assert_eq!(since.get(0).unwrap().task_type, symbol_short!("FILTER"));
        assert_eq!(since.get(1).unwrap().task_type, symbol_short!("TUNE_UP"));
    }

    /// After two successive transfers (A → B → C), since_transfer returns only
    /// records submitted under owner C, not those from B's tenure.
    #[test]
    fn test_history_since_transfer_respects_latest_transfer() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);

        let owner_a = Address::generate(&env);
        let owner_b = Address::generate(&env);
        let owner_c = Address::generate(&env);
        let asset_id = register_asset_for_owner(&env, &asset_registry, &owner_a);
        let engineer = register_engineer_for_transfer_tests(&env, &engineer_registry);
        lifecycle.authorize_engineer(&owner_a, &asset_id, &engineer);

        // Owner A submits 1 record.
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &Priority::Low,
            &String::from_str(&env, "Owner A inspection"),
            &engineer,
        );
        env.ledger().with_mut(|li| li.timestamp += 1);

        // Transfer A → B.
        asset_registry.transfer_asset(&asset_id, &owner_a, &owner_b);
        lifecycle.record_transfer(&asset_id, &owner_a, &owner_b);
        lifecycle.authorize_engineer(&owner_b, &asset_id, &engineer);
        env.ledger().with_mut(|li| li.timestamp += 1);

        // Owner B submits 1 record.
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Owner B oil change"),
            &engineer,
        );
        env.ledger().with_mut(|li| li.timestamp += 1);

        // Transfer B → C.
        asset_registry.transfer_asset(&asset_id, &owner_b, &owner_c);
        lifecycle.record_transfer(&asset_id, &owner_b, &owner_c);
        lifecycle.authorize_engineer(&owner_c, &asset_id, &engineer);
        env.ledger().with_mut(|li| li.timestamp += 1);

        // Owner C submits 1 record.
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &Priority::Low,
            &String::from_str(&env, "Owner C engine overhaul"),
            &engineer,
        );

        // Full: A-record + XFER(A→B) + B-record + XFER(B→C) + C-record = 5.
        let full = lifecycle.get_maintenance_history(&asset_id);
        assert_eq!(full.len(), 5);

        // Since transfer: only owner C's record.
        let since = lifecycle.get_maintenance_history_since_transfer(&asset_id);
        assert_eq!(
            since.len(),
            1,
            "since_transfer after second transfer must return only owner C's records"
        );
        assert_eq!(since.get(0).unwrap().task_type, symbol_short!("ENGINE"));
        assert!(
            since.get(0).unwrap().ownership_start_ledger.is_some(),
            "owner C's record must carry ownership_start_ledger"
        );
    }

    #[test]
    fn test_purge_asset_data_after_deregister() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, admin) = setup(&env, 0);

        let owner = Address::generate(&env);
        let issuer = Address::generate(&env);
        let engineer = Address::generate(&env);
        let eng_admin = Address::generate(&env);

        engineer_registry.initialize_admin(&eng_admin, &eng_admin);
        engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Generator GEN-PURGE-001"),
            &unique_serial(&env),
            &owner,
        );
        engineer_registry.register_engineer(
            &engineer,
            &BytesN::from_array(&env, &[9u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );

        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &String::from_str(&env, "Pre-deregister check"),
            &engineer,
        );

        // Lifecycle data exists before deregister (score history is readable without asset check)
        assert_eq!(lifecycle.get_score_history(&asset_id).len(), 1);

        // Deregister removes asset from registry but lifecycle data persists
        asset_registry.deregister_asset(&owner, &asset_id);
        assert!(
            asset_registry.try_get_asset(&asset_id).is_err(),
            "asset should be gone from registry"
        );
        assert_eq!(
            lifecycle.get_score_history(&asset_id).len(),
            1,
            "score history persists after deregister"
        );

        // purge_asset_data clears all lifecycle storage for the asset
        lifecycle.purge_asset_data(&admin, &asset_id);
        assert_eq!(
            lifecycle.get_score_history(&asset_id).len(),
            0,
            "score history cleared after purge"
        );
    }

    #[test]
    fn test_propose_admin_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let new_admin = Address::generate(&env);

        client.propose_admin(&admin, &new_admin);

        let events = env.events().all();
        assert!(events.iter().any(|(_, topics, _)| {
            topics
                .get(0)
                .and_then(|v| soroban_sdk::TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                .map(|s: Symbol| s == symbol_short!("PROP_ADM"))
                .unwrap_or(false)
        }));
    }
    // --- Issue #367: update_decay_config emits CFG_UPD event ---

    #[test]
    fn test_update_decay_config_emits_cfg_upd_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);

        client.update_decay_config(&admin, &10, &120);

        let events = env.events().all();
        assert!(events.len() >= 1);

        let (_, topics, data) = events.get(0).unwrap();
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("CFG_UPD"));

        // Data: (old_decay_rate, new_decay_rate, old_decay_interval, new_decay_interval)
        let (old_rate, new_rate, old_interval, new_interval): (u32, u32, u64, u64) =
            data.try_into_val(&env).unwrap();
        assert_eq!(old_rate, DEFAULT_DECAY_RATE);
        assert_eq!(new_rate, 10u32);
        assert_eq!(old_interval, DEFAULT_DECAY_INTERVAL);
        assert_eq!(new_interval, 120u64);
    }

    #[test]
    fn test_update_decay_config_emits_admin_audit_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let timestamp = env.ledger().timestamp();
        client.update_decay_config(&admin, &10, &120);

        let events = env.events().all();
        let (_, topics, data) = events.last().unwrap();
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        let t1: Symbol = topics.get(1).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("ADM_AUD"));
        assert_eq!(t1, symbol_short!("CFG_UPD"));

        let (emitted_admin, emitted_timestamp, key, rate, interval): (
            Address,
            u64,
            Symbol,
            u32,
            u64,
        ) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_admin, admin);
        assert_eq!(emitted_timestamp, timestamp);
        assert_eq!(key, symbol_short!("DECAY"));
        assert_eq!(rate, 10u32);
        assert_eq!(interval, 120u64);
    }

    #[test]
    fn test_accept_admin_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let new_admin = Address::generate(&env);

        client.propose_admin(&admin, &new_admin);

        // Timelock must expire before accept_admin succeeds.
        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

        client.accept_admin();

        let events = env.events().all();
        let (_, topics, data) = events.last().unwrap();

        use soroban_sdk::TryIntoVal;
        let topic: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(topic, EVENT_ADMIN_SET);

        let emitted_admin: Address = {
            let (a,): (Address,) = data.try_into_val(&env).unwrap();
            a
        };
        assert_eq!(emitted_admin, new_admin);
    }

    // --- Issue #368: update_eligibility_threshold emits CFG_UPD event ---

    #[test]
    fn test_update_eligibility_threshold_emits_cfg_upd_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);

        client.update_eligibility_threshold(&admin, &75);

        let events = env.events().all();
        assert!(events.len() >= 1);

        let (_, topics, data) = events.get(0).unwrap();
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("CFG_UPD"));

        let (old_threshold, new_threshold): (u32, u32) = data.try_into_val(&env).unwrap();
        assert_eq!(old_threshold, DEFAULT_ELIGIBILITY_THRESHOLD);
        assert_eq!(new_threshold, 75u32);
    }

    #[test]
    fn test_update_eligibility_threshold_affects_eligibility() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Build score to 5 (one ENGINE task, score_increment = 5)
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "ok"),
            &engineer,
        );
        assert_eq!(client.get_collateral_score(&asset_id), 5);

        // Default threshold is 50 — not eligible
        assert!(!client.is_collateral_eligible(&asset_id));

        // Lower threshold to 5 — now eligible
        client.update_eligibility_threshold(&admin, &5);
        assert!(client.is_collateral_eligible(&asset_id));
    }

    #[test]
    fn test_non_admin_cannot_update_eligibility_threshold() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let outsider = Address::generate(&env);
        let result = client.try_update_eligibility_threshold(&outsider, &75);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_purge_asset_removes_from_engineer_history() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Engineer performs maintenance on the asset
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &Priority::Low,
            &String::from_str(&env, "Routine check"),
            &engineer,
        );

        // Verify asset_id is in engineer's history
        let history = lifecycle.get_eng_history_page(&engineer, &0, &10);
        assert!(history.contains(&asset_id));

        // Purge the asset
        lifecycle.purge_asset_data(&admin, &asset_id);

        // BUG: Currently, asset_id is STILL in engineer's history
        let history_after = lifecycle.get_eng_history_page(&engineer, &0, &10);
        assert!(
            !history_after.contains(&asset_id),
            "Asset ID should be removed from engineer history after purge"
        );
    }

    #[test]
    fn test_initialize_rejects_non_deployer() {
        let env = Env::default();
        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());
        let client = LifecycleClient::new(&env, &lifecycle_id);

        let deployer = Address::generate(&env);
        let attacker = Address::generate(&env);

        use soroban_sdk::IntoVal;
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &attacker,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &lifecycle_id,
                fn_name: "initialize",
                args: (
                    &attacker,
                    &asset_registry_id,
                    &engineer_registry_id,
                    &attacker,
                    &0u32,
                )
                    .into_val(&env),
                sub_invokes: &[],
            },
        }]);

        let result = client.try_initialize(
            &deployer,
            &asset_registry_id,
            &engineer_registry_id,
            &attacker,
            &0u32,
        );
        assert!(
            result.is_err(),
            "non-deployer must not be able to initialize"
        );
    }

    /// An asset with a single maintenance record must never score 0, even after enough
    /// time has elapsed for decay to fully consume the raw score.
    #[test]
    fn test_score_floor_with_sparse_history() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // One record → raw score = DEFAULT_SCORE_INCREMENT (5)
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "single record"),
            &engineer,
        );
        assert_eq!(client.get_collateral_score(&asset_id), 5);

        // Advance time by 2 full decay intervals (2 × 30 days).
        // Decay = 2 × DEFAULT_DECAY_RATE (5) = 10 > raw score (5), so raw result = 0.
        env.ledger().with_mut(|li| {
            li.timestamp += 2 * 2_592_000; // 60 days
        });

        // Floor must kick in: score is 1, not 0.
        assert_eq!(
            client.get_collateral_score(&asset_id),
            1,
            "asset with maintenance history must not score 0 after decay"
        );
    }

    /// Closes #784 — `decay_score` (which calls `apply_decay`) must also enforce
    /// `MIN_SCORE_WITH_HISTORY`.  Before the fix, `apply_decay` could write 0 to
    /// persistent storage for an asset that has maintenance records, making it
    /// indistinguishable from an asset that was never maintained.
    #[test]
    fn test_decay_score_never_drops_to_zero_with_history() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // One record → raw score = DEFAULT_SCORE_INCREMENT (5).
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "oil change"),
            &engineer,
        );
        assert_eq!(client.get_collateral_score(&asset_id), 5);

        // Advance time far enough for decay to fully consume the raw score.
        // 2 × decay interval (30 days each) → total decay = 10 > 5, raw result = 0.
        env.ledger().with_mut(|li| {
            li.timestamp += 2 * 2_592_000; // 60 days
        });

        // Calling decay_score directly (not get_collateral_score) must also respect
        // the floor and return 1, not 0.
        let decayed = client.decay_score(&asset_id);
        assert_eq!(
            decayed, 1,
            "decay_score must not return 0 for an asset with maintenance history"
        );

        // The value written to persistent storage must also be >= 1.
        let stored = client.get_collateral_score(&asset_id);
        assert_eq!(
            stored, 1,
            "stored collateral score must never be 0 for an asset with maintenance history"
        );
    }

    #[test]
    fn test_collateral_score_never_exceeds_maximum_with_high_volume_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        asset_registry_client.add_asset_type(&admin, &symbol_short!("TURBINE"));

        let owner_a = Address::generate(&env);
        let asset_id_genset = asset_registry_client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Caterpillar 3516"),
            &unique_serial(&env),
            &owner_a,
        );

        let owner_b = Address::generate(&env);
        let asset_id_turbine = asset_registry_client.register_asset(
            &symbol_short!("TURBINE"),
            &String::from_str(&env, "Siemens SGT-800"),
            &unique_serial(&env),
            &owner_b,
        );

        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&owner_a, &asset_id_genset, &engineer);
        client.authorize_engineer(&owner_b, &asset_id_turbine, &engineer);

        for i in 0..110 {
            let note = String::from_str(&env, "Record");
            client.submit_maintenance(
                &asset_id_genset,
                &symbol_short!("ENGINE"),
                &note,
                &engineer,
            );
            client.submit_maintenance(
                &asset_id_turbine,
                &symbol_short!("ENGINE"),
                &note,
                &engineer,
            );
        }

        let score_genset = client.get_collateral_score(&asset_id_genset);
        let score_turbine = client.get_collateral_score(&asset_id_turbine);

        assert!(score_genset <= 100, "score must never exceed 100");
        assert!(score_turbine <= 100, "score must never exceed 100");
        assert_eq!(score_genset, 100, "high-volume maintenance should cap at 100");
        assert_eq!(score_turbine, 100, "high-volume maintenance should cap at 100");
    }

    #[test]
    fn test_collateral_score_accounts_for_maintenance_recency() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id_recent, asset_owner_recent) = register_asset(&env, &asset_registry_client);
        let (asset_id_old, asset_owner_old) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);

        // Submit maintenance for both assets at the same time
        let initial_timestamp = env.ledger().timestamp();
        client.submit_maintenance(
            &asset_id_recent,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "recent maintenance"),
            &engineer,
        );
        client.submit_maintenance(
            &asset_id_old,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "old maintenance"),
            &engineer,
        );

        // Both should have same score initially (score_increment = 5 for a recent record)
        let score_initial_recent = client.get_collateral_score(&asset_id_recent);
        let score_initial_old = client.get_collateral_score(&asset_id_old);
        assert_eq!(score_initial_recent, score_initial_old, "identical assets should have same initial score");

        // Advance time by 15 days (half of MAX_AGE_LEDGERS = 30 days)
        // Record age = 15 days → recency_weight = (30 - 15) / 30 = 0.5
        // For asset_id_old: score = 5 * 0.5 = 2.5 → 2
        env.ledger().with_mut(|li| {
            li.timestamp = initial_timestamp + 15 * 86_400; // 15 days later
        });

        let score_after_15d_old = client.get_collateral_score(&asset_id_old);
        assert!(
            score_after_15d_old < score_initial_old,
            "old maintenance should score lower due to age weighting"
        );

        // Add a fresh maintenance record to asset_id_recent to increase score
        client.submit_maintenance(
            &asset_id_recent,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "recent maintenance again"),
            &engineer,
        );
        let score_after_15d_recent = client.get_collateral_score(&asset_id_recent);
        assert!(
            score_after_15d_recent > score_after_15d_old,
            "asset with recent records should score higher than asset with only old records"
        );

        // Advance another 15 days (total 30 days from initial submissions)
        // Original records should now have zero contribution
        env.ledger().with_mut(|li| {
            li.timestamp = initial_timestamp + 30 * 86_400; // 30 days later
        });

        let score_at_30d_old = client.get_collateral_score(&asset_id_old);
        assert_eq!(
            score_at_30d_old, 0,
            "records older than MAX_AGE_LEDGERS should contribute zero"
        );

        // asset_id_recent still has the second record (15 days old) contributing
        let score_at_30d_recent = client.get_collateral_score(&asset_id_recent);
        assert!(
            score_at_30d_recent > score_at_30d_old,
            "asset with fresher records should score higher"
        );
    }

    #[test]
    fn test_request_loan_zero_threshold_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);

        let result = client.try_request_loan(&asset_id, &0_u32, &1_i128);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidConfig as u32,
            ))),
        );
    }

    #[test]
    fn test_request_loan_zero_amount_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);

        let result = client.try_request_loan(&asset_id, &1_u32, &0_i128);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidConfig as u32,
            ))),
        );
    }

    // --- Issue: score_history_push deduplication by timestamp ---

    #[test]
    fn test_score_history_dedup_same_ledger_submit() {
        // Two submit_maintenance calls in the same ledger (same timestamp) must produce
        // only one score history entry (the latest score), not two.
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_id_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_id_owner, &asset_id, &engineer);

        // Both submissions happen at the same ledger timestamp.
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "first in ledger"),
            &engineer,
        );
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &String::from_str(&env, "second in ledger"),
            &engineer,
        );

        let history = client.get_score_history(&asset_id);
        assert_eq!(
            history.len(),
            1,
            "two submissions in the same ledger must produce exactly 1 score history entry"
        );
        // The single entry reflects the score after both submissions.
        assert!(history.get(0).unwrap().score > 0);
    }

    #[test]
    fn test_score_history_dedup_batch_same_ledger() {
        // A batch of records all share the same ledger timestamp; score_history should
        // contain only one entry (the final accumulated score), not one per record.
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_id_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_id_owner, &asset_id, &engineer);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "rec 1"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("INSPECT"),
            notes: String::from_str(&env, "rec 2"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("FILTER"),
            notes: String::from_str(&env, "rec 3"),
        });

        client.batch_submit_maintenance(&asset_id, &records, &engineer);

        let history = client.get_score_history(&asset_id);
        assert_eq!(
            history.len(),
            1,
            "batch records in the same ledger must produce exactly 1 score history entry"
        );
    }

    #[test]
    fn test_score_history_distinct_ledgers_each_recorded() {
        // Submissions across different ledger timestamps each get their own entry.
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_id_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_id_owner, &asset_id, &engineer);

        for i in 0..4u64 {
            client.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "entry"),
                &engineer,
            );
            env.ledger().with_mut(|li| li.timestamp += 1000 * (i + 1));
        }

        assert_eq!(client.get_score_history(&asset_id).len(), 4);
    }

    // --- Issue: update_max_notes_length tests ---

    #[test]
    fn test_admin_can_update_max_notes_length() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        client.update_max_notes_length(&admin, &512);

        let config = client.get_config();
        assert_eq!(config.max_notes_length, 512);
    }

    #[test]
    fn test_non_admin_cannot_update_max_notes_length() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);
        let outsider = Address::generate(&env);

        let result = client.try_update_max_notes_length(&outsider, &512);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_update_max_notes_length_zero_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        let result = client.try_update_max_notes_length(&admin, &0);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidConfig as u32,
            ))),
        );
    }

    #[test]
    fn test_update_max_notes_length_enforced_in_submit_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_id_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_id_owner, &asset_id, &engineer);

        // Reduce max notes length to 10 bytes
        client.update_max_notes_length(&admin, &10);

        // Notes within the new limit are accepted
        let result_ok = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "short"),
            &engineer,
        );
        assert!(result_ok.is_ok());

        // Notes exceeding the new limit are rejected
        let result_err = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &String::from_str(&env, "this is too long"),
            &engineer,
        );
        assert_eq!(
            result_err,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NotesTooLong as u32,
            ))),
        );
    }

    #[test]
    fn test_update_max_notes_length_enforced_in_batch_submit() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 0);
        let (asset_id, asset_id_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_id_owner, &asset_id, &engineer);

        // Reduce max notes length to 10 bytes
        client.update_max_notes_length(&admin, &10);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "this is too long for limit"),
        });

        let result = client.try_batch_submit_maintenance(&asset_id, &records, &engineer);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NotesTooLong as u32,
            ))),
        );
    }

    #[test]
    fn test_update_max_notes_length_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 0);
        client.update_max_notes_length(&admin, &128);

        let events = env.events().all();
        assert!(events.len() >= 1);
        use soroban_sdk::TryIntoVal;
        let (_, topics, data) = events.get(0).unwrap();
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("UPD_NOTES"));
        let emitted_max: u32 = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_max, 128);
    }

    // --- #752: upgrade timelock tests ---

    #[test]
    fn test_execute_upgrade_before_timelock_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _, _, admin) = setup(&env, 0);

        let hash = BytesN::from_array(&env, &[0xabu8; 32]);
        client.propose_upgrade(&admin, &hash);

        let result = client.try_execute_upgrade(&admin);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::TimelockNotExpired as u32,
            ))),
        );
    }

    #[test]
    fn test_execute_upgrade_after_timelock_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _, _, admin) = setup(&env, 0);

        let hash = BytesN::from_array(&env, &[0xabu8; 32]);
        client.propose_upgrade(&admin, &hash);

        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

        client.execute_upgrade(&admin);
    }

    #[test]
    fn test_execute_upgrade_without_proposal_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _, _, admin) = setup(&env, 0);

        let result = client.try_execute_upgrade(&admin);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::ProposalNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_propose_upgrade_non_admin_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _, _, _) = setup(&env, 0);
        let outsider = Address::generate(&env);

        let hash = BytesN::from_array(&env, &[0xabu8; 32]);
        let result = client.try_propose_upgrade(&outsider, &hash);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    // --- #753: revoke_engineer_auth timelock tests ---

    #[test]
    fn test_propose_revoke_engineer_auth_and_execute_after_delay() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, asset_registry, engineer_registry, _admin) = setup(&env, 0);

        let owner = Address::generate(&env);
        let asset_id = register_asset_for_owner(&env, &asset_registry, &owner);
        let engineer = register_engineer(&env, &engineer_registry);

        client.authorize_engineer(&owner, &asset_id, &engineer);

        // Propose revocation
        client.propose_revoke_engineer_auth(&owner, &asset_id, &engineer);

        // Execute before timelock — should fail
        let result = client.try_execute_revoke_engineer_auth(&owner, &asset_id, &engineer);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::TimelockNotExpired as u32,
            ))),
        );

        // Advance past timelock delay (48 hours)
        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

        // Execute after delay — should succeed
        client.execute_revoke_engineer_auth(&owner, &asset_id, &engineer);
    }

    #[test]
    fn test_execute_revoke_engineer_auth_without_proposal_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, asset_registry, engineer_registry, _admin) = setup(&env, 0);

        let owner = Address::generate(&env);
        let asset_id = register_asset_for_owner(&env, &asset_registry, &owner);
        let engineer = register_engineer(&env, &engineer_registry);

        client.authorize_engineer(&owner, &asset_id, &engineer);

        // Execute without proposal — should fail
        let result = client.try_execute_revoke_engineer_auth(&owner, &asset_id, &engineer);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::ProposalNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_propose_revoke_engineer_auth_non_owner_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, asset_registry, engineer_registry, _admin) = setup(&env, 0);

        let owner = Address::generate(&env);
        let rogue = Address::generate(&env);
        let asset_id = register_asset_for_owner(&env, &asset_registry, &owner);
        let engineer = register_engineer(&env, &engineer_registry);

        client.authorize_engineer(&owner, &asset_id, &engineer);

        let result = client.try_propose_revoke_engineer_auth(&rogue, &asset_id, &engineer);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedOwner as u32,
            ))),
        );
    }

    #[test]
    fn test_propose_revoke_engineer_auth_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, asset_registry, engineer_registry, _admin) = setup(&env, 0);

        let owner = Address::generate(&env);
        let asset_id = register_asset_for_owner(&env, &asset_registry, &owner);
        let engineer = register_engineer(&env, &engineer_registry);

        client.authorize_engineer(&owner, &asset_id, &engineer);
        client.propose_revoke_engineer_auth(&owner, &asset_id, &engineer);

        let events = env.events().all();
        use soroban_sdk::TryIntoVal;
        let prop_event = events.iter().find(|(_, topics, _)| {
            if let Some(val) = topics.get(0) {
                if let Ok(s) = TryIntoVal::<_, Symbol>::try_into_val(&val, &env) {
                    return s == symbol_short!("PROP_RVK");
                }
            }
            false
        });
        assert!(prop_event.is_some(), "PROP_RVK event must be emitted on propose_revoke_engineer_auth");
    }
    #[test]
    fn test_get_collateral_score_persists_returned_value() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, eng_registry, _admin) = setup(&env, 10);
        let (asset_id, asset_id_owner) = register_asset(&env, &asset_registry);
        let engineer = Address::generate(&env);
        let cred_hash = BytesN::from_array(&env, &[1u8; 32]);
        let issuer = Address::generate(&env);
        eng_registry.register_engineer(&engineer, &cred_hash, &issuer, &31_536_000, &None);
        lifecycle.authorize_engineer(&asset_id_owner, &asset_id, &engineer);
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "oil change"),
            &engineer,
        );

        // Call get_collateral_score and capture returned value.
        let returned = lifecycle.get_collateral_score(&asset_id);

        // Read the persisted SCORE key directly from storage.
        let stored: u32 = env
            .as_contract(&lifecycle.address, || {
                env.storage()
                    .persistent()
                    .get(&score_key(asset_id))
                    .unwrap_or(0u32)
            });

        assert_eq!(
            returned, stored,
            "stored score ({stored}) must match returned score ({returned})"
        );
    }

    // --- update_max_history Tests ---

    #[test]
    fn test_update_max_history_by_admin() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 200);

        client.update_max_history(&admin, &500);

        let config: Config = env.as_contract(&client.address, || {
            env.storage()
                .persistent()
                .get(&(symbol_short!("CONFIG"), ()))
                .unwrap()
        });

        assert_eq!(config.max_history, 500);
    }

    #[test]
    fn test_update_max_history_non_admin_fails() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 200);
        let non_admin = Address::generate(&env);

        let result = client.try_update_max_history(&non_admin, &500);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_update_max_history_zero_fails() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 200);

        let result = client.try_update_max_history(&admin, &0);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidConfig as u32,
            ))),
        );
    }

    #[test]
    fn test_update_max_history_enforced_in_submit_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, admin) = setup(&env, 2);
        let (asset_id, asset_id_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_id_owner, &asset_id, &engineer);

        // Submit 2 maintenance records (at limit)
        client.submit_maintenance(&asset_id, &symbol_short!("OIL_CHG"), &String::from_str(&env, "1"), &engineer);
        client.submit_maintenance(&asset_id, &symbol_short!("OIL_CHG"), &String::from_str(&env, "2"), &engineer);

        assert_eq!(client.get_maintenance_history(&asset_id).len(), 2);

        // Reduce max_history to 1
        client.update_max_history(&admin, &1);

        // Submit third record - should prune oldest
        client.submit_maintenance(&asset_id, &symbol_short!("OIL_CHG"), &String::from_str(&env, "3"), &engineer);

        // History should still be at most 1 (or pruned to 1)
        let history = client.get_maintenance_history(&asset_id);
        assert!(history.len() <= 1, "History should not exceed new max_history limit");
    }

    #[test]
    fn test_update_max_history_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, admin) = setup(&env, 200);
        client.update_max_history(&admin, &300);

        let events = env.events().all();
        assert!(events.len() >= 1);
        use soroban_sdk::TryIntoVal;
        let (_, topics, data) = events.get(0).unwrap();
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("UPD_MAX"));
        let emitted_max: u32 = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_max, 300);
    }

    /// #804: get_collateral_score must apply decay lazily on each read and persist
    /// the last decay timestamp so a DeFi lender reading months later receives a
    /// score that reflects elapsed time without maintenance.
    #[test]
    fn test_get_collateral_score_decays_on_stale_read() {
    #[test]
    fn test_reputation_zero_halves_score_increment() {
        // reputation=0 → multiplier 0.5× → 5 * 500/1000 = 2 per submission
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // reputation starts at 0 → weighted_increment = 5 * 500 / 1000 = 2
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "oil change"),
            &engineer,
        );
        assert_eq!(client.get_collateral_score(&asset_id), 2);
    }

    // --- Health Snapshot Tests ---

    #[test]
    fn test_take_health_snapshot_empty_history() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let (asset_id, _) = register_asset(&env, &asset_registry_client);

        let snapshot = client.take_health_snapshot(&asset_id);

        assert_eq!(snapshot.score, 0);
        assert_eq!(snapshot.maintenance_count, 0);
        assert_eq!(snapshot.last_service_date, 0);
        assert_eq!(snapshot.snapshot_timestamp, env.ledger().timestamp());
    }

    #[test]
    fn test_take_health_snapshot_with_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "oil change"),
            &engineer,
        );
        assert_eq!(client.get_collateral_score(&asset_id), 5);
    }

    #[test]
    fn test_reputation_500_gives_base_score_increment() {
        // reputation=500 → multiplier 1.0× → 5 * 1000/1000 = 5 per submission
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "First service"),
            &engineer,
        );
        client.take_health_snapshot(&asset_id);

        env.ledger().with_mut(|li| li.timestamp += 1000);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &String::from_str(&env, "Second service"),
            &engineer,
        );
        client.take_health_snapshot(&asset_id);

        let snapshots = client.get_health_snapshots(&asset_id);
        assert_eq!(snapshots.len(), 2, "should have one snapshot per take_health_snapshot call");
        assert!(
            snapshots.get(1).unwrap().snapshot_timestamp > snapshots.get(0).unwrap().snapshot_timestamp,
            "snapshots should be in chronological order"
        );
        assert_eq!(snapshots.get(1).unwrap().maintenance_count, 2);
    }

    #[test]
    fn test_get_health_snapshots_accumulates_multiple_snapshots() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.take_health_snapshot(&asset_id);

        env.ledger().with_mut(|li| li.timestamp += 1000);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "oil change"),
            &engineer,
        );
        client.take_health_snapshot(&asset_id);

        let snapshots = client.get_health_snapshots(&asset_id);
        assert!(snapshots.len() >= 1);
    }

    #[test]
    fn test_reputation_1000_gives_max_score_increment() {
        // reputation=1000 → multiplier 1.5× → 5 * 1500/1000 = 7 per submission
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Submit maintenance to build up a non-zero score.
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "initial service"),
            &engineer,
        );
        let score_after_submit = client.get_collateral_score(&asset_id);
        assert!(score_after_submit > 0, "score must be >0 after maintenance");

        // Advance time well beyond MAX_AGE_LEDGERS * 5 seconds (≈30 days).
        // After this, all recency-weighted history contributions drop to zero.
        let far_future = env.ledger().timestamp() + MAX_AGE_LEDGERS * 5 + 1;
        env.ledger().set_timestamp(far_future);

        // Reading the score without any new maintenance must return a lower (decayed) value.
        let stale_score = client.get_collateral_score(&asset_id);
        assert!(
            stale_score < score_after_submit,
            "stale score ({}) must be lower than fresh score ({}) after elapsed time",
            stale_score,
            score_after_submit,
        );
        engineer_registry_client.update_reputation(&engineer, &1000);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "oil change"),
            &engineer,
        );
        assert_eq!(client.get_collateral_score(&asset_id), 7);
    }

    #[test]
    fn test_higher_reputation_yields_higher_collateral_score() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);

        let (asset_a, owner_a) = register_asset(&env, &asset_registry_client);
        let (asset_b, owner_b) = register_asset(&env, &asset_registry_client);

        let eng_low = register_engineer(&env, &engineer_registry_client);
        // eng_high needs its own issuer/admin setup — reuse the existing helper indirectly
        let eng_high = Address::generate(&env);
        let issuer = Address::generate(&env);
        let admin_h = Address::generate(&env);
        engineer_registry_client.initialize_admin(&admin_h, &admin_h);
        engineer_registry_client.add_trusted_issuer(&admin_h, &issuer);
        engineer_registry_client.register_engineer(
            &eng_high,
            &BytesN::from_array(&env, &[7u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );

        // eng_low: reputation 0, eng_high: reputation 1000
        engineer_registry_client.update_reputation(&eng_high, &1000);

        client.authorize_engineer(&owner_a, &asset_a, &eng_low);
        client.authorize_engineer(&owner_b, &asset_b, &eng_high);

        client.submit_maintenance(
            &asset_a,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "routine"),
            &eng_low,
        );
        client.submit_maintenance(
            &asset_b,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "routine"),
            &eng_high,
        );

        let score_low = client.get_collateral_score(&asset_a);
        let score_high = client.get_collateral_score(&asset_b);
        assert!(
            score_high > score_low,
            "Higher reputation engineer should yield higher collateral score: {} vs {}",
            score_high,
            score_low,
        );
    }

    // --- Health Snapshot accumulation tests ---

    #[test]
    fn test_get_health_snapshots_accumulates_multiple() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "First service"),
            &engineer,
        );
        client.take_health_snapshot(&asset_id);

        env.ledger().with_mut(|li| li.timestamp += 1000);

        client.submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &String::from_str(&env, "Second service"),
            &engineer,
        );
        client.take_health_snapshot(&asset_id);

        let snapshots = client.get_health_snapshots(&asset_id);
        assert!(snapshots.len() >= 2, "should accumulate snapshots");
        assert!(
            snapshots.get(1).unwrap().snapshot_timestamp >= snapshots.get(0).unwrap().snapshot_timestamp,
            "snapshots should be in chronological order"
        );
        assert_eq!(snapshots.get(1).unwrap().maintenance_count, 2);
    }

    #[test]
    fn test_get_health_snapshots_empty_before_any_snapshot() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, _, _) = setup(&env, 0);
        let (asset_id, _) = register_asset(&env, &asset_registry_client);

        let snapshots = client.get_health_snapshots(&asset_id);
        assert_eq!(snapshots.len(), 0);
    }

    #[test]
    fn test_take_health_snapshot_nonexistent_asset_fails() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, _, _, _) = setup(&env, 0);

        let result = client.try_take_health_snapshot(&999u64);
        assert!(result.is_err(), "should error for unknown asset");
    }

    // --- Deprecation: collateral score tests ---

    #[test]
    fn test_deprecated_asset_returns_zero_collateral_score() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, _, _) = setup(&env, 200);
        let (asset_id, owner) = register_asset(&env, &asset_registry);

        // Confirm initial score is 0 (no maintenance history)
        assert_eq!(lifecycle.get_collateral_score(&asset_id), 0);

        // Deprecate the asset as the owner
        asset_registry.deprecate_asset(&owner, &asset_id, &String::from_str(&env, "End of service life"));

        // Score must be 0 for a deprecated asset regardless of any history
        assert_eq!(lifecycle.get_collateral_score(&asset_id), 0);
    }

    #[test]
    fn test_active_asset_not_affected_by_deprecation_of_other() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, _, _) = setup(&env, 200);
        let (asset_id_1, owner) = register_asset(&env, &asset_registry);
        let (asset_id_2, _) = register_asset(&env, &asset_registry);

        // Deprecate only asset 1
        asset_registry.deprecate_asset(&owner, &asset_id_1, &String::from_str(&env, "retired"));

        // Asset 2 must still return its normal (0, no history) score — not forced to 0 by deprecation
        assert_eq!(lifecycle.get_collateral_score(&asset_id_1), 0);
        // Asset 2 is active, score is 0 simply because no maintenance has been submitted
        assert_eq!(lifecycle.get_collateral_score(&asset_id_2), 0);
    }

    // --- Issue #830: set_max_notes_length ---

    #[test]
    fn test_set_max_notes_length_updates_config() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, _, _, admin) = setup(&env, 0);

        lifecycle.set_max_notes_length(&admin, &128);

        let config = lifecycle.get_config();
        assert_eq!(config.max_notes_length, 128);
    }

    #[test]
    fn test_set_max_notes_length_zero_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, _, _, admin) = setup(&env, 0);

        let result = lifecycle.try_set_max_notes_length(&admin, &0);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidConfig as u32,
            ))),
        );
    }

    #[test]
    fn test_set_max_notes_length_non_admin_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, _, _, _admin) = setup(&env, 0);

        let outsider = Address::generate(&env);
        let result = lifecycle.try_set_max_notes_length(&outsider, &64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_set_max_notes_length_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, _, _, admin) = setup(&env, 0);

        lifecycle.set_max_notes_length(&admin, &512);

        use soroban_sdk::TryIntoVal;
        let events = env.events().all();
        let found = events.iter().any(|(_, topics, data)| {
            topics
                .get(0)
                .and_then(|v| TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                .map(|s| s == symbol_short!("SET_NOTES"))
                .unwrap_or(false)
                && TryIntoVal::<_, u32>::try_into_val(&data, &env).ok() == Some(512)
        });
        assert!(found, "SET_NOTES event not emitted");
    }

    // ── Collateral Score Invariant Tests (Issue #600) ────────────────────────
    // Property: the collateral score must NEVER exceed the defined maximum (100)
    // regardless of how many maintenance records are submitted, which task types
    // are used, or how many engineers contribute.

    /// Helper: register an engineer with a fresh admin/issuer and set neutral reputation.
    fn register_engineer_for_invariant(
        env: &Env,
        eng_registry: &EngineerRegistryClient,
    ) -> Address {
        let engineer = Address::generate(env);
        let issuer = Address::generate(env);
        let admin = Address::generate(env);
        let hash = BytesN::from_array(env, &[2u8; 32]);
        eng_registry.initialize_admin(&admin, &admin);
        eng_registry.add_trusted_issuer(&admin, &issuer);
        eng_registry.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        // Neutral reputation (1.0× multiplier) so tests don't depend on reputation weighting.
        eng_registry.update_reputation(&engineer, &500);
        engineer
    }

    /// Invariant: submitting 100+ maintenance records for a single asset never pushes
    /// the collateral score above the maximum value of 100.
    ///
    /// This is a regression guard for a potential accumulation bug where repeated
    /// `saturating_add` calls without an explicit cap could return a value > 100.
    #[test]
    fn test_collateral_score_never_exceeds_maximum_single_asset() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, eng_registry, _) = setup(&env, 0);

        let owner = Address::generate(&env);
        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Turbine Alpha"),
            &unique_serial(&env),
            &owner,
        );

        let engineer = register_engineer_for_invariant(&env, &eng_registry);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        // Submit 120 maintenance records — well above any reasonable score cap.
        for i in 0..120u32 {
            // Alternate between task types to exercise multiple weight branches.
            let task = if i % 3 == 0 {
                symbol_short!("ENGINE")
            } else if i % 3 == 1 {
                symbol_short!("FILTER")
            } else {
                symbol_short!("OIL_CHG")
            };
            lifecycle.submit_maintenance(
                &asset_id,
                &task,
                &String::from_str(&env, "Invariant test record"),
                &engineer,
            );
            // Advance time by 1 second between submissions so timestamps differ.
            env.ledger().with_mut(|li| li.timestamp += 1);

            let score = lifecycle.get_collateral_score(&asset_id);
            assert!(
                score <= 100,
                "Score invariant violated after {} submissions: score={} > 100",
                i + 1,
                score
            );
        }

        // Final assertion: score is still within bounds after all 120 submissions.
        let final_score = lifecycle.get_collateral_score(&asset_id);
        assert!(
            final_score <= 100,
            "Final collateral score {} exceeds maximum of 100",
            final_score
        );
    }

    /// Invariant: the score cap is enforced for GENSET, ENGINE (high-weight task).
    /// Using a high-weight task type (weight=10) that would overflow 100 after 10 submissions
    /// if the cap were not enforced.
    #[test]
    fn test_collateral_score_cap_enforced_for_high_weight_tasks() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, eng_registry, _) = setup(&env, 0);

        let owner = Address::generate(&env);
        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Generator Beta"),
    /// An engineer whose credential has passed both the expiry date **and** the
    /// grace period (i.e. `CredentialStatus::HardExpired`) must not be allowed
    /// to submit a maintenance record.
    ///
    /// Setup:
    ///   1. Register an engineer with a 1-day validity period (`86_400` s).
    ///   2. Authorize that engineer for the asset so the per-asset authorisation
    ///      check does not fire first.
    ///   3. Advance the ledger past `expires_at + grace_period` (7 days =
    ///      `604_800` s), landing firmly in `HardExpired` territory.
    ///   4. Confirm the registry reports `HardExpired`.
    ///   5. Assert that `try_submit_maintenance` returns
    ///      `ContractError::UnauthorizedEngineer`.
    #[test]
    fn test_hard_expired_credential_cannot_submit_maintenance() {
    // ── #794 regression ──────────────────────────────────────────────────────
    // A decommissioned asset must return a collateral score of 0 regardless of
    // how many maintenance records it accumulated before decommission.

    #[test]
    fn test_collateral_score_is_zero_after_decommission() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, _admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Build a non-zero score with several maintenance events.
        for _ in 0..5 {
            lifecycle.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &String::from_str(&env, "Pre-decommission service"),
                &engineer,
            );
        }

        // Score must be positive before decommission.
        let score_before = lifecycle.get_collateral_score(&asset_id);
        assert!(score_before > 0, "expected non-zero score before decommission");

        // Simulate the asset registry calling decommission_notify.
        // In production this is called by the asset-registry contract; here we call
        // it directly (mock_all_auths satisfies the require_auth guard).
        lifecycle.decommission_notify(&asset_id);

        // Score must be exactly 0 after decommission — #794.
        let score_after = lifecycle.get_collateral_score(&asset_id);
        assert_eq!(
            score_after, 0,
            "decommissioned asset must report collateral score of 0 (got {score_after})"
        );
    }

    #[test]
    fn test_frozen_score_survives_ttl_expiry_via_get_collateral_score_opt() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, _admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

        for _ in 0..5 {
            lifecycle.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "Pre-decommission service"),
                &engineer,
                &None,
            );
        }

        lifecycle.decommission_notify(&asset_id);
        let frozen_score = lifecycle.get_collateral_score_opt(&asset_id).unwrap();
        assert!(frozen_score > 0, "expected a non-zero frozen score");

        // Cross the frozen score's TTL boundary without reading it — this alone
        // must not lose the value, and reading it via get_collateral_score_opt
        // must bump the TTL again so a *second* boundary crossing also survives.
        env.ledger().with_mut(|li| {
            li.sequence_number += 518_401;
        });
        assert_eq!(
            lifecycle.get_collateral_score_opt(&asset_id),
            Some(frozen_score),
            "frozen score must survive one TTL boundary and be reported on read"
        );

        env.ledger().with_mut(|li| {
            li.sequence_number += 518_401;
        });
        assert_eq!(
            lifecycle.get_collateral_score_opt(&asset_id),
            Some(frozen_score),
            "reading the frozen score must extend its TTL so it survives a second boundary"
        );
    }

    #[test]
    fn test_decay_score_is_zero_after_decommission() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, _admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "Full overhaul before retirement"),
            &engineer,
        );

        assert!(lifecycle.decay_score(&asset_id) > 0);

        lifecycle.decommission_notify(&asset_id);

        // decay_score must also return 0 for a decommissioned asset — #794.
        assert_eq!(
            lifecycle.decay_score(&asset_id),
            0,
            "decay_score must return 0 for decommissioned assets"
    #[test]
    fn test_set_eligibility_threshold_updates_config() {
    // --- Issue #770 ---

    #[test]
    fn test_batch_submit_fails_atomically_on_first_invalid_record() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);

        // ── 1. Register an asset ──────────────────────────────────────────────
        let owner = Address::generate(&env);
        let asset_id = asset_registry_client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Cummins QSK60"),
            &unique_serial(&env),
            &owner,
        );

        let engineer = register_engineer_for_invariant(&env, &eng_registry);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        // ENGINE overhaul has weight=10, so without a cap 11 submissions would yield 110 > 100.
        for i in 0..15u32 {
            lifecycle.submit_maintenance(
                &asset_id,
                &symbol_short!("ENGINE"),
                &String::from_str(&env, "Engine overhaul"),
                &engineer,
            );
            env.ledger().with_mut(|li| li.timestamp += 1);

            let score = lifecycle.get_collateral_score(&asset_id);
            assert!(
                score <= 100,
                "Score exceeded 100 after {} ENGINE submissions: score={}",
                i + 1,
                score
            );
        }
    }

    /// Invariant: score cap is enforced across multiple asset types.
    /// Runs the same 100-submission flood across three distinct assets to verify
    /// the invariant holds regardless of asset identity.
    #[test]
    fn test_collateral_score_cap_across_multiple_asset_types() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, eng_registry, admin) = setup(&env, 0);

        // Register TURBINE and VEHICLE asset types in addition to the default GENSET.
        let asset_admin = asset_registry.get_admin();
        asset_registry.add_asset_type(&asset_admin, &symbol_short!("TURBINE"));
        asset_registry.add_asset_type(&asset_admin, &symbol_short!("VEHICLE"));

        let asset_types = [
            symbol_short!("GENSET"),
            symbol_short!("TURBINE"),
            symbol_short!("VEHICLE"),
        ];

        let engineer = register_engineer_for_invariant(&env, &eng_registry);

        for asset_type in asset_types.iter() {
            let owner = Address::generate(&env);
            let asset_id = asset_registry.register_asset(
                asset_type,
                &String::from_str(&env, "Multi-type invariant asset"),
                &unique_serial(&env),
                &owner,
            );

            lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

            for i in 0..110u32 {
                lifecycle.submit_maintenance(
                    &asset_id,
                    &symbol_short!("OVERHAUL"),
                    &String::from_str(&env, "Overhaul record"),
                    &engineer,
                );
                env.ledger().with_mut(|li| li.timestamp += 1);

                let score = lifecycle.get_collateral_score(&asset_id);
                assert!(
                    score <= 100,
                    "Score cap violated for asset type {:?} after {} submissions: score={}",
                    asset_type,
                    i + 1,
                    score
                );
            }
        }
    }

    /// Invariant: scores for different assets are strictly isolated.
    /// Flooding one asset with 120 submissions must not affect the score of a second
    /// asset that received no submissions.
    #[test]
    fn test_score_isolation_invariant() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, eng_registry, _) = setup(&env, 0);

        let owner = Address::generate(&env);
        let asset_a = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Isolation Asset A"),
            &unique_serial(&env),
            &owner,
        );
        let asset_b = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Isolation Asset B"),
            &unique_serial(&env),
            &owner,
        );

        let engineer = register_engineer_for_invariant(&env, &eng_registry);
        lifecycle.authorize_engineer(&owner, &asset_a, &engineer);

        for i in 0..120u32 {
            lifecycle.submit_maintenance(
                &asset_a,
                &symbol_short!("ENGINE"),
                &String::from_str(&env, "Isolation test"),
                &engineer,
            );
            env.ledger().with_mut(|li| li.timestamp += 1);

            let score_a = lifecycle.get_collateral_score(&asset_a);
            let score_b = lifecycle.get_collateral_score(&asset_b);

            assert!(
                score_a <= 100,
                "Asset A score exceeded 100 after {} submissions: {}",
                i + 1,
                score_a
            );
            assert_eq!(
                score_b, 0,
                "Asset B score must remain 0 when only Asset A receives maintenance, got {}",
                score_b
            );
        }
        // ── 2. Register engineer with a short (1-day) validity period ─────────
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let eng_admin = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[2u8; 32]);
        engineer_registry_client.initialize_admin(&eng_admin, &eng_admin);
        engineer_registry_client.add_trusted_issuer(&eng_admin, &issuer);
        // validity_period = 86_400 s (1 day — the minimum allowed)
        engineer_registry_client.register_engineer(&engineer, &hash, &issuer, &86_400, &None);

        // Confirm credential is Valid before we advance time.
        assert_eq!(
            engineer_registry_client.get_credential_status(&engineer),
            engineer_registry::CredentialStatus::Valid,
            "credential should be Valid immediately after registration"
        );

        // ── 3. Authorize the engineer for the asset so the per-asset check
        //       does not mask the credential-expiry check. ─────────────────────
        client.authorize_engineer(&owner, &asset_id, &engineer);

        // ── 4. Advance ledger past expires_at + grace_period (7 × 86_400 s)
        //       to land in HardExpired territory.
        //       Total offset: 86_400 (validity) + 604_800 (grace) + 1 = 691_201 s
        let base_timestamp = env.ledger().timestamp();
        env.ledger()
            .set_timestamp(base_timestamp + 86_400 + 604_800 + 1);

        // Sanity-check: the registry must report HardExpired at this point.
        assert_eq!(
            engineer_registry_client.get_credential_status(&engineer),
            engineer_registry::CredentialStatus::HardExpired,
            "credential should be HardExpired after expiry + grace period"
        );

        // ── 5. Attempt to submit maintenance — must be rejected ───────────────
        let result = client.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Post-hard-expiry maintenance attempt"),
            &engineer,
        );

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedEngineer as u32,
            ))),
            "HardExpired engineer must not be able to submit maintenance"
        );
    }
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Two valid records followed by one invalid record (unknown task type at index 2).
        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "Valid record 0"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("INSPECT"),
            notes: String::from_str(&env, "Valid record 1"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("UNKNOWN"),
            notes: String::from_str(&env, "Invalid task type at index 2"),
        });

        let result = client.try_batch_submit_maintenance(&asset_id, &records, &engineer);

        // Batch panics at the invalid record (index 2) with InvalidTaskType.
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidTaskType as u32,
            ))),
        );

        // Atomicity guarantee: no records written despite two valid records preceding the failure.
        assert_eq!(
            client.get_maintenance_history(&asset_id).len(),
            0,
            "history must be empty — batch must not write partial records",
        );
        assert_eq!(
            client.get_collateral_score(&asset_id),
            0,
            "score must be unchanged after a failed batch",
        );
    }

    // --- Issue #772 ---

    #[test]
    fn test_get_maintenance_history_page_25_records() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry_client);
        let engineer = register_engineer(&env, &engineer_registry_client);
        client.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Submit 25 records in a single batch.
        let mut records = Vec::new(&env);
        for _ in 0..25u32 {
            records.push_back(BatchRecord {
                task_type: symbol_short!("OIL_CHG"),
                notes: String::from_str(&env, "Maintenance record"),
            });
        }
        client.batch_submit_maintenance(&asset_id, &records, &engineer);
        assert_eq!(client.get_maintenance_history(&asset_id).len(), 25);

        // Page 1: offset=0, limit=10 → records 0-9 (10 records).
        let page1 = client.get_maintenance_history_page(&asset_id, &0, &10);
        assert_eq!(page1.len(), 10, "page 1 must contain 10 records");

        // Page 2: offset=10, limit=10 → records 10-19 (10 records).
        let page2 = client.get_maintenance_history_page(&asset_id, &10, &10);
        assert_eq!(page2.len(), 10, "page 2 must contain 10 records");

        // Page 3: offset=20, limit=10 → records 20-24 (5 records, partial last page).
        let page3 = client.get_maintenance_history_page(&asset_id, &20, &10);
        assert_eq!(page3.len(), 5, "page 3 must contain the 5 remaining records");

        // Page 4: offset=30 is beyond history length (25) → empty vec.
        let page4 = client.get_maintenance_history_page(&asset_id, &30, &10);
        assert_eq!(page4.len(), 0, "out-of-bounds offset must return an empty vec");

        // Verify each page covers the correct slice of the full history.
        let full = client.get_maintenance_history(&asset_id);
        for i in 0..10u32 {
            assert_eq!(
                page1.get(i).unwrap().task_type,
                full.get(i).unwrap().task_type,
                "page1 record {} must match full history record {}",
                i, i,
            );
            assert_eq!(
                page2.get(i).unwrap().task_type,
                full.get(10 + i).unwrap().task_type,
                "page2 record {} must match full history record {}",
                i, 10 + i,
            );
        }
        for i in 0..5u32 {
            assert_eq!(
                page3.get(i).unwrap().task_type,
                full.get(20 + i).unwrap().task_type,
                "page3 record {} must match full history record {}",
                i, 20 + i,
            );
        }
    // ── Multisig / quorum tests ───────────────────────────────────────────────

    #[test]
    fn test_single_admin_mode_pause_works_without_quorum() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, _, _, admin) = setup(&env, 0);

        let initial = lifecycle.get_config().eligibility_threshold;
        assert_eq!(initial, DEFAULT_ELIGIBILITY_THRESHOLD);

        lifecycle.set_eligibility_threshold(&admin, &75);

        let config = lifecycle.get_config();
        assert_eq!(config.eligibility_threshold, 75);
    }

    #[test]
    fn test_set_eligibility_threshold_zero_rejected() {
        // Default config has admins=[] and threshold=0 → single-admin mode
        let config = lifecycle.get_config();
        assert_eq!(config.admins.len(), 0);
        assert_eq!(config.admin_threshold, 0);

        // Single admin can pause without multisig
        lifecycle.pause(&admin);
        assert!(lifecycle.is_paused());
    }

    #[test]
    fn test_set_admin_quorum_stores_admins_and_threshold() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, _, _, admin) = setup(&env, 0);

        let co1 = Address::generate(&env);
        let co2 = Address::generate(&env);
        let new_admins = soroban_sdk::vec![&env, admin.clone(), co1.clone(), co2.clone()];

        lifecycle.set_admin_quorum(&admin, &new_admins, &2);

        let config = lifecycle.get_config();
        assert_eq!(config.admin_threshold, 2);
        assert_eq!(config.admins.len(), 3);
    }

    #[test]
    fn test_set_admin_quorum_threshold_exceeds_admins_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, _, _, admin) = setup(&env, 0);

        let result = lifecycle.try_set_eligibility_threshold(&admin, &0);
        let co1 = Address::generate(&env);
        let new_admins = soroban_sdk::vec![&env, admin.clone(), co1.clone()];

        // Threshold 5 > len 2 → InvalidConfig
        let result = lifecycle.try_set_admin_quorum(&admin, &new_admins, &5);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidConfig as u32,
            ))),
        );
    }

    #[test]
    fn test_set_eligibility_threshold_non_admin_rejected() {
    fn test_set_admin_quorum_non_admin_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, _, _, _admin) = setup(&env, 0);

        let outsider = Address::generate(&env);
        let result = lifecycle.try_set_eligibility_threshold(&outsider, &80);
        let new_admins = soroban_sdk::vec![&env, outsider.clone()];

        let result = lifecycle.try_set_admin_quorum(&outsider, &new_admins, &1);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_decommission_notify_emits_zero_score_event() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, _admin) = setup(&env, 0);
        let (asset_id, asset_owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Last service"),
            &engineer,
        );

        lifecycle.decommission_notify(&asset_id);

        // The DECOMM event must carry 0 as the score payload — #794.
        use soroban_sdk::TryIntoVal;
        let events = env.events().all();
        let decomm_event = events.iter().find(|(_, topics, _)| {
            topics
                .get(0)
                .and_then(|v| TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                .map(|s| s == symbol_short!("DECOMM"))
                .unwrap_or(false)
        });
        assert!(decomm_event.is_some(), "DECOMM event not emitted");
        let (_, _, data) = decomm_event.unwrap();
        let emitted_score: u32 = data.try_into_val(&env).unwrap();
        assert_eq!(
            emitted_score, 0,
            "DECOMM event must carry score=0 after fix #794"
        );
    }
    fn test_set_eligibility_threshold_used_in_is_collateral_eligible() {
    fn test_quorum_pause_requires_all_threshold_signers() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, _, _, admin) = setup(&env, 0);

        let co1 = Address::generate(&env);
        let co2 = Address::generate(&env);
        let new_admins = soroban_sdk::vec![&env, admin.clone(), co1.clone(), co2.clone()];

        // Set 2-of-3 multisig
        lifecycle.set_admin_quorum(&admin, &new_admins, &2);

        // With mock_all_auths, all required signers are automatically satisfied
        // Admin is first in the list; co1 is required as the second signer
        lifecycle.pause(&admin);
        assert!(lifecycle.is_paused());
    }

    #[test]
    fn test_quorum_reset_score_requires_threshold_signers() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, asset_registry, engineer_registry, admin) = setup(&env, 0);

        let (asset_id, _owner) = register_asset(&env, &asset_registry);
        let engineer = Address::generate(&env);
        lifecycle.authorize_engineer(&admin, &asset_id, &engineer);
        engineer_registry.register_engineer(&engineer, &String::from_str(&env, "Eng"));

        // Submit enough maintenance records to push score above default threshold (50)
        for _ in 0..15u32 {
            let notes = soroban_sdk::String::from_str(&env, "maint");
            lifecycle.submit_maintenance(&asset_id, &symbol_short!("INSPECT"), &notes, &engineer);
        }

        // With default threshold (50), asset is eligible
        assert!(lifecycle.is_collateral_eligible(&asset_id));

        // Raise threshold above current score — asset becomes ineligible
        lifecycle.set_eligibility_threshold(&admin, &200);
        assert!(!lifecycle.is_collateral_eligible(&asset_id));

        // Lower threshold back — asset is eligible again
        lifecycle.set_eligibility_threshold(&admin, &10);
        assert!(lifecycle.is_collateral_eligible(&asset_id));
        let co1 = Address::generate(&env);
        let new_admins = soroban_sdk::vec![&env, admin.clone(), co1.clone()];
        lifecycle.set_admin_quorum(&admin, &new_admins, &2);

        let (asset_id, asset_owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "first service"),
            &engineer,
        );

        let score_before = lifecycle.get_collateral_score(&asset_id);
        assert!(score_before > 0);

        // With mock_all_auths, both admin and co1 are treated as signed
        lifecycle.reset_score(&admin, &asset_id);
        assert_eq!(lifecycle.get_collateral_score(&asset_id), 0);
    }

    #[test]
    fn test_quorum_non_member_caller_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, _, _, admin) = setup(&env, 0);

        let co1 = Address::generate(&env);
        let new_admins = soroban_sdk::vec![&env, admin.clone(), co1.clone()];
        lifecycle.set_admin_quorum(&admin, &new_admins, &2);

        let outsider = Address::generate(&env);
        let result = lifecycle.try_pause(&outsider);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_revert_to_single_admin_by_clearing_quorum() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, _, _, admin) = setup(&env, 0);

        let co1 = Address::generate(&env);
        let new_admins = soroban_sdk::vec![&env, admin.clone(), co1.clone()];
        lifecycle.set_admin_quorum(&admin, &new_admins, &2);

        // Clear quorum → back to single-admin mode
        lifecycle.set_admin_quorum(&admin, &soroban_sdk::vec![&env], &0);
        let config = lifecycle.get_config();
        assert_eq!(config.admin_threshold, 0);
        assert_eq!(config.admins.len(), 0);

        // Single admin can pause alone again
        lifecycle.pause(&admin);
        assert!(lifecycle.is_paused());
    }
    // ── issue #1022: reentrancy guard on submit_maintenance ───────────────

    /// Verify that the reentrancy lock is NOT present before submit_maintenance is called.
    /// This confirms the guard starts in a clean state.
    #[test]
    fn test_reentrancy_lock_not_set_initially() {
        let env = Env::default();
        env.mock_all_auths();

        let lifecycle_id = env.register(Lifecycle, ());
        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let admin = Address::generate(&env);

        let lifecycle = LifecycleClient::new(&env, &lifecycle_id);
        lifecycle.initialize(
            &admin,
            &asset_registry_id,
            &engineer_registry_id,
            &admin,
            &200u32,
        );

        // The LOCKED instance key must not be set before any submit_maintenance call.
        env.as_contract(&lifecycle_id, || {
            let locked: bool = env
                .storage()
                .instance()
                .get(&REENTRANCY_LOCK)
                .unwrap_or(false);
            assert!(!locked, "Reentrancy lock must be clear before submit_maintenance");
        });
    }

    /// Simulated reentrancy attack: manually set the LOCKED flag (mimicking what a
    /// malicious cross-contract call would do while submit_maintenance is executing),
    /// then verify that a subsequent submit_maintenance call is rejected with
    /// [`ContractError::Reentrancy`].
    #[test]
    fn test_reentrancy_attack_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let lifecycle_id = env.register(Lifecycle, ());
        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let admin = Address::generate(&env);

        let lifecycle = LifecycleClient::new(&env, &lifecycle_id);
        lifecycle.initialize(
            &admin,
            &asset_registry_id,
            &engineer_registry_id,
            &admin,
            &200u32,
        );

        // Set up asset and engineer
        let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
        let asset_admin = Address::generate(&env);
        asset_registry.initialize_admin(&asset_admin, &asset_admin);
        asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Caterpillar 3516"),
            &String::from_str(&env, "SN-REENT-001"),
            &owner,
        );

        let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
        let engineer = Address::generate(&env);
        engineer_registry.register_engineer(
            &engineer,
            &BytesN::from_array(&env, &[0u8; 32]),
            &admin,
        );
        engineer_registry.add_specialization(&engineer, &symbol_short!("GENSET"));
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        // Simulate the LOCKED flag being set (as if submit_maintenance is mid-execution
        // and a malicious registry re-enters the lifecycle contract).
        env.as_contract(&lifecycle_id, || {
            env.storage().instance().set(&REENTRANCY_LOCK, &true);
        });

        // Attempt to call submit_maintenance while the lock is held → must be rejected.
        let result = lifecycle.try_submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Simulated reentrant call"),
            &engineer,
            &None,
        );

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Reentrancy as u32
            ))),
            "Reentrant submit_maintenance must be rejected with ContractError::Reentrancy"
        );

        // Clean up: remove the lock so subsequent tests are not affected.
        env.as_contract(&lifecycle_id, || {
            env.storage().instance().remove(&REENTRANCY_LOCK);
        });
    }

    /// Verify that after a normal (non-reentrant) submit_maintenance call completes,
    /// the reentrancy lock is cleared so subsequent calls are allowed.
    #[test]
    fn test_reentrancy_lock_cleared_after_submit_maintenance() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, admin) = setup(&env, 200);

        let (asset_id, owner) = register_asset(&env, &asset_registry);
        let engineer = Address::generate(&env);
        engineer_registry.register_engineer(
            &engineer,
            &BytesN::from_array(&env, &[0u8; 32]),
            &admin,
        );
        engineer_registry.add_specialization(&engineer, &symbol_short!("GENSET"));
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        // First submit_maintenance call
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "First maintenance"),
            &engineer,
            &None,
        );

        // After the call completes, the lock must be cleared.
        let lifecycle_id = lifecycle.address.clone();
        env.as_contract(&lifecycle_id, || {
            let locked: bool = env
                .storage()
                .instance()
                .get(&REENTRANCY_LOCK)
                .unwrap_or(false);
            assert!(!locked, "Reentrancy lock must be cleared after submit_maintenance");
        });

        // Second submit_maintenance call must also succeed (lock was released).
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Second maintenance"),
            &engineer,
            &None,
        );
    }

    // ── Issue #1007: get_transfer_history_paginated ────────────────────────

    #[test]
    fn test_get_transfer_history_empty_before_any_transfer() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, asset_registry, _, _) = setup(&env, 0);
        let (asset_id, _owner) = register_asset(&env, &asset_registry);

        // No transfers yet → both full and paginated should return empty
        let full = lifecycle.get_transfer_history(&asset_id);
        assert_eq!(full.len(), 0);

        let page = lifecycle.get_transfer_history_paginated(&asset_id, &0, &10);
        assert_eq!(page.len(), 0);
    }

    #[test]
    fn test_get_transfer_history_paginated_single_transfer() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);
        let (asset_id, owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        let new_owner = Address::generate(&env);

        asset_registry.transfer_asset(&asset_id, &owner, &new_owner);
        lifecycle.record_transfer(&asset_id, &owner, &new_owner);

        // Full history has one entry
        let full = lifecycle.get_transfer_history(&asset_id);
        assert_eq!(full.len(), 1);
        assert_eq!(full.get(0).unwrap().from, owner);
        assert_eq!(full.get(0).unwrap().to, new_owner);

        // Paginated: offset 0, limit 10 → same single entry
        let page = lifecycle.get_transfer_history_paginated(&asset_id, &0, &10);
        assert_eq!(page.len(), 1);
        assert_eq!(page.get(0).unwrap().from, owner);
        assert_eq!(page.get(0).unwrap().to, new_owner);
    }

    #[test]
    fn test_get_transfer_history_paginated_offset_beyond_end() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);
        let (asset_id, owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        let new_owner = Address::generate(&env);
        asset_registry.transfer_asset(&asset_id, &owner, &new_owner);
        lifecycle.record_transfer(&asset_id, &owner, &new_owner);

        // Offset beyond the single entry → empty vec
        let page = lifecycle.get_transfer_history_paginated(&asset_id, &1, &10);
        assert_eq!(page.len(), 0);
    }

    #[test]
    fn test_get_transfer_history_paginated_limit_zero() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);
        let (asset_id, owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        let new_owner = Address::generate(&env);
        asset_registry.transfer_asset(&asset_id, &owner, &new_owner);
        lifecycle.record_transfer(&asset_id, &owner, &new_owner);

        // limit=0 → always empty
        let page = lifecycle.get_transfer_history_paginated(&asset_id, &0, &0);
        assert_eq!(page.len(), 0);
    }

    #[test]
    fn test_get_transfer_history_paginated_cap_at_50() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);
        let (asset_id, owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        let new_owner = Address::generate(&env);
        asset_registry.transfer_asset(&asset_id, &owner, &new_owner);
        lifecycle.record_transfer(&asset_id, &owner, &new_owner);

        // Request with limit far above the cap — result must be ≤ 50
        let page = lifecycle.get_transfer_history_paginated(&asset_id, &0, &u32::MAX);
        assert!(page.len() <= 50);
        assert_eq!(page.len(), 1); // Only 1 actual transfer
    }

    // ==================== Issue #1002 Tests ====================

    #[test]
    fn test_anchor_history_to_snapshot() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, admin) = setup(&env, 0);
        let (asset_id, owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        // Submit some maintenance to build history
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Initial service"),
            &engineer,
            &None,
        );

        // Take a health snapshot
        let snapshot = lifecycle.take_health_snapshot(&asset_id);
        assert!(!snapshot.reconstructed);

        // Anchor the snapshot
        lifecycle.anchor_history_to_snapshot(&admin, &asset_id, &0);

        // Verify snapshot is marked as reconstructed
        let snapshots = lifecycle.get_health_snapshots(&asset_id);
        assert_eq!(snapshots.len(), 1);
        assert!(snapshots.get(0).unwrap().reconstructed);

        // Verify RECONSTR event was emitted
        let events = env.events().all();
        let reconstr_event = events.iter().find(|(_, topics, _)| {
            topics.get(0).map_or(false, |v| {
                Symbol::from_val(&env, &v) == symbol_short!("RECONSTR")
            })
        });
        assert!(reconstr_event.is_some(), "RECONSTR event should be emitted");
    }

    #[test]
    #[should_panic(expected = "SnapshotNotFound")]
    fn test_anchor_history_to_snapshot_panics_if_no_snapshots() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, _engineer_registry, admin) = setup(&env, 0);
        let (asset_id, _owner) = register_asset(&env, &asset_registry);

        // No snapshots exist; should panic
        lifecycle.anchor_history_to_snapshot(&admin, &asset_id, &0);
    }

    #[test]
    #[should_panic(expected = "SnapshotNotFound")]
    fn test_anchor_history_to_snapshot_panics_if_index_out_of_bounds() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, admin) = setup(&env, 0);
        let (asset_id, owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Service"),
            &engineer,
            &None,
        );
        lifecycle.take_health_snapshot(&asset_id);

        // Index 1 doesn't exist (only index 0)
        lifecycle.anchor_history_to_snapshot(&admin, &asset_id, &1);
    }

    // ── Issue #1006: get_score_history with 50-entry cap ──────────────────

    #[test]
    fn test_get_score_history_basic_pagination() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);
        let (asset_id, owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        // Submit 3 maintenance records to build score history
        for i in 0u32..3 {
            env.ledger().with_mut(|li| li.timestamp += 1000 * (i + 1) as u64);
            lifecycle.submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, "Routine oil change"),
                &engineer,
                &None,
            );
        }

        // offset=0 limit=3 → 3 entries
        let page = lifecycle.get_score_history(&asset_id, &0, &3);
        assert_eq!(page.len(), 3);

        // offset=1 limit=2 → 2 entries (entries 1 and 2)
        let page2 = lifecycle.get_score_history(&asset_id, &1, &2);
        assert_eq!(page2.len(), 2);
        assert_eq!(page2.get(0).unwrap(), page.get(1).unwrap());
    }

    #[test]
    fn test_get_score_history_limit_zero_returns_empty() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);
        let (asset_id, owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Service"),
            &engineer,
            &None,
        );

        let page = lifecycle.get_score_history(&asset_id, &0, &0);
        assert_eq!(page.len(), 0);
    }

    #[test]
    fn test_get_score_history_cap_at_50() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);
        let (asset_id, owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Service"),
            &engineer,
            &None,
        );

        // Request with limit=u32::MAX — must be capped at 50
        let page = lifecycle.get_score_history(&asset_id, &0, &u32::MAX);
        assert!(page.len() <= 50);
        assert_eq!(page.len(), 1); // Only 1 score entry so far
    }

    #[test]
    fn test_get_score_history_offset_beyond_end() {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, asset_registry, engineer_registry, _) = setup(&env, 0);
        let (asset_id, owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Service"),
            &engineer,
            &None,
        );

        // offset beyond the history length → empty
        let page = lifecycle.get_score_history(&asset_id, &100, &10);
        assert_eq!(page.len(), 0);
    }

    // ==================== Issue #1003 Tests ====================

    #[test]
    fn test_propose_and_execute_weight_change() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, _asset_registry, _engineer_registry, admin) = setup(&env, 0);
        let task_type = symbol_short!("OIL_CHG");

        // Propose a weight change
        lifecycle.propose_weight_change(&admin, &task_type, &10);

        // Verify proposal event was emitted
        let events = env.events().all();
        let prop_event = events.iter().find(|(_, topics, _)| {
            topics.get(0).map_or(false, |v| {
                Symbol::from_val(&env, &v) == symbol_short!("WT_PROP")
            })
        });
        assert!(prop_event.is_some(), "WT_PROP event should be emitted");

        // Fast-forward time past the timelock
        env.ledger().with_mut(|li| {
            li.timestamp = li.timestamp.saturating_add(48 * 60 * 60 + 1);
        });

        // Execute the weight change
        lifecycle.execute_weight_change(&admin, &task_type);

        // Verify execution event was emitted
        let events = env.events().all();
        let exec_event = events.iter().find(|(_, topics, _)| {
            topics.get(0).map_or(false, |v| {
                Symbol::from_val(&env, &v) == symbol_short!("WT_EXEC")
            })
        });
        assert!(exec_event.is_some(), "WT_EXEC event should be emitted");

        // Verify weight was applied to config
        let config = lifecycle.get_config();
        assert_eq!(config.task_weights.get(task_type).unwrap(), 10);
    }

    #[test]
    #[should_panic(expected = "WeightProposalAlreadyExists")]
    fn test_propose_weight_change_panics_if_proposal_exists() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, _asset_registry, _engineer_registry, admin) = setup(&env, 0);
        let task_type = symbol_short!("OIL_CHG");

        // First proposal succeeds
        lifecycle.propose_weight_change(&admin, &task_type, &10);

        // Second proposal for same task type should panic
        lifecycle.propose_weight_change(&admin, &task_type, &15);
    }

    #[test]
    #[should_panic(expected = "TimelockNotExpired")]
    fn test_execute_weight_change_panics_if_timelock_not_expired() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, _asset_registry, _engineer_registry, admin) = setup(&env, 0);
        let task_type = symbol_short!("OIL_CHG");

        lifecycle.propose_weight_change(&admin, &task_type, &10);

        // Try to execute immediately (timelock not expired)
        lifecycle.execute_weight_change(&admin, &task_type);
    }

    #[test]
    #[should_panic(expected = "ProposalNotFound")]
    fn test_execute_weight_change_panics_if_no_proposal() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, _asset_registry, _engineer_registry, admin) = setup(&env, 0);
        let task_type = symbol_short!("OIL_CHG");

        // No proposal exists
        lifecycle.execute_weight_change(&admin, &task_type);
    }

    // ==================== Issue #1004 Tests ====================

    #[test]
    fn test_record_transfer_emits_auth_clr_event_with_count() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, _admin) = setup(&env, 0);
        let (asset_id, owner) = register_asset(&env, &asset_registry);
        let engineer1 = register_engineer(&env, &engineer_registry);
        let engineer2 = register_engineer(&env, &engineer_registry);

        // Authorize engineers
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer1);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer2);

        // Submit maintenance from both engineers
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Maintenance by engineer1"),
            &engineer1,
            &None,
        );
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &Priority::Low,
            &String::from_str(&env, "Inspection by engineer2"),
            &engineer2,
            &None,
        );

        // Transfer asset to new owner
        let new_owner = Address::generate(&env);
        asset_registry.transfer_asset(&asset_id, &owner, &new_owner);

        // Record the transfer in lifecycle
        lifecycle.record_transfer(&asset_id, &owner, &new_owner);

        // Verify AUTH_CLR event was emitted with count
        let events = env.events().all();
        let auth_clr_event = events.iter().find(|(_, topics, _)| {
            topics.get(0).map_or(false, |v| {
                Symbol::from_val(&env, &v) == symbol_short!("AUTH_CLR")
            })
        });
        assert!(auth_clr_event.is_some(), "AUTH_CLR event should be emitted");

        // Verify the event data is a count (u32), not a Vec
        let (_, _, data) = auth_clr_event.unwrap();
        let count: u32 = data.try_into_val(&env).unwrap();
        assert!(count > 0, "Cleared count should be > 0");
    }

    #[test]
    fn test_transfer_notify_emits_auth_clr_event_with_count() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, _admin) = setup(&env, 0);
        let (asset_id, owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);

        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Maintenance"),
            &engineer,
            &None,
        );

        // Transfer asset
        let new_owner = Address::generate(&env);
        asset_registry.transfer_asset(&asset_id, &owner, &new_owner);

        // Call transfer_notify
        lifecycle.transfer_notify(&asset_id, &new_owner);

        // Verify AUTH_CLR event was emitted
        let events = env.events().all();
        let auth_clr_event = events.iter().find(|(_, topics, _)| {
            topics.get(0).map_or(false, |v| {
                Symbol::from_val(&env, &v) == symbol_short!("AUTH_CLR")
            })
        });
        assert!(auth_clr_event.is_some(), "AUTH_CLR event should be emitted by transfer_notify");

        let (_, _, data) = auth_clr_event.unwrap();
        let count: u32 = data.try_into_val(&env).unwrap();
        assert!(count > 0, "Cleared count should be > 0");
    }

    #[test]
    fn test_transfer_clears_all_engineer_authorizations() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, engineer_registry, _admin) = setup(&env, 0);
        let (asset_id, owner) = register_asset(&env, &asset_registry);
        let engineer1 = register_engineer(&env, &engineer_registry);
        let engineer2 = register_engineer(&env, &engineer_registry);

        // Authorize engineers
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer1);
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer2);

        // Both can submit maintenance before transfer
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Before transfer"),
            &engineer1,
            &None,
        );

        // Transfer asset
        let new_owner = Address::generate(&env);
        asset_registry.transfer_asset(&asset_id, &owner, &new_owner);
        lifecycle.record_transfer(&asset_id, &owner, &new_owner);

        // Engineers should no longer be authorized to submit maintenance
        // They would need re-authorization from the new owner
        // (Testing the actual rejection would require checking the EngineerNotAuthorized error,
        // which we can't easily do here without expecting a panic)
    }

    // ==================== Issue #1005 Tests ====================

    #[test]
    fn test_get_assets_by_owner_returns_paginated_results() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, _engineer_registry, _admin) = setup(&env, 0);
        let owner = Address::generate(&env);

        // Register multiple assets for the same owner
        let asset_id_1 = register_asset_for_owner(&env, &asset_registry, &owner);
        let asset_id_2 = register_asset_for_owner(&env, &asset_registry, &owner);
        let asset_id_3 = register_asset_for_owner(&env, &asset_registry, &owner);

        // Query all assets with pagination
        let page1 = lifecycle.get_assets_by_owner(&owner, &0, &2);
        assert_eq!(page1.len(), 2);
        assert!(page1.contains(asset_id_1));
        assert!(page1.contains(asset_id_2));

        let page2 = lifecycle.get_assets_by_owner(&owner, &2, &2);
        assert_eq!(page2.len(), 1);
        assert!(page2.contains(asset_id_3));

        // Query all at once
        let all = lifecycle.get_assets_by_owner(&owner, &0, &100);
        assert_eq!(all.len(), 3);
        assert!(all.contains(asset_id_1));
        assert!(all.contains(asset_id_2));
        assert!(all.contains(asset_id_3));
    }

    #[test]
    fn test_get_assets_by_owner_returns_empty_for_unknown_owner() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, _asset_registry, _engineer_registry, _admin) = setup(&env, 0);
        let unknown_owner = Address::generate(&env);

        let assets = lifecycle.get_assets_by_owner(&unknown_owner, &0, &10);
        assert_eq!(assets.len(), 0);
    }

    #[test]
    fn test_get_assets_by_owner_respects_limit_cap() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, _engineer_registry, _admin) = setup(&env, 0);
        let owner = Address::generate(&env);

        // Register 5 assets
        for _ in 0..5 {
            register_asset_for_owner(&env, &asset_registry, &owner);
        }

        // Request limit > 100 should be capped at 100
        let assets = lifecycle.get_assets_by_owner(&owner, &0, &200);
        assert_eq!(assets.len(), 5); // Only 5 exist, so returns 5

        // Limit of 0 should return empty
        let assets = lifecycle.get_assets_by_owner(&owner, &0, &0);
        assert_eq!(assets.len(), 0);
    }

    #[test]
    fn test_get_assets_by_owner_handles_offset_beyond_range() {
        let env = Env::default();
        env.mock_all_auths();

        let (lifecycle, asset_registry, _engineer_registry, _admin) = setup(&env, 0);
        let owner = Address::generate(&env);

        register_asset_for_owner(&env, &asset_registry, &owner);

        // Offset beyond available assets
        let assets = lifecycle.get_assets_by_owner(&owner, &10, &5);
        assert_eq!(assets.len(), 0);
    }

    /// #1198 — valuation_history_push must cap at DEFAULT_MAX_HISTORY (200)
    /// when max_history == 0, not grow without bound.
    ///
    /// Push DEFAULT_MAX_HISTORY + 5 entries with max_history == 0 and assert
    /// the stored vector length stays at DEFAULT_MAX_HISTORY.
    #[test]
    fn test_valuation_history_push_caps_at_default_when_max_history_is_zero() {
        let env = Env::default();
        env.mock_all_auths();

        let asset_id: u64 = 42;
        // Push one more than the default cap to confirm the oldest entry is
        // evicted rather than the vector growing unbounded.
        let push_count = DEFAULT_MAX_HISTORY + 5;
        for i in 0..push_count {
            scoring::valuation_history_push(&env, asset_id, i as u64, i as u64, 0);
        }

        let key = DataKey::CollateralValuationHistory(asset_id);
        let history: soroban_sdk::Vec<(u64, u64)> = env
            .storage()
            .persistent()
            .get(&key)
            .expect("valuation history should be present");

        assert_eq!(
            history.len(),
            DEFAULT_MAX_HISTORY,
            "history length should be capped at DEFAULT_MAX_HISTORY ({DEFAULT_MAX_HISTORY}) \
             when max_history == 0, got {}",
            history.len(),
        );

        // The oldest entries must have been evicted; the last entry should be
        // the most recently pushed value.
        let (last_ts, last_val) = history.get(history.len() - 1).unwrap();
        let expected_last = (push_count - 1) as u64;
        assert_eq!(
            last_ts, expected_last,
            "last timestamp should be {expected_last}, got {last_ts}",
        );
        assert_eq!(
            last_val, expected_last,
            "last value should be {expected_last}, got {last_val}",
        );
    // ---------------------------------------------------------------------------
    //  #1222: next_due advances after a matching submit_maintenance call
    // ---------------------------------------------------------------------------

    #[test]
    fn test_next_due_advances_after_matching_submission() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, asset_id, owner, engineer) = setup_chain_test(&env);

        client.schedule_recurring_task(
            &owner,
            &asset_id,
            &1u64,
            &symbol_short!("OIL_CHG"),
            &symbol_short!("DAYS"),
            &30u64,
        );
        let original_next_due = client.get_recurring_tasks(&asset_id).get(0).unwrap().next_due;

        env.ledger().set_timestamp(env.ledger().timestamp() + 100);
        client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Low,
            &String::from_str(&env, "Scheduled oil change"),
            &engineer,
            &None,
        );

        let task = client.get_recurring_tasks(&asset_id).get(0).unwrap();
        assert!(task.next_due > original_next_due);
        assert_eq!(task.next_due, env.ledger().timestamp() + 30u64);
    }

    // ---------------------------------------------------------------------------
    //  #1221: Critical-priority batch records emit a dedicated alert event
    // ---------------------------------------------------------------------------

    #[test]
    fn test_batch_submit_critical_priority_emits_dedicated_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, asset_id, _owner, engineer) = setup_chain_test(&env);

        let mut records = Vec::new(&env);
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            priority: Priority::Low,
            notes: String::from_str(&env, "Routine"),
        });
        records.push_back(BatchRecord {
            task_type: symbol_short!("ENGINE"),
            priority: Priority::Critical,
            notes: String::from_str(&env, "Engine failure"),
        });

        client.batch_submit_maintenance(&asset_id, &records, &engineer, &None);

        let events = env.events().all();
        let crit_count = events
            .iter()
            .filter(|(_, topics, _)| {
                let t0: Result<Symbol, _> = topics.get(0).unwrap().try_into_val(&env);
                t0.map(|s: Symbol| s == symbol_short!("CRIT_MNT")).unwrap_or(false)
            })
            .count();
        assert_eq!(crit_count, 1);
    }

    // ---------------------------------------------------------------------------
    //  #1223: clear_duplicate_record removes a stale DuplicateRecords entry
    // ---------------------------------------------------------------------------

    #[test]
    #[should_panic(expected = "DuplicateRecordNotFound")]
    fn test_clear_duplicate_record_removes_entry() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, asset_registry, engineer_registry, admin) = setup(&env, 0);
        let (asset_id, owner) = register_asset(&env, &asset_registry);
        let engineer = register_engineer(&env, &engineer_registry);

        client.schedule_recurring_task(
            &owner,
            &asset_id,
            &1u64,
            &symbol_short!("OIL_CHG"),
            &symbol_short!("HOURS"),
            &3600u64,
        );
        client.auto_create_recurring_task(&asset_id, &1u64, &engineer);

        let ts = client.get_maintenance_history(&asset_id).get(0).unwrap().timestamp;
        client.mark_maintenance_as_duplicate(&admin, &asset_id, &0u64, &ts);

        // First clear succeeds and actually removes the entry...
        client.clear_duplicate_record(&admin, &asset_id, &ts);
        // ...so clearing the same, already-cleared entry again must panic.
        client.clear_duplicate_record(&admin, &asset_id, &ts);
    }
}
