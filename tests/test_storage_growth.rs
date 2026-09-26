//! Regression coverage for bounded snapshot storage.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short, testutils::Address as _, testutils::Ledger, Address, Env, String,
};

#[test]
fn repeated_health_snapshots_do_not_grow_storage_past_configured_cap() {
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

    asset_registry.initialize_admin(&admin, &admin);
    asset_registry.add_asset_type(&admin, &symbol_short!("GENSET"));
    engineer_registry.initialize_admin(&admin, &admin);
    lifecycle.initialize(&admin, &asset_registry_id, &engineer_registry_id, &admin, &0);
    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Bounded snapshot test"),
        &String::from_str(&env, "SNAPSHOT-CAP-001"),
        &owner,
    );

    lifecycle.update_max_snapshots(&admin, &3);
    for _ in 0..100 {
        env.ledger().set_timestamp(env.ledger().timestamp() + 1);
        lifecycle.take_health_snapshot(&asset_id);
    }

    let snapshots = lifecycle.get_health_snapshots(&asset_id);
    assert_eq!(snapshots.len(), 3, "snapshot storage must remain capped");
    assert_eq!(
        snapshots.get(0).unwrap().snapshot_timestamp,
        env.ledger().timestamp() - 2,
        "oldest retained snapshot should be the first entry in the cap window"
    );
    assert!(
        snapshots.iter().all(|snapshot| !snapshot.reconstructed),
        "retention must preserve snapshot records without mutating their state"
    );
}
