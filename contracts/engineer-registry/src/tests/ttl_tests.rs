use super::*;
use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
use soroban_sdk::{IntoVal, Symbol};

#[test]
fn registered_engineer_survives_ledger_advancement() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup(&env);
    let engineer = Address::generate(&env);
    let issuer = Address::generate(&env);
    let hash = BytesN::from_array(&env, &[1u8; 32]);
    client.add_trusted_issuer(&admin, &issuer);
    client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

    env.ledger().with_mut(|ledger| ledger.sequence_number += 500);
    assert_eq!(
        client.verify_engineer(&engineer, &None::<Symbol>),
        CredentialStatus::Valid
    );
}

#[test]
fn registered_engineer_entry_has_a_long_lived_ttl() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup(&env);
    let engineer = Address::generate(&env);
    let issuer = Address::generate(&env);
    let hash = BytesN::from_array(&env, &[1u8; 32]);
    client.add_trusted_issuer(&admin, &issuer);
    client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

    let contract_id = client.address.clone();
    let ttl = env.as_contract(&contract_id, || {
        env.storage().persistent().get_ttl(&engineer_key(&engineer))
    });

    assert!(ttl >= TTL_THRESHOLD);
}

#[test]
fn set_engineer_service_area_emits_event() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup(&env);
    let engineer = Address::generate(&env);
    let issuer = Address::generate(&env);
    let hash = BytesN::from_array(&env, &[1u8; 32]);
    client.add_trusted_issuer(&admin, &issuer);
    client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

    let area = Symbol::new(&env, "emea");
    client.set_engineer_service_area(&engineer, &area);

    let events = env.events().all();
    let last = events.last().unwrap();
    assert_eq!(last.0, client.address.clone());
    assert_eq!(
        last.1,
        (Symbol::new(&env, "service_area_set"), engineer.clone()).into_val(&env)
    );
    assert_eq!(last.2, area.into_val(&env));
}
