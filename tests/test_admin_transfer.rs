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
fn test_propose_admin_two_step_transfer() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

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
    let (client, admin) = setup_contract(&env);

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
fn test_propose_admin_only_by_current_admin() {
    let env = Env::default();
    let (client, _admin) = setup_contract(&env);

    let non_admin = Address::generate(&env);
    let new_admin = Address::generate(&env);

    env.mock_all_auths();

    // Non-admin tries to propose - should fail
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.propose_admin(&non_admin, &new_admin);
    }));
    assert!(result.is_err());
}

#[test]
fn test_multiple_admin_transfers() {
    let env = Env::default();
    let (client, admin1) = setup_contract(&env);

    let admin2 = Address::generate(&env);
    let admin3 = Address::generate(&env);

    env.mock_all_auths();

    // First transfer: admin1 -> admin2
    client.propose_admin(&admin1, &admin2);
    client.accept_admin(&admin2);

    // Verify admin2 is now admin
    let borrower = Address::generate(&env);
    client.request_loan(&borrower, &1000, &0u64);
    client.pause(&admin2);
    client.unpause(&admin2);

    // Second transfer: admin2 -> admin3
    client.propose_admin(&admin2, &admin3);
    client.accept_admin(&admin3);

    // Verify admin3 is now admin
    client.pause(&admin3);

    // admin2 should not be able to unpause
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.unpause(&admin2);
    }));
    assert!(result.is_err());

    // admin3 can unpause
    client.unpause(&admin3);
}

#[test]
fn test_pending_admin_cleared_after_acceptance() {
    let env = Env::default();
    let (client, admin) = setup_contract(&env);

    let new_admin = Address::generate(&env);
    let another_admin = Address::generate(&env);

    env.mock_all_auths();

    // Propose new admin
    client.propose_admin(&admin, &new_admin);

    // New admin accepts
    client.accept_admin(&new_admin);

    // Try to propose another admin with old pending admin - should fail
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.accept_admin(&another_admin);
    }));
    assert!(result.is_err());
}
