use super::*;
use soroban_sdk::testutils::{Address as _, Ledger as _};

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
