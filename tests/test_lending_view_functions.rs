use lending::{LendingContract, LendingContractClient, LoanStatus};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, String,
};

#[test]
fn test_get_collateral_value_applies_score_and_ltv() {
    let env = Env::default();
    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    // 10,000 × 80% score × 75% LTV = 6,000.
    assert_eq!(client.get_collateral_value(&10_000, &80, &7_500), 6_000);
}

#[test]
fn test_get_collateral_value_floors_and_clamps_inputs() {
    let env = Env::default();
    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    // Score and LTV are bounded; integer arithmetic floors the result.
    assert_eq!(client.get_collateral_value(&101, &150, &20_000), 101);
    assert_eq!(client.get_collateral_value(&99, &50, &5_000), 24);
}

#[test]
fn test_loan_status_none() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let deployer = Address::generate(&env);
    let borrower = Address::generate(&env);

    client.initialize(&deployer, &admin, &token);

    let status = client.loan_status(&borrower);
    assert_eq!(status, LoanStatus::None);
}

#[test]
fn test_loan_status_active() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let deployer = Address::generate(&env);
    let borrower = Address::generate(&env);

    client.initialize(&deployer, &admin, &token);
    client.request_loan(&borrower, &1000u64, &0u64);

    let status = client.loan_status(&borrower);
    assert_eq!(status, LoanStatus::Active);
}

#[test]
fn test_loan_status_repaid() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let deployer = Address::generate(&env);
    let borrower = Address::generate(&env);

    client.initialize(&deployer, &admin, &token);
    client.request_loan(&borrower, &1000u64, &0u64);

    let status = client.loan_status(&borrower);
    assert_eq!(status, LoanStatus::Active);

    client.repay(&borrower);

    let status = client.loan_status(&borrower);
    assert_eq!(status, LoanStatus::Repaid);
}

#[test]
fn test_loan_status_defaulted() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let deployer = Address::generate(&env);
    let borrower = Address::generate(&env);

    client.initialize(&deployer, &admin, &token);
    client.request_loan(&borrower, &1000u64, &0u64);

    let status = client.loan_status(&borrower);
    assert_eq!(status, LoanStatus::Active);

    client.slash(&admin, &borrower);

    let status = client.loan_status(&borrower);
    assert_eq!(status, LoanStatus::Defaulted);
}

#[test]
fn test_vouch_exists_false() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let deployer = Address::generate(&env);
    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    client.initialize(&deployer, &admin, &token);

    let exists = client.vouch_exists(&voucher, &borrower);
    assert!(!exists);
}

#[test]
fn test_vouch_exists_true() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let deployer = Address::generate(&env);
    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    client.initialize(&deployer, &admin, &token);
    client.vouch(&borrower, &voucher, &100u64);

    let exists = client.vouch_exists(&voucher, &borrower);
    assert!(exists);
}

#[test]
fn test_vouch_exists_multiple_vouchers() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let deployer = Address::generate(&env);
    let borrower = Address::generate(&env);
    let voucher1 = Address::generate(&env);
    let voucher2 = Address::generate(&env);
    let voucher3 = Address::generate(&env);

    client.initialize(&deployer, &admin, &token);
    client.vouch(&borrower, &voucher1, &100u64);
    client.vouch(&borrower, &voucher2, &200u64);

    assert!(client.vouch_exists(&voucher1, &borrower));
    assert!(client.vouch_exists(&voucher2, &borrower));
    assert!(!client.vouch_exists(&voucher3, &borrower));
}

#[test]
fn test_total_vouched_zero() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let deployer = Address::generate(&env);
    let borrower = Address::generate(&env);

    client.initialize(&deployer, &admin, &token);

    let total = client.total_vouched(&borrower);
    assert_eq!(total, 0i128);
}

#[test]
fn test_total_vouched_single_voucher() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let deployer = Address::generate(&env);
    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    client.initialize(&deployer, &admin, &token);
    client.vouch(&borrower, &voucher, &500u64);

    let total = client.total_vouched(&borrower);
    assert_eq!(total, 500i128);
}

#[test]
fn test_total_vouched_multiple_vouchers() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let deployer = Address::generate(&env);
    let borrower = Address::generate(&env);
    let voucher1 = Address::generate(&env);
    let voucher2 = Address::generate(&env);
    let voucher3 = Address::generate(&env);

    client.initialize(&deployer, &admin, &token);
    client.vouch(&borrower, &voucher1, &100u64);
    client.vouch(&borrower, &voucher2, &250u64);
    client.vouch(&borrower, &voucher3, &150u64);

    let total = client.total_vouched(&borrower);
    assert_eq!(total, 500i128);
}

#[test]
fn test_is_eligible_below_threshold() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let deployer = Address::generate(&env);
    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    client.initialize(&deployer, &admin, &token);
    client.vouch(&borrower, &voucher, &100u64);

    let eligible = client.is_eligible(&borrower, &500i128);
    assert!(!eligible);
}

#[test]
fn test_is_eligible_at_threshold() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let deployer = Address::generate(&env);
    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    client.initialize(&deployer, &admin, &token);
    client.vouch(&borrower, &voucher, &500u64);

    let eligible = client.is_eligible(&borrower, &500i128);
    assert!(eligible);
}

#[test]
fn test_is_eligible_above_threshold() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let deployer = Address::generate(&env);
    let borrower = Address::generate(&env);
    let voucher1 = Address::generate(&env);
    let voucher2 = Address::generate(&env);

    client.initialize(&deployer, &admin, &token);
    client.vouch(&borrower, &voucher1, &300u64);
    client.vouch(&borrower, &voucher2, &300u64);

    let eligible = client.is_eligible(&borrower, &500i128);
    assert!(eligible);
}

#[test]
fn test_is_eligible_no_vouches() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let deployer = Address::generate(&env);
    let borrower = Address::generate(&env);

    client.initialize(&deployer, &admin, &token);

    let eligible = client.is_eligible(&borrower, &100i128);
    assert!(!eligible);
}
