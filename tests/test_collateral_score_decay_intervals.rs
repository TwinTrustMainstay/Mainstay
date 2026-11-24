use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

// ─── shared test helpers ───────────────────────────────────────────────────

/// Fully boot all three contracts and return their clients together with an
/// `admin` address that holds the lifecycle admin role.
fn setup_contracts(
    env: &Env,
) -> (
    AssetRegistryClient,
    EngineerRegistryClient,
    LifecycleClient,
    Address,
) {
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(env, &lifecycle_id);

    let admin = Address::generate(env);
    let issuer = Address::generate(env);

    asset_registry.initialize_admin(&admin, &admin);
    asset_registry.add_asset_type(&admin, &symbol_short!("GENSET"));
    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);
    // max_history = 0  →  use the contract default (200)
    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &0,
    );

    (asset_registry, engineer_registry, lifecycle, admin)
}

/// Register a fresh asset owned by a newly-generated address.
/// Returns `(asset_id, owner)`.
fn register_asset(
    env: &Env,
    asset_registry: &AssetRegistryClient,
    label: &str,
    serial: &str,
) -> (u64, Address) {
    let owner = Address::generate(env);
    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(env, label),
        &String::from_str(env, serial),
        &owner,
    );
    (asset_id, owner)
}

/// Register a new engineer with a unique credential hash, add a trusted issuer
/// to the registry (using `mock_all_auths`), authorize the engineer for
/// `asset_id`, and return the engineer address.
fn register_and_authorize_engineer(
    env: &Env,
    engineer_registry: &EngineerRegistryClient,
    lifecycle: &LifecycleClient,
    asset_id: u64,
    owner: &Address,
    hash_seed: u8,
) -> Address {
    let engineer = Address::generate(env);
    let issuer = Address::generate(env);
    let credential_hash = BytesN::from_array(env, &[hash_seed; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);
    lifecycle.authorize_engineer(owner, &asset_id, &engineer);
    engineer
}

// ─── tests ─────────────────────────────────────────────────────────────────

/// #1034 — Task 1
/// Score decrements by `decay_rate` for every elapsed `decay_interval` across
/// three consecutive intervals.
///
/// Setup:
///   • Custom decay: rate = 3 pts, interval = 60 s
///   • Submit enough maintenance to reach an initial stored score > 3 × 3 = 9
///   • Advance ledger by 3 × 60 = 180 s
///   • Call `decay_score` and assert score = initial − 9
#[test]
fn test_score_decrements_by_decay_rate_over_three_intervals() {
    let env = Env::default();
    env.mock_all_auths();

    let (asset_registry, engineer_registry, lifecycle, admin) = setup_contracts(&env);
    let (asset_id, owner) = register_asset(&env, &asset_registry, "Decay-3-interval asset", "SN-D3-001");
    let engineer = register_and_authorize_engineer(
        &env, &engineer_registry, &lifecycle, asset_id, &owner, 10,
    );

    // Submit 6 ENGINE records to build a stored score.
    // With a brand-new engineer (reputation = 0) the effective increment per
    // submission is floor(score_increment × 500 / 1000) = floor(5 × 0.5) = 2.
    // 6 submissions → 12 pts stored — comfortably above the 9-pt decay target.
    for _ in 0..6u32 {
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "overhaul"),
            &engineer,
            &None,
        );
        // Advance 1 s between submissions so timestamps are unique.
        env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    }

    let initial_score = lifecycle.get_collateral_score(&asset_id);
    assert!(
        initial_score > 9,
        "pre-condition: initial score {} must be > 9 to have room for 3-interval decay",
        initial_score,
    );

    // Configure deterministic decay: 3 pts per 60-second interval.
    let decay_rate: u32 = 3;
    let decay_interval: u64 = 60;
    lifecycle.update_decay_config(&admin, &decay_rate, &decay_interval);

    // Advance exactly 3 full intervals (180 s).
    env.ledger().set_timestamp(env.ledger().timestamp() + 3 * decay_interval);

    let decayed_score = lifecycle.decay_score(&asset_id);
    let expected = initial_score.saturating_sub(3 * decay_rate);

    assert_eq!(
        decayed_score,
        expected,
        "after 3 intervals (rate={}, interval={}s): expected score {} but got {}",
        decay_rate,
        decay_interval,
        expected,
        decayed_score,
    );
}

/// #1034 — Task 2
/// Score is clamped at `MIN_SCORE_WITH_HISTORY` (= 1) for an asset that *has*
/// maintenance records, even after enough time has elapsed to drive the raw
/// computed value below 1.
///
/// The floor ensures a maintained asset is always distinguishable from one
/// with zero maintenance history.
#[test]
fn test_score_clamped_at_min_score_with_history_for_maintained_asset() {
    let env = Env::default();
    env.mock_all_auths();

    let (asset_registry, engineer_registry, lifecycle, admin) = setup_contracts(&env);
    let (asset_id, owner) = register_asset(&env, &asset_registry, "Floor-clamp asset", "SN-FL-001");
    let engineer = register_and_authorize_engineer(
        &env, &engineer_registry, &lifecycle, asset_id, &owner, 20,
    );

    // One ENGINE submission to ensure the asset has maintenance history.
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("ENGINE"),
        &String::from_str(&env, "minor service"),
        &engineer,
        &None,
    );

    // Set aggressive decay: 50 pts per 10-second interval.
    // Any existing stored score will be fully wiped within the first interval.
    lifecycle.update_decay_config(&admin, &50, &10);

    // Advance far beyond one interval so raw_score saturates to 0.
    env.ledger().set_timestamp(env.ledger().timestamp() + 10_000);

    // decay_score must return MIN_SCORE_WITH_HISTORY (1), not 0, because the
    // asset has at least one maintenance record.
    let floor_score = lifecycle.decay_score(&asset_id);
    assert_eq!(
        floor_score,
        1,
        "asset with maintenance history must be clamped at MIN_SCORE_WITH_HISTORY=1, got {}",
        floor_score,
    );

    // A score of 1 is below the eligibility threshold (50) — not eligible.
    assert!(
        !lifecycle.is_collateral_eligible(&asset_id),
        "asset with floor score of 1 must not be collateral-eligible",
    );
}

/// #1034 — Task 3
/// An asset with *no* maintenance records scores exactly 0 from `decay_score`.
///
/// The MIN_SCORE_WITH_HISTORY floor must NOT apply when the asset has no
/// history — the result must be 0, not 1.
#[test]
fn test_score_is_zero_for_asset_with_no_maintenance_history() {
    let env = Env::default();
    env.mock_all_auths();

    let (asset_registry, _engineer_registry, lifecycle, admin) = setup_contracts(&env);

    // Register asset but deliberately submit no maintenance records.
    let (asset_id, _owner) = register_asset(&env, &asset_registry, "No-history asset", "SN-NH-001");

    // Use non-default decay params to confirm no code path special-cases them.
    lifecycle.update_decay_config(&admin, &5, &60);

    // Advance time; there is nothing to decay and no history floor applies.
    env.ledger().set_timestamp(env.ledger().timestamp() + 600);

    let score = lifecycle.decay_score(&asset_id);
    assert_eq!(
        score,
        0,
        "asset with no maintenance history must score 0, got {}",
        score,
    );

    assert!(
        !lifecycle.is_collateral_eligible(&asset_id),
        "asset with no maintenance history must not be collateral-eligible",
    );
}

/// #1034 — Task 4
/// Score decrements by exactly `decay_rate` with each successive
/// single-interval advance until the `MIN_SCORE_WITH_HISTORY` floor is hit.
///
/// This verifies the step-by-step precision of the decay logic: one
/// `decay_score` call per interval must shed exactly `decay_rate` points
/// every time until the floor clamps the result at 1.
#[test]
fn test_score_decrements_step_by_step_each_interval() {
    let env = Env::default();
    env.mock_all_auths();

    let (asset_registry, engineer_registry, lifecycle, admin) = setup_contracts(&env);
    let (asset_id, owner) = register_asset(&env, &asset_registry, "Step-decay asset", "SN-SD-001");
    let engineer = register_and_authorize_engineer(
        &env, &engineer_registry, &lifecycle, asset_id, &owner, 30,
    );

    // Build a score using 6 ENGINE submissions (≈ 12 pts stored at reputation 0).
    for _ in 0..6u32 {
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "overhaul"),
            &engineer,
            &None,
        );
        env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    }

    let initial_score = lifecycle.get_collateral_score(&asset_id);

    // Small, easy-to-reason-about decay config.
    let decay_rate: u32 = 2;
    let decay_interval: u64 = 30;
    lifecycle.update_decay_config(&admin, &decay_rate, &decay_interval);

    // Walk through intervals one at a time, verifying the score drops by
    // exactly `decay_rate` each step until the floor is reached.
    let mut expected_score = initial_score;
    let mut steps: u32 = 0;

    loop {
        env.ledger().set_timestamp(env.ledger().timestamp() + decay_interval);

        let new_score = lifecycle.decay_score(&asset_id);

        if expected_score <= decay_rate {
            // At or below the floor — the contract must clamp to 1.
            assert_eq!(
                new_score,
                1,
                "step {}: expected MIN_SCORE_WITH_HISTORY=1 at floor, got {}",
                steps,
                new_score,
            );
            break;
        }

        expected_score = expected_score.saturating_sub(decay_rate);
        assert_eq!(
            new_score,
            expected_score,
            "step {}: expected score {} after one interval of decay but got {}",
            steps,
            expected_score,
            new_score,
        );

        steps += 1;
        // Safety valve: a score starting at most 100, decaying by 2 each step,
        // reaches the floor in at most 50 steps.
        assert!(
            steps <= 100,
            "decay did not converge to floor within 100 steps \
             (initial_score={}, decay_rate={})",
            initial_score,
            decay_rate,
        );
    }

    // After the loop the final *stored* score must still be 1.
    let final_score = lifecycle.get_collateral_score(&asset_id);
    assert_eq!(
        final_score,
        1,
        "final stored score must be MIN_SCORE_WITH_HISTORY=1, got {}",
        final_score,
    );
}
