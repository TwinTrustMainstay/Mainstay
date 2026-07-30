// tests/test_health_snapshot_accuracy.rs
//
// Issue #1040 — Add Test: take_health_snapshot stores correct
// maintenance_count and last_service_date
//
// Verifies that take_health_snapshot stores accurate values for
// maintenance_count, last_service_date, and score that match the current
// state of the asset.
//
// Tasks:
//   1. Submit 3 maintenance records.
//   2. Call take_health_snapshot.
//   3. Assert snapshot maintenance_count == 3, last_service_date matches the
//      last submission, score matches current score.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{symbol_short, testutils::Address as _, testutils::Ledger, Address, BytesN, Env, String};

#[test]
fn test_take_health_snapshot_stores_correct_maintenance_count_and_last_service_date() {
    let env = Env::default();
    env.mock_all_auths();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    let asset_admin = Address::generate(&env);
    let eng_admin = Address::generate(&env);
    let lc_admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));

    engineer_registry.initialize_admin(&eng_admin, &eng_admin);
    engineer_registry.add_trusted_issuer(&eng_admin, &issuer);

    lifecycle.initialize(
        &lc_admin,
        &asset_registry_id,
        &engineer_registry_id,
        &lc_admin,
        &0,
    );

    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Generator for snapshot test"),
        &String::from_str(&env, "SN-1040-001"),
        &owner,
    );

    let credential_hash = BytesN::from_array(&env, &[0x40u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);

    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // 1. Submit 3 maintenance records, each at a distinct timestamp so the
    //    "last submission" is unambiguous.
    let task_types = [
        symbol_short!("OIL_CHG"),
        symbol_short!("FILTER"),
        symbol_short!("INSPECT"),
    ];
    let mut last_submission_timestamp = 0u64;
    for (i, task_type) in task_types.iter().enumerate() {
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 1_000 * (i as u64 + 1));
        lifecycle.submit_maintenance(
            &asset_id,
            task_type,
            &String::from_str(&env, "scheduled maintenance"),
            &engineer,
        );
        last_submission_timestamp = env.ledger().timestamp();
    }

    let history = lifecycle.get_maintenance_history(&asset_id);
    assert_eq!(history.len(), 3, "exactly 3 maintenance records must be recorded");

    // 2. Call take_health_snapshot.
    let current_score = lifecycle.get_collateral_score(&asset_id);
    let snapshot = lifecycle.take_health_snapshot(&asset_id);

    // 3. Assert the snapshot accurately reflects the asset's current state.
    assert_eq!(snapshot.maintenance_count, 3, "snapshot must count all 3 submitted records");
    assert_eq!(
        snapshot.last_service_date, last_submission_timestamp,
        "snapshot's last_service_date must match the timestamp of the last submission"
    );
    assert_eq!(
        snapshot.score, current_score,
        "snapshot's score must match the asset's current collateral score"
    );
}
