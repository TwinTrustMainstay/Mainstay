use lending::{ContractError, LendingContract, LendingContractClient, LoanStatus};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, IntoVal,
};

fn setup_contract(env: &Env) -> (LendingContractClient, Address, Address, Address) {
    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(env, &contract_id);

    let deployer = Address::generate(env);
    let admin = Address::generate(env);
    let token = Address::generate(env);

    env.mock_all_auths();
    client.initialize(&deployer, &admin, &token, &5000);

    (client, admin, token, deployer)
}

#[test]
fn test_configurable_slash_rate_at_initialize() {
    let env = Env::default();
    let (client, admin, _token, _deployer) = setup_contract(&env);

    // Verify slash rate was set to 5000 (50%)
    // This is tested indirectly through slash behavior
    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    env.mock_all_auths();
    client.request_loan(&borrower, &1000, &0u64);
    client.vouch(&borrower, &voucher, &100);
    client.slash(&admin, &borrower);

    // With 5000 bps (50%), 50 should be slashed, 50 returned
    // Slash balance should be 50
    assert_eq!(client.get_slash_balance(), 50);
}

#[test]
fn test_loan_deadline_set_on_request() {
    let env = Env::default();
    let (client, _admin, _token, _deployer) = setup_contract(&env);

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
    let (client, admin, _token, _deployer) = setup_contract(&env);

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
fn test_pause_disables_vouch() {
    let env = Env::default();
    let (client, admin, _token, _deployer) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    env.mock_all_auths();
    client.request_loan(&borrower, &1000, &0u64);

    // Pause the contract
    client.pause(&admin);

    // Try to vouch - should fail
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.vouch(&borrower, &voucher, &100);
    }));
    assert!(result.is_err());
}

#[test]
fn test_pause_disables_request_loan() {
    let env = Env::default();
    let (client, admin, _token, _deployer) = setup_contract(&env);

    let borrower = Address::generate(&env);

    env.mock_all_auths();

    // Pause the contract
    client.pause(&admin);

    // Try to request loan - should fail
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.request_loan(&borrower, &1000, &0u64);
    }));
    assert!(result.is_err());
}

#[test]
fn test_pause_disables_repay() {
    let env = Env::default();
    let (client, admin, _token, _deployer) = setup_contract(&env);

    let borrower = Address::generate(&env);

    env.mock_all_auths();
    client.request_loan(&borrower, &1000, &0u64);

    // Pause the contract
    client.pause(&admin);

    // Try to repay - should fail
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.repay(&borrower);
    }));
    assert!(result.is_err());
}

#[test]
fn test_unpause_restores_functionality() {
    let env = Env::default();
    let (client, admin, _token, _deployer) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    env.mock_all_auths();

    // Pause and unpause
    client.pause(&admin);
    client.unpause(&admin);

    // Should be able to request loan
    client.request_loan(&borrower, &1000, &0u64);

    // Should be able to vouch
    client.vouch(&borrower, &voucher, &100);

    assert_eq!(client.get_vouches(&borrower).len(), 1);
}

#[test]
fn test_propose_admin_two_step_transfer() {
    let env = Env::default();
    let (client, admin, _token, _deployer) = setup_contract(&env);

    let new_admin = Address::generate(&env);

    env.mock_all_auths();

    // Current admin proposes new admin
    client.propose_admin(&admin, &new_admin);

    // New admin accepts
    client.accept_admin(&new_admin);

    // Verify new admin is now the admin by calling an admin function
    let borrower = Address::generate(&env);
    client.request_loan(&borrower, &1000, &0u64);
    client.pause(&new_admin);

    // Old admin should not be able to unpause
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.unpause(&admin);
    }));
    assert!(result.is_err());

    // New admin can unpause
    client.unpause(&new_admin);
}

#[test]
fn test_accept_admin_only_by_pending_admin() {
    let env = Env::default();
    let (client, admin, _token, _deployer) = setup_contract(&env);

    let new_admin = Address::generate(&env);
    let other = Address::generate(&env);

    env.mock_all_auths();

    // Propose new admin
    client.propose_admin(&admin, &new_admin);

    // Someone else tries to accept - should fail
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.accept_admin(&other);
    }));
    assert!(result.is_err());

    // Correct pending admin accepts
    client.accept_admin(&new_admin);
}

#[test]
fn test_pause_only_by_admin() {
    let env = Env::default();
    let (client, admin, _token, _deployer) = setup_contract(&env);

    let non_admin = Address::generate(&env);

    env.mock_all_auths();

    // Non-admin tries to pause - should fail
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.pause(&non_admin);
    }));
    assert!(result.is_err());

    // Admin can pause
    client.pause(&admin);
}

#[test]
fn test_auto_slash_callable_by_anyone() {
    let env = Env::default();
    let (client, _admin, _token, _deployer) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);
    let anyone = Address::generate(&env);

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
fn test_slash_rate_applied_correctly() {
    let env = Env::default();
    let (client, admin, _token, _deployer) = setup_contract(&env);

    let borrower = Address::generate(&env);
    let voucher1 = Address::generate(&env);
    let voucher2 = Address::generate(&env);

    env.mock_all_auths();
    client.request_loan(&borrower, &1000, &0u64);
    client.vouch(&borrower, &voucher1, &200);
    client.vouch(&borrower, &voucher2, &300);

    client.slash(&admin, &borrower);

    // With 5000 bps (50%):
    // voucher1: 200 * 50% = 100 slashed
    // voucher2: 300 * 50% = 150 slashed
    // Total: 250
    assert_eq!(client.get_slash_balance(), 250);
}

#[test]
fn test_deadline_prevents_repay_after_slash() {
    let env = Env::default();
    let (client, admin, _token, _deployer) = setup_contract(&env);

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
