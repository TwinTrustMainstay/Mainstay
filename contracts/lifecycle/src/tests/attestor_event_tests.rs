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

#[test]
fn assign_task_emits_event_with_task_id_and_assignee() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry_client, _, _) = setup(&env, 0);
    let (asset_id, owner) = register_asset(&env, &asset_registry_client);

    let assignee = Address::generate(&env);
    client.assign_task(&owner, &asset_id, &assignee);

    let events = env.events().all();
    let assign_event = events.iter().find(|(_, topics, _)| {
        topics.len() == 2
            && topics
                .get(0)
                .and_then(|v| TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                .map(|s: Symbol| s == symbol_short!("TASK_ASN"))
                .unwrap_or(false)
    });
    let (_, topics, data) = assign_event.expect("expected TASK_ASN assignment event");

    let emitted_asset: u64 = topics.get(1).unwrap().try_into_val(&env).unwrap();
    assert_eq!(emitted_asset, asset_id);

    let emitted_assignee: Address = data.try_into_val(&env).unwrap();
    assert_eq!(emitted_assignee, assignee);
}

#[test]
fn update_assignment_status_emits_event_on_transition() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry_client, _, _) = setup(&env, 0);
    let (asset_id, owner) = register_asset(&env, &asset_registry_client);

    let assignee = Address::generate(&env);
    client.assign_task(&owner, &asset_id, &assignee);
    client.update_assignment_status(&assignee, &asset_id, &symbol_short!("ACTIVE"));

    let events = env.events().all();
    let status_event = events.iter().find(|(_, topics, _)| {
        topics.len() == 2
            && topics
                .get(0)
                .and_then(|v| TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                .map(|s: Symbol| s == symbol_short!("TASK_STS"))
                .unwrap_or(false)
    });
    let (_, topics, data) = status_event.expect("expected TASK_STS status event");

    let emitted_asset: u64 = topics.get(1).unwrap().try_into_val(&env).unwrap();
    assert_eq!(emitted_asset, asset_id);

    let emitted_status: Symbol = data.try_into_val(&env).unwrap();
    assert_eq!(emitted_status, symbol_short!("ACTIVE"));
}
