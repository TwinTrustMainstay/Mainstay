#![cfg(test)]

use lending::{LendingContract, LendingContractClient};
use soroban_sdk::{testutils::Address as _, token, Address, Env};

fn setup_contract(env: &Env) -> (LendingContractClient, Address, Address) {
    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(env, &contract_id);

    let deployer = Address::generate(env);
    let admin = Address::generate(env);
    let token_admin = Address::generate(env);

    let token_id = env.register_stellar_asset_contract(token_admin.clone());

    env.mock_all_auths();
    client.initialize(&deployer, &admin, &token_id, &0);

    (client, token_id, contract_id)
}

#[test]
fn test_set_min_stake_by_admin() {
    let env = Env::default();
    let (client, token_id, contract_id) = setup_contract(&env);

    let admin = Address::generate(&env);
    let deployer = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_id);
    token_client.mint(&token_admin, &contract_id, &100000);

    client.set_min_stake(&admin, &200);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    token_client.mint(&token_admin, &voucher, &1000);

    client.request_loan(&borrower, &100, &0u64);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.vouch(&borrower, &voucher, &100);
    }));
    assert!(result.is_err());
}

#[test]
fn test_set_min_stake_allows_valid_stake() {
    let env = Env::default();
    let (client, token_id, contract_id) = setup_contract(&env);

    let admin = Address::generate(&env);
    let deployer = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_id);
    token_client.mint(&token_admin, &contract_id, &100000);

    client.set_min_stake(&admin, &100);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    token_client.mint(&token_admin, &voucher, &1000);

    client.request_loan(&borrower, &100, &0u64);
    client.vouch(&borrower, &voucher, &100);

    let vouches = client.get_vouches(&borrower);
    assert_eq!(vouches.len(), 1);
    assert_eq!(vouches.get(0).unwrap().stake, 100);
}

#[test]
fn test_set_min_stake_non_admin_fails() {
    let env = Env::default();
    let (client, _token_id, _contract_id) = setup_contract(&env);

    let non_admin = Address::generate(&env);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.set_min_stake(&non_admin, &200);
    }));
    assert!(result.is_err());
}

#[test]
fn test_set_min_stake_zero() {
    let env = Env::default();
    let (client, token_id, contract_id) = setup_contract(&env);

    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_id);
    token_client.mint(&token_admin, &contract_id, &100000);

    client.set_min_stake(&admin, &0);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    token_client.mint(&token_admin, &voucher, &1000);

    client.request_loan(&borrower, &100, &0u64);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.vouch(&borrower, &voucher, &1);
    }));
    assert!(result.is_err());
}

#[test]
fn test_set_min_stake_high_value() {
    let env = Env::default();
    let (client, token_id, contract_id) = setup_contract(&env);

    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_id);
    token_client.mint(&token_admin, &contract_id, &100000);

    client.set_min_stake(&admin, &5000);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    token_client.mint(&token_admin, &voucher, &10000);

    client.request_loan(&borrower, &100, &0u64);
    client.vouch(&borrower, &voucher, &5000);

    let vouches = client.get_vouches(&borrower);
    assert_eq!(vouches.len(), 1);
    assert_eq!(vouches.get(0).unwrap().stake, 5000);
}
