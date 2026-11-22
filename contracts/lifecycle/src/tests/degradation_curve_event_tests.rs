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
fn register_degradation_curve_emits_event_with_category_and_version() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup_lifecycle(&env);
    let category = symbol_short!("GENSET");

    client.register_degradation_curve(&admin, &category, &5u32, &2u32, &10u32);

    let data = event_data(&env, symbol_short!("DEG_CRV"))
        .expect("DEG_CRV event must be emitted by register_degradation_curve");
    let emitted: (Symbol, u32) = data.try_into_val(&env).unwrap();
    assert_eq!(emitted, (category, 1u32));
}
