use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

/// #768 — Verify that decay_score returns exactly 0 after a very long time has
/// elapsed and does not underflow or wrap to a negative value.
#[test]
fn test_decay_score_clamps_at_zero_after_long_elapsed_time() {
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
        &String::from_str(&env, "Decay clamp test asset"),
        &String::from_str(&env, "SN-DECAY-001"),
        &owner,
    );

    let credential_hash = BytesN::from_array(&env, &[2u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);

    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // Submit several ENGINE records to build a non-zero stored score.
    for _ in 0..5u32 {
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "overhaul"),
            &engineer, &None,
        );
        env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    }

    // Advance the ledger by 10 years (315,360,000 seconds).
    // This is far beyond the 30-day decay interval and the 30-day recency window,
    // so both the time-based decay path and the recency-weighted path must return 0.
    let ten_years_seconds: u64 = 315_360_000;
    env.ledger().set_timestamp(env.ledger().timestamp() + ten_years_seconds);

    // decay_score uses saturating subtraction — it must clamp to 0, not underflow.
    let decayed = lifecycle.decay_score(&asset_id);
    assert_eq!(
        decayed, 0,
        "expected decay_score to return 0 after 10 years of elapsed time, got {}",
        decayed,
    );

    // is_collateral_eligible uses recency-weighted compute_decay.
    // All records are older than MAX_AGE_LEDGERS (30 days), so the raw score is 0.
    // The MIN_SCORE_WITH_HISTORY floor raises it to 1, which is below the
    // eligibility threshold of 50, so the asset must not be eligible.
    assert!(
        !lifecycle.is_collateral_eligible(&asset_id),
        "asset with only decade-old maintenance records should not be collateral-eligible",
    );
}
