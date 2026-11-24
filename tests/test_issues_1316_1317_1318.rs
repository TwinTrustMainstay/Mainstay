/// Tests for issues #1316, #1317, and #1318
/// #1316: Asset transfer proposal counter-offer mechanism
/// #1317: Multi-asset bundle registration for fleet operators
/// #1318: Collateral pool for multi-asset loans

use asset_registry::{AssetRegistry, AssetRegistryClient, TransferCondition, BundleRegistration, CollateralPool};
use soroban_sdk::{symbol_short, testutils::Address as _, Address, Env, String, BytesN};

fn unique_serial(env: &Env) -> String {
    String::from_str(
        env,
        &format!("SN-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        )
    )
}

fn setup(env: &Env) -> (AssetRegistryClient, Address) {
    let contract_id = env.register(AssetRegistry, ());
    let client = AssetRegistryClient::new(env, &contract_id);

    let admin = Address::generate(env);
    env.mock_all_auths();

    client.initialize_admin(&admin, &admin);
    client.add_asset_type(&admin, &symbol_short!("GENSET"));

    (client, admin)
}

// ============================================================================
// Issue #1316 Tests: Transfer Condition Counter-Offers
// ============================================================================

#[test]
fn test_issue_1316_propose_transfer_condition() {
    let env = Env::default();
    let (client, _admin) = setup(&env);

    let owner = Address::generate(&env);
    let recipient = Address::generate(&env);

    env.mock_all_auths();

    // Register an asset
    let asset_id = client.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Diesel Generator"),
        &unique_serial(&env),
        &owner,
    );

    // Initiate transfer
    client.initiate_ownership_transfer(&asset_id, &recipient);

    // Recipient proposes condition: score must reach 50
    client.propose_transfer_condition(&asset_id, &symbol_short!("SCORE_MIN"), &50u64);

    // Verify condition was set
    let condition = client.get_transfer_condition(&asset_id);
    assert!(condition.is_some(), "condition should be set");

    let cond = condition.unwrap();
    assert_eq!(cond.threshold, 50u64, "threshold should be 50");
}

#[test]
fn test_issue_1316_condition_blocks_transfer() {
    let env = Env::default();
    let (client, _admin) = setup(&env);

    let owner = Address::generate(&env);
    let recipient = Address::generate(&env);

    env.mock_all_auths();

    let asset_id = client.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Diesel Generator"),
        &unique_serial(&env),
        &owner,
    );

    client.initiate_ownership_transfer(&asset_id, &recipient);
    client.propose_transfer_condition(&asset_id, &symbol_short!("SCORE_MIN"), &500u64);

    // Try to accept transfer (should fail because score is too low)
    let result = client.try_accept_ownership_transfer(&asset_id);
    assert!(result.is_err(), "transfer should be blocked by condition");
}

// ============================================================================
// Issue #1317 Tests: Bundle Registration
// ============================================================================

#[test]
fn test_issue_1317_register_asset_bundle() {
    let env = Env::default();
    let (client, admin) = setup(&env);

    env.mock_all_auths();

    // Create a dummy merkle root
    let merkle_root: BytesN<32> = env.crypto().sha256(&String::from_str(&env, "test").to_xdr(&env)).into();

    // Register bundle
    client.register_asset_bundle(&admin, &merkle_root, &100u32);

    // Verify bundle was registered
    let bundle = client.get_bundle(&merkle_root);
    assert!(bundle.is_some(), "bundle should be registered");

    let b = bundle.unwrap();
    assert_eq!(b.asset_count, 100u32, "asset count should be 100");
}

#[test]
fn test_issue_1317_bundle_only_admin() {
    let env = Env::default();
    let (client, _admin) = setup(&env);

    let non_admin = Address::generate(&env);
    let merkle_root: BytesN<32> = env.crypto().sha256(&String::from_str(&env, "test").to_xdr(&env)).into();

    env.mock_all_auths();

    // Try to register bundle as non-admin (should fail)
    let result = client.try_register_asset_bundle(&non_admin, &merkle_root, &100u32);
    assert!(result.is_err(), "non-admin should not be able to register bundle");
}

// ============================================================================
// Issue #1318 Tests: Collateral Pool
// ============================================================================

#[test]
fn test_issue_1318_create_collateral_pool() {
    let env = Env::default();
    let (client, _admin) = setup(&env);

    let owner = Address::generate(&env);
    let lender = Address::generate(&env);

    env.mock_all_auths();

    // Register multiple assets
    let asset_ids = vec![
        &env,
        client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Generator 1"),
            &unique_serial(&env),
            &owner,
        ),
        client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Generator 2"),
            &unique_serial(&env),
            &owner,
        ),
    ];

    // Create collateral pool
    let pool_id = client.create_collateral_pool(&lender, &asset_ids);

    // Verify pool was created
    let pool = client.get_collateral_pool(&pool_id);
    assert!(pool.is_some(), "pool should be created");

    let p = pool.unwrap();
    assert_eq!(p.pool_id, pool_id, "pool ID should match");
    assert_eq!(p.asset_ids.len() as u32, 2u32, "pool should have 2 assets");
    assert!(p.is_locked, "pool should be locked");
}

#[test]
fn test_issue_1318_pool_requires_multiple_assets() {
    let env = Env::default();
    let (client, _admin) = setup(&env);

    let owner = Address::generate(&env);
    let lender = Address::generate(&env);

    env.mock_all_auths();

    let asset_id = client.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Generator 1"),
        &unique_serial(&env),
        &owner,
    );

    let single_asset = vec![&env, asset_id];

    // Try to create pool with single asset (should fail)
    let result = client.try_create_collateral_pool(&lender, &single_asset);
    assert!(result.is_err(), "pool with single asset should fail");
}

#[test]
fn test_issue_1318_release_collateral_pool() {
    let env = Env::default();
    let (client, _admin) = setup(&env);

    let owner = Address::generate(&env);
    let lender = Address::generate(&env);

    env.mock_all_auths();

    let asset_ids = vec![
        &env,
        client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Generator 1"),
            &unique_serial(&env),
            &owner,
        ),
        client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Generator 2"),
            &unique_serial(&env),
            &owner,
        ),
    ];

    let pool_id = client.create_collateral_pool(&lender, &asset_ids);

    // Release the pool
    client.release_collateral_pool(&lender, &pool_id);

    // Verify pool is unlocked
    let pool = client.get_collateral_pool(&pool_id);
    assert!(pool.is_some(), "pool should still exist");

    let p = pool.unwrap();
    assert!(!p.is_locked, "pool should be unlocked after release");
}
