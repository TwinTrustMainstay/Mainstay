//! Interleaving coverage for operations that update shared asset state.
//!
//! Soroban transactions are isolated, so the deterministic test equivalent of
//! concurrent callers is to interleave independent callers against one ledger
//! state and verify that neither update is lost.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short, testutils::Address as _, Address, BytesN, Env, String,
};

#[test]
fn interleaved_maintenance_submissions_preserve_both_callers() {
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
    let engineer_a = Address::generate(&env);
    let engineer_b = Address::generate(&env);

    asset_registry.initialize_admin(&admin, &admin);
    asset_registry.add_asset_type(&admin, &symbol_short!("GENSET"));
    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);
    lifecycle.initialize(&admin, &asset_registry_id, &engineer_registry_id, &admin, &0);

    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Concurrent operation test"),
        &String::from_str(&env, "CONCURRENT-001"),
        &owner,
    );
    engineer_registry.register_engineer(
        &engineer_a,
        &BytesN::from_array(&env, &[1u8; 32]),
        &issuer,
        &31_536_000,
        &None,
    );
    engineer_registry.register_engineer(
        &engineer_b,
        &BytesN::from_array(&env, &[2u8; 32]),
        &issuer,
        &31_536_000,
        &None,
    );
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer_a);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer_b);

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &String::from_str(&env, "caller A"),
        &engineer_a,
    );
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("FILTER"),
        &String::from_str(&env, "caller B"),
        &engineer_b,
    );

    let history = lifecycle.get_maintenance_history(&asset_id);
    assert_eq!(history.len(), 2, "interleaved writes must not lose either record");
    assert_eq!(
        lifecycle.get_engineer_maintenance_history(&engineer_a).len(),
        1
    );
    assert_eq!(
        lifecycle.get_engineer_maintenance_history(&engineer_b).len(),
        1
    );
}
