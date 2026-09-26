//! Field-by-field state snapshot assertions for lifecycle transitions.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short, testutils::Address as _, testutils::Ledger, Address, BytesN, Env, String,
};

#[test]
fn health_snapshot_changes_match_the_maintenance_state_transition() {
    let env = Env::default();
    env.mock_all_auths();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());
    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);
    let admin = Address::generate(&env);
    let owner = Address::generate(&env);
    let issuer = Address::generate(&env);
    let engineer = Address::generate(&env);

    asset_registry.initialize_admin(&admin, &admin);
    asset_registry.add_asset_type(&admin, &symbol_short!("GENSET"));
    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);
    lifecycle.initialize(&admin, &asset_registry_id, &engineer_registry_id, &admin, &0);
    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "State snapshot test"),
        &String::from_str(&env, "STATE-SNAPSHOT-001"),
        &owner,
    );
    engineer_registry.register_engineer(
        &engineer,
        &BytesN::from_array(&env, &[7u8; 32]),
        &issuer,
        &31_536_000,
        &None,
    );
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    let before_timestamp = env.ledger().timestamp();
    let before = lifecycle.take_health_snapshot(&asset_id);
    assert_eq!(before.snapshot_timestamp, before_timestamp);
    assert_eq!(before.score, 0);
    assert_eq!(before.maintenance_count, 0);
    assert_eq!(before.last_service_date, 0);
    assert!(!before.reconstructed);

    env.ledger().set_timestamp(before_timestamp + 1);
    let maintenance_timestamp = env.ledger().timestamp();
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &String::from_str(&env, "state transition"),
        &engineer,
    );

    let after = lifecycle.take_health_snapshot(&asset_id);
    assert_eq!(after.snapshot_timestamp, maintenance_timestamp);
    assert!(after.score > 0);
    assert_eq!(after.maintenance_count, 1);
    assert_eq!(after.last_service_date, maintenance_timestamp);
    assert!(!after.reconstructed);
}
