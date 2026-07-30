#![cfg(test)]

use lending::{LendingContract, LendingContractClient};
use soroban_sdk::{testutils::Address as _, Address, Env};

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
fn test_set_max_loan_amount_enforced() {
    let env = Env::default();
    let (client, _token_id, _contract_id) = setup_contract(&env);

    let admin = Address::generate(&env);
    client.set_max_loan_amount(&admin, &500);

    let borrower = Address::generate(&env);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.request_loan(&borrower, &1000, &0u64);
    }));
    assert!(result.is_err());
}

#[test]
fn test_set_max_loan_amount_allows_valid_amount() {
    let env = Env::default();
    let (client, _token_id, _contract_id) = setup_contract(&env);

    let admin = Address::generate(&env);
    client.set_max_loan_amount(&admin, &500);

    let borrower = Address::generate(&env);
    client.request_loan(&borrower, &300, &0u64);

    let loan = client.get_loan(&borrower);
    assert!(loan.is_some());
    assert_eq!(loan.unwrap().amount, 300);
}

#[test]
fn test_set_max_loan_amount_boundary() {
    let env = Env::default();
    let (client, _token_id, _contract_id) = setup_contract(&env);

    let admin = Address::generate(&env);
    client.set_max_loan_amount(&admin, &500);

    let borrower = Address::generate(&env);
    client.request_loan(&borrower, &500, &0u64);

    let loan = client.get_loan(&borrower);
    assert!(loan.is_some());
    assert_eq!(loan.unwrap().amount, 500);
}

#[test]
fn test_set_max_loan_amount_non_admin_fails() {
    let env = Env::default();
    let (client, _token_id, _contract_id) = setup_contract(&env);

    let non_admin = Address::generate(&env);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.set_max_loan_amount(&non_admin, &500);
    }));
    assert!(result.is_err());
}

#[test]
fn test_set_max_loan_amount_zero() {
    let env = Env::default();
    let (client, _token_id, _contract_id) = setup_contract(&env);

    let admin = Address::generate(&env);
    client.set_max_loan_amount(&admin, &0);

    let borrower = Address::generate(&env);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.request_loan(&borrower, &1, &0u64);
    }));
    assert!(result.is_err());
}

#[test]
fn test_set_max_loan_amount_large_value() {
    let env = Env::default();
    let (client, _token_id, _contract_id) = setup_contract(&env);

    let admin = Address::generate(&env);
    client.set_max_loan_amount(&admin, &u64::MAX);

    let borrower = Address::generate(&env);
    client.request_loan(&borrower, &u64::MAX - 1, &0u64);

    let loan = client.get_loan(&borrower);
    assert!(loan.is_some());
    assert_eq!(loan.unwrap().amount, u64::MAX - 1);
}

#[test]
fn test_set_max_loan_amount_multiple_updates() {
    let env = Env::default();
    let (client, _token_id, _contract_id) = setup_contract(&env);

    let admin = Address::generate(&env);

    client.set_max_loan_amount(&admin, &1000);
    let borrower1 = Address::generate(&env);
    client.request_loan(&borrower1, &1000, &0u64);
    assert!(client.get_loan(&borrower1).is_some());

    client.set_max_loan_amount(&admin, &500);
    let borrower2 = Address::generate(&env);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.request_loan(&borrower2, &1000, &0u64);
    }));
    assert!(result.is_err());

    client.request_loan(&borrower2, &500, &0u64);
    assert!(client.get_loan(&borrower2).is_some());
}
