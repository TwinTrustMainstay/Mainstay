use super::*;
use soroban_sdk::symbol_short;

#[test]
fn maintenance_history_has_a_long_lived_ttl() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry, engineer_registry, _) = setup(&env, 0);
    let (asset_id, owner) = register_asset(&env, &asset_registry);
    let engineer = register_engineer(&env, &engineer_registry);
    client.authorize_engineer(&owner, &asset_id, &engineer);

    client.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &Priority::Low,
        &String::from_str(&env, "Routine check"),
        &engineer,
        &None,
    );
    let contract_id = client.address.clone();
    let ttl = env.as_contract(&contract_id, || {
        env.storage().persistent().get_ttl(&history_key(asset_id))
    });

    assert!(ttl >= TTL_THRESHOLD);
}
