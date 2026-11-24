use super::*;
use soroban_sdk::symbol_short;
use soroban_sdk::testutils::{Address as _, Ledger as _};

#[test]
fn registered_asset_survives_ledger_advancement() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(AssetRegistry, ());
    let client = AssetRegistryClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize_admin(&admin, &admin);
    client.add_asset_type(&admin, &symbol_short!("GENSET"));

    let owner = Address::generate(&env);
    let metadata = String::from_str(&env, "Generator");
    let asset_id = reg(
        &client,
        &env,
        symbol_short!("GENSET"),
        metadata,
        &owner,
    );

    env.ledger().with_mut(|ledger| ledger.sequence_number += 500);
    assert_eq!(client.get_asset(&asset_id).asset_id, asset_id);
}

#[test]
fn registered_asset_entry_has_a_long_lived_ttl() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(AssetRegistry, ());
    let client = AssetRegistryClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize_admin(&admin, &admin);
    client.add_asset_type(&admin, &symbol_short!("GENSET"));

    let owner = Address::generate(&env);
    let asset_id = reg(
        &client,
        &env,
        symbol_short!("GENSET"),
        String::from_str(&env, "Compressor"),
        &owner,
    );
    let ttl = env.as_contract(&contract_id, || {
        env.storage().persistent().get_ttl(&asset_key(asset_id))
    });

    assert!(ttl >= TTL_THRESHOLD);
}
