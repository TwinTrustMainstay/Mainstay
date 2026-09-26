//! Regression coverage for security fixes that must remain enforced at the
//! public contract boundary.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use soroban_sdk::{symbol_short, testutils::Address as _, Address, Env, String};

#[test]
fn duplicate_serial_numbers_are_rejected_across_owners() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AssetRegistry, ());
    let registry = AssetRegistryClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let first_owner = Address::generate(&env);
    let second_owner = Address::generate(&env);

    registry.initialize_admin(&admin, &admin);
    registry.add_asset_type(&admin, &symbol_short!("GENSET"));

    let serial = String::from_str(&env, "SECURITY-REGRESSION-001");
    let first_id = registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "First asset"),
        &serial,
        &first_owner,
    );

    let duplicate = registry.try_register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Second asset"),
        &serial,
        &second_owner,
    );
    assert!(duplicate.is_err(), "a serial number must identify one asset globally");
    assert_eq!(
        registry.get_asset_by_serial_number(&serial).unwrap().asset_id,
        first_id,
        "a rejected duplicate must not replace the original serial index"
    );
}
