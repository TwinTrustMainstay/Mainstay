use super::*;
use ::engineer_registry::EngineerRegistry;
use asset_registry::AssetRegistry;
use soroban_sdk::{symbol_short, testutils::{Address as _, Events}, Address, Symbol, TryIntoVal, Val};

fn setup_lifecycle(env: &Env) -> (LifecycleClient<'_>, Address) {
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());
    let admin = Address::generate(env);
    let client = LifecycleClient::new(env, &lifecycle_id);
    client.initialize(&admin, &asset_registry_id, &engineer_registry_id, &admin, &0);
    (client, admin)
}

fn event_data(env: &Env, topic: Symbol) -> Option<Val> {
    env.events()
        .all()
        .iter()
        .find(|(_, topics, _)| {
            topics.len() == 1
                && topics
                    .get(0)
                    .and_then(|v| TryIntoVal::<_, Symbol>::try_into_val(&v, env).ok())
                    .map(|s| s == topic)
                    .unwrap_or(false)
        })
        .map(|event| event.2)
}

#[test]
fn submit_external_score_emits_event_with_provider_and_score() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup_lifecycle(&env);
    let provider = Address::generate(&env);
    client.register_score_provider(&admin, &provider);
    let asset_id = 1u64;

    client.submit_external_score(&asset_id, &75u32, &provider);

    let data = event_data(&env, symbol_short!("EXT_SCR"))
        .expect("EXT_SCR event must be emitted by submit_external_score");
    let emitted: (u64, Address, u32) = data.try_into_val(&env).unwrap();
    assert_eq!(emitted, (asset_id, provider, 75u32));
}
