use lending::{LendingContract, LendingContractClient, LoanStatus};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env,
};

fn setup_contract(env: &Env) -> (LendingContractClient, Address) {
    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(env, &contract_id);

    let deployer = Address::generate(env);
    let admin = Address::generate(env);
    let token = Address::generate(env);

    env.mock_all_auths();
    client.initialize(&deployer, &admin, &token, &5000);

    (client, admin)
}

#[test]
fn test_loan_deadline_set_on_request() {
    let env = Env::default();
    let (client, _admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    env.mock_all_auths();

    let initial_time = env.ledger().timestamp();
    client.request_loan(&borrower, &1000, &0u64);

    let loan = client.get_loan(&borrower).unwrap();
    // Default duration is 2_592_000 seconds (30 days)
    assert_eq!(loan.deadline, initial_time + 2_592_000);
}

#[test]
fn test_auto_slash_after_deadline() {
    let env = Env::default();
    let (client, _admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    env.mock_all_auths();
    client.request_loan(&borrower, &1000, &0u64);
    client.vouch(&borrower, &voucher, &100);

    let loan = client.get_loan(&borrower).unwrap();
    let deadline = loan.deadline;

    // Try to auto_slash before deadline - should fail
    env.ledger().set_timestamp(deadline - 1);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.auto_slash(&borrower);
    }));
    assert!(result.is_err());

    // Move past deadline
    env.ledger().set_timestamp(deadline + 1);
    client.auto_slash(&borrower);

    // Loan should be defaulted
    let loan = client.get_loan(&borrower).unwrap();
    assert_eq!(loan.status, LoanStatus::Defaulted);

    // Slash balance should be 50 (50% of 100)
    assert_eq!(client.get_slash_balance(), 50);
}

#[test]
fn test_auto_slash_callable_by_anyone() {
    let env = Env::default();
    let (client, _admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    env.mock_all_auths();
    client.request_loan(&borrower, &1000, &0u64);
    client.vouch(&borrower, &voucher, &100);

    let loan = client.get_loan(&borrower).unwrap();
    env.ledger().set_timestamp(loan.deadline + 1);

    // Anyone can call auto_slash
    client.auto_slash(&borrower);

    let loan = client.get_loan(&borrower).unwrap();
    assert_eq!(loan.status, LoanStatus::Defaulted);
}

#[test]
fn test_deadline_prevents_repay_after_slash() {
    let env = Env::default();
    let (client, _admin) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    env.mock_all_auths();
    client.request_loan(&borrower, &1000, &0u64);
    client.vouch(&borrower, &voucher, &100);

    let loan = client.get_loan(&borrower).unwrap();
    env.ledger().set_timestamp(loan.deadline + 1);

    // Auto-slash after deadline
    client.auto_slash(&borrower);

    // Try to repay - should fail because loan is defaulted
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.repay(&borrower);
    }));
    assert!(result.is_err());
}
