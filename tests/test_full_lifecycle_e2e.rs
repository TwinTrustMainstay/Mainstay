use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient, EngineerStatus, CredentialStatus};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String, Symbol,
};

#[test]
fn test_full_lifecycle_e2e() {
    let env = Env::default();
    env.mock_all_auths();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    let admin = Address::generate(&env);
    let asset_admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));
    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);
    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &0,
    );

    let metadata = String::from_str(&env, "Lifecycle E2E generator");
    let asset_id = asset_registry.register_asset(&symbol_short!("GENSET"), &metadata, &owner);
    let asset = asset_registry.get_asset(&asset_id);
    assert_eq!(asset.asset_id, asset_id);
    assert_eq!(asset.asset_type, symbol_short!("GENSET"));
    assert_eq!(asset.metadata, metadata);
    assert_eq!(asset.owner, owner);

    let credential_hash = BytesN::from_array(&env, &[7u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);
    let engineer_record = engineer_registry.get_engineer(&engineer);
    assert_eq!(engineer_record.address, engineer);
    assert_eq!(engineer_record.credential_hash, credential_hash);
    assert_eq!(engineer_record.issuer, issuer);
    assert!(engineer_record.active);

    assert_eq!(
        engineer_registry.verify_engineer(&engineer, &None::<Symbol>),
        engineer_registry::CredentialStatus::Valid
    );
    assert_eq!(
        engineer_registry.get_engineer_status(&engineer),
        EngineerStatus::Active
    );

    for i in 0..5u32 {
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "Engine overhaul"),
            &engineer, &None,
        );

        let history = lifecycle.get_maintenance_history(&asset_id);
        assert_eq!(history.len(), i + 1);
        let record = history.get(i).unwrap();
        assert_eq!(record.asset_id, asset_id);
        assert_eq!(record.task_type, symbol_short!("ENGINE"));
        assert_eq!(record.engineer, engineer);
        assert_eq!(lifecycle.get_collateral_score(&asset_id), (i + 1) * 10);

        env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    }

    let score = lifecycle.get_collateral_score(&asset_id);
    assert_eq!(score, 50);
    assert_eq!(lifecycle.get_score_history(&asset_id).len(), 5);
    assert!(lifecycle.is_collateral_eligible(&asset_id));

    let last_record = lifecycle.get_last_maintenance(&asset_id).unwrap();
    assert_eq!(last_record.asset_id, asset_id);
    assert_eq!(last_record.task_type, symbol_short!("ENGINE"));
    assert_eq!(last_record.engineer, engineer);
}

#[test]
fn test_asset_transfer_preserves_history() {
    let env = Env::default();
    env.mock_all_auths();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    let admin = Address::generate(&env);
    let asset_admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let owner = Address::generate(&env);
    let new_owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));
    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);
    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &0,
    );

    let metadata = String::from_str(&env, "Transfer asset");
    let asset_id = asset_registry.register_asset(&symbol_short!("GENSET"), &metadata, &owner);

    let credential_hash = BytesN::from_array(&env, &[7u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);

    for i in 0..3u32 {
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "Maintenance task"),
            &engineer, &None,
        );
        env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    }

    let pre_score = lifecycle.get_collateral_score(&asset_id);
    assert_eq!(pre_score, 30);

    // Transfer asset to new owner
    asset_registry.transfer_asset(&asset_id, &owner, &new_owner);
    lifecycle.record_transfer(&asset_id, &owner, &new_owner);

    // Assert maintenance history unchanged for new owner (sentinel appended but 3 records still exist)
    let history = lifecycle.get_maintenance_history(&asset_id);
    assert_eq!(history.len(), 4); // 3 maintenance + 1 sentinel
    
    // Check previous maintenance records are unchanged
    assert_eq!(history.get(0).unwrap().engineer, engineer);
    assert_eq!(history.get(1).unwrap().engineer, engineer);
    assert_eq!(history.get(2).unwrap().engineer, engineer);
    
    // Check sentinel
    let sentinel = history.get(3).unwrap();
    assert_eq!(sentinel.task_type, symbol_short!("XFER"));
    assert_eq!(sentinel.engineer, new_owner);

    // Assert collateral score unchanged after transfer
    let post_score = lifecycle.get_collateral_score(&asset_id);
    assert_eq!(post_score, 30);
    assert_eq!(lifecycle.get_score_history(&asset_id).len(), 3);
}
