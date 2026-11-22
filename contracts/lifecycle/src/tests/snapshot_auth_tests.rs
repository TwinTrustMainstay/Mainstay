use super::*;

#[test]
fn take_health_snapshot_rejects_non_owner_caller() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry_client, _, _) = setup(&env, 0);
    let (asset_id, _owner) = register_asset(&env, &asset_registry_client);

    let stranger = Address::generate(&env);
    let result = client.try_take_health_snapshot(&stranger, &asset_id);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            ContractError::UnauthorizedOwner as u32
        )))
    );
}

#[test]
fn take_health_snapshot_accepts_asset_owner() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry_client, _, _) = setup(&env, 0);
    let (asset_id, owner) = register_asset(&env, &asset_registry_client);

    let snapshot = client.take_health_snapshot(&owner, &asset_id);

    assert_eq!(snapshot.maintenance_count, 0);
}
