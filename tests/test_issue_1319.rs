/// Tests for issue #1319: On-chain engineer dispute resolution mechanism

use lifecycle::{Lifecycle, LifecycleClient, DisputeRecord, Priority};
use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use soroban_sdk::{symbol_short, testutils::Address as _, Address, Env, String, Vec};

fn unique_serial(env: &Env) -> String {
    String::from_str(
        env,
        &format!("SN-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        )
    )
}

fn setup(env: &Env) -> (LifecycleClient, AssetRegistryClient, EngineerRegistryClient, Address) {
    let lifecycle_contract_id = env.register(Lifecycle, ());
    let asset_registry_contract_id = env.register(AssetRegistry, ());
    let engineer_registry_contract_id = env.register(EngineerRegistry, ());

    let lifecycle_client = LifecycleClient::new(env, &lifecycle_contract_id);
    let asset_registry_client = AssetRegistryClient::new(env, &asset_registry_contract_id);
    let engineer_registry_client = EngineerRegistryClient::new(env, &engineer_registry_contract_id);

    let admin = Address::generate(env);

    env.mock_all_auths();

    // Initialize contracts
    lifecycle_client.initialize_admin(&admin);
    asset_registry_client.initialize_admin(&admin, &admin);
    engineer_registry_client.initialize_admin(&admin, &admin);

    // Link contracts
    lifecycle_client.set_asset_registry(&admin, &asset_registry_contract_id);
    lifecycle_client.set_engineer_registry(&admin, &engineer_registry_contract_id);
    asset_registry_client.set_lifecycle_contract(&admin, &lifecycle_contract_id);

    // Set up task weight
    lifecycle_client.set_task_weight(&admin, &symbol_short!("OIL_CHG"), &100u32);

    (lifecycle_client, asset_registry_client, engineer_registry_client, admin)
}

#[test]
fn test_issue_1319_dispute_maintenance_record() {
    let env = Env::default();
    let (lifecycle_client, asset_registry_client, engineer_registry_client, _admin) = setup(&env);

    let asset_owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    env.mock_all_auths();

    // Register asset
    let asset_id = asset_registry_client.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Diesel Generator"),
        &unique_serial(&env),
        &asset_owner,
    );

    // Register engineer
    engineer_registry_client.register_engineer(
        &engineer,
        &String::from_str(&env, "John Doe"),
        &symbol_short!("GENSET"),
    );

    // Authorize engineer
    lifecycle_client.authorize_engineer(&asset_owner, &asset_id, &engineer);

    // Submit maintenance record
    lifecycle_client.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &Priority::Medium,
        &String::from_str(&env, "Oil changed"),
        &engineer,
        &None,
    );

    // Get the timestamp of the record for dispute
    let history = lifecycle_client.get_maintenance_history(&asset_id);
    assert!(!history.is_empty(), "should have maintenance record");

    let record_timestamp = history.get(0).unwrap().timestamp;

    // Dispute the record
    lifecycle_client.dispute_record(
        &asset_id,
        &record_timestamp,
        &String::from_str(&env, "No service was actually performed"),
    );

    // Verify dispute was recorded
    let disputes = lifecycle_client.get_disputes(&asset_id);
    assert!(!disputes.is_empty(), "should have dispute record");

    let dispute = disputes.get(0).unwrap();
    assert_eq!(dispute.asset_id, asset_id, "dispute should be for correct asset");
    assert_eq!(dispute.maintenance_timestamp, record_timestamp, "dispute should reference correct record");
    assert!(!dispute.is_resolved, "dispute should not be resolved yet");
}

#[test]
fn test_issue_1319_dispute_non_existent_record() {
    let env = Env::default();
    let (lifecycle_client, asset_registry_client, _engineer_registry_client, _admin) = setup(&env);

    let asset_owner = Address::generate(&env);

    env.mock_all_auths();

    let asset_id = asset_registry_client.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Diesel Generator"),
        &unique_serial(&env),
        &asset_owner,
    );

    // Try to dispute non-existent record (should fail)
    let result = lifecycle_client.try_dispute_record(
        &asset_id,
        &12345u64,
        &String::from_str(&env, "No such record"),
    );
    assert!(result.is_err(), "disputing non-existent record should fail");
}

#[test]
fn test_issue_1319_admin_resolve_dispute() {
    let env = Env::default();
    let (lifecycle_client, asset_registry_client, engineer_registry_client, admin) = setup(&env);

    let asset_owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    env.mock_all_auths();

    // Register asset and engineer
    let asset_id = asset_registry_client.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Diesel Generator"),
        &unique_serial(&env),
        &asset_owner,
    );

    engineer_registry_client.register_engineer(
        &engineer,
        &String::from_str(&env, "John Doe"),
        &symbol_short!("GENSET"),
    );

    lifecycle_client.authorize_engineer(&asset_owner, &asset_id, &engineer);

    // Submit and dispute maintenance
    lifecycle_client.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &Priority::Medium,
        &String::from_str(&env, "Oil changed"),
        &engineer,
        &None,
    );

    let history = lifecycle_client.get_maintenance_history(&asset_id);
    let record_timestamp = history.get(0).unwrap().timestamp;

    lifecycle_client.dispute_record(
        &asset_id,
        &record_timestamp,
        &String::from_str(&env, "Fraudulent claim"),
    );

    // Admin resolves the dispute
    lifecycle_client.resolve_dispute(
        &admin,
        &asset_id,
        &record_timestamp,
        &symbol_short!("UPHELD"),
    );

    // Verify dispute is resolved
    let disputes = lifecycle_client.get_disputes(&asset_id);
    let dispute = disputes.get(0).unwrap();
    assert!(dispute.is_resolved, "dispute should be resolved");
    assert_eq!(dispute.admin_decision.unwrap(), symbol_short!("UPHELD"), "decision should be UPHELD");
}

#[test]
fn test_issue_1319_non_admin_cannot_resolve() {
    let env = Env::default();
    let (lifecycle_client, asset_registry_client, engineer_registry_client, _admin) = setup(&env);

    let asset_owner = Address::generate(&env);
    let engineer = Address::generate(&env);
    let non_admin = Address::generate(&env);

    env.mock_all_auths();

    let asset_id = asset_registry_client.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Diesel Generator"),
        &unique_serial(&env),
        &asset_owner,
    );

    engineer_registry_client.register_engineer(
        &engineer,
        &String::from_str(&env, "John Doe"),
        &symbol_short!("GENSET"),
    );

    lifecycle_client.authorize_engineer(&asset_owner, &asset_id, &engineer);

    lifecycle_client.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &Priority::Medium,
        &String::from_str(&env, "Oil changed"),
        &engineer,
        &None,
    );

    let history = lifecycle_client.get_maintenance_history(&asset_id);
    let record_timestamp = history.get(0).unwrap().timestamp;

    lifecycle_client.dispute_record(
        &asset_id,
        &record_timestamp,
        &String::from_str(&env, "Fraudulent claim"),
    );

    // Try to resolve as non-admin (should fail)
    let result = lifecycle_client.try_resolve_dispute(
        &non_admin,
        &asset_id,
        &record_timestamp,
        &symbol_short!("UPHELD"),
    );
    assert!(result.is_err(), "non-admin should not be able to resolve disputes");
}

#[test]
fn test_issue_1319_multiple_disputes() {
    let env = Env::default();
    let (lifecycle_client, asset_registry_client, engineer_registry_client, admin) = setup(&env);

    let asset_owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    env.mock_all_auths();

    let asset_id = asset_registry_client.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Diesel Generator"),
        &unique_serial(&env),
        &asset_owner,
    );

    engineer_registry_client.register_engineer(
        &engineer,
        &String::from_str(&env, "John Doe"),
        &symbol_short!("GENSET"),
    );

    lifecycle_client.authorize_engineer(&asset_owner, &asset_id, &engineer);

    // Submit multiple records
    for i in 0..3 {
        lifecycle_client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &Priority::Medium,
            &String::from_str(&env, &format!("Oil changed {}", i)),
            &engineer,
            &None,
        );
        env.ledger().set_timestamp(env.ledger().timestamp() + 1000);
    }

    // Dispute all of them
    let history = lifecycle_client.get_maintenance_history(&asset_id);
    for record in history.iter() {
        lifecycle_client.dispute_record(
            &asset_id,
            &record.timestamp,
            &String::from_str(&env, "Disputing this maintenance"),
        );
    }

    // Verify all disputes are recorded
    let disputes = lifecycle_client.get_disputes(&asset_id);
    assert_eq!(disputes.len() as u32, 3u32, "should have 3 disputes");

    // Resolve first dispute
    let first_dispute_timestamp = disputes.get(0).unwrap().maintenance_timestamp;
    lifecycle_client.resolve_dispute(
        &admin,
        &asset_id,
        &first_dispute_timestamp,
        &symbol_short!("REJECTED"),
    );

    // Verify only first is resolved
    let updated_disputes = lifecycle_client.get_disputes(&asset_id);
    assert!(updated_disputes.get(0).unwrap().is_resolved, "first dispute should be resolved");
    assert!(!updated_disputes.get(1).unwrap().is_resolved, "second dispute should not be resolved");
}
