// tests/test_collateral_eligible_after_decay.rs
//
// Issue #1035 — Add Test: is_collateral_eligible returns false after score decays below threshold
//
// Verifies that an asset that was previously collateral-eligible becomes
// ineligible once enough ledger time passes for its recency-weighted score
// to fall below the eligibility threshold (default: 50).
//
// Strategy
// --------
// 1. Register asset + engineer, submit enough maintenance records so that
//    `is_collateral_eligible` returns `true` (score ≥ threshold = 50).
// 2. Set a fast decay configuration so we can advance a small ledger delta
//    and reliably drive the stored score below 50.
// 3. Advance the ledger timestamp well past the recency window so that the
//    recency-weighted `compute_decay` score also drops below 50.
// 4. Assert `is_collateral_eligible` returns `false`.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

/// Mirrors `DEFAULT_ELIGIBILITY_THRESHOLD` in `contracts/lifecycle/src/lib.rs`.
const ELIGIBILITY_THRESHOLD: u32 = 50;

/// MAX_AGE_LEDGERS ≈ 518_400 ledgers ≈ 30 days (1 ledger ≈ 5 s).
/// Advancing past this threshold zeroes the recency-weighted contribution of
/// every maintenance record, making the compute_decay score 0.
const MAX_AGE_SECONDS: u64 = 518_400 * 5; // 2_592_000 s ≈ 30 days

#[test]
fn test_is_collateral_eligible_returns_false_after_decay_below_threshold() {
    let env = Env::default();
    env.mock_all_auths();

    // ── Deploy contracts ──────────────────────────────────────────────────────
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    // ── Addresses ────────────────────────────────────────────────────────────
    let asset_admin = Address::generate(&env);
    let eng_admin = Address::generate(&env);
    let lc_admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    // ── Initialise registries ─────────────────────────────────────────────────
    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));

    engineer_registry.initialize_admin(&eng_admin, &eng_admin);
    engineer_registry.add_trusted_issuer(&eng_admin, &issuer);

    // max_history = 0 means unlimited
    lifecycle.initialize(
        &lc_admin,
        &asset_registry_id,
        &engineer_registry_id,
        &lc_admin,
        &0,
    );

    // ── Register asset ────────────────────────────────────────────────────────
    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Industrial generator — decay eligibility test"),
        &String::from_str(&env, "SN-DECAY-ELIG-001"),
        &owner,
    );

    // ── Register engineer with a 1-year credential ───────────────────────────
    let credential_hash = BytesN::from_array(&env, &[7u8; 32]);
    engineer_registry.register_engineer(
        &engineer,
        &credential_hash,
        &issuer,
        &31_536_000,
        &None,
    );

    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // ── Step 1: Build a score above the eligibility threshold ─────────────────
    // Each ENGINE submission adds score_increment (default 5) to the stored
    // score.  12 submissions × 5 = 60 ≥ threshold (50).
    for i in 0..12u32 {
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "Pre-decay overhaul"),
            &engineer,
            &None,
        );
        // Advance timestamp by 1 second per submission to prevent deduplication.
        env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    }

    // Confirm the asset is currently eligible.
    assert!(
        lifecycle.is_collateral_eligible(&asset_id),
        "asset should be collateral-eligible after {} ENGINE submissions (score ≥ {})",
        12,
        ELIGIBILITY_THRESHOLD,
    );

    // ── Step 2: Configure fast decay so we can drive the stored score to 0 ────
    // rate = 10 points/interval, interval = 60 s → 6 intervals over 360 s
    // removes 60 points from the stored score (60 – 60 = 0).
    lifecycle.update_decay_config(&lc_admin, &10, &60);

    // Advance just enough for the stored-score path to zero out.
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 360 + 1);
    lifecycle.decay_score(&asset_id);

    // ── Step 3: Advance past MAX_AGE so recency-weighted score also drops ─────
    // compute_decay gives each record zero weight once age ≥ MAX_AGE_LEDGERS.
    // Advancing 31 days ensures all 12 records are fully aged out.
    let thirty_one_days: u64 = 31 * 24 * 60 * 60; // 2_678_400 s
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + thirty_one_days);

    // ── Step 4: Assert ineligibility ─────────────────────────────────────────
    assert!(
        !lifecycle.is_collateral_eligible(&asset_id),
        "asset should NOT be collateral-eligible after score has decayed below threshold ({})",
        ELIGIBILITY_THRESHOLD,
    );
}
