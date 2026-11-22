use super::*;
use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::{symbol_short, testutils::Events, Address, TryIntoVal};

#[test]
fn sign_maintenance_record_emits_event_with_record_reference_and_signer() {
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
        &String::from_str(&env, "Oil change"),
        &engineer,
        &None,
    );

    let record = client.get_maintenance_history(&asset_id).get(0).unwrap();
    let signing_key = SigningKey::from_bytes(&[9u8; 32]);
    let public_key = BytesN::from_array(&env, &signing_key.verifying_key().to_bytes());
    let digest = hash_maintenance_record(&env, &record);
    let mut message = [0u8; 32];
    digest.copy_into_slice(&mut message);
    let signature = BytesN::from_array(&env, &signing_key.sign(&message).to_bytes());

    client.sign_maintenance_record(
        &asset_id,
        &record.timestamp,
        &engineer,
        &public_key,
        &signature,
    );

    let events = env.events().all();
    let found = events
        .iter()
        .find(|(_, topics, _)| {
            topics.len() == 1
                && topics
                    .get(0)
                    .and_then(|v| TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                    .map(|s| s == symbol_short!("MNT_SIG"))
                    .unwrap_or(false)
        })
        .expect("MNT_SIG event must be emitted by sign_maintenance_record");

    let payload: (u64, u64, Address) = found.2.try_into_val(&env).unwrap();
    assert_eq!(payload, (asset_id, record.timestamp, engineer));
}
