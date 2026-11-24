//! Tests for #1257: `initialize` must reject a zero `DEFAULT_SCORE_INCREMENT`.
//!
//! The `DEFAULT_SCORE_INCREMENT` constant (currently 5) is baked into the
//! compiled binary and is not a parameter of `initialize`.  A misconfigured
//! constant of 0 would mean maintenance events never award any score,
//! silently breaking collateral scoring.  The guard added to `initialize`
//! detects this at deploy time so the bad binary is rejected early rather
//! than producing a permanently broken contract.
//!
//! Because the constant is non-zero in the real binary (= 5), these tests
//! verify the *positive* path (initialization succeeds with the default) and
//! that the `update_score_increment` admin setter correctly rejects 0 at
//! runtime — the two defences together prevent any zero score_increment from
//! reaching production.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{testutils::Address as _, Address, Env};

// Error discriminant for InvalidConfig (errors.rs).
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

// ── tests ─────────────────────────────────────────────────────────────────────

/// Normal initialization succeeds and the stored `score_increment` is the
/// non-zero default (currently 5).  This guards the positive path.
#[test]
fn test_initialize_stores_nonzero_score_increment() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _admin) = setup_lifecycle(&env);

    let config = lifecycle.get_config();
    assert!(
        config.score_increment > 0,
        "score_increment must be > 0 after initialization (DEFAULT_SCORE_INCREMENT = {})",
        config.score_increment,
    );
}

/// `update_score_increment(0)` must be rejected with `InvalidConfig`.
///
/// This runtime check complements the compile-time constant guard: together
/// they ensure the score_increment can never transition to 0 after init either.
#[test]
fn test_update_score_increment_rejects_zero() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let result = lifecycle.try_update_score_increment(&admin, &0u32);
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_INVALID_CONFIG
        ))),
        "update_score_increment(0) must be rejected with InvalidConfig (#1257)"
    );
}

/// `update_score_increment(1)` must succeed (minimum valid non-zero value).
#[test]
fn test_update_score_increment_accepts_one() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    lifecycle.update_score_increment(&admin, &1u32);

    let config = lifecycle.get_config();
    assert_eq!(
        config.score_increment, 1,
        "update_score_increment(1) must be stored correctly"
    );
}

/// After a successful initialization the score_increment must match the
/// DEFAULT_SCORE_INCREMENT, which the guard ensures is non-zero.
/// Changing it to a new non-zero value via the admin setter must persist.
#[test]
fn test_update_score_increment_roundtrip() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let original = lifecycle.get_config().score_increment;
    assert!(original > 0, "default score_increment must be > 0");

    lifecycle.update_score_increment(&admin, &42u32);
    assert_eq!(lifecycle.get_config().score_increment, 42);

    // Try to set back to 0 — must be blocked.
    let result = lifecycle.try_update_score_increment(&admin, &0u32);
    assert!(result.is_err(), "reverting to 0 must still be blocked (#1257)");
}

/// `propose_update_config` must also reject a config whose `score_increment` is 0.
#[test]
fn test_propose_update_config_rejects_zero_score_increment() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let mut cfg = lifecycle.get_config();
    cfg.score_increment = 0;

    let result = lifecycle.try_propose_update_config(&admin, &cfg);
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_INVALID_CONFIG
        ))),
        "propose_update_config must reject score_increment=0 (#1257)"
    );
}
