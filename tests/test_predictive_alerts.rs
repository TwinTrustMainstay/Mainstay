//! Tests for predictive maintenance alerts.
//!
//! Validates:
//! - `calculate_predicted_next_service` with varying record counts
//! - Moving average interval prediction accuracy
//! - `get_maintenance_alerts` overdue detection
//! - Insufficient data edge cases

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

/// Error code for `InsufficientPredictionData` in lifecycle contract.
const ERR_INSUFFICIENT_DATA: u32 = 24;

fn setup(env: &Env) -> (
    LifecycleClient,
    AssetRegistryClient,
    EngineerRegistryClient,
    Address, // admin
    Address, // owner
    u64,     // asset_id
    Address, // engineer
) {
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(env, &lifecycle_id);

    let admin = Address::generate(env);
    let owner = Address::generate(env);
    let issuer = Address::generate(env);
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

    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(env, "Predictive test asset"),
        &String::from_str(env, "SN-PRED-001"),
        &owner,
    );

    let credential_hash = BytesN::from_array(env, &[1u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    (lifecycle, asset_registry, engineer_registry, admin, owner, asset_id, engineer)
}

#[test]
fn test_predict_insufficient_data() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _asset_registry, _engineer_registry, _admin, _owner, asset_id, engineer) = setup(&env);

    // Submit only 1 record — need at least 2 for interval prediction
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "oil change"),
        &engineer,
    );

    let result = lifecycle.try_calculate_predicted_next_service(
        &asset_id,
        &symbol_short!("OIL_CHG"),
    );

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            ERR_INSUFFICIENT_DATA,
        ))),
        "Single record must return InsufficientPredictionData"
    );
}

#[test]
fn test_predict_with_two_records() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _asset_registry, _engineer_registry, _admin, _owner, asset_id, engineer) = setup(&env);

    // Submit first record at T=1000
    env.ledger().set_timestamp(1000);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "first oil change"),
        &engineer,
    );

    // Submit second record at T=1000 + 7 days (604800 seconds)
    env.ledger().set_timestamp(1000 + 604_800);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "second oil change"),
        &engineer,
    );

    // Prediction should be: last_ts + avg_interval = (1000 + 604800) + 604800
    // = 1000 + 1_209_600
    let predicted = lifecycle.calculate_predicted_next_service(
        &asset_id,
        &symbol_short!("OIL_CHG"),
    );

    // With 7-day interval, prediction should be ~7 days from last service
    let expected = 1000 + 604_800 + 604_800;
    assert_eq!(predicted, expected, "Prediction must match simple moving average");
}

#[test]
fn test_predict_with_varying_intervals() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _asset_registry, _engineer_registry, _admin, _owner, asset_id, engineer) = setup(&env);

    // Intervals: 3 days, 5 days, 4 days → avg = 4 days (345600 seconds)
    let intervals: [u64; 3] = [
        3 * 24 * 60 * 60,  // 3 days
        5 * 24 * 60 * 60,  // 5 days
        4 * 24 * 60 * 60,  // 4 days
    ];
    let mut ts: u64 = 1000;

    for interval in &intervals {
        ts += interval;
        env.ledger().set_timestamp(ts);
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("FILTER"),
            &String::from_str(&env, &format!("filter change at {}", ts)),
            &engineer,
        );
    }

    let predicted = lifecycle.calculate_predicted_next_service(
        &asset_id,
        &symbol_short!("FILTER"),
    );

    // avg = (3 + 5 + 4) / 3 = 4 days
    let avg_interval = (intervals[0] + intervals[1] + intervals[2]) / 3;
    let expected = ts + avg_interval;
    assert_eq!(predicted, expected, "Moving average must handle varying intervals");
}

#[test]
fn test_predict_mixed_task_types() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _asset_registry, _engineer_registry, _admin, _owner, asset_id, engineer) = setup(&env);

    // Submit OIL_CHG at T=1000 and T=1000+7d
    env.ledger().set_timestamp(1000);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "oil change 1"),
        &engineer,
    );

    env.ledger().set_timestamp(1000 + 7 * 24 * 60 * 60);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "oil change 2"),
        &engineer,
    );

    // Submit INSPECT at T=2000 and T=2000+14d (different task)
    env.ledger().set_timestamp(2000);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &String::from_str(&env, "inspection 1"),
        &engineer,
    );

    env.ledger().set_timestamp(2000 + 14 * 24 * 60 * 60);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &String::from_str(&env, "inspection 2"),
        &engineer,
    );

    // OIL_CHG prediction must only use OIL_CHG records
    let oil_pred = lifecycle.calculate_predicted_next_service(
        &asset_id,
        &symbol_short!("OIL_CHG"),
    );
    let oil_expected = (1000 + 7 * 24 * 60 * 60) + 7 * 24 * 60 * 60;
    assert_eq!(oil_pred, oil_expected, "OIL_CHG prediction must ignore INSPECT records");

    // INSPECT prediction must only use INSPECT records
    let insp_pred = lifecycle.calculate_predicted_next_service(
        &asset_id,
        &symbol_short!("INSPECT"),
    );
    let insp_expected = (2000 + 14 * 24 * 60 * 60) + 14 * 24 * 60 * 60;
    assert_eq!(insp_pred, insp_expected, "INSPECT prediction must ignore OIL_CHG records");
}

#[test]
fn test_alerts_empty_when_no_data() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _asset_registry, _engineer_registry, _admin, _owner, asset_id, _engineer) = setup(&env);

    let alerts = lifecycle.get_maintenance_alerts(&asset_id);
    assert!(alerts.is_empty(), "No alerts for asset with no maintenance records");
}

#[test]
fn test_alerts_insufficient_data_no_panic() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _asset_registry, _engineer_registry, _admin, _owner, asset_id, engineer) = setup(&env);

    // Single record — insufficient for prediction
    env.ledger().set_timestamp(1000);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "single oil change"),
        &engineer,
    );

    // get_maintenance_alerts must not panic; it should return empty
    let alerts = lifecycle.get_maintenance_alerts(&asset_id);
    assert!(
        alerts.is_empty(),
        "Alerts must be empty when insufficient data for prediction"
    );
}

#[test]
fn test_alerts_when_prediction_passed() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _asset_registry, _engineer_registry, _admin, _owner, asset_id, engineer) = setup(&env);

    // Two OIL_CHG records 7 days apart at T=1000 and T=1000+7d
    env.ledger().set_timestamp(1000);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "oil change 1"),
        &engineer,
    );

    env.ledger().set_timestamp(1000 + 7 * 24 * 60 * 60);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "oil change 2"),
        &engineer,
    );

    // Advance time 10 days past the last service — overdue!
    env.ledger()
        .set_timestamp(1000 + 7 * 24 * 60 * 60 + 10 * 24 * 60 * 60);

    let alerts = lifecycle.get_maintenance_alerts(&asset_id);
    assert!(!alerts.is_empty(), "Must have alerts when predicted date has passed");
    assert_eq!(alerts.len(), 1, "Should have exactly one alert for OIL_CHG");

    let (alert_type, _overdue_since) = alerts.get(0).unwrap();
    assert_eq!(
        alert_type,
        symbol_short!("OIL_CHG"),
        "Alert must be for the overdue OIL_CHG task type"
    );
}

#[test]
fn test_alerts_multiple_overdue_types() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _asset_registry, _engineer_registry, _admin, _owner, asset_id, engineer) = setup(&env);

    // Two OIL_CHG records 7 days apart
    env.ledger().set_timestamp(1000);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "oil change 1"),
        &engineer,
    );
    env.ledger().set_timestamp(1000 + 7 * 24 * 60 * 60);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "oil change 2"),
        &engineer,
    );

    // Two FILTER records 30 days apart
    env.ledger().set_timestamp(2000);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("FILTER"),
        &String::from_str(&env, "filter change 1"),
        &engineer,
    );
    env.ledger().set_timestamp(2000 + 30 * 24 * 60 * 60);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("FILTER"),
        &String::from_str(&env, "filter change 2"),
        &engineer,
    );

    // Advance past both predictions
    env.ledger()
        .set_timestamp(2000 + 30 * 24 * 60 * 60 + 60 * 24 * 60 * 60);

    let alerts = lifecycle.get_maintenance_alerts(&asset_id);
    assert_eq!(alerts.len(), 2, "Must have 2 overdue alerts");
}

#[test]
fn test_alerts_not_overdue_when_prediction_future() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _asset_registry, _engineer_registry, _admin, _owner, asset_id, engineer) = setup(&env);

    // Two records 7 days apart at T=1000 and T=1000+7d
    env.ledger().set_timestamp(1000);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "oil change 1"),
        &engineer,
    );

    env.ledger().set_timestamp(1000 + 7 * 24 * 60 * 60);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "oil change 2"),
        &engineer,
    );

    // Set time to just after the last service but well before prediction
    // Prediction = last_ts + 7d, so 1 day after last is not overdue
    env.ledger()
        .set_timestamp(1000 + 7 * 24 * 60 * 60 + 1 * 24 * 60 * 60);

    let alerts = lifecycle.get_maintenance_alerts(&asset_id);
    assert!(
        alerts.is_empty(),
        "No alerts when prediction is still in the future"
    );
}

#[test]
fn test_predict_minimum_clamp() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _asset_registry, _engineer_registry, _admin, _owner, asset_id, engineer) = setup(&env);

    // Two records 1 minute apart — interval is tiny, should be clamped to 24h minimum
    env.ledger().set_timestamp(1000);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &String::from_str(&env, "back to back 1"),
        &engineer,
    );

    env.ledger().set_timestamp(1060); // 60 seconds later
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &String::from_str(&env, "back to back 2"),
        &engineer,
    );

    let predicted = lifecycle.calculate_predicted_next_service(
        &asset_id,
        &symbol_short!("INSPECT"),
    );

    // Min clamp is 24h, so prediction >= last_ts + 24h
    let min_expected = 1060 + 24 * 60 * 60;
    assert!(
        predicted >= min_expected,
        "Prediction for tiny interval must be clamped to minimum 24h"
    );
}
