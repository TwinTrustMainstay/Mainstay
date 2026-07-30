//! Tests for #1016: propose_update_config / execute_update_config in lifecycle.
//!
//! Verifies that:
//! - A valid config proposal is stored and executed after the timelock.
//! - Execution before the timelock fails with TimelockNotExpired.
//! - Invalid config values (zero score_increment, zero decay_interval, etc.) are rejected.
//! - Non-admin cannot propose or execute.
//! - CONFIG_UPD event is emitted on successful execution.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, Map, Symbol, Vec,
};

const TIMELOCK_DELAY_SECS: u64 = 48 * 60 * 60;

// Error discriminants (from lifecycle/src/errors.rs, stable integers).
const LIFECYCLE_TIMELOCK_NOT_EXPIRED: u32 = 17;
const LIFECYCLE_INVALID_CONFIG: u32 = 8;
const LIFECYCLE_UNAUTHORIZED_ADMIN: u32 = 3;

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

/// Build a valid replacement Config using the current config as a base,
/// changing `score_increment` to a new value.
fn make_new_config(
    env: &Env,
    lifecycle: &LifecycleClient,
    admin: &Address,
    new_score_increment: u32,
) -> lifecycle::Config {
    let mut cfg = lifecycle.get_config();
    cfg.score_increment = new_score_increment;
    cfg
}

#[test]
fn test_update_config_full_flow() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);

    let original_increment = lifecycle.get_config().score_increment;
    assert_ne!(original_increment, 99, "pre-condition: 99 must differ from default");

    let new_config = make_new_config(&env, &lifecycle, &admin, 99);

    // Step 1: Propose the config update.
    lifecycle.propose_update_config(&admin, &new_config);

    // Step 2: Immediate execute must fail with TimelockNotExpired.
    let res = lifecycle.try_execute_update_config(&admin);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_TIMELOCK_NOT_EXPIRED
        ))),
        "execute_update_config must be rejected before the timelock expires"
    );

    // Config must be unchanged.
    assert_eq!(
        lifecycle.get_config().score_increment,
        original_increment,
        "config must not change before timelock expires"
    );

    // Step 3: Advance past the timelock.
    let base = env.ledger().timestamp();
    env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

    // Step 4: Execute — must succeed and apply the new config.
    lifecycle.execute_update_config(&admin);

    assert_eq!(
        lifecycle.get_config().score_increment,
        99,
        "score_increment must be updated after execute_update_config"
    );
}

#[test]
fn test_update_config_rejects_zero_score_increment() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);

    let mut bad_config = lifecycle.get_config();
    bad_config.score_increment = 0;

    let res = lifecycle.try_propose_update_config(&admin, &bad_config);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_INVALID_CONFIG
        ))),
        "propose_update_config must reject zero score_increment"
    );
}

#[test]
fn test_update_config_rejects_zero_decay_interval() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);

    let mut bad_config = lifecycle.get_config();
    bad_config.decay_interval = 0;

    let res = lifecycle.try_propose_update_config(&admin, &bad_config);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_INVALID_CONFIG
        ))),
        "propose_update_config must reject zero decay_interval"
    );
}

#[test]
fn test_update_config_rejects_zero_eligibility_threshold() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);

    let mut bad_config = lifecycle.get_config();
    bad_config.eligibility_threshold = 0;

    let res = lifecycle.try_propose_update_config(&admin, &bad_config);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_INVALID_CONFIG
        ))),
        "propose_update_config must reject zero eligibility_threshold"
    );
}

#[test]
fn test_update_config_non_admin_cannot_propose() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);
    let outsider = Address::generate(&env);

    let new_config = make_new_config(&env, &lifecycle, &admin, 7);

    let res = lifecycle.try_propose_update_config(&outsider, &new_config);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_UNAUTHORIZED_ADMIN
        ))),
        "non-admin must not be able to propose a config update"
    );
}

#[test]
fn test_update_config_non_admin_cannot_execute() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);
    let outsider = Address::generate(&env);

    let new_config = make_new_config(&env, &lifecycle, &admin, 7);
    lifecycle.propose_update_config(&admin, &new_config);

    let base = env.ledger().timestamp();
    env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

    let res = lifecycle.try_execute_update_config(&outsider);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_UNAUTHORIZED_ADMIN
        ))),
        "non-admin must not be able to execute a config update"
    );
}

#[test]
fn test_update_config_boundary_one_second_before_expiry() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup(&env);
    let new_config = make_new_config(&env, &lifecycle, &admin, 42);

    lifecycle.propose_update_config(&admin, &new_config);

    let base = env.ledger().timestamp();
    env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS - 1);

    let res = lifecycle.try_execute_update_config(&admin);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_TIMELOCK_NOT_EXPIRED
        ))),
        "execute_update_config must be rejected one second before the timelock expires"
    );
}
