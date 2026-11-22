use super::*;

#[test]
fn authorize_attestor_emits_event_with_asset_id_and_attestor() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry_client, _, _) = setup(&env, 0);
    let (asset_id, owner) = register_asset(&env, &asset_registry_client);

    let attestor = Address::generate(&env);
    client.authorize_attestor(&owner, &asset_id, &attestor);

    let events = env.events().all();
    let grant_event = events.iter().find(|(_, topics, _)| {
        topics.len() == 2
            && topics
                .get(0)
                .and_then(|v| TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                .map(|s: Symbol| s == symbol_short!("ATTST_ON"))
                .unwrap_or(false)
    });
    let (_, topics, data) = grant_event.expect("expected ATTST_ON attestor event");

    let emitted_asset: u64 = topics.get(1).unwrap().try_into_val(&env).unwrap();
    assert_eq!(emitted_asset, asset_id);

    let emitted_attestor: Address = data.try_into_val(&env).unwrap();
    assert_eq!(emitted_attestor, attestor);
}
