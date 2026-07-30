//! Verifies that admin operations protected by a timelock cannot be executed
//! before the configured delay has elapsed.
//!
//! Exercises the lifecycle contract's timelock-protected config-update flow
//! (`propose_config_update` → `execute_update_score_increment`). The test
//! guards against a regression where the timelock check is accidentally removed
//! or weakened, which would let admin ops take effect immediately.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, Env,
};

/// On-ledger delay used by the lifecycle timelock (48h, in seconds).
/// Mirrors `TIMELOCK_DELAY_SECS` in `contracts/lifecycle/src/lib.rs`; keep
/// these values in sync if the production constant ever changes.
const TIMELOCK_DELAY_SECS: u64 = 48 * 60 * 60;

/// `lifecycle` keeps `ContractError` in a private module, so the test refers
/// to the error code by its stable integer to avoid changing the contract's
/// public surface purely for testing. Source of truth:
/// `contracts/lifecycle/src/errors.rs` → `ContractError::TimelockNotExpired`.
const LIFECYCLE_TIMELOCK_NOT_EXPIRED: u32 = 17;

/// Register the ecosystem contracts and initialize lifecycle with an admin.
/// Returns the lifecycle client and the admin address used as the timelock auth signer.
/// Reuses `admin` for all signer roles to match the convention in
/// `tests/test_full_lifecycle_e2e.rs`.
fn setup(env: &Env) -> (LifecycleClient, Address) {
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(env, &lifecycle_id);

    let admin = Address::generate(env);

    asset_registry.initialize_admin(&admin, &admin);
    engineer_registry.initialize_admin(&admin, &admin);
    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &0,
    );

    (lifecycle, admin)
}

#[test]
fn test_admin_timelock_cannot_be_bypassed() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);

    // Capture the existing config so we can prove the rejected attempt did
    // not mutate state.
    let config_before = lifecycle.get_config();
    assert_ne!(
        config_before.score_increment, 42,
        "test pre-condition: 42 must differ from the default score_increment"
    );

    // ── Step 1: Propose the admin operation as the current admin. ──
    lifecycle.propose_config_update(&admin, &symbol_short!("SC_INC"));

    // ── Step 2: Attempt to execute immediately. The timelock MUST reject it. ──
    let res = lifecycle.try_execute_update_score_increment(&admin, &42u32);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_TIMELOCK_NOT_EXPIRED,
        ))),
        "execute must fail with TimelockNotExpired before the timelock expires",
    );

    // State must remain unchanged after the rejected attempt.
    let config_after_reject = lifecycle.get_config();
    assert_eq!(
        config_after_reject.score_increment, config_before.score_increment,
        "score_increment must not change when execute is rejected by the timelock",
    );

    // ── Step 3: Advance the ledger timestamp past TIMELOCK_DELAY_SECS so
    //          the proposal becomes eligible to execute. ──
    let base = env.ledger().timestamp();
    env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

    // ── Step 4: Execute — must now succeed and update the config. ──
    lifecycle.execute_update_score_increment(&admin, &42u32);
    let config_after_execute = lifecycle.get_config();
    assert_eq!(
        config_after_execute.score_increment, 42,
        "score_increment must reflect the value passed to execute after the delay expires",
    );
}

#[test]
fn test_admin_timelock_rejects_execution_one_second_before_expiry() {
    // Boundary check: pins the `< TIMELOCK_DELAY_SECS` comparison in
    // `require_timelock_ready` so that flipping it to `<=` would be caught.
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);

    lifecycle.propose_config_update(&admin, &symbol_short!("SC_INC"));

    // Advance `TIMELOCK_DELAY_SECS - 1` seconds — still inside the lockout.
    let base = env.ledger().timestamp();
    env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS - 1);

    let res = lifecycle.try_execute_update_score_increment(&admin, &7u32);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_TIMELOCK_NOT_EXPIRED,
        ))),
        "execute must remain rejected one second before TIMELOCK_DELAY_SECS",
    );
}

// --- Admin transfer timelock tests ---

#[test]
fn test_admin_transfer_timelock_rejects_accept_before_expiry() {
    // Verify that `accept_admin` cannot be executed before the timelock
    // delay has elapsed, preventing a compromised admin from instantly
    // transferring control.
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);
    let new_admin = Address::generate(&env);

    // Step 1: Propose the admin transfer.
    lifecycle.propose_admin(&admin, &new_admin);

    // Step 2: Attempt to accept immediately. The timelock MUST reject it.
    let res = lifecycle.try_accept_admin();
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_TIMELOCK_NOT_EXPIRED,
        ))),
        "accept_admin must fail with TimelockNotExpired before the timelock expires",
    );

    // Admin must remain unchanged after the rejected attempt.
    assert_eq!(
        lifecycle.get_config().admin, admin,
        "admin must not change when accept_admin is rejected by the timelock",
    );
}

#[test]
fn test_admin_transfer_timelock_allows_accept_after_expiry() {
    // Verify that `accept_admin` succeeds after the timelock delay has elapsed.
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);
    let new_admin = Address::generate(&env);

    // Step 1: Propose the admin transfer.
    lifecycle.propose_admin(&admin, &new_admin);

    // Step 2: Advance past the timelock delay.
    let base = env.ledger().timestamp();
    env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

    // Step 3: Accept — must now succeed.
    lifecycle.accept_admin();

    assert_eq!(
        lifecycle.get_config().admin, new_admin,
        "admin must be updated after timelock expires",
    );
}

#[test]
fn test_admin_transfer_rejects_non_admin_propose() {
    // Verify that a non-admin cannot propose an admin transfer.
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);
    let outsider = Address::generate(&env);
    let new_admin = Address::generate(&env);

    let res = lifecycle.try_propose_admin(&outsider, &new_admin);
    assert!(res.is_err(), "non-admin must not be able to propose admin transfer");
    // Verify admin is unchanged
    assert_eq!(
        lifecycle.get_config().admin, admin,
        "admin must remain unchanged after unauthorized propose attempt",
    );
}

// --- Pause/unpause timelock tests ---

#[test]
fn test_pause_timelock_cannot_be_bypassed() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);

    // Propose pause
    lifecycle.propose_pause(&admin);

    // Attempt to execute immediately - must fail
    let res = lifecycle.try_execute_pause(&admin);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_TIMELOCK_NOT_EXPIRED,
        ))),
        "execute_pause must fail with TimelockNotExpired before the timelock expires",
    );

    // Contract must not be paused yet
    assert!(!lifecycle.is_paused(), "contract must not be paused before timelock expires");
}

#[test]
fn test_pause_timelock_allows_execute_after_expiry() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);

    lifecycle.propose_pause(&admin);

    let base = env.ledger().timestamp();
    env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

    lifecycle.execute_pause(&admin);

    assert!(lifecycle.is_paused(), "contract must be paused after timelock expires");
}

#[test]
fn test_unpause_timelock_cannot_be_bypassed() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);

    // First pause the contract
    lifecycle.propose_pause(&admin);
    let base = env.ledger().timestamp();
    env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);
    lifecycle.execute_pause(&admin);
    assert!(lifecycle.is_paused());

    // Propose unpause
    lifecycle.propose_unpause(&admin);

    // Attempt to execute immediately - must fail
    let res = lifecycle.try_execute_unpause(&admin);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_TIMELOCK_NOT_EXPIRED,
        ))),
        "execute_unpause must fail with TimelockNotExpired before the timelock expires",
    );

    // Contract must still be paused
    assert!(lifecycle.is_paused(), "contract must remain paused before timelock expires");
}

#[test]
fn test_unpause_timelock_allows_execute_after_expiry() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);

    // First pause the contract
    lifecycle.propose_pause(&admin);
    let base = env.ledger().timestamp();
    env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);
    lifecycle.execute_pause(&admin);
    assert!(lifecycle.is_paused());

    // Propose unpause
    lifecycle.propose_unpause(&admin);

    let base2 = env.ledger().timestamp();
    env.ledger().set_timestamp(base2 + TIMELOCK_DELAY_SECS + 1);

    lifecycle.execute_unpause(&admin);

    assert!(!lifecycle.is_paused(), "contract must be unpaused after timelock expires");
}
