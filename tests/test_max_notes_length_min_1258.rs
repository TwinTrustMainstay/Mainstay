//! Tests for #1258: `max_notes_length` must have a minimum value of 10.
//!
//! If `max_notes_length` is set to 0 the notes-length check resolves to
//! `notes.len() > 0`, accepting any non-empty string unconditionally.
//! Values 1–9 are similarly too short to be meaningful.
//!
//! Verifies that:
//! - `update_max_notes_length` rejects values 0–9 with `InvalidConfig`.
//! - `set_max_notes_length` rejects values 0–9 with `InvalidConfig`.
//! - Both setters accept value 10 (the minimum).
//! - Both setters accept values larger than 10.
//! - `propose_update_config` also rejects a `max_notes_length < 10`.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    symbol_short, Address, Env, Map, Vec,
};

const TIMELOCK_DELAY_SECS: u64 = 48 * 60 * 60;

// Error discriminants (from lifecycle/src/errors.rs).
const LIFECYCLE_INVALID_CONFIG: u32 = 8;

// ── helpers ──────────────────────────────────────────────────────────────────

fn setup_lifecycle(env: &Env) -> (LifecycleClient, Address) {
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let lifecycle = LifecycleClient::new(env, &lifecycle_id);
    let admin = Address::generate(env);

    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    asset_registry.initialize_admin(&admin, &admin);

    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
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

// ── update_max_notes_length tests ─────────────────────────────────────────────

/// `update_max_notes_length(0)` must be rejected with `InvalidConfig`.
#[test]
fn test_update_max_notes_length_rejects_zero() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let result = lifecycle.try_update_max_notes_length(&admin, &0u32);
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_INVALID_CONFIG
        ))),
        "update_max_notes_length(0) must be rejected (#1258)"
    );
}

/// Values 1–9 must all be rejected (too short to be meaningful).
#[test]
fn test_update_max_notes_length_rejects_below_minimum() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    for len in 1u32..10u32 {
        let result = lifecycle.try_update_max_notes_length(&admin, &len);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                LIFECYCLE_INVALID_CONFIG
            ))),
            "update_max_notes_length({}) must be rejected (#1258)",
            len
        );
    }
}

/// The minimum allowed value of 10 must be accepted.
#[test]
fn test_update_max_notes_length_accepts_minimum() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    lifecycle.update_max_notes_length(&admin, &10u32);

    let config = lifecycle.get_config();
    assert_eq!(
        config.max_notes_length, 10,
        "update_max_notes_length(10) must be accepted (#1258)"
    );
}

/// Values larger than 10 (e.g. the default 256) must be accepted.
#[test]
fn test_update_max_notes_length_accepts_large_value() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    lifecycle.update_max_notes_length(&admin, &512u32);

    let config = lifecycle.get_config();
    assert_eq!(config.max_notes_length, 512);
}

// ── set_max_notes_length tests ────────────────────────────────────────────────

/// `set_max_notes_length(0)` must be rejected with `InvalidConfig`.
#[test]
fn test_set_max_notes_length_rejects_zero() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let result = lifecycle.try_set_max_notes_length(&admin, &0u32);
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_INVALID_CONFIG
        ))),
        "set_max_notes_length(0) must be rejected (#1258)"
    );
}

/// Values 1–9 must all be rejected by `set_max_notes_length`.
#[test]
fn test_set_max_notes_length_rejects_below_minimum() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    for len in 1u32..10u32 {
        let result = lifecycle.try_set_max_notes_length(&admin, &len);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                LIFECYCLE_INVALID_CONFIG
            ))),
            "set_max_notes_length({}) must be rejected (#1258)",
            len
        );
    }
}

/// The minimum allowed value of 10 must be accepted by `set_max_notes_length`.
#[test]
fn test_set_max_notes_length_accepts_minimum() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    lifecycle.set_max_notes_length(&admin, &10u32);

    let config = lifecycle.get_config();
    assert_eq!(
        config.max_notes_length, 10,
        "set_max_notes_length(10) must be accepted (#1258)"
    );
}

// ── propose_update_config validation tests ────────────────────────────────────

/// `propose_update_config` must reject a `Config` whose `max_notes_length` is 0.
#[test]
fn test_propose_update_config_rejects_zero_max_notes_length() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let mut cfg = lifecycle.get_config();
    cfg.max_notes_length = 0;

    let result = lifecycle.try_propose_update_config(&admin, &cfg);
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_INVALID_CONFIG
        ))),
        "propose_update_config must reject max_notes_length=0 (#1258)"
    );
}

/// `propose_update_config` must reject a `Config` whose `max_notes_length` is 9.
#[test]
fn test_propose_update_config_rejects_max_notes_length_below_minimum() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let mut cfg = lifecycle.get_config();
    cfg.max_notes_length = 9;

    let result = lifecycle.try_propose_update_config(&admin, &cfg);
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_INVALID_CONFIG
        ))),
        "propose_update_config must reject max_notes_length=9 (#1258)"
    );
}

/// `propose_update_config` must accept a `Config` whose `max_notes_length` is 10.
#[test]
fn test_propose_update_config_accepts_min_max_notes_length() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let mut cfg = lifecycle.get_config();
    cfg.max_notes_length = 10;

    // Should not panic — proposal is valid.
    lifecycle.propose_update_config(&admin, &cfg);
}
