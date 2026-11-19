//! Tests that the market oracle address and market condition data use separate
//! storage keys (#1866).

#![cfg(test)]

use lending::{ContractError, LendingContract, LendingContractClient};
use soroban_sdk::{testutils::Address as _, Address, Env};

fn setup_contract(env: &Env) -> LendingContractClient {
    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(env, &contract_id);

    let deployer = Address::generate(env);
    let admin = Address::generate(env);
    let token_id = env.register_stellar_asset_contract(Address::generate(env));

    env.mock_all_auths();
    client.initialize(&deployer, &admin, &token_id, &0);
    client
}

#[test]
fn test_oracle_set_then_conditions_update_then_adjust_does_not_trap() {
    let env = Env::default();
    let client = setup_contract(&env);
    let admin = client.get_admin();
    let oracle = Address::generate(&env);

    client.set_market_oracle(&admin, &oracle);
    client.update_market_conditions(&admin, &4_000, &-250);

    // Previously the oracle address was read back as a MarketCondition and trapped.
    let _ltv = client.adjust_ltv_for_conditions(&admin);
}

#[test]
fn test_oracle_address_alone_is_not_market_data() {
    let env = Env::default();
    let client = setup_contract(&env);
    let admin = client.get_admin();

    client.set_market_oracle(&admin, &Address::generate(&env));

    let err = client
        .try_adjust_ltv_for_conditions(&admin)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::OracleNotConfigured.into());
}

#[test]
fn test_market_update_does_not_overwrite_oracle_address() {
    let env = Env::default();
    let client = setup_contract(&env);
    let admin = client.get_admin();

    client.update_market_conditions(&admin, &1_000, &0);
    client.set_market_oracle(&admin, &Address::generate(&env));
    client.update_market_conditions(&admin, &5_000, &100);

    // Still succeeds: the market update did not replace the oracle address with
    // a MarketCondition, and the latest snapshot is readable.
    let _ltv = client.adjust_ltv_for_conditions(&admin);
}
