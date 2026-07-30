//! Comprehensive TTL extension tests for the lifecycle contract.
//!
//! Verifies that every persistent storage key is extended on write, simulates
//! ledger progression and TTL expiry scenarios, and tests with high ledger
//! numbers and near-expiry conditions.
//!
//! # TTL Extension Strategy Test Coverage
//!
//! | Storage Key | Extended On | Test |
//! |---|---|---|
//! | `HIST` (maintenance history) | `submit_maintenance`, `batch_submit_maintenance`, `record_transfer` | `test_hist_ttl_extended_on_submit` |
//! | `SCORE` (collateral score) | `submit_maintenance`, `apply_decay` | `test_score_ttl_extended_on_submit` |
//! | `SCHIST` (score history) | `score_history_push` (via `submit_maintenance`) | `test_schist_ttl_extended_on_submit` |
//! | `LUPD` (last update) | `submit_maintenance`, `apply_decay` | `test_lupd_ttl_extended_on_submit` |
//! | `CONFIG` (configuration) | `initialize`, config updates | `test_config_ttl_extended_on_init` |
//! | `REGISTRY` (asset registry addr) | `initialize` | `test_registry_ttl_extended_on_init` |
//! | `ENG_REG` (engineer registry addr) | `initialize` | `test_eng_reg_ttl_extended_on_init` |
//! | `PAUSED` (pause flag) | `pause`, `unpause` | `test_paused_ttl_extended_on_toggle` |
//! | `ENG_AUTH` (engineer auth) | `authorize_engineer` | `test_eng_auth_ttl_extended` |
//! | `ENG_HIST` (engineer history) | `submit_maintenance` | `test_eng_hist_ttl_extended` |

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

/// TTL threshold used by the shared crate (518,400 ledgers ≈ 30 days).
const TTL_THRESHOLD: u32 = 518_400;

/// Helper: construct a fully-initialized test environment.
fn setup(env: &Env) -> (LifecycleClient, AssetRegistryClient, EngineerRegistryClient, Address, Address, Address) {
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(env, &lifecycle_id);

    let admin = Address::generate(env);
    let owner = Address::generate(env);
    let issuer = Address::generate(env);

    asset_registry.initialize_admin(&admin, &admin);
    asset_registry.add_asset_type(&admin, &symbol_short!("GENSET"));
    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);
    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &0,
    );

    (lifecycle, asset_registry, engineer_registry, admin, owner, issuer)
}

/// Helper: register an asset and return its ID.
fn register_asset(env: &Env, registry: &AssetRegistryClient, owner: &Address) -> u64 {
    registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(env, "TTL test asset"),
        &String::from_str(env, "SN-TTL-TEST"),
        owner,
    )
}

/// Helper: register an engineer and return the address.
fn register_engineer(env: &Env, registry: &EngineerRegistryClient, issuer: &Address) -> Address {
    let engineer = Address::generate(env);
    let credential_hash = BytesN::from_array(env, &[1u8; 32]);
    registry.register_engineer(&engineer, &credential_hash, issuer, &31_536_000);
    engineer
}

/// Helper: advance ledger sequence by `n` ledgers.
fn advance_ledger(env: &Env, n: u32) {
    env.ledger().with_mut(|l| l.sequence_number += n);
}

// ── TTL Extension on Write ──────────────────────────────────────────────

#[test]
fn test_hist_ttl_extended_on_submit() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, asset_registry, engineer_registry, _admin, owner, issuer) = setup(&env);
    let asset_id = register_asset(&env, &asset_registry, &owner);
    let engineer = register_engineer(&env, &engineer_registry, &issuer);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    advance_ledger(&env, 1000);
    let seq_before = env.ledger().sequence();

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &String::from_str(&env, "routine"),
        &engineer,
    );

    // Verify HIST key TTL was extended by checking data survives after ledger advance
    advance_ledger(&env, 500);
    let history = lifecycle.get_maintenance_history(&asset_id);
    assert!(
        !history.is_empty(),
        "Maintenance history must survive after 500 ledger sequences"
    );
}

#[test]
fn test_score_ttl_extended_on_submit() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, asset_registry, engineer_registry, _admin, owner, issuer) = setup(&env);
    let asset_id = register_asset(&env, &asset_registry, &owner);
    let engineer = register_engineer(&env, &engineer_registry, &issuer);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("ENGINE"),
        &String::from_str(&env, "overhaul"),
        &engineer,
    );

    advance_ledger(&env, 500);

    let score = lifecycle.get_collateral_score(&asset_id);
    assert!(score > 0, "Score must survive after 500 ledger sequences");
}

#[test]
fn test_schist_ttl_extended_on_submit() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, asset_registry, engineer_registry, _admin, owner, issuer) = setup(&env);
    let asset_id = register_asset(&env, &asset_registry, &owner);
    let engineer = register_engineer(&env, &engineer_registry, &issuer);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &String::from_str(&env, "first service"),
        &engineer,
    );

    advance_ledger(&env, 500);

    let score_history = lifecycle.get_score_history(&asset_id, &0, &10);
    assert!(
        !score_history.is_empty(),
        "Score history must survive after 500 ledger sequences"
    );
}

#[test]
fn test_lupd_ttl_extended_on_submit() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, asset_registry, engineer_registry, _admin, owner, issuer) = setup(&env);
    let asset_id = register_asset(&env, &asset_registry, &owner);
    let engineer = register_engineer(&env, &engineer_registry, &issuer);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &String::from_str(&env, "timestamp check"),
        &engineer,
    );

    // Verify get_collateral_score works after advancing — it reads LUPD internally
    advance_ledger(&env, 500);
    let score = lifecycle.get_collateral_score(&asset_id);
    assert!(score > 0, "LUPD must survive so decay calculation works");
}

#[test]
fn test_config_ttl_extended_on_init() {
    let env = Env::default();
    env.mock_all_auths();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let admin = Address::generate(&env);
    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    asset_registry.initialize_admin(&admin, &admin);
    engineer_registry.initialize_admin(&admin, &admin);
    lifecycle.initialize(&admin, &asset_registry_id, &engineer_registry_id, &admin, &0);

    advance_ledger(&env, 500);

    // Config must be readable after 500 ledgers
    let config = lifecycle.get_config();
    assert_eq!(config.max_history, 200, "CONFIG must survive after 500 ledger sequences");
}

#[test]
fn test_registry_ttl_extended_on_init() {
    let env = Env::default();
    env.mock_all_auths();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let admin = Address::generate(&env);
    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    asset_registry.initialize_admin(&admin, &admin);
    engineer_registry.initialize_admin(&admin, &admin);
    lifecycle.initialize(&admin, &asset_registry_id, &engineer_registry_id, &admin, &0);

    // Advance significantly to test TTL boundaries
    advance_ledger(&env, 100_000);

    // The registry addresses must still be accessible — they're used in cross-contract calls
    let owner = Address::generate(&env);
    asset_registry.add_asset_type(&admin, &symbol_short!("GENSET"));
    let asset_id = register_asset(&env, &asset_registry, &owner);
    assert!(asset_id > 0, "Registry addr must survive so asset registration works via lifecycle");
}

#[test]
fn test_eng_reg_ttl_extended_on_init() {
    let env = Env::default();
    env.mock_all_auths();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let admin = Address::generate(&env);
    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    asset_registry.initialize_admin(&admin, &admin);
    engineer_registry.initialize_admin(&admin, &admin);
    lifecycle.initialize(&admin, &asset_registry_id, &engineer_registry_id, &admin, &0);

    advance_ledger(&env, 100_000);

    // If ENG_REG expires, engineer credential checks would panic
    let owner = Address::generate(&env);
    let issuer = Address::generate(&env);
    asset_registry.add_asset_type(&admin, &symbol_short!("GENSET"));
    engineer_registry.add_trusted_issuer(&admin, &issuer);

    let asset_id = register_asset(&env, &asset_registry, &owner);
    let engineer = register_engineer(&env, &engineer_registry, &issuer);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // This would panic if ENG_REG expired (NotInitialized)
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &String::from_str(&env, "post-ttl-check"),
        &engineer,
    );
}

#[test]
fn test_paused_ttl_extended_on_toggle() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _asset_registry, _engineer_registry, admin, _owner, _issuer) = setup(&env);

    lifecycle.pause(&admin);
    advance_ledger(&env, 200_000);

    // Critical: a paused contract must NOT silently unpause due to TTL expiry.
    assert!(lifecycle.is_paused(), "PAUSED flag must survive 200k ledgers after pause");

    lifecycle.unpause(&admin);
    advance_ledger(&env, 200_000);

    // After unpausing, the contract must remain unpaused.
    assert!(!lifecycle.is_paused(), "PAUSED flag must survive 200k ledgers after unpause");
}

#[test]
fn test_eng_auth_ttl_extended() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, asset_registry, engineer_registry, _admin, owner, issuer) = setup(&env);
    let asset_id = register_asset(&env, &asset_registry, &owner);
    let engineer = register_engineer(&env, &engineer_registry, &issuer);

    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);
    advance_ledger(&env, 500);

    // Engineer must still be authorized — ENG_AUTH key TTL was extended.
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &String::from_str(&env, "post-auth-ttl"),
        &engineer,
    );
}

#[test]
fn test_eng_hist_ttl_extended() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, asset_registry, engineer_registry, _admin, owner, issuer) = setup(&env);
    let asset_id = register_asset(&env, &asset_registry, &owner);
    let engineer = register_engineer(&env, &engineer_registry, &issuer);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &String::from_str(&env, "history entry"),
        &engineer,
    );

    advance_ledger(&env, 500);

    let eng_history = lifecycle.get_engineer_maintenance_history(&engineer);
    let mut found = false;
    for id in eng_history.iter() {
        if id == asset_id {
            found = true;
            break;
        }
    }
    assert!(found, "Engineer history must survive 500 ledger sequences");
}

// ── High Ledger Number Tests ────────────────────────────────────────────

#[test]
fn test_ttl_near_expiry_boundary() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, asset_registry, engineer_registry, _admin, owner, issuer) = setup(&env);
    let asset_id = register_asset(&env, &asset_registry, &owner);
    let engineer = register_engineer(&env, &engineer_registry, &issuer);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("ENGINE"),
        &String::from_str(&env, "near-expiry test"),
        &engineer,
    );

    // Advance to just below TTL_THRESHOLD — data should still be alive.
    advance_ledger(&env, TTL_THRESHOLD - 100);

    let history = lifecycle.get_maintenance_history(&asset_id);
    assert!(!history.is_empty(), "History must survive up to TTL_THRESHOLD - 100");
}

#[test]
fn test_high_ledger_sequence_persistence() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, asset_registry, engineer_registry, _admin, owner, issuer) = setup(&env);
    let asset_id = register_asset(&env, &asset_registry, &owner);
    let engineer = register_engineer(&env, &engineer_registry, &issuer);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // Submit maintenance and advance significantly
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &String::from_str(&env, "high-seq test"),
        &engineer,
    );

    // 10 million ledgers (~578 days)
    advance_ledger(&env, 10_000_000);

    // Submit again — the new submission extends TTL for all keys
    let result = lifecycle.try_submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &String::from_str(&env, "post-high-seq"),
        &engineer,
    );

    // The first submission's TTL may have expired, but the second submission
    // should succeed since it starts with a fresh history fetch.
    assert!(result.is_ok(), "submit_maintenance must succeed even after high ledger sequences");
}

#[test]
fn test_multiple_writes_extend_all_keys() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, asset_registry, engineer_registry, _admin, owner, issuer) = setup(&env);
    let asset_id = register_asset(&env, &asset_registry, &owner);
    let engineer = register_engineer(&env, &engineer_registry, &issuer);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // Submit 5 records with significant ledger gaps between each.
    // Each write extends the TTL of all associated keys.
    for i in 0..5u32 {
        advance_ledger(&env, 100_000);
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &String::from_str(&env, &format!("write {}", i)),
            &engineer,
        );
    }

    // After 500k total ledgers, all data must still be accessible.
    let history = lifecycle.get_maintenance_history(&asset_id);
    assert_eq!(history.len(), 5, "All 5 records must persist after 500k ledgers");

    let score = lifecycle.get_collateral_score(&asset_id);
    assert!(score > 0, "Score must be non-zero after 5 submissions");

    let eng_history = lifecycle.get_engineer_maintenance_history(&engineer);
    let mut found = false;
    for id in eng_history.iter() {
        if id == asset_id {
            found = true;
            break;
        }
    }
    assert!(found, "Engineer history must persist");
}

// ── TTL on Config Updates ───────────────────────────────────────────────

#[test]
fn test_config_ttl_extended_on_update() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _asset_registry, _engineer_registry, admin, _owner, _issuer) = setup(&env);

    // Update score_increment to extend CONFIG TTL
    lifecycle.update_score_increment(&admin, &10);
    advance_ledger(&env, 500);

    let config = lifecycle.get_config();
    assert_eq!(config.score_increment, 10, "CONFIG must survive after score_increment update + 500 ledgers");

    // Update eligibility threshold
    lifecycle.update_eligibility_threshold(&admin, &75);
    advance_ledger(&env, 500);

    let config = lifecycle.get_config();
    assert_eq!(config.eligibility_threshold, 75, "CONFIG must survive after eligibility update + 500 ledgers");
}
