// tests/test_is_collateral_eligible_decay_applied.rs
//
// Issue #994 — is_collateral_eligible applies decay before checking the threshold
//
// Before this fix, is_collateral_eligible read the stored score without
// first applying time-decay.  An asset last serviced months ago could still
// show its original high score and appear eligible even though its real
// decayed score was below the threshold.
//
// Strategy
// --------
// 1. Register asset + engineer, submit enough records so the asset is eligible.
// 2. Configure a fast decay (10 pts / 60 s interval).
// 3. Advance the clock far enough that applying decay drives the stored score
//    to 0 AND age out all records past MAX_AGE so recency weighting also
//    returns 0.
// 4. Call is_collateral_eligible and assert it returns false WITHOUT needing
//    a prior explicit decay_score() call — the function must do it internally.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

const ELIGIBILITY_THRESHOLD: u32 = 50;

/// 31 days in seconds — enough to age out all maintenance records from the
/// recency window (MAX_AGE_LEDGERS ≈ 518 400 ledgers × 5 s = 2 592 000 s ≈
/// 30 days).
const THIRTY_ONE_DAYS_SECS: u64 = 31 * 24 * 60 * 60;

#[test]
fn test_is_collateral_eligible_returns_false_after_decay_without_explicit_decay_call() {
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
        &String::from_str(&env, "Decay eligibility test asset — #994"),
        &String::from_str(&env, "SN-994-001"),
        &owner,
    );

    let credential_hash = BytesN::from_array(&env, &[9u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // Submit 12 ENGINE records to build score ≥ threshold (50).
    // Each ENGINE submission contributes score_increment (default 5).
    // 12 × 5 = 60 ≥ 50.
    for _ in 0..12u32 {
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "pre-decay overhaul"),
            &engineer,
            &None,
        );
        env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    }

    // Sanity check: asset is eligible before decay.
    assert!(
        lifecycle.is_collateral_eligible(&asset_id),
        "asset should be eligible after 12 ENGINE submissions (score ≥ {})",
        ELIGIBILITY_THRESHOLD
    );

    // Configure fast decay: 10 points per 60-second interval.
    // 12 × 5 = 60 stored points; 6 intervals × 10 = 60 decay → score reaches 0
    // after 360 seconds.
    lifecycle.update_decay_config(&admin, &10, &60);
    env.ledger().set_timestamp(env.ledger().timestamp() + 360 + 1);

    // Also advance past MAX_AGE so the recency-weighted score hits 0.
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + THIRTY_ONE_DAYS_SECS);

    // The key assertion: is_collateral_eligible must apply decay internally.
    // No explicit decay_score() call is made here.
    assert!(
        !lifecycle.is_collateral_eligible(&asset_id),
        "is_collateral_eligible must apply decay internally and return false \
         when the decayed score is below the threshold ({})",
        ELIGIBILITY_THRESHOLD
    );
}

#[test]
fn test_is_collateral_eligible_still_true_before_decay_period_elapses() {
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
        &String::from_str(&env, "Decay eligibility test asset B — #994"),
        &String::from_str(&env, "SN-994-002"),
        &owner,
    );

    let credential_hash = BytesN::from_array(&env, &[10u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // Build score well above threshold.
    for _ in 0..20u32 {
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "overhaul"),
            &engineer,
            &None,
        );
        env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    }

    // Advance only 10 seconds — not enough for even one decay interval.
    env.ledger().set_timestamp(env.ledger().timestamp() + 10);

    // Asset must still be eligible.
    assert!(
        lifecycle.is_collateral_eligible(&asset_id),
        "asset should remain eligible when insufficient time has elapsed for decay"
    );
}
