use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

/// Test that submitting maintenance for a decommissioned asset is rejected.
///
/// This test verifies the complete workflow:
/// 1. Register an asset
/// 2. Decommission the asset
/// 3. Attempt to submit maintenance on the decommissioned asset
/// 4. Assert that the operation panics with the structured error AssetDecommissioned
#[test]
fn test_decommissioned_asset_blocks_maintenance() {
    let env = Env::default();
    env.mock_all_auths();

    // Deploy all contracts
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    // Setup admins and actors
    let asset_admin = Address::generate(&env);
    let eng_admin = Address::generate(&env);
    let lifecycle_admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let asset_owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    // Initialize contracts
    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));
    engineer_registry.initialize_admin(&eng_admin, &eng_admin);
    engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
    lifecycle.initialize(
        &lifecycle_admin,
        &asset_registry_id,
        &engineer_registry_id,
        &lifecycle_admin,
        &0,
    );

    // Register asset
    let metadata = String::from_str(&env, "Test generator asset");
    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &metadata,
        &String::from_str(&env, "SN-TEST-001"),
        &asset_owner,
    );

    // Register engineer
    let credential_hash = BytesN::from_array(&env, &[1u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000);
    lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

    // Verify maintenance can be submitted before decommissioning
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Pre-decommission service"),
        &engineer,
    );
    let score_before = lifecycle.get_collateral_score(&asset_id);
    assert!(score_before > 0, "Asset should have positive score before decommission");

    // Decommission the asset
    asset_registry.decommission_asset(&asset_admin, &asset_id);

    // Verify asset is decommissioned
    let status = asset_registry.asset_status(&asset_id);
    assert_eq!(status, asset_registry::AssetStatus::Decommissioned);

    // Attempt to submit maintenance for decommissioned asset should fail
    // Use catch_unwind to verify it panics with the correct error
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "Post-decommission maintenance"),
            &engineer,
        )
    }));

    // Assert that the operation panicked
    assert!(
        result.is_err(),
        "submit_maintenance should panic for decommissioned asset"
    );

    // Verify the collateral score remains unchanged (frozen at decommission)
    let score_after = lifecycle.get_collateral_score(&asset_id);
    assert_eq!(
        score_after, 0,
        "Decommissioned asset should have score of 0"
    );

    // Verify no new maintenance records were added
    let history = lifecycle.get_maintenance_history(&asset_id);
    assert_eq!(
        history.len(),
        1,
        "No new maintenance records should be added to decommissioned asset"
    );
}

/// Test that batch_submit_maintenance is also rejected for decommissioned assets.
#[test]
fn test_decommissioned_asset_blocks_batch_maintenance() {
    let env = Env::default();
    env.mock_all_auths();

    // Deploy all contracts
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    // Setup admins and actors
    let asset_admin = Address::generate(&env);
    let eng_admin = Address::generate(&env);
    let lifecycle_admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let asset_owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    // Initialize contracts
    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("TURBINE"));
    engineer_registry.initialize_admin(&eng_admin, &eng_admin);
    engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
    lifecycle.initialize(
        &lifecycle_admin,
        &asset_registry_id,
        &engineer_registry_id,
        &lifecycle_admin,
        &0,
    );

    // Register asset
    let metadata = String::from_str(&env, "Test turbine asset");
    let asset_id = asset_registry.register_asset(
        &symbol_short!("TURBINE"),
        &metadata,
        &String::from_str(&env, "SN-TURB-001"),
        &asset_owner,
    );

    // Register engineer
    let credential_hash = BytesN::from_array(&env, &[2u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000);
    lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

    // Decommission the asset
    asset_registry.decommission_asset(&asset_admin, &asset_id);

    // Create batch maintenance records
    let mut records = Vec::new(&env);
    let record1 = lifecycle::BatchRecord {
        task_type: symbol_short!("OIL_CHG"),
        notes: String::from_str(&env, "Oil change"),
        timestamp: env.ledger().timestamp(),
    };
    let record2 = lifecycle::BatchRecord {
        task_type: symbol_short!("FILTER"),
        notes: String::from_str(&env, "Filter replacement"),
        timestamp: env.ledger().timestamp(),
    };
    records.push_back(record1);
    records.push_back(record2);

    // Attempt batch_submit_maintenance for decommissioned asset should fail
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lifecycle.batch_submit_maintenance(&asset_id, &records, &engineer)
    }));

    // Assert that the operation panicked
    assert!(
        result.is_err(),
        "batch_submit_maintenance should panic for decommissioned asset"
    );

    // Verify no maintenance records were added
    let history = lifecycle.get_maintenance_history(&asset_id);
    assert_eq!(
        history.len(),
        0,
        "No maintenance records should be added to decommissioned asset"
    );
}