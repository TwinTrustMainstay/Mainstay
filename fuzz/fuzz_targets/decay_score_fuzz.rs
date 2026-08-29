//! Fuzz target for the score decay computation (`decay_score` / `apply_decay`).
//!
//! `apply_decay` computes `total_decay = decay_intervals * decay_rate`, where
//! `decay_intervals = elapsed_time / decay_interval`. Because `decay_rate` and
//! `decay_interval` are admin-configurable, and `decay_intervals` grows without
//! bound as elapsed time grows, this multiplication can overflow `u32` for
//! extreme configuration values (e.g. `decay_rate` near `u32::MAX` combined
//! with a very small `decay_interval`). This target randomises `decay_rate`,
//! `decay_interval`, and the amount of elapsed time (a proxy for driving
//! `current_score` through many decay steps) to search for panics or
//! out-of-range results.
//!
//! # Usage
//!
//! ```bash
//! cd fuzz && cargo fuzz run decay_score_fuzz -- -max_total_time=7200
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use arbitrary::Arbitrary;

/// The contract's collateral score is clamped to this range; kept in sync
/// with `lifecycle::MAX_COLLATERAL_SCORE` (private to the lifecycle crate).
const MAX_COLLATERAL_SCORE: u32 = 100;

/// Structured fuzz input for the decay computation.
#[derive(Arbitrary, Debug)]
struct DecayFuzzInput {
    /// Raw decay_rate; remapped away from 0 (rejected by update_decay_config).
    decay_rate_seed: u32,
    /// Raw decay_interval; remapped away from 0 (rejected by update_decay_config).
    decay_interval_seed: u64,
    /// Elapsed seconds before decay_score is invoked, driving decay_intervals.
    elapsed_seconds: u64,
    /// Number of maintenance records to submit before decay is applied.
    record_count: u8,
}

fuzz_target!(|input: DecayFuzzInput| {
    use soroban_sdk::{
        symbol_short,
        testutils::{Address as _, Ledger},
        Address, BytesN, Env, String,
    };
    use asset_registry::{AssetRegistry, AssetRegistryClient};
    use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
    use lifecycle::{Lifecycle, LifecycleClient};

    // decay_rate == 0 and decay_interval == 0 are rejected by
    // update_decay_config (InvalidConfig), so remap into valid ranges while
    // still covering the boundary (1) and extreme (u32::MAX / u64::MAX) ends.
    let decay_rate = if input.decay_rate_seed == 0 {
        1
    } else {
        input.decay_rate_seed
    };
    let decay_interval: u64 = if input.decay_interval_seed == 0 {
        1
    } else {
        input.decay_interval_seed
    };

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
    asset_registry.add_asset_type(&admin, &symbol_short!("FUZZ"));
    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);
    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &0,
    );

    let asset_id = asset_registry.register_asset(
        &symbol_short!("FUZZ"),
        &String::from_str(&env, "Fuzz decay asset"),
        &String::from_str(&env, "SN-DECAY"),
        &owner,
    );

    let credential_hash = BytesN::from_array(&env, &[2u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // Apply the randomised decay configuration before any scoring happens,
    // so submit_maintenance and decay_score both use it.
    let _ = lifecycle.try_update_decay_config(&admin, &decay_rate, &decay_interval);

    // Submit a bounded number of maintenance records to build up history and
    // a non-zero starting score (capped to keep the fuzz loop bounded).
    let records = (input.record_count % 8) as u32;
    for _ in 0..records {
        let _ = lifecycle.try_submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &String::from_str(&env, "routine"),
            &engineer,
        );
    }

    // Advance ledger time by the fuzzed elapsed duration to drive
    // decay_intervals arbitrarily high, then apply decay.
    env.ledger()
        .with_mut(|li| li.timestamp = li.timestamp.saturating_add(input.elapsed_seconds));

    let result = lifecycle.try_get_collateral_score(&asset_id);
    if let Ok(Ok(score)) = result {
        assert!(
            score <= MAX_COLLATERAL_SCORE,
            "decay result {} exceeded MAX_COLLATERAL_SCORE with decay_rate={}, decay_interval={}, elapsed={}",
            score,
            decay_rate,
            decay_interval,
            input.elapsed_seconds
        );
        // score is a u32, so it is >= 0 by construction; the assertion above
        // is the meaningful half of the [0, MAX_COLLATERAL_SCORE] invariant.
    }

    let _ = lifecycle.try_decay_score(&asset_id);
});
