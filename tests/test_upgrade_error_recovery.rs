//! Error recovery tests for failed contract upgrades.
//!
//! These tests validate that the contracts handle upgrade failure scenarios
//! gracefully and that on-chain state is never corrupted by a failed upgrade
//! attempt. They cover:
//!
//! 1. **Double-initialization protection**: Calling `initialize` on an
//!    already-initialized contract must panic with `AlreadyInitialized`.
//! 2. **Timelock enforcement**: `execute_upgrade` before the timelock delay
//!    elapses must panic with `TimelockNotExpired`.
//! 3. **Unauthorized upgrade proposals**: Only the admin can propose upgrades.
//! 4. **Paused contract blocking upgrades**: Upgrades are rejected while paused.
//! 5. **Data integrity after failed upgrade**: Storage is untouched when an
//!    upgrade attempt fails.
//! 6. **Proposal lifecycle**: propose → execute flow, already-executed
//!    rejection, and idempotent proposal overwrite.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient, CredentialStatus};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

// ═══════════════════════════════════════════════════════════════════════════
// Double-initialization protection
// ═══════════════════════════════════════════════════════════════════════════

/// Calling `initialize_admin` twice on the asset-registry must panic.
#[test]
fn test_asset_registry_double_init_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AssetRegistry, ());
    let client = AssetRegistryClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    client.initialize_admin(&admin, &admin);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.initialize_admin(&admin, &admin);
    }));
    assert!(
        result.is_err(),
        "Double initialize_admin on asset-registry must panic"
    );
}

/// Calling `initialize_admin` twice on the engineer-registry must panic.
#[test]
fn test_engineer_registry_double_init_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(EngineerRegistry, ());
    let client = EngineerRegistryClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    client.initialize_admin(&admin, &admin);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.initialize_admin(&admin, &admin);
    }));
    assert!(
        result.is_err(),
        "Double initialize_admin on engineer-registry must panic"
    );
}

/// Calling `initialize` twice on the lifecycle contract must panic.
#[test]
fn test_lifecycle_double_init_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(Lifecycle, ());
    let client = LifecycleClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let asset_registry = Address::generate(&env);
    let engineer_registry = Address::generate(&env);

    client.initialize(&admin, &asset_registry, &engineer_registry, &admin, &200);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.initialize(&admin, &asset_registry, &engineer_registry, &admin, &200);
    }));
    assert!(
        result.is_err(),
        "Double initialize on lifecycle must panic"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Timelock enforcement for upgrades
// ═══════════════════════════════════════════════════════════════════════════

/// `execute_upgrade` before the timelock delay elapses must panic with
/// `TimelockNotExpired`.
#[test]
fn test_execute_upgrade_before_timelock_fails_asset_registry() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AssetRegistry, ());
    let client = AssetRegistryClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    client.initialize_admin(&admin, &admin);

    let new_wasm_hash = BytesN::from_array(&env, &[0xAAu8; 32]);
    client.propose_upgrade(&admin, &new_wasm_hash);

    // Execute immediately — timelock has not expired (48 hours)
    let result = client.try_execute_upgrade(&admin);
    assert!(
        result.is_err(),
        "execute_upgrade before timelock expiry must fail"
    );
}

/// `execute_upgrade` before the timelock delay elapses must panic for
/// engineer-registry.
#[test]
fn test_execute_upgrade_before_timelock_fails_engineer_registry() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(EngineerRegistry, ());
    let client = EngineerRegistryClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    client.initialize_admin(&admin, &admin);

    let new_wasm_hash = BytesN::from_array(&env, &[0xBBu8; 32]);
    client.propose_upgrade(&admin, &new_wasm_hash);

    let result = client.try_execute_upgrade(&admin);
    assert!(
        result.is_err(),
        "execute_upgrade before timelock expiry must fail in engineer-registry"
    );
}

/// `execute_upgrade` before the timelock delay elapses must panic for lifecycle.
#[test]
fn test_execute_upgrade_before_timelock_fails_lifecycle() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(Lifecycle, ());
    let client = LifecycleClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let asset_registry = Address::generate(&env);
    let engineer_registry = Address::generate(&env);

    client.initialize(&admin, &asset_registry, &engineer_registry, &admin, &200);

    let new_wasm_hash = BytesN::from_array(&env, &[0xCCu8; 32]);
    client.propose_upgrade(&admin, &new_wasm_hash);

    let result = client.try_execute_upgrade(&admin);
    assert!(
        result.is_err(),
        "execute_upgrade before timelock expiry must fail in lifecycle"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Unauthorized upgrade proposals
// ═══════════════════════════════════════════════════════════════════════════

/// Only the admin can propose an upgrade. Non-admin proposals must fail.
#[test]
fn test_non_admin_cannot_propose_upgrade() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(EngineerRegistry, ());
    let client = EngineerRegistryClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let outsider = Address::generate(&env);

    client.initialize_admin(&admin, &admin);

    let new_wasm_hash = BytesN::from_array(&env, &[0xDDu8; 32]);

    // outsider (not admin) tries to propose upgrade
    let result = client.try_propose_upgrade(&outsider, &new_wasm_hash);
    assert!(
        result.is_err(),
        "Non-admin must not be able to propose an upgrade"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Paused contract blocks upgrades
// ═══════════════════════════════════════════════════════════════════════════

/// Proposing an upgrade while the contract is paused must fail.
#[test]
fn test_paused_contract_rejects_upgrade_proposal() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(EngineerRegistry, ());
    let client = EngineerRegistryClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    client.initialize_admin(&admin, &admin);

    // Pause the contract
    client.pause(&admin);
    assert!(client.is_paused());

    // Try to propose upgrade while paused
    let new_wasm_hash = BytesN::from_array(&env, &[0xEEu8; 32]);
    let result = client.try_propose_upgrade(&admin, &new_wasm_hash);
    assert!(
        result.is_err(),
        "Upgrade proposal must be rejected when contract is paused"
    );

    // Unpause and retry — must succeed
    client.unpause(&admin);
    assert!(!client.is_paused());

    let result2 = client.try_propose_upgrade(&admin, &new_wasm_hash);
    assert!(
        result2.is_ok(),
        "Upgrade proposal must succeed after unpause"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Data integrity after failed upgrade
// ═══════════════════════════════════════════════════════════════════════════

/// When an upgrade attempt fails (e.g. timelock not expired), all existing
/// data must remain intact and accessible.
#[test]
fn test_data_integrity_after_failed_upgrade() {
    let env = Env::default();
    env.mock_all_auths();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    // Seed data
    asset_registry.initialize_admin(&admin, &admin);
    asset_registry.add_asset_type(&admin, &symbol_short!("GENSET"));

    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);

    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &200,
    );

    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Data integrity asset"),
        &String::from_str(&env, "SN-INT-001"),
        &owner,
    );

    let credential_hash = BytesN::from_array(&env, &[0xFFu8; 32]);
    engineer_registry.register_engineer(
        &engineer,
        &credential_hash,
        &issuer,
        &31_536_000,
        &Some(String::from_str(&env, "Data integrity engineer")),
    );

    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("ENGINE"),
        &String::from_str(&env, "Pre-failure maintenance"),
        &engineer,
    );

    // Capture pre-failure state
    let pre_score = lifecycle.get_collateral_score(&asset_id);
    let pre_history_len = lifecycle.get_maintenance_history(&asset_id).len();
    let pre_status = engineer_registry.verify_engineer(&engineer);

    // Attempt a failed upgrade on engineer-registry (timelock not expired)
    let upgrade_hash = BytesN::from_array(&env, &[0x11u8; 32]);
    engineer_registry.propose_upgrade(&admin, &upgrade_hash);

    let upgrade_result = engineer_registry.try_execute_upgrade(&admin);
    assert!(
        upgrade_result.is_err(),
        "execute_upgrade before timelock must fail"
    );

    // Verify data integrity post-failure
    assert_eq!(
        lifecycle.get_collateral_score(&asset_id),
        pre_score,
        "Collateral score must be unchanged after failed upgrade"
    );
    assert_eq!(
        lifecycle.get_maintenance_history(&asset_id).len(),
        pre_history_len,
        "Maintenance history must be intact after failed upgrade"
    );
    assert_eq!(
        engineer_registry.verify_engineer(&engineer),
        pre_status,
        "Engineer credential status must be unchanged after failed upgrade"
    );

    // Asset data intact
    let asset = asset_registry.get_asset(&asset_id);
    assert_eq!(asset.asset_type, symbol_short!("GENSET"));
    assert_eq!(asset.owner, owner);

    // Engineer data intact
    let eng_record = engineer_registry.get_engineer(&engineer);
    assert!(eng_record.active);
    assert_eq!(eng_record.issuer, issuer);
}

// ═══════════════════════════════════════════════════════════════════════════
// Upgrade proposal lifecycle
// ═══════════════════════════════════════════════════════════════════════════

/// Executing an already-executed upgrade proposal must be rejected.
#[test]
fn test_execute_already_executed_upgrade_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AssetRegistry, ());
    let client = AssetRegistryClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    client.initialize_admin(&admin, &admin);

    let new_wasm_hash = BytesN::from_array(&env, &[0x22u8; 32]);
    client.propose_upgrade(&admin, &new_wasm_hash);

    // Advance past timelock
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 48 * 60 * 60 + 1);

    // First execution should succeed (in test mode, the #[cfg(not(test))]
    // guards prevent actual wasm update but the logic before that passes)
    let result1 = client.try_execute_upgrade(&admin);
    assert!(
        result1.is_ok(),
        "First execute_upgrade after timelock must succeed"
    );

    // Second execution must fail (already executed)
    let result2 = client.try_execute_upgrade(&admin);
    assert!(
        result2.is_err(),
        "execute_upgrade on already-executed proposal must fail"
    );
}

/// Proposing a second upgrade while one is pending overwrites the pending
/// proposal (idempotent overwrite behavior) — the second proposal is
/// accepted, which is the correct behavior for upgrading to a newer hash
/// before the timelock on the first expires.
#[test]
fn test_second_upgrade_proposal_overwrites_pending() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AssetRegistry, ());
    let client = AssetRegistryClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    client.initialize_admin(&admin, &admin);

    let hash1 = BytesN::from_array(&env, &[0x33u8; 32]);
    client.propose_upgrade(&admin, &hash1);

    // Second proposal with a different hash: should succeed (overwrites)
    let hash2 = BytesN::from_array(&env, &[0x44u8; 32]);
    let result = client.try_propose_upgrade(&admin, &hash2);
    assert!(
        result.is_ok(),
        "Second upgrade proposal must succeed (overwrites pending)"
    );

    // Advance past timelock and execute — the most recent hash (hash2)
    // is the one that should take effect
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 48 * 60 * 60 + 1);

    let exec_result = client.try_execute_upgrade(&admin);
    assert!(
        exec_result.is_ok(),
        "execute_upgrade after timelock must succeed with latest hash"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Successful upgrade with timelock satisfaction
// ═══════════════════════════════════════════════════════════════════════════

/// After the timelock delay elapses, `execute_upgrade` must succeed.
/// (In test mode, the actual WASM update is gated by `#[cfg(not(test))]`
/// but the pre-update validations are exercised.)
#[test]
fn test_successful_upgrade_after_timelock() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(EngineerRegistry, ());
    let client = EngineerRegistryClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    client.initialize_admin(&admin, &admin);

    let new_wasm_hash = BytesN::from_array(&env, &[0x55u8; 32]);
    client.propose_upgrade(&admin, &new_wasm_hash);

    // Advance past the 48-hour timelock
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 48 * 60 * 60 + 1);

    let result = client.try_execute_upgrade(&admin);
    assert!(
        result.is_ok(),
        "execute_upgrade after timelock must succeed"
    );

    // Verify admin is still intact after upgrade
    let stored_admin = client.get_admin();
    assert_eq!(stored_admin, admin, "Admin must persist after successful upgrade");
}

// ═══════════════════════════════════════════════════════════════════════════
// Cross-contract recovery: lifecycle must keep working even if one
// dependency attempted a failed upgrade
// ═══════════════════════════════════════════════════════════════════════════

/// If the engineer-registry has a failed upgrade, the lifecycle contract
/// must still function normally (no cascading failure).
#[test]
fn test_lifecycle_operates_after_dependency_upgrade_failure() {
    let env = Env::default();
    env.mock_all_auths();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    // Seed data
    asset_registry.initialize_admin(&admin, &admin);
    asset_registry.add_asset_type(&admin, &symbol_short!("GENSET"));

    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);

    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &200,
    );

    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Cascading failure test asset"),
        &String::from_str(&env, "SN-CAS-001"),
        &owner,
    );

    let credential_hash = BytesN::from_array(&env, &[0x66u8; 32]);
    engineer_registry.register_engineer(
        &engineer,
        &credential_hash,
        &issuer,
        &31_536_000,
        &None,
    );

    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // Cause a failed upgrade on engineer-registry
    let upgrade_hash = BytesN::from_array(&env, &[0x77u8; 32]);
    engineer_registry.propose_upgrade(&admin, &upgrade_hash);

    // Execute before timelock → fails
    let _ = engineer_registry.try_execute_upgrade(&admin);

    // Lifecycle must still function: engineer verification, maintenance
    assert_eq!(
        engineer_registry.verify_engineer(&engineer),
        CredentialStatus::Valid,
        "Engineer must still be verified after dependency upgrade failure"
    );

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("ENGINE"),
        &String::from_str(&env, "Post-dependency-failure maintenance"),
        &engineer,
    );

    let score = lifecycle.get_collateral_score(&asset_id);
    assert!(
        score > 0,
        "Lifecycle must still produce collateral score after dependency upgrade failure"
    );
}
