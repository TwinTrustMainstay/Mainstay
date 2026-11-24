//! Tests for #1015: Asset deprecation warning event and deprecated_at timestamp.
//!
//! Verifies that:
//! - `deprecated_at` is set to the deprecation timestamp when `deprecate_asset` is called.
//! - `deprecated_at` is `None` on freshly registered assets.
//! - `DEPR_WARN` event is emitted when the asset has an active lien (is_locked == true).
//! - No `DEPR_WARN` event is emitted when the asset has no lien.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Events, Ledger},
    Address, Env, String, Val, Vec,
};

fn setup(env: &Env) -> (AssetRegistryClient, Address, Address) {
    let registry_id = env.register(AssetRegistry, ());
    let client = AssetRegistryClient::new(env, &registry_id);
    let admin = Address::generate(env);
    let owner = Address::generate(env);
    client.initialize_admin(&admin, &admin);
    client.add_asset_type(&admin, &symbol_short!("GENSET"));
    (client, admin, owner)
}

fn register_asset(env: &Env, client: &AssetRegistryClient, owner: &Address) -> u64 {
    client.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(env, "Test generator"),
        owner,
    )
}

#[test]
fn test_deprecated_at_is_none_on_fresh_asset() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, _admin, owner) = setup(&env);
    let asset_id = register_asset(&env, &client, &owner);

    let asset = client.get_asset(&asset_id);
    assert!(
        asset.deprecated_at.is_none(),
        "deprecated_at must be None on a freshly registered asset"
    );
}

#[test]
fn test_deprecated_at_set_on_deprecation() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, _admin, owner) = setup(&env);
    let asset_id = register_asset(&env, &client, &owner);

    let ts_before = env.ledger().timestamp();
    env.ledger().set_timestamp(ts_before + 100);
    let expected_ts = env.ledger().timestamp();

    client.deprecate_asset(
        &owner,
        &asset_id,
        &String::from_str(&env, "End of service life"),
    );

    let asset = client.get_asset(&asset_id);
    assert_eq!(
        asset.deprecated_at,
        Some(expected_ts),
        "deprecated_at must be set to the ledger timestamp at deprecation"
    );
}

#[test]
fn test_depr_warn_emitted_when_lien_exists() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, _admin, owner) = setup(&env);
    let asset_id = register_asset(&env, &client, &owner);

    // Place a lien on the asset so is_locked == true.
    let lending_contract = Address::generate(&env);
    let loan_id: u64 = 42;
    client.lock_asset_as_collateral(&lending_contract, &asset_id, &loan_id);

    // Verify lien is active.
    let asset = client.get_asset(&asset_id);
    assert!(asset.is_locked, "asset must be locked after lock_asset_as_collateral");

    client.deprecate_asset(
        &owner,
        &asset_id,
        &String::from_str(&env, "Retiring under lien"),
    );

    // A DEPR_WARN event must have been emitted.
    let events = env.events().all();
    let depr_warn_topic = symbol_short!("DEPR_WARN");
    let found = events.iter().any(|e| {
        let topics: Vec<Val> = e.0;
        topics
            .iter()
            .any(|t| t == Val::from(depr_warn_topic))
    });
    assert!(
        found,
        "DEPR_WARN event must be emitted when deprecating an asset with an active lien"
    );
}

#[test]
fn test_depr_warn_not_emitted_without_lien() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, _admin, owner) = setup(&env);
    let asset_id = register_asset(&env, &client, &owner);

    // No lien placed — is_locked must be false.
    let asset = client.get_asset(&asset_id);
    assert!(!asset.is_locked, "asset must not be locked without a lien");

    client.deprecate_asset(
        &owner,
        &asset_id,
        &String::from_str(&env, "Normal retirement"),
    );

    // DEPR_WARN must NOT have been emitted.
    let events = env.events().all();
    let depr_warn_topic = symbol_short!("DEPR_WARN");
    let found = events.iter().any(|e| {
        let topics: Vec<Val> = e.0;
        topics
            .iter()
            .any(|t| t == Val::from(depr_warn_topic))
    });
    assert!(
        !found,
        "DEPR_WARN must NOT be emitted when deprecating an asset without a lien"
    );
}
