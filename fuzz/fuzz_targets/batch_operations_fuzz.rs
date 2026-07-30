//! Fuzz target for batch operations with edge case sizes.
//!
//! Exercises batch_submit_maintenance with batch sizes from 0 to MAX_BATCH_SIZE+1
//! and edge-case record contents (empty strings, maximum-length strings, etc.).
//!
//! # Usage
//!
//! ```bash
//! cd fuzz && cargo fuzz run batch_operations_fuzz -- -max_total_time=7200
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use arbitrary::Arbitrary;

/// Structured fuzz input for batch operations.
#[derive(Arbitrary, Debug)]
struct BatchFuzzInput {
    /// Number of batch records (0..100, clamped to 0-55 for safety).
    batch_count: u8,
    /// Whether records should have empty notes.
    empty_notes: bool,
    /// Whether records should use max-length notes.
    max_notes: bool,
    /// Raw bytes for notes content.
    notes_seed: Vec<u8>,
}

fuzz_target!(|input: BatchFuzzInput| {
    use soroban_sdk::{
        symbol_short,
        testutils::{Address as _, Ledger},
        Address, BytesN, Env, String,
    };
    use asset_registry::{AssetRegistry, AssetRegistryClient};
    use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
    use lifecycle::{Lifecycle, LifecycleClient, MAX_BATCH_SIZE};

    let env = Env::default();
    env.mock_all_auths();

    // Bootstrap the full contract stack
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
        &String::from_str(&env, "Batch fuzz asset"),
        &String::from_str(&env, "SN-BATCH-FUZZ"),
        &owner,
    );

    let credential_hash = BytesN::from_array(&env, &[1u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // Clamp batch count to a reasonable range for fuzzing
    let count = (input.batch_count % 55).min(MAX_BATCH_SIZE as u8 + 5) as u32;

    // Build batch records
    let notes_content = if input.empty_notes {
        String::from_str(&env, "")
    } else if input.max_notes {
        let s: String = (0..300).map(|_| 'x').collect();
        String::from_str(&env, &s)
    } else {
        String::from_str(&env, "fuzz batch note")
    };

    let mut batch: soroban_sdk::Vec<lifecycle::BatchRecord> = soroban_sdk::Vec::new(&env);
    for i in 0..count {
        batch.push_back(lifecycle::BatchRecord {
            task_type: symbol_short!("FUZZ"),
            notes: notes_content.clone(),
        });
        // Bump timestamp to avoid dedup
        env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    }

    // Call batch_submit_maintenance — must never panic
    let _ = lifecycle.try_batch_submit_maintenance(&asset_id, &batch, &engineer);

    // Verify state integrity after batch operation
    let history = lifecycle.get_maintenance_history(&asset_id);
    let score = lifecycle.get_collateral_score(&asset_id);
    assert!(score <= 100, "Score must never exceed 100 after fuzz input");
});
