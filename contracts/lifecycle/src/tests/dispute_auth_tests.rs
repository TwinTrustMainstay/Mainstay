use super::*;

/// Submits one maintenance record for `asset_id` and returns its timestamp.
fn submit_one_record(
    env: &Env,
    client: &LifecycleClient,
    engineer_registry_client: &EngineerRegistryClient,
    asset_id: u64,
    owner: &Address,
) -> u64 {
    let engineer = register_engineer(env, engineer_registry_client);
    client.authorize_engineer(owner, &asset_id, &engineer);
    client.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &Priority::Low,
        &String::from_str(env, "oil change"),
        &engineer,
        &None,
    );
    env.ledger().timestamp()
}

#[test]
fn dispute_record_rejects_unregistered_caller() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
    let (asset_id, owner) = register_asset(&env, &asset_registry_client);
    let ts = submit_one_record(&env, &client, &engineer_registry_client, asset_id, &owner);

    let stranger = Address::generate(&env);
    let result = client.try_dispute_record(
        &stranger,
        &asset_id,
        &ts,
        &String::from_str(&env, "no service performed"),
    );

    assert!(result.is_err(), "unregistered caller must not be able to dispute");
    assert_eq!(client.get_disputes(&asset_id).len(), 0);
}

#[test]
fn dispute_record_accepts_asset_owner() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
    let (asset_id, owner) = register_asset(&env, &asset_registry_client);
    let ts = submit_one_record(&env, &client, &engineer_registry_client, asset_id, &owner);

    client.dispute_record(
        &owner,
        &asset_id,
        &ts,
        &String::from_str(&env, "owner dispute"),
    );

    assert_eq!(client.get_disputes(&asset_id).len(), 1);
}

#[test]
fn dispute_record_accepts_authorized_attestor() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry_client, engineer_registry_client, _) = setup(&env, 0);
    let (asset_id, owner) = register_asset(&env, &asset_registry_client);
    let ts = submit_one_record(&env, &client, &engineer_registry_client, asset_id, &owner);

    let attestor = Address::generate(&env);
    client.authorize_attestor(&owner, &asset_id, &attestor);
    client.dispute_record(
        &attestor,
        &asset_id,
        &ts,
        &String::from_str(&env, "attestor dispute"),
    );

    assert_eq!(client.get_disputes(&asset_id).len(), 1);
}
