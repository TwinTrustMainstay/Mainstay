use super::*;

#[test]
fn unauthorized_caller_cannot_record_score_baseline() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry, _, _) = setup(&env, 0);
    let (asset_id, _owner) = register_asset(&env, &asset_registry);
    let stranger = Address::generate(&env);

    assert_eq!(
        client.try_record_score_baseline(&stranger, &asset_id, &50),
        Err(Ok(ContractError::UnauthorizedAttestor))
    );
}

#[test]
fn admin_and_authorized_attestor_can_record_score_baseline() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry, _, admin) = setup(&env, 0);
    let (asset_id, owner) = register_asset(&env, &asset_registry);
    let attestor = Address::generate(&env);

    client.record_score_baseline(&admin, &asset_id, &50);
    client.authorize_attestor(&owner, &asset_id, &attestor);
    client.record_score_baseline(&attestor, &asset_id, &55);
}
