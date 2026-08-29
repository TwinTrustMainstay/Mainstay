use lending::{LendingContract, LendingContractClient};
use soroban_sdk::{testutils::Address as _, Address, Env};

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
fn test_pause_disables_vouch() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

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
    let (client, admin) = setup_contract(&env);

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
    let (client, admin) = setup_contract(&env);

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
    let (client, admin) = setup_contract(&env);

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
fn test_pause_only_by_admin() {
    let env = Env::default();
    let (client, _admin) = setup_contract(&env);

    let non_admin = Address::generate(&env);

    env.mock_all_auths();

    // Non-admin tries to pause - should fail
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.pause(&non_admin);
    }));
    assert!(result.is_err());
}

#[test]
fn test_unpause_only_by_admin() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

    let non_admin = Address::generate(&env);

    env.mock_all_auths();

    // Admin pauses
    client.pause(&admin);

    // Non-admin tries to unpause - should fail
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.unpause(&non_admin);
    }));
    assert!(result.is_err());

    // Admin can unpause
    client.unpause(&admin);
}
