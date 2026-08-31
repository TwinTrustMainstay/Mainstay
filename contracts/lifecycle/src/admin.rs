//! Admin-only logic for the Lifecycle contract.
//!
//! Each function here implements the body of an admin entry point. The
//! `#[contractimpl]` impl block in `lib.rs` contains thin wrappers that
//! delegate directly to these functions so that:
//!
//! - Soroban's ABI generation (which requires all entry points to live in the
//!   single `#[contractimpl]` block) continues to work correctly.
//! - Admin logic is isolated in one place for maintainability.

use crate::errors::ContractError;
use crate::scoring::score_history_push;
use crate::storage::{
    history_key, last_update_key, score_history_key, score_key, scoring_weights_key,
};
use crate::types::{Config, DataKey, MaintenanceRecord, ScoreEntry};
use crate::{
    ensure_not_paused, get_asset_registry_addr, get_engineer_registry_addr, is_zero_address,
    parse_frequency_weights, require_admin, require_quorum, set_asset_registry_addr,
    set_engineer_registry_addr, store_timelock, require_timelock_ready,
    CONFIG, PAUSED_KEY, PENDING_ADMIN_KEY,
    EVENT_ADMIN_SET, EVENT_PROP_ADMIN, EVENT_REG_AST, EVENT_REG_ENG, EVENT_RST_SCR,
    TTL_THRESHOLD, TTL_TARGET, MAX_ADMINS,
};
use crate::events::EVENT_PRUNED;
use shared::extend_persistent_ttl;
use soroban_sdk::{panic_with_error, symbol_short, Address, Bytes, BytesN, Env, Symbol, Vec};

pub(crate) fn propose_config_update(env: Env, admin: Address, op: Symbol) {
    ensure_not_paused(&env);
    require_admin(&env, &admin);
    store_timelock(&env, op);
}

pub(crate) fn pause(env: Env, admin: Address) {
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    require_quorum(&env, &config, &admin);
    env.storage().persistent().set(&PAUSED_KEY, &true);
    extend_persistent_ttl(&env, &PAUSED_KEY);
    env.events().publish((symbol_short!("PAUSED"),), (admin.clone(),));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("PAUSED")),
        (admin, env.ledger().timestamp()),
    );
}

pub(crate) fn unpause(env: Env, admin: Address) {
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    require_quorum(&env, &config, &admin);
    env.storage().persistent().set(&PAUSED_KEY, &false);
    extend_persistent_ttl(&env, &PAUSED_KEY);
    env.events().publish((symbol_short!("UNPAUSED"),), (admin.clone(),));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("UNPAUSED")),
        (admin, env.ledger().timestamp()),
    );
}

pub(crate) fn propose_pause(env: Env, admin: Address) {
    ensure_not_paused(&env);
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    require_quorum(&env, &config, &admin);
    store_timelock(&env, symbol_short!("PAUSE"));
    env.events().publish((symbol_short!("PROP_PAUSE"),), (admin.clone(),));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("PROP_PAUSE")),
        (admin, env.ledger().timestamp()),
    );
}

pub(crate) fn execute_pause(env: Env, admin: Address) {
    require_timelock_ready(&env, symbol_short!("PAUSE"));
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    require_quorum(&env, &config, &admin);
    env.storage().persistent().set(&PAUSED_KEY, &true);
    extend_persistent_ttl(&env, &PAUSED_KEY);
    env.events().publish((symbol_short!("PAUSED"),), (admin.clone(),));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("PAUSED")),
        (admin, env.ledger().timestamp()),
    );
}

pub(crate) fn propose_unpause(env: Env, admin: Address) {
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    store_timelock(&env, symbol_short!("UNPAUSE"));
    env.events().publish((symbol_short!("PROP_UNPAUSE"),), (admin.clone(),));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("PROP_UNPAUSE")),
        (admin, env.ledger().timestamp()),
    );
}

pub(crate) fn execute_unpause(env: Env, admin: Address) {
    require_timelock_ready(&env, symbol_short!("UNPAUSE"));
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    require_quorum(&env, &config, &admin);
    env.storage().persistent().set(&PAUSED_KEY, &false);
    extend_persistent_ttl(&env, &PAUSED_KEY);
    env.events().publish((symbol_short!("UNPAUSED"),), (admin.clone(),));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("UNPAUSED")),
        (admin, env.ledger().timestamp()),
    );
}

pub(crate) fn pause(env: Env, admin: Address) {
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    require_quorum(&env, &config, &admin);
    env.storage().persistent().set(&PAUSED_KEY, &true);
    extend_persistent_ttl(&env, &PAUSED_KEY);
    env.events().publish((symbol_short!("PAUSED"),), (admin.clone(),));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("PAUSED")),
        (admin, env.ledger().timestamp()),
    );
}

pub(crate) fn unpause(env: Env, admin: Address) {
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    require_quorum(&env, &config, &admin);
    env.storage().persistent().set(&PAUSED_KEY, &false);
    extend_persistent_ttl(&env, &PAUSED_KEY);
    env.events().publish((symbol_short!("UNPAUSED"),), (admin.clone(),));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("UNPAUSED")),
        (admin, env.ledger().timestamp()),
    );
}

pub(crate) fn propose_pause(env: Env, admin: Address) {
    ensure_not_paused(&env);
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    require_quorum(&env, &config, &admin);
    store_timelock(&env, symbol_short!("PAUSE"));
    env.events().publish((symbol_short!("PROP_PAUSE"),), (admin.clone(),));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("PROP_PAUSE")),
        (admin, env.ledger().timestamp()),
    );
}

pub(crate) fn execute_pause(env: Env, admin: Address) {
    require_timelock_ready(&env, symbol_short!("PAUSE"));
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    require_quorum(&env, &config, &admin);
    env.storage().persistent().set(&PAUSED_KEY, &true);
    extend_persistent_ttl(&env, &PAUSED_KEY);
    env.events().publish((symbol_short!("PAUSED"),), (admin.clone(),));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("PAUSED")),
        (admin, env.ledger().timestamp()),
    );
}

pub(crate) fn propose_unpause(env: Env, admin: Address) {
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    store_timelock(&env, symbol_short!("UNPAUSE"));
    env.events().publish((symbol_short!("PROP_UNPAUSE"),), (admin.clone(),));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("PROP_UNPAUSE")),
        (admin, env.ledger().timestamp()),
    );
}

pub(crate) fn execute_unpause(env: Env, admin: Address) {
    require_timelock_ready(&env, symbol_short!("UNPAUSE"));
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    require_quorum(&env, &config, &admin);
    env.storage().persistent().set(&PAUSED_KEY, &false);
    extend_persistent_ttl(&env, &PAUSED_KEY);
    env.events().publish((symbol_short!("UNPAUSED"),), (admin.clone(),));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("UNPAUSED")),
        (admin, env.ledger().timestamp()),
    );
}

pub(crate) fn propose_admin(env: Env, admin: Address, new_admin: Address) {
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    if env.storage().instance().has(&PENDING_ADMIN_KEY) {
        panic_with_error!(&env, ContractError::PendingAdminAlreadyExists);
    }
    env.storage().instance().set(&PENDING_ADMIN_KEY, &new_admin);
    store_timelock(&env, symbol_short!("ADM_XFER"));
    env.events().publish((EVENT_PROP_ADMIN,), (admin.clone(), new_admin.clone()));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("PROP_ADM")),
        (admin, env.ledger().timestamp(), new_admin),
    );
}

pub(crate) fn accept_admin(env: Env) {
    require_timelock_ready(&env, symbol_short!("ADM_XFER"));
    let pending_admin: Address = env.storage().instance().get(&PENDING_ADMIN_KEY)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    pending_admin.require_auth();
    let mut config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    config.admin = pending_admin.clone();
    env.storage().persistent().set(&CONFIG, &config);
    extend_persistent_ttl(&env, &CONFIG);
    env.storage().instance().remove(&PENDING_ADMIN_KEY);
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("ADMIN_SET")),
        (pending_admin.clone(), env.ledger().timestamp()),
    );
    env.events().publish((EVENT_ADMIN_SET,), (pending_admin,));
}

/// Internal implementation of `set_admin_quorum`.
///
/// Called by the public contract entry-point after it has already performed
/// input validation (auth, threshold check, and duplicate check).  This layer
/// repeats the duplicate check as defence-in-depth so the invariant holds even
/// if the function is ever called from another internal site.
///
/// # Uniqueness requirement (#1195)
/// `new_admins` must contain only distinct addresses.  Panics with
/// [`ContractError::DuplicateAdmin`] if any address appears more than once.
pub(crate) fn set_admin_quorum(env: Env, admin: Address, new_admins: Vec<Address>, threshold: u32) {
    ensure_not_paused(&env);
    admin.require_auth();
    let mut config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    if threshold > 0 && threshold > new_admins.len() {
        panic_with_error!(&env, ContractError::InvalidConfig);
    }
    // #1259: Hard cap — admin lists must not exceed MAX_ADMINS (10).
    // require_quorum iterates the full list; an unbounded list could push
    // per-call compute toward Soroban instruction limits (DoS vector).
    if new_admins.len() > MAX_ADMINS {
        panic_with_error!(&env, ContractError::TooManyAdmins);
    }
    // #1195: Reject lists that contain any repeated address.  A duplicate
    // inflates the apparent quorum count so that fewer real signers than
    // `threshold` could satisfy `require_quorum`, undermining the security
    // guarantee.  All addresses must be unique.
    //
    // We check uniqueness with a nested O(n²) scan — acceptable because
    // admin lists are expected to be small (≤ ~10 entries) and the
    // soroban_sdk::Vec does not expose a sorted or hashed variant.
    let n = new_admins.len();
    let mut i: u32 = 0;
    while i < n {
        let a = new_admins.get(i).unwrap();
        let mut j = i + 1;
        while j < n {
            if new_admins.get(j).unwrap() == a {
                panic_with_error!(&env, ContractError::DuplicateAdmin);
            }
            j += 1;
        }
        i += 1;
    }
    config.admins = new_admins.clone();
    config.admin_threshold = threshold;
    env.storage().persistent().set(&CONFIG, &config);
    env.storage().persistent().extend_ttl(&CONFIG, TTL_THRESHOLD, TTL_TARGET);
    env.events().publish(
        (symbol_short!("SET_QRUM"), admin.clone()),
        (new_admins, threshold),
    );
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("SET_QRUM")),
        (admin, env.ledger().timestamp(), threshold),
    );
}

pub(crate) fn update_score_increment(env: Env, admin: Address, score_increment: u32) {
    ensure_not_paused(&env);
    admin.require_auth();
    if score_increment == 0 {
        panic_with_error!(&env, ContractError::InvalidConfig);
    }
    let mut config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    let old = config.score_increment;
    config.score_increment = score_increment;
    env.storage().persistent().set(&CONFIG, &config);
    extend_persistent_ttl(&env, &CONFIG);
    env.events().publish((symbol_short!("CFG_UPD"),), (old, score_increment));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("CFG_UPD")),
        (admin, env.ledger().timestamp(), symbol_short!("SCORE_INC"), score_increment),
    );
}

pub(crate) fn update_decay_config(env: Env, admin: Address, decay_rate: u32, decay_interval: u64) {
    ensure_not_paused(&env);
    admin.require_auth();
    if decay_rate == 0 || decay_interval == 0 {
        panic_with_error!(&env, ContractError::InvalidConfig);
    }
    let mut config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    let old_rate = config.decay_rate;
    let old_interval = config.decay_interval;
    config.decay_rate = decay_rate;
    config.decay_interval = decay_interval;
    env.events().publish(
        (symbol_short!("CFG_UPD"),),
        (old_rate, decay_rate, old_interval, decay_interval),
    );
    env.storage().persistent().set(&CONFIG, &config);
    extend_persistent_ttl(&env, &CONFIG);
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("CFG_UPD")),
        (admin, env.ledger().timestamp(), symbol_short!("DECAY"), decay_rate, decay_interval),
    );
}

pub(crate) fn update_eligibility_threshold(env: Env, admin: Address, threshold: u32) {
    ensure_not_paused(&env);
    admin.require_auth();
    let mut config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    if threshold == 0 {
        panic_with_error!(&env, ContractError::InvalidConfig);
    }
    let old = config.eligibility_threshold;
    config.eligibility_threshold = threshold;
    env.storage().persistent().set(&CONFIG, &config);
    extend_persistent_ttl(&env, &CONFIG);
    env.events().publish((symbol_short!("CFG_UPD"),), (old, threshold));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("CFG_UPD")),
        (admin, env.ledger().timestamp(), symbol_short!("ELIG"), threshold),
    );
}

pub(crate) fn update_max_history(env: Env, admin: Address, new_max: u32) {
    ensure_not_paused(&env);
    admin.require_auth();
    if new_max == 0 {
        panic_with_error!(&env, ContractError::InvalidConfig);
    }
    let mut config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    config.max_history = new_max;
    env.storage().persistent().set(&CONFIG, &config);
    extend_persistent_ttl(&env, &CONFIG);
    env.events().publish((symbol_short!("UPD_MAX"), admin.clone()), new_max);
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("CFG_UPD")),
        (admin, env.ledger().timestamp(), symbol_short!("MAX_HIST"), new_max),
    );
}

/// Admin-only function to update the per-engineer history cap.
///
/// # Arguments
/// * `admin`   - The admin address that must match the stored config admin.
/// * `new_max` - New cap on the number of asset IDs kept in each engineer's
///               history list (must be > 0).
///
/// # Panics
/// - [`ContractError::NotInitialized`] if the contract has not been initialised.
/// - [`ContractError::UnauthorizedAdmin`] if the caller is not the admin.
/// - [`ContractError::InvalidConfig`] if `new_max` is 0.
pub(crate) fn update_max_engineer_history(env: Env, admin: Address, new_max: u32) {
    ensure_not_paused(&env);
    admin.require_auth();
    if new_max == 0 {
        panic_with_error!(&env, ContractError::InvalidConfig);
    }
    let mut config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    config.max_engineer_history = new_max;
    env.storage().persistent().set(&CONFIG, &config);
    extend_persistent_ttl(&env, &CONFIG);
    env.events().publish(
        (symbol_short!("UPD_ENGH"), admin.clone()),
        new_max,
    );
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("CFG_UPD")),
        (admin, env.ledger().timestamp(), symbol_short!("ENG_HIST"), new_max),
    );
}

/// Admin-only function to update the per-asset health-snapshot retention cap.
///
/// # Arguments
/// * `admin`   - The admin address that must match the stored config admin.
/// * `new_max` - New cap on the number of snapshots kept per asset in
///               `HealthSnapshots(asset_id)` (must be > 0). When
///               `take_health_snapshot` would exceed this cap, the oldest
///               snapshots are evicted first.
///
/// # Panics
/// - [`ContractError::NotInitialized`] if the contract has not been initialised.
/// - [`ContractError::UnauthorizedAdmin`] if the caller is not the admin.
/// - [`ContractError::InvalidConfig`] if `new_max` is 0.
pub(crate) fn update_max_snapshots(env: Env, admin: Address, new_max: u32) {
    ensure_not_paused(&env);
    admin.require_auth();
    if new_max == 0 {
        panic_with_error!(&env, ContractError::InvalidConfig);
    }
    let mut config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    config.max_snapshots = new_max;
    env.storage().persistent().set(&CONFIG, &config);
    extend_persistent_ttl(&env, &CONFIG);
    env.events().publish(
        (symbol_short!("UPD_SNAP"), admin.clone()),
        new_max,
    );
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("CFG_UPD")),
        (admin, env.ledger().timestamp(), symbol_short!("MAX_SNAP"), new_max),
    );
}

pub(crate) fn update_max_notes_length(env: Env, admin: Address, new_max: u32) {
    ensure_not_paused(&env);
    admin.require_auth();
    // #1258: A max_notes_length < 10 lets notes trivially bypass meaningful
    // content validation. Zero in particular turns the length guard into
    // `notes.len() > 0`, accepting any non-empty string unconditionally.
    if new_max < 10 {
        panic_with_error!(&env, ContractError::InvalidConfig);
    }
    let mut config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    config.max_notes_length = new_max;
    env.storage().persistent().set(&CONFIG, &config);
    extend_persistent_ttl(&env, &CONFIG);
    env.events().publish((symbol_short!("UPD_NOTES"), admin.clone()), new_max);
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("CFG_UPD")),
        (admin, env.ledger().timestamp(), symbol_short!("MAX_NOTE"), new_max),
    );
}

pub(crate) fn set_max_notes_length(env: Env, admin: Address, length: u32) {
    ensure_not_paused(&env);
    admin.require_auth();
    // #1258: Minimum 10 characters to prevent trivial bypass of notes validation.
    if length < 10 {
        panic_with_error!(&env, ContractError::InvalidConfig);
    }
    let mut config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    config.max_notes_length = length;
    env.storage().persistent().set(&CONFIG, &config);
    extend_persistent_ttl(&env, &CONFIG);
    env.events().publish((symbol_short!("SET_NOTES"), admin.clone()), length);
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("CFG_UPD")),
        (admin, env.ledger().timestamp(), symbol_short!("MAX_NOTE"), length),
    );
}

pub(crate) fn set_eligibility_threshold(env: Env, admin: Address, value: u32) {
    ensure_not_paused(&env);
    admin.require_auth();
    if value == 0 {
        panic_with_error!(&env, ContractError::InvalidConfig);
    }
    let mut config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    let old = config.eligibility_threshold;
    config.eligibility_threshold = value;
    env.storage().persistent().set(&CONFIG, &config);
    env.storage().persistent().extend_ttl(&CONFIG, TTL_THRESHOLD, TTL_TARGET);
    env.events().publish((symbol_short!("SET_ELIG"), admin.clone()), (old, value));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("CFG_UPD")),
        (admin, env.ledger().timestamp(), symbol_short!("ELIG_THR"), value),
    );
}

pub(crate) fn set_task_weight(env: Env, admin: Address, task_type: Symbol, weight: u32) {
    ensure_not_paused(&env);
    admin.require_auth();
    if weight == 0 {
        panic_with_error!(&env, ContractError::InvalidConfig);
    }
    let mut config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    config.task_weights.set(task_type.clone(), weight);
    env.storage().persistent().set(&CONFIG, &config);
    extend_persistent_ttl(&env, &CONFIG);
    env.events().publish((symbol_short!("TSK_WT"),), (task_type.clone(), weight));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("TSK_WT")),
        (admin, env.ledger().timestamp(), task_type, weight),
    );
}

pub(crate) fn update_scoring_weights(
    env: Env,
    admin: Address,
    asset_type: Symbol,
    weights_json: Bytes,
) {
    ensure_not_paused(&env);
    require_admin(&env, &admin);
    if parse_frequency_weights(&weights_json).is_none() {
        panic_with_error!(&env, ContractError::InvalidConfig);
    }
    let key = scoring_weights_key(&env, &asset_type);
    env.storage().persistent().set(&key, &weights_json);
    env.storage().persistent().extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
    env.events().publish(
        (symbol_short!("SCR_WT"), asset_type.clone()),
        weights_json.clone(),
    );
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("SCR_WT")),
        (admin, env.ledger().timestamp(), asset_type, weights_json),
    );
}

pub(crate) fn update_asset_registry(env: Env, admin: Address, new_registry: Address) {
    ensure_not_paused(&env);
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    if is_zero_address(&env, &new_registry) {
        panic_with_error!(&env, ContractError::ZeroAddress);
    }
    if new_registry == get_engineer_registry_addr(&env) {
        panic_with_error!(&env, ContractError::SameRegistryAddress);
    }
    set_asset_registry_addr(&env, &new_registry);
    env.events().publish((EVENT_REG_AST,), (admin.clone(), new_registry.clone()));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("REG_AST")),
        (admin, env.ledger().timestamp(), new_registry),
    );
}

pub(crate) fn update_engineer_registry(env: Env, admin: Address, new_registry: Address) {
    ensure_not_paused(&env);
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    if is_zero_address(&env, &new_registry) {
        panic_with_error!(&env, ContractError::ZeroAddress);
    }
    if new_registry == get_asset_registry_addr(&env) {
        panic_with_error!(&env, ContractError::SameRegistryAddress);
    }
    set_engineer_registry_addr(&env, &new_registry);
    env.events().publish((EVENT_REG_ENG,), (admin.clone(), new_registry.clone()));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("REG_ENG")),
        (admin, env.ledger().timestamp(), new_registry),
    );
}

pub(crate) fn propose_upgrade(env: Env, admin: Address, new_wasm_hash: BytesN<32>) {
    ensure_not_paused(&env);
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    store_timelock(&env, symbol_short!("UPGRADE"));
    env.storage().persistent().set(&symbol_short!("PEND_UPG"), &new_wasm_hash);
    extend_persistent_ttl(&env, &symbol_short!("PEND_UPG"));
    env.events().publish(
        (symbol_short!("PROP_UPG"), admin.clone()),
        (new_wasm_hash, env.ledger().timestamp()),
    );
}

pub(crate) fn execute_upgrade(env: Env, admin: Address) {
    ensure_not_paused(&env);
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    require_timelock_ready(&env, symbol_short!("UPGRADE"));
    let new_wasm_hash: BytesN<32> = env
        .storage().persistent().get(&symbol_short!("PEND_UPG"))
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::ProposalNotFound));
    env.storage().persistent().remove(&symbol_short!("PEND_UPG"));
    env.events().publish(
        (symbol_short!("UPGRADE"), admin.clone()),
        new_wasm_hash.clone(),
    );
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("UPGRADE")),
        (admin, env.ledger().timestamp(), new_wasm_hash.clone()),
    );
    #[cfg(not(test))]
    {
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }
}

pub(crate) fn reset_score(env: Env, admin: Address, asset_id: u64) {
    ensure_not_paused(&env);
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    require_quorum(&env, &config, &admin);
    let now = env.ledger().timestamp();
    let empty: Vec<MaintenanceRecord> = Vec::new(&env);
    env.storage().persistent().set(&history_key(asset_id), &empty);
    extend_persistent_ttl(&env, &history_key(asset_id));
    env.storage().persistent().set(&score_key(asset_id), &0u32);
    extend_persistent_ttl(&env, &score_key(asset_id));
    env.storage().persistent().set(&last_update_key(asset_id), &now);
    extend_persistent_ttl(&env, &last_update_key(asset_id));
    score_history_push(&env, asset_id, ScoreEntry { timestamp: now, score: 0 }, config.max_history);
    env.events().publish((EVENT_RST_SCR, asset_id), (admin.clone(), now));
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("RST_SCR")),
        (admin, now, asset_id),
    );
}

pub(crate) fn prune_asset_history(env: Env, admin: Address, asset_id: u64) {
    ensure_not_paused(&env);
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    // Prune maintenance history
    let hist_key = history_key(asset_id);
    if let Some(history) = env.storage().persistent().get::<_, Vec<MaintenanceRecord>>(&hist_key) {
        if history.len() > config.max_history {
            let start = history.len() - config.max_history;
            let pruned_count = start as u32;
            let oldest_ts = history.get(0).unwrap().timestamp;
            let mut kept: Vec<MaintenanceRecord> = Vec::new(&env);
            for i in start..history.len() { kept.push_back(history.get(i).unwrap()); }
            // The new oldest record's `previous_record_hash` still points at a now-pruned
            // record; clear it so the hash chain doesn't reference missing history.
            if let Some(mut oldest) = kept.get(0) {
                oldest.previous_record_hash = None;
                kept.set(0, oldest);
            }
            env.storage().persistent().set(&hist_key, &kept);
            extend_persistent_ttl(&env, &hist_key);
            env.events().publish((EVENT_PRUNED,), (asset_id, pruned_count, oldest_ts));
            // Remove engineers whose records were entirely pruned
            let mut retained: Vec<Address> = Vec::new(&env);
            for i in start..history.len() {
                let eng = history.get(i).unwrap().engineer;
                let mut found = false;
                for e in retained.iter() { if e == eng { found = true; break; } }
                if !found { retained.push_back(eng); }
            }
            let mut removed: Vec<Address> = Vec::new(&env);
            for i in 0..start {
                let eng = history.get(i).unwrap().engineer;
                let mut in_retained = false;
                for e in retained.iter() { if e == eng { in_retained = true; break; } }
                if in_retained { continue; }
                let mut already = false;
                for e in removed.iter() { if e == eng { already = true; break; } }
                if !already {
                    removed.push_back(eng.clone());
                    crate::engineer_history_remove(&env, &eng, asset_id);
                }
            }
        }
    }
    // Prune score history
    let sc_key = score_history_key(asset_id);
    if let Some(sh) = env.storage().persistent().get::<_, Vec<ScoreEntry>>(&sc_key) {
        if sh.len() > config.max_history {
            let start = sh.len() - config.max_history;
            let mut kept: Vec<ScoreEntry> = Vec::new(&env);
            for i in start..sh.len() { kept.push_back(sh.get(i).unwrap()); }
            env.storage().persistent().set(&sc_key, &kept);
            extend_persistent_ttl(&env, &sc_key);
        }
    }
    // Prune valuation history
    let val_key = DataKey::CollateralValuationHistory(asset_id);
    if let Some(vh) = env.storage().persistent().get::<_, Vec<(u64, u64)>>(&val_key) {
        if vh.len() > config.max_history {
            let start = vh.len() - config.max_history;
            let mut kept: Vec<(u64, u64)> = Vec::new(&env);
            for i in start..vh.len() { kept.push_back(vh.get(i).unwrap()); }
            env.storage().persistent().set(&val_key, &kept);
            env.storage().persistent().extend_ttl(&val_key, TTL_THRESHOLD, TTL_TARGET);
        }
    }
    env.events().publish((symbol_short!("PRUNE"), admin.clone()), asset_id);
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("PRUNE")),
        (admin, env.ledger().timestamp(), asset_id),
    );
}

pub(crate) fn purge_asset_data(env: Env, admin: Address, asset_id: u64) {
    ensure_not_paused(&env);
    admin.require_auth();
    let config: Config = env.storage().persistent().get(&CONFIG)
        .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
    if config.admin != admin {
        panic_with_error!(&env, ContractError::UnauthorizedAdmin);
    }
    let hist_key = history_key(asset_id);
    if let Some(history) = env.storage().persistent().get::<_, Vec<MaintenanceRecord>>(&hist_key) {
        let mut engineers: Vec<Address> = Vec::new(&env);
        for record in history.iter() {
            let eng = record.engineer;
            let mut found = false;
            for e in engineers.iter() { if e == eng { found = true; break; } }
            if !found {
                engineers.push_back(eng.clone());
                crate::engineer_history_remove(&env, &eng, asset_id);
            }
        }
    }
    env.storage().persistent().remove(&history_key(asset_id));
    env.storage().persistent().remove(&score_key(asset_id));
    env.storage().persistent().remove(&score_history_key(asset_id));
    env.storage().persistent().remove(&DataKey::CollateralValuationHistory(asset_id));
    env.storage().persistent().remove(&last_update_key(asset_id));
    env.events().publish((symbol_short!("PURGE"), admin.clone()), asset_id);
    env.events().publish(
        (symbol_short!("ADM_AUD"), symbol_short!("PURGE")),
        (admin, env.ledger().timestamp(), asset_id),
    );
}
