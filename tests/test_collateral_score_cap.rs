use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

/// #766 — Verify that submitting a very large number of maintenance records
/// never pushes the collateral score above the maximum of 100.
#[test]
fn test_collateral_score_does_not_exceed_100_under_high_volume_maintenance() {
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
    let engineer = Address::generate(&env);
    let issuer = Address::generate(&env);

    asset_registry.initialize_admin(&admin, &admin);
    asset_registry.add_asset_type(&admin, &symbol_short!("GENSET"));
    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);
    lifecycle.initialize(&admin, &asset_registry_id, &engineer_registry_id, &admin, &0);

    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "High-volume maintenance test asset"),
        &String::from_str(&env, "SN-HIGHVOL-001"),
        &owner,
    );

    let credential_hash = BytesN::from_array(&env, &[1u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);

    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // Submit 210 ENGINE records — well above the default max_history of 200.
    // After each submission the score must remain within [0, 100].
    for i in 0..210u32 {
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "overhaul"),
            &engineer, &None,
        );
        // Advance timestamp by 1 second so each record gets a unique ledger
        // timestamp and score-history deduplication does not collapse entries.
        env.ledger().set_timestamp(env.ledger().timestamp() + 1);

        let score = lifecycle.get_collateral_score(&asset_id);
        assert!(
            score <= 100,
            "collateral score {} exceeded 100 after {} submissions",
            score,
            i + 1,
        );
    }

    // After saturation, the asset must be collateral-eligible (score >= threshold of 50).
    assert!(
        lifecycle.is_collateral_eligible(&asset_id),
        "asset should be collateral-eligible after 210 maintenance submissions",
    );
}
