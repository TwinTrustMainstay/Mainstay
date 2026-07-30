/// Test #1043: Verify that transfer_asset is rejected when is_locked = true 
/// (asset is under a lien).
/// 
/// This test ensures that the asset registry properly prevents ownership transfers 
/// when an asset is locked as collateral:
/// - Locked assets cannot be transferred (rejected with AssetLocked error)
/// - After unlocking, the same asset can be transferred successfully

use asset_registry::{AssetRegistry, AssetRegistryClient};
use soroban_sdk::{symbol_short, testutils::Address as _, Address, Env, String};

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

fn setup_for_lien_test(env: &Env) -> (AssetRegistryClient, Address, Address, u64) {
    let contract_id = env.register(AssetRegistry, ());
    let client = AssetRegistryClient::new(env, &contract_id);

    let admin = Address::generate(env);
    let owner = Address::generate(env);

    env.mock_all_auths();

    // Initialize contract
    client.initialize_admin(&admin, &admin);
    client.add_asset_type(&admin, &symbol_short!("GENSET"));

    // Register an asset
    let asset_id = client.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(env, "Caterpillar CAT-3516 Diesel Generator"),
        &unique_serial(env),
        &owner,
    );

    (client, admin, owner, asset_id)
}

#[test]
fn test_transfer_blocked_when_asset_locked_as_collateral() {
    let env = Env::default();
    let (client, admin, owner, asset_id) = setup_for_lien_test(&env);

    let new_owner = Address::generate(&env);
    let lending_contract = Address::generate(&env);

    env.mock_all_auths();

    // Set up lending contract
    client.set_lending_contract(&admin, &lending_contract);

    // Lock the asset as collateral with a loan
    let loan_id = 1u64;
    client.lock_asset_as_collateral(&lending_contract, &asset_id, &loan_id);

    // Verify asset is locked
    let asset = client.get_asset(&asset_id);
    assert!(
        asset.is_locked,
        "asset must be locked after lock_asset_as_collateral"
    );

    // Attempt to transfer the locked asset - should fail
    let result = client.try_transfer_asset(&asset_id, &owner, &new_owner);
    assert!(
        result.is_err(),
        "transfer of locked asset must fail with error"
    );

    // Verify ownership has not changed
    let asset_after = client.get_asset(&asset_id);
    assert_eq!(
        asset_after.owner, owner,
        "ownership must remain unchanged after failed transfer attempt"
    );
}

#[test]
fn test_transfer_allowed_after_asset_unlocked() {
    let env = Env::default();
    let (client, admin, owner, asset_id) = setup_for_lien_test(&env);

    let new_owner = Address::generate(&env);
    let lending_contract = Address::generate(&env);

    env.mock_all_auths();

    // Set up lending contract
    client.set_lending_contract(&admin, &lending_contract);

    // Lock the asset as collateral
    let loan_id = 1u64;
    client.lock_asset_as_collateral(&lending_contract, &asset_id, &loan_id);

    // Verify asset is locked
    let asset_locked = client.get_asset(&asset_id);
    assert!(asset_locked.is_locked, "asset must be locked");

    // Unlock the asset
    client.unlock_asset_from_collateral(&lending_contract, &asset_id, &loan_id);

    // Verify asset is no longer locked
    let asset_unlocked = client.get_asset(&asset_id);
    assert!(
        !asset_unlocked.is_locked,
        "asset must be unlocked after unlock_asset_from_collateral"
    );

    // Transfer should now succeed
    client.transfer_asset(&asset_id, &owner, &new_owner);

    // Verify ownership has changed
    let asset_after = client.get_asset(&asset_id);
    assert_eq!(
        asset_after.owner, new_owner,
        "ownership must be transferred after asset is unlocked"
    );
}

#[test]
fn test_transfer_fails_until_lien_released() {
    let env = Env::default();
    let (client, admin, owner, asset_id) = setup_for_lien_test(&env);

    let new_owner = Address::generate(&env);
    let lending_contract = Address::generate(&env);

    env.mock_all_auths();

    // Set up lending contract
    client.set_lending_contract(&admin, &lending_contract);

    // Lock the asset with loan_id = 1
    let loan_id_1 = 1u64;
    client.lock_asset_as_collateral(&lending_contract, &asset_id, &loan_id_1);

    // Attempt transfer - fails
    let result1 = client.try_transfer_asset(&asset_id, &owner, &new_owner);
    assert!(result1.is_err(), "transfer must fail while locked");

    // Unlock the asset
    client.unlock_asset_from_collateral(&lending_contract, &asset_id, &loan_id_1);

    // Now transfer should succeed
    client.transfer_asset(&asset_id, &owner, &new_owner);

    let asset = client.get_asset(&asset_id);
    assert_eq!(asset.owner, new_owner);
}

#[test]
fn test_lien_metadata_cleared_after_unlock() {
    let env = Env::default();
    let (client, admin, _owner, asset_id) = setup_for_lien_test(&env);

    let lending_contract = Address::generate(&env);
    let loan_id = 42u64;

    env.mock_all_auths();

    // Set up lending contract
    client.set_lending_contract(&admin, &lending_contract);

    // Lock the asset
    client.lock_asset_as_collateral(&lending_contract, &asset_id, &loan_id);

    // Verify lien metadata
    let asset_locked = client.get_asset(&asset_id);
    assert_eq!(asset_locked.lender, Some(lending_contract.clone()));
    assert_eq!(asset_locked.loan_id, Some(loan_id));

    // Unlock
    client.unlock_asset_from_collateral(&lending_contract, &asset_id, &loan_id);

    // Verify lien metadata is cleared
    let asset_unlocked = client.get_asset(&asset_id);
    assert!(asset_unlocked.lender.is_none(), "lender must be cleared after unlock");
    assert!(asset_unlocked.loan_id.is_none(), "loan_id must be cleared after unlock");
    assert!(!asset_unlocked.is_locked, "is_locked must be false after unlock");
}

#[test]
fn test_freshly_registered_asset_not_locked() {
    let env = Env::default();
    let (client, _admin, _owner, asset_id) = setup_for_lien_test(&env);

    let asset = client.get_asset(&asset_id);
    assert!(
        !asset.is_locked,
        "freshly registered asset must not be locked"
    );
    assert!(asset.lender.is_none(), "freshly registered asset must have no lender");
    assert!(asset.loan_id.is_none(), "freshly registered asset must have no loan_id");
}
