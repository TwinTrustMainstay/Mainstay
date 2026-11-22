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
fn register_score_provider_emits_event_with_provider() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup_lifecycle(&env);
    let provider = Address::generate(&env);

    client.register_score_provider(&admin, &provider);

    let data = event_data(&env, symbol_short!("PROV_REG"))
        .expect("PROV_REG event must be emitted by register_score_provider");
    let emitted: Address = data.try_into_val(&env).unwrap();
    assert_eq!(emitted, provider);
}

#[test]
fn remove_score_provider_emits_event_with_provider() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup_lifecycle(&env);
    let provider = Address::generate(&env);
    client.register_score_provider(&admin, &provider);

    client.remove_score_provider(&admin, &provider);

    let data = event_data(&env, symbol_short!("PROV_RM"))
        .expect("PROV_RM event must be emitted by remove_score_provider");
    let emitted: Address = data.try_into_val(&env).unwrap();
    assert_eq!(emitted, provider);
}
