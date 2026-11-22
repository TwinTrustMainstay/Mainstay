use super::*;

#[test]
fn take_health_snapshot_emits_event_with_asset_id_and_timestamp() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry_client, _, _) = setup(&env, 0);
    let (asset_id, owner) = register_asset(&env, &asset_registry_client);

    env.ledger().set_timestamp(1_700_000_000);
    client.take_health_snapshot(&owner, &asset_id);

    let events = env.events().all();
    let snap_event = events.iter().find(|(_, topics, _)| {
        topics.len() == 2
            && topics
                .get(0)
                .and_then(|v| TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                .map(|s: Symbol| s == symbol_short!("HLTH_SNP"))
                .unwrap_or(false)
    });
    let (_, topics, data) = snap_event.expect("expected HLTH_SNP snapshot event");

    let emitted_asset: u64 = topics.get(1).unwrap().try_into_val(&env).unwrap();
    assert_eq!(emitted_asset, asset_id);

    let emitted_ts: u64 = data.try_into_val(&env).unwrap();
    assert_eq!(emitted_ts, 1_700_000_000);
}
