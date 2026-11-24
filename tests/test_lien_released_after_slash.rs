// tests/test_lien_released_after_slash.rs
//
// Issue #995 — asset lien is released when a loan is slashed
//
// Before this fix, slash() marked the loan as Defaulted but left the
// LienRecord in storage.  The asset remained locked as collateral
// indefinitely, blocking the owner from transferring or re-collateralizing it.
//
// Strategy
// --------
// 1. Record a lien against an asset via record_lien().
// 2. Slash the borrower's loan.
// 3. Assert that get_liens() returns an empty vec (lien removed).

use lending::{LendingContract, LendingContractClient};
use soroban_sdk::{
    testutils::Address as _,
    Address, Env,
};

fn setup_lending(env: &Env) -> (LendingContractClient, Address, Address) {
    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(env, &contract_id);

    let deployer = Address::generate(env);
    let admin = Address::generate(env);
    let token = Address::generate(env);

    env.mock_all_auths();
    client.initialize(&deployer, &admin, &token, &5000);

    (client, admin, token)
}

#[test]
fn test_lien_is_released_after_slash() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _token) = setup_lending(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);
    let lender = Address::generate(&env);
    let asset_id: u64 = 42;
    let loan_id: u64 = 1;

    // Record a lien (simulating collateral lockup before a slash event).
    client.record_lien(&admin, &asset_id, &lender, &loan_id, &1000);

    // Verify lien was stored.
    let liens_before = client.get_liens(&asset_id);
    assert_eq!(
        liens_before.len(),
        1,
        "one lien should exist before slash"
    );
    assert_eq!(liens_before.get(0).unwrap().loan_id, loan_id);

    // Set up and slash the loan.
    client.request_loan(&borrower, &0); // loan id = 1 is created
    client.vouch(&borrower, &voucher, &50);
    client.slash(&admin, &borrower);

    // Verify lien was released.
    let liens_after = client.get_liens(&asset_id);
    assert!(
        liens_after.is_empty(),
        "lien must be released after slash so the asset is no longer locked"
    );
}

#[test]
fn test_slash_without_lien_does_not_panic() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _token) = setup_lending(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    // No lien recorded — slash should still succeed without panicking.
    client.request_loan(&borrower, &0);
    client.vouch(&borrower, &voucher, &50);
    client.slash(&admin, &borrower);

    let loan = client.get_loan(&borrower).unwrap();
    assert_eq!(
        loan.status,
        lending::LoanStatus::Defaulted,
        "loan must be marked Defaulted even when no lien was recorded"
    );
}

#[test]
fn test_multiple_liens_only_matching_loan_id_is_released() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin, _token) = setup_lending(&env);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);
    let lender = Address::generate(&env);

    // Asset 10 has two separate lien records for two different loan IDs.
    let asset_id: u64 = 10;
    let loan_id_1: u64 = 1; // the one that will be slashed
    let loan_id_2: u64 = 99; // unrelated lien — must survive

    client.record_lien(&admin, &asset_id, &lender, &loan_id_1, &500);
    client.record_lien(&admin, &asset_id, &lender, &loan_id_2, &750);

    // Slash the loan that corresponds to loan_id_1.
    client.request_loan(&borrower, &0); // creates loan with id = 1
    client.vouch(&borrower, &voucher, &50);
    client.slash(&admin, &borrower);

    // loan_id_1 lien should be gone; loan_id_2 lien should remain.
    let liens_after = client.get_liens(&asset_id);
    assert_eq!(
        liens_after.len(),
        1,
        "only the slashed lien should be removed; unrelated lien must remain"
    );
    assert_eq!(
        liens_after.get(0).unwrap().loan_id,
        loan_id_2,
        "the surviving lien must be the unrelated one (loan_id = 99)"
    );
}
