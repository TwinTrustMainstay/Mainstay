use super::*;

#[test]
fn asset_without_maintenance_is_not_collateral_eligible() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry, _, _) = setup(&env, 0);
    let (asset_id, _) = register_asset(&env, &asset_registry);

    assert!(!client.is_collateral_eligible(&asset_id));
}

#[test]
fn unknown_asset_returns_a_contract_error_instead_of_panicking() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _, _) = setup(&env, 0);
    let result = client.try_is_collateral_eligible(&u64::MAX);

    assert!(result.is_err());
}
