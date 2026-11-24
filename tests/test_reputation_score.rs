//! Tests for #1018: Engineer reputation_score increment on submit_maintenance.
//!
//! Verifies that:
//! - Each accepted `submit_maintenance` call increments the engineer's reputation_score by 1.
//! - The score is capped at 1000 (handled by the engineer-registry).
//! - A `REP_UPD` event is emitted with old and new scores.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Events},
    Address, BytesN, Env, String, Vec,
};

const TIMELOCK_DELAY_SECS: u64 = 48 * 60 * 60;

/// Set up the three-contract ecosystem and return clients plus key addresses.
fn setup(
    env: &Env,
) -> (
    AssetRegistryClient,
    EngineerRegistryClient,
    LifecycleClient,
    Address, // admin
    Address, // issuer
    Address, // owner
    Address, // engineer
) {
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(env, &lifecycle_id);

    let admin = Address::generate(env);
    let issuer = Address::generate(env);
    let owner = Address::generate(env);
    let engineer = Address::generate(env);

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

    // Register engineer with a specialization matching the asset type.
    let credential_hash = BytesN::from_array(env, &[9u8; 32]);
    engineer_registry.register_engineer(
        &engineer,
        &credential_hash,
        &issuer,
        &31_536_000,
        &None,
    );
    engineer_registry.add_specialization(&issuer, &engineer, &symbol_short!("GENSET"));

    (
        asset_registry,
        engineer_registry,
        lifecycle,
        admin,
        issuer,
        owner,
        engineer,
    )
}

/// Register an asset and authorize the engineer for it. Returns the asset_id.
fn register_and_authorize(
    env: &Env,
    asset_registry: &AssetRegistryClient,
    lifecycle: &LifecycleClient,
    owner: &Address,
    engineer: &Address,
) -> u64 {
    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(env, "Test generator"),
        owner,
    );
    lifecycle.authorize_engineer(owner, &asset_id, engineer);
    asset_id
}

#[test]
fn test_reputation_increments_on_submit_maintenance() {
    let env = Env::default();
    env.mock_all_auths();

    let (asset_registry, engineer_registry, lifecycle, _admin, _issuer, owner, engineer) =
        setup(&env);

    let asset_id = register_and_authorize(&env, &asset_registry, &lifecycle, &owner, &engineer);

    // Initial reputation must be 0.
    assert_eq!(
        engineer_registry.get_reputation(&engineer),
        0,
        "reputation must start at 0"
    );

    // First submission: reputation should become 1.
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Oil change"),
        &engineer,
        &None,
    );

    assert_eq!(
        engineer_registry.get_reputation(&engineer),
        1,
        "reputation must be 1 after first submit_maintenance"
    );

    // Second submission: reputation should become 2.
    env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Oil change 2"),
        &engineer,
        &None,
    );

    assert_eq!(
        engineer_registry.get_reputation(&engineer),
        2,
        "reputation must be 2 after second submit_maintenance"
    );
}

#[test]
fn test_reputation_emits_rep_upd_event() {
    let env = Env::default();
    env.mock_all_auths();

    let (asset_registry, _engineer_registry, lifecycle, _admin, _issuer, owner, engineer) =
        setup(&env);

    let asset_id = register_and_authorize(&env, &asset_registry, &lifecycle, &owner, &engineer);

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Oil change"),
        &engineer,
        &None,
    );

    // At least one event with topic "REP_UPD" must have been published.
    let events = env.events().all();
    let rep_upd_topic = symbol_short!("REP_UPD");
    let found = events.iter().any(|e| {
        let topics: Vec<soroban_sdk::Val> = e.0;
        topics
            .iter()
            .any(|t| t == soroban_sdk::Val::from(rep_upd_topic))
    });
    assert!(found, "REP_UPD event must be emitted after submit_maintenance");
}

#[test]
fn test_reputation_capped_at_1000() {
    let env = Env::default();
    env.mock_all_auths();

    let (asset_registry, engineer_registry, lifecycle, _admin, _issuer, owner, engineer) =
        setup(&env);

    let asset_id = register_and_authorize(&env, &asset_registry, &lifecycle, &owner, &engineer);

    // Drive reputation to exactly 1000 via direct update_reputation, then verify
    // one more submit_maintenance leaves it at 1000 (clamped).
    engineer_registry.update_reputation(&engineer, &999);
    assert_eq!(engineer_registry.get_reputation(&engineer), 999);

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Oil change"),
        &engineer,
        &None,
    );

    assert_eq!(
        engineer_registry.get_reputation(&engineer),
        1000,
        "reputation must be exactly 1000 after reaching the cap"
    );

    // One more submission: must stay at 1000.
    env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Oil change again"),
        &engineer,
        &None,
    );

    assert_eq!(
        engineer_registry.get_reputation(&engineer),
        1000,
        "reputation must remain at 1000 after exceeding the cap"
    );
}
