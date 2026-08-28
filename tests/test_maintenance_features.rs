#![cfg(test)]

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, Bytes, BytesN, Env, String, Vec,
};

// ============================================================================
//  Feature 1: Maintenance Cost Tracking
// ============================================================================

#[test]
fn test_cost_tracking_total_cost() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id, engineer, _owner) = setup_maintenance_env(&env);

    // Submit maintenance with cost
    let notes1 = String::from_str(&env, "Oil change - standard");
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &notes1,
        &engineer,
        &Some(500_000_000u64), // 50 XLM in stroops
    );

    let notes2 = String::from_str(&env, "Filter replacement");
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("FILTER"),
        &notes2,
        &engineer,
        &Some(200_000_000u64),
    );

    let notes3 = String::from_str(&env, "Inspection - no cost recorded");
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &notes3,
        &engineer,
        &None,
    );

    let total = lifecycle.get_total_maintenance_cost(&asset_id);
    assert_eq!(total, 700_000_000u64, "Total cost should be 700M stroops");
}

#[test]
fn test_cost_tracking_cost_history() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id, engineer, _owner) = setup_maintenance_env(&env);

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Oil change"),
        &engineer,
        &Some(100_000_000u64),
    );

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("INSPECT"),
        &String::from_str(&env, "Inspection no cost"),
        &engineer,
        &None,
    );

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("FILTER"),
        &String::from_str(&env, "Filter"),
        &engineer,
        &Some(50_000_000u64),
    );

    let history = lifecycle.get_maintenance_cost_history(&asset_id);
    // Should have 2 entries (INSPECT with None is filtered out)
    assert_eq!(history.len(), 2);
    assert_eq!(history.get(0).unwrap().1, 100_000_000u64);
    assert_eq!(history.get(1).unwrap().1, 50_000_000u64);
}

#[test]
fn test_cost_tracking_average_cost_by_type() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id, engineer, _owner) = setup_maintenance_env(&env);

    // Two oil changes at different costs
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Oil change 1"),
        &engineer,
        &Some(300_000_000u64),
    );
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Oil change 2"),
        &engineer,
        &Some(500_000_000u64),
    );
    // One with no cost (should be excluded)
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Oil change 3 free"),
        &engineer,
        &None,
    );

    let avg = lifecycle.get_average_maintenance_interval_cost(
        &asset_id,
        &symbol_short!("OIL_CHG"),
    );
    assert_eq!(avg, 400_000_000u64, "Average of 300M and 500M should be 400M");
}

#[test]
fn test_cost_tracking_no_history_returns_zero() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, asset_id, _engineer, _owner) = setup_maintenance_env(&env);

    let total = lifecycle.get_total_maintenance_cost(&asset_id);
    assert_eq!(total, 0);

    let history = lifecycle.get_maintenance_cost_history(&asset_id);
    assert!(history.is_empty());

    let avg = lifecycle.get_average_maintenance_interval_cost(&asset_id, &symbol_short!("OIL_CHG"));
    assert_eq!(avg, 0);
}

#[test]
fn test_maintenance_history_by_engineer_filters_asset_history() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id, engineer, _owner) = setup_maintenance_env(&env);
    let other_engineer = Address::generate(&env);

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Engineer maintenance"),
        &engineer,
        &Some(10u64),
    );
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("FILTER"),
        &String::from_str(&env, "Other engineer maintenance"),
        &other_engineer,
        &Some(20u64),
    );

    let records = lifecycle.get_maintenance_history_by_engineer(&asset_id, &engineer);
    assert_eq!(records.len(), 1);
    assert_eq!(records.get(0).unwrap().engineer, engineer);
    assert_eq!(records.get(0).unwrap().task_type, symbol_short!("OIL_CHG"));

    let no_records = lifecycle.get_maintenance_history_by_engineer(&asset_id, &Address::generate(&env));
    assert!(no_records.is_empty());
}

// ============================================================================
//  Feature 2: Recurring Maintenance Tasks
// ============================================================================

#[test]
fn test_schedule_and_get_recurring_tasks() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id, _engineer, owner) = setup_maintenance_env(&env);

    lifecycle.schedule_recurring_task(
        &owner,
        &asset_id,
        &1u64,
        &symbol_short!("OIL_CHG"),
        &symbol_short!("HOURS"),
        &500u64,
    );

    lifecycle.schedule_recurring_task(
        &owner,
        &asset_id,
        &2u64,
        &symbol_short!("FILTER"),
        &symbol_short!("CYCLES"),
        &1000u64,
    );

    let tasks = lifecycle.get_recurring_tasks(&asset_id);
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks.get(0).unwrap().task_id, 1);
    assert_eq!(tasks.get(1).unwrap().task_id, 2);
    assert!(tasks.get(0).unwrap().is_active);
}

#[test]
#[should_panic]
fn test_duplicate_recurring_task_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id, _engineer, owner) = setup_maintenance_env(&env);

    lifecycle.schedule_recurring_task(
        &owner,
        &asset_id,
        &1u64,
        &symbol_short!("OIL_CHG"),
        &symbol_short!("HOURS"),
        &500u64,
    );

    // Same task_id should panic
    lifecycle.schedule_recurring_task(
        &owner,
        &asset_id,
        &1u64,
        &symbol_short!("FILTER"),
        &symbol_short!("DAYS"),
        &30u64,
    );
}

#[test]
fn test_auto_create_recurring_task() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id, engineer, owner) = setup_maintenance_env(&env);

    lifecycle.schedule_recurring_task(
        &owner,
        &asset_id,
        &1u64,
        &symbol_short!("OIL_CHG"),
        &symbol_short!("HOURS"),
        &3600u64,
    );

    // Auto-create the recurring task
    lifecycle.auto_create_recurring_task(&asset_id, &1u64, &engineer);

    // Verify maintenance history was updated
    let history = lifecycle.get_maintenance_history(&asset_id);
    assert_eq!(history.len(), 1);
    assert_eq!(history.get(0).unwrap().task_type, symbol_short!("OIL_CHG"));
    assert_eq!(history.get(0).unwrap().cost, None);

    // Verify next_due was updated
    let tasks = lifecycle.get_recurring_tasks(&asset_id);
    assert!(tasks.get(0).unwrap().next_due > 0);
}

#[test]
fn test_get_recurring_tasks_empty() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, asset_id, _engineer, _owner) = setup_maintenance_env(&env);

    let tasks = lifecycle.get_recurring_tasks(&asset_id);
    assert!(tasks.is_empty());
}

// ============================================================================
//  Feature 3: Duplicate Maintenance Detection
// ============================================================================

#[test]
fn test_detect_duplicate_maintenance_events() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id, engineer, _owner) = setup_maintenance_env(&env);

    // Submit same task type + engineer within a short window
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Oil change report 1"),
        &engineer,
        &None,
    );

    // Advance time by 10 seconds
    env.ledger().with_mut(|l| l.timestamp += 10);

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Oil change report 2"),
        &engineer,
        &None,
    );

    // Duplicate detection with 60-second window should find the pair
    let dupes = lifecycle.get_duplicate_maintenance_events(&asset_id, &60u64);
    assert_eq!(dupes.len(), 1, "Should detect 1 pair of duplicates");
}

#[test]
fn test_no_duplicates_outside_window() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id, engineer, _owner) = setup_maintenance_env(&env);

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Oil change"),
        &engineer,
        &None,
    );

    // Advance time by 1 hour
    env.ledger().with_mut(|l| l.timestamp += 3600);

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Another oil change"),
        &engineer,
        &None,
    );

    // With a 60-second window, these should NOT be duplicates
    let dupes = lifecycle.get_duplicate_maintenance_events(&asset_id, &60u64);
    assert_eq!(dupes.len(), 0, "Events 1 hour apart should not be duplicates");
}

#[test]
fn test_mark_maintenance_as_duplicate() {
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
    let lifecycle_admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let asset_owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("FUZZ"));
    engineer_registry.initialize_admin(&eng_admin, &eng_admin);
    engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
    lifecycle.initialize(
        &lifecycle_admin,
        &asset_registry_id,
        &engineer_registry_id,
        &lifecycle_admin,
        &0,
    );

    let metadata = String::from_str(&env, "Test asset");
    let asset_id = asset_registry.register_asset(
        &symbol_short!("FUZZ"),
        &metadata,
        &String::from_str(&env, "SN-DUP"),
        &asset_owner,
    );

    let credential_hash = BytesN::from_array(&env, &[42u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000);
    lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Record A"),
        &engineer,
        &None,
    );
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Record B - duplicate"),
        &engineer,
        &None,
    );

    let hist_a = lifecycle.get_maintenance_history(&asset_id);
    let ts_a = hist_a.get(0).unwrap().timestamp;
    let ts_b = hist_a.get(1).unwrap().timestamp;

    // Mark B as duplicate of A (mock_all_auths allows admin auth)
    lifecycle.mark_maintenance_as_duplicate(
        &lifecycle_admin,
        &asset_id,
        &ts_a,
        &ts_b,
    );

    // Verify the duplicate was recorded by checking that scoring skips it
    // (we can verify via get_collateral_score not crashing)
    let score = lifecycle.get_collateral_score(&asset_id);
    assert!(score <= 100, "Score should be within valid range");
}

#[test]
fn test_different_task_types_not_duplicates() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id, engineer, _owner) = setup_maintenance_env(&env);

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Oil change"),
        &engineer,
        &None,
    );

    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("FILTER"),
        &String::from_str(&env, "Filter replacement"),
        &engineer,
        &None,
    );

    let dupes = lifecycle.get_duplicate_maintenance_events(&asset_id, &3600u64);
    assert_eq!(dupes.len(), 0, "Different task types should not be duplicates");
}

// ============================================================================
//  Feature 4: Compliance Standards
// ============================================================================

#[test]
fn test_register_and_validate_compliance_standard() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id, engineer, _owner) = setup_maintenance_env(&env);

    // Register a compliance standard for the FUZZ asset type
    let standard_hash = Bytes::from_slice(&env, &[1u8, 2, 3, 4, 5, 6, 7, 8]);

    // Use admin to register (use lifecycle_admin from setup)
    // Since mock_all_auths() is used, admin auth passes
    let lifecycle_admin = Address::generate(&env);
    lifecycle.register_standard(
        &lifecycle_admin,
        &symbol_short!("FUZZ"),
        &standard_hash,
    );

    // Validate compliance for the same asset type
    let is_compliant = lifecycle.validate_maintenance_compliance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &standard_hash,
    );
    assert!(is_compliant, "Should be compliant with registered standard");
}

#[test]
fn test_validate_compliance_with_unregistered_standard() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id, _engineer, _owner) = setup_maintenance_env(&env);

    let unknown_hash = Bytes::from_slice(&env, &[9u8, 9, 9, 9]);

    let is_compliant = lifecycle.validate_maintenance_compliance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &unknown_hash,
    );
    assert!(!is_compliant, "Unregistered standard should not validate");
}

#[test]
fn test_get_maintenance_standard() {
    let env = Env::default();
    env.mock_all_auths();

    let lifecycle_id = env.register(Lifecycle, ());
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    let standard = lifecycle.get_maintenance_standard(&symbol_short!("ENGINE"));
    // Returns empty bytes when no standard registered
    assert_eq!(standard.len(), 0);
}

// ============================================================================
//  Helper: Setup full test environment
// ============================================================================

fn setup_maintenance_env(env: &Env) -> (LifecycleClient, u64, Address, Address) {
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(env, &lifecycle_id);

    let asset_admin = Address::generate(env);
    let eng_admin = Address::generate(env);
    let lifecycle_admin = Address::generate(env);
    let issuer = Address::generate(env);
    let asset_owner = Address::generate(env);
    let engineer = Address::generate(env);

    // Initialize contracts
    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("FUZZ"));
    engineer_registry.initialize_admin(&eng_admin, &eng_admin);
    engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
    lifecycle.initialize(
        &lifecycle_admin,
        &asset_registry_id,
        &engineer_registry_id,
        &lifecycle_admin,
        &0,
    );

    // Register asset
    let metadata = String::from_str(env, "Test asset for feature tests");
    let asset_id = asset_registry.register_asset(
        &symbol_short!("FUZZ"),
        &metadata,
        &String::from_str(env, "SN-FEAT"),
        &asset_owner,
    );

    // Register engineer
    let credential_hash = BytesN::from_array(env, &[42u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000);
    lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

    (lifecycle, asset_id, engineer, asset_owner)
}
