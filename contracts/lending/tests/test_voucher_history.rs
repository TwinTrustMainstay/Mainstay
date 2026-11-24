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
fn test_voucher_history_empty() {
    let env = Env::default();
    let (client, _token_id, _contract_id) = setup_contract(&env);

    let voucher = Address::generate(&env);
    let history = client.voucher_history(&voucher);
    assert_eq!(history.len(), 0);
}

#[test]
fn test_voucher_history_single_vouch() {
    let env = Env::default();
    let (client, token_id, contract_id) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_id);
    token_client.mint(&token_admin, &voucher, &1000);

    client.request_loan(&borrower, &100, &0u64);
    client.vouch(&borrower, &voucher, &100);

    let history = client.voucher_history(&voucher);
    assert_eq!(history.len(), 1);
    assert_eq!(history.get(0).unwrap(), borrower);
}

#[test]
fn test_voucher_history_multiple_borrowers() {
    let env = Env::default();
    let (client, token_id, contract_id) = setup_contract(&env);

    let borrower1 = Address::generate(&env);
    let borrower2 = Address::generate(&env);
    let voucher = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_id);
    token_client.mint(&token_admin, &voucher, &10000);

    client.request_loan(&borrower1, &100, &0u64);
    client.vouch(&borrower1, &voucher, &100);

    client.request_loan(&borrower2, &200, &0u64);
    client.vouch(&borrower2, &voucher, &200);

    let history = client.voucher_history(&voucher);
    assert_eq!(history.len(), 2);
    assert_eq!(history.get(0).unwrap(), borrower1);
    assert_eq!(history.get(1).unwrap(), borrower2);
}

#[test]
fn test_voucher_history_multiple_vouchers() {
    let env = Env::default();
    let (client, token_id, contract_id) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher1 = Address::generate(&env);
    let voucher2 = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_id);
    token_client.mint(&token_admin, &voucher1, &10000);
    token_client.mint(&token_admin, &voucher2, &10000);

    client.request_loan(&borrower, &100, &0u64);
    client.vouch(&borrower, &voucher1, &100);
    client.vouch(&borrower, &voucher2, &100);

    let history1 = client.voucher_history(&voucher1);
    assert_eq!(history1.len(), 1);
    assert_eq!(history1.get(0).unwrap(), borrower);

    let history2 = client.voucher_history(&voucher2);
    assert_eq!(history2.len(), 1);
    assert_eq!(history2.get(0).unwrap(), borrower);
}
