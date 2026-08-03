#![cfg(test)]

use lending::{LendingContract, LendingContractClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env, String,
};

fn setup_contract(env: &Env) -> (LendingContractClient, Address, Address, Address) {
    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(env, &contract_id);

    let deployer = Address::generate(env);
    let admin = Address::generate(env);
    let token_admin = Address::generate(env);

    let token_contract_id = env.register(token::Contract, ());
    let token_client = token::Client::new(env, &token_contract_id);

    token_client.initialize(
        &token_admin,
        &18,
        &String::from_str(env, "Test Token"),
        &String::from_str(env, "TEST"),
    );

    client.initialize(&deployer, &admin, &token_contract_id, &200);

    (client, admin, token_contract_id, token_admin)
}

#[test]
fn test_initialize_with_yield_bps() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _token, _token_admin) = setup_contract(&env);

    let loan = client.get_loan(&admin);
    assert!(loan.is_none());
}

#[test]
fn test_withdraw_vouch_success() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, token_addr, token_admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_addr);
    token_client.mint(&voucher, &5000);

    client.vouch(&borrower, &voucher, &1000);

    let vouches = client.get_vouches(&borrower);
    assert_eq!(vouches.len(), 1);

    client.withdraw_vouch(&voucher, &borrower);

    let vouches_after = client.get_vouches(&borrower);
    assert_eq!(vouches_after.len(), 0);
}

#[test]
fn test_withdraw_vouch_with_active_loan_fails() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, _admin, token_addr, _token_admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_addr);
    token_client.mint(&voucher, &5000);

    client.vouch(&borrower, &voucher, &1000);
    client.request_loan(&borrower, &5000, &0u64);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.withdraw_vouch(&voucher, &borrower);
    }));

    assert!(result.is_err());
}

#[test]
fn test_get_credit_score_no_history() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, _admin, _token, _token_admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let score = client.get_credit_score(&borrower);
    assert_eq!(score, 0);
}

#[test]
fn test_get_credit_score_after_repayment() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, _admin, token_addr, token_admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_addr);
    token_client.mint(&voucher, &10000);
    token_client.mint(&env.current_contract_address(), &10000);

    client.vouch(&borrower, &voucher, &1000);
    client.request_loan(&borrower, &5000, &0u64);
    client.repay(&borrower);

    let score = client.get_credit_score(&borrower);
    assert_eq!(score, 100);
}

#[test]
fn test_get_credit_score_after_default() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, token_addr, _token_admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_addr);
    token_client.mint(&voucher, &10000);

    client.vouch(&borrower, &voucher, &1000);
    client.request_loan(&borrower, &5000, &0u64);
    client.slash(&admin, &borrower);

    let score = client.get_credit_score(&borrower);
    assert_eq!(score, 0);
}

#[test]
fn test_get_credit_score_mixed_history() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, token_addr, _token_admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher1 = Address::generate(&env);
    let voucher2 = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_addr);
    token_client.mint(&voucher1, &10000);
    token_client.mint(&voucher2, &10000);
    token_client.mint(&env.current_contract_address(), &10000);

    client.vouch(&borrower, &voucher1, &1000);
    client.request_loan(&borrower, &5000, &0u64);
    client.repay(&borrower);

    let score_after_repay = client.get_credit_score(&borrower);
    assert_eq!(score_after_repay, 100);

    client.vouch(&borrower, &voucher2, &1000);
    client.request_loan(&borrower, &5000, &0u64);
    client.slash(&admin, &borrower);

    let score_after_default = client.get_credit_score(&borrower);
    assert_eq!(score_after_default, 50);
}

#[test]
fn test_repay_with_configurable_yield() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, _admin, token_addr, _token_admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_addr);
    token_client.mint(&voucher, &10000);
    token_client.mint(&env.current_contract_address(), &10000);

    client.vouch(&borrower, &voucher, &1000);
    client.request_loan(&borrower, &5000, &0u64);

    let voucher_balance_before = token_client.balance(&voucher);
    client.repay(&borrower);
    let voucher_balance_after = token_client.balance(&voucher);

    let yield_received = voucher_balance_after - voucher_balance_before;
    assert_eq!(yield_received, 2);
}

#[test]
fn test_withdraw_vouch_multiple_vouchers() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, _admin, token_addr, _token_admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher1 = Address::generate(&env);
    let voucher2 = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_addr);
    token_client.mint(&voucher1, &5000);
    token_client.mint(&voucher2, &5000);

    client.vouch(&borrower, &voucher1, &1000);
    client.vouch(&borrower, &voucher2, &1000);

    let vouches = client.get_vouches(&borrower);
    assert_eq!(vouches.len(), 2);

    client.withdraw_vouch(&voucher1, &borrower);

    let vouches_after = client.get_vouches(&borrower);
    assert_eq!(vouches_after.len(), 1);
    assert_eq!(vouches_after.get(0).unwrap().voucher, voucher2);
}

#[test]
fn test_borrower_record_created_on_loan_request() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, _admin, _token, _token_admin) = setup_contract(&env);

    let borrower = Address::generate(&env);

    let score_before = client.get_credit_score(&borrower);
    assert_eq!(score_before, 0);

    client.request_loan(&borrower, &5000, &0u64);

    let score_after = client.get_credit_score(&borrower);
    assert_eq!(score_after, 0);
}

#[test]
fn test_repayment_count_increments() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, _admin, token_addr, _token_admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_addr);
    token_client.mint(&voucher, &10000);
    token_client.mint(&env.current_contract_address(), &10000);

    client.vouch(&borrower, &voucher, &1000);
    client.request_loan(&borrower, &5000, &0u64);
    client.repay(&borrower);

    let score1 = client.get_credit_score(&borrower);
    assert_eq!(score1, 100);

    client.vouch(&borrower, &voucher, &1000);
    client.request_loan(&borrower, &5000, &0u64);
    client.repay(&borrower);

    let score2 = client.get_credit_score(&borrower);
    assert_eq!(score2, 100);
}

#[test]
fn test_default_count_increments() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, token_addr, _token_admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_addr);
    token_client.mint(&voucher, &10000);

    client.vouch(&borrower, &voucher, &1000);
    client.request_loan(&borrower, &5000, &0u64);
    client.slash(&admin, &borrower);

    let score1 = client.get_credit_score(&borrower);
    assert_eq!(score1, 0);

    client.vouch(&borrower, &voucher, &1000);
    client.request_loan(&borrower, &5000, &0u64);
    client.slash(&admin, &borrower);

    let score2 = client.get_credit_score(&borrower);
    assert_eq!(score2, 0);
}

#[test]
fn test_credit_score_calculation_accuracy() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, token_addr, _token_admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    let token_client = token::Client::new(&env, &token_addr);
    token_client.mint(&voucher, &50000);
    token_client.mint(&env.current_contract_address(), &50000);

    client.vouch(&borrower, &voucher, &1000);
    client.request_loan(&borrower, &5000, &0u64);
    client.repay(&borrower);

    let score_1_0 = client.get_credit_score(&borrower);
    assert_eq!(score_1_0, 100);

    client.vouch(&borrower, &voucher, &1000);
    client.request_loan(&borrower, &5000, &0u64);
    client.repay(&borrower);

    let score_2_0 = client.get_credit_score(&borrower);
    assert_eq!(score_2_0, 100);

    client.vouch(&borrower, &voucher, &1000);
    client.request_loan(&borrower, &5000, &0u64);
    client.slash(&admin, &borrower);

    let score_2_1 = client.get_credit_score(&borrower);
    assert_eq!(score_2_1, 66);

    client.vouch(&borrower, &voucher, &1000);
    client.request_loan(&borrower, &5000, &0u64);
    client.slash(&admin, &borrower);

    let score_2_2 = client.get_credit_score(&borrower);
    assert_eq!(score_2_2, 50);
}

#[test]
fn test_credit_score_zero_for_new_borrower() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, _admin, _token, _token_admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let score = client.get_credit_score(&borrower);
    assert_eq!(score, 0);
}

#[test]
fn test_initialize_with_custom_yield_rate() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let deployer = Address::generate(&env);
    let admin = Address::generate(&env);

    let token_contract_id = env.register(token::Contract, ());
    let token_client = token::Client::new(&env, &token_contract_id);

    token_client.initialize(
        &admin,
        &18,
        &String::from_str(&env, "Test Token"),
        &String::from_str(&env, "TEST"),
    );

    let custom_yield_bps = 500;
    client.initialize(&deployer, &admin, &token_contract_id, &custom_yield_bps);

    let loan = client.get_loan(&admin);
    assert!(loan.is_none());
}

#[test]
fn test_repay_with_custom_yield_rate() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let deployer = Address::generate(&env);
    let admin = Address::generate(&env);

    let token_contract_id = env.register(token::Contract, ());
    let token_client = token::Client::new(&env, &token_contract_id);

    token_client.initialize(
        &admin,
        &18,
        &String::from_str(&env, "Test Token"),
        &String::from_str(&env, "TEST"),
    );

    let custom_yield_bps = 500;
    client.initialize(&deployer, &admin, &token_contract_id, &custom_yield_bps);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    token_client.mint(&voucher, &10000);
    token_client.mint(&env.current_contract_address(), &10000);

    client.vouch(&borrower, &voucher, &1000);
    client.request_loan(&borrower, &5000, &0u64);

    let voucher_balance_before = token_client.balance(&voucher);
    client.repay(&borrower);
    let voucher_balance_after = token_client.balance(&voucher);

    let yield_received = voucher_balance_after - voucher_balance_before;
    assert_eq!(yield_received, 5);
}

#[test]
fn test_yield_rate_zero_bps() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let deployer = Address::generate(&env);
    let admin = Address::generate(&env);

    let token_contract_id = env.register(token::Contract, ());
    let token_client = token::Client::new(&env, &token_contract_id);

    token_client.initialize(
        &admin,
        &18,
        &String::from_str(&env, "Test Token"),
        &String::from_str(&env, "TEST"),
    );

    let zero_yield_bps = 0;
    client.initialize(&deployer, &admin, &token_contract_id, &zero_yield_bps);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    token_client.mint(&voucher, &10000);
    token_client.mint(&env.current_contract_address(), &10000);

    client.vouch(&borrower, &voucher, &1000);
    client.request_loan(&borrower, &5000, &0u64);

    let voucher_balance_before = token_client.balance(&voucher);
    client.repay(&borrower);
    let voucher_balance_after = token_client.balance(&voucher);

    let yield_received = voucher_balance_after - voucher_balance_before;
    assert_eq!(yield_received, 0);
}
