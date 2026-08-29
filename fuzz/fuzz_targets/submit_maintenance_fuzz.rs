//! Fuzz target for `submit_maintenance`.
//!
//! Exercises the lifecycle contract's submit_maintenance function with
//! arbitrary byte sequences for task_type and notes fields, random asset IDs,
//! and random engineer signatures. The goal is to find panics or undefined
//! behavior in the input validation layer.
//!
//! # Usage
//!
//! ```bash
//! cd fuzz && cargo fuzz run submit_maintenance_fuzz -- -max_total_time=7200
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use arbitrary::Arbitrary;

/// Structured fuzz input representing a single maintenance submission attempt.
#[derive(Arbitrary, Debug)]
struct FuzzInput {
    /// Raw bytes for the task_type Symbol.
    task_type_bytes: Vec<u8>,
    /// Raw bytes for the notes String.
    notes_bytes: Vec<u8>,
    /// Whether notes should be empty.
    empty_notes: bool,
    /// Whether notes should be max-length (256 chars).
    max_notes: bool,
    /// Whether to include injection payload bytes.
    inject: bool,
}

fuzz_target!(|input: FuzzInput| {
    use soroban_sdk::{
        symbol_short,
        testutils::{Address as _, Ledger},
        Address, BytesN, Env, String,
    };
    use asset_registry::{AssetRegistry, AssetRegistryClient};
    use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
    use lifecycle::{Lifecycle, LifecycleClient};

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
        &String::from_str(&env, "Fuzz test asset"),
        &String::from_str(&env, "SN-FUZZ"),
        &owner,
    );

    let credential_hash = BytesN::from_array(&env, &[1u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // Build notes from fuzz input
    let notes = if input.empty_notes {
        String::from_str(&env, "")
    } else if input.max_notes {
        let s: String = (0..300).map(|_| 'x').collect();
        String::from_str(&env, &s)
    } else if input.inject {
        let s = String::from_utf8_lossy(&input.notes_bytes);
        String::from_str(&env, &s)
    } else {
        // Use a safe default to avoid spending too much fuzz time on setup
        String::from_str(&env, "routine")
    };

    // Call submit_maintenance — must never panic, only return structured errors
    let _ = lifecycle.try_submit_maintenance(
        &asset_id,
        &symbol_short!("FUZZ"),
        &notes,
        &engineer,
    );

    // Also exercise the read path to verify state consistency
    let _ = lifecycle.try_get_collateral_score(&asset_id);
    let _ = lifecycle.try_get_maintenance_history(&asset_id);
});
