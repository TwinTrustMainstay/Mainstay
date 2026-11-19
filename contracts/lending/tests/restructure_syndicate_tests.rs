#![cfg(test)]

use lending::{LendingContract, LendingContractClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env,
};

/// Registers the lending contract with a real Stellar asset as its token and
/// funds the contract so loans can be disbursed.
fn setup(env: &Env) -> (Address, Address, Address) {
    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(env, &contract_id);

    let deployer = Address::generate(env);
    let admin = Address::generate(env);
    let token_admin = Address::generate(env);
    let token_id = env.register_stellar_asset_contract(token_admin.clone());

    env.mock_all_auths();
    client.initialize(&deployer, &admin, &token_id, &0);

    let asset = token::StellarAssetClient::new(env, &token_id);
    asset.mint(&contract_id, &1_000_000);

    (contract_id, admin, token_id)
}

/// Creates an active loan for a fresh borrower and returns (borrower, loan_id).
fn active_loan(env: &Env, client: &LendingContractClient, amount: u64) -> (Address, u64) {
    let borrower = Address::generate(env);
    client.request_loan(&borrower, &amount, &0u64);
    let loan = client.get_loan(&borrower).expect("loan must exist");
    (borrower, loan.id)
}

// ── #1861: accept_restructure cannot be replayed ──

#[test]
fn accept_restructure_cannot_be_replayed() {
    let env = Env::default();
    let (contract_id, admin, _token) = setup(&env);
    let client = LendingContractClient::new(&env, &contract_id);

    let (borrower, loan_id) = active_loan(&env, &client, 1_000);
    let new_deadline = env.ledger().timestamp() + 10_000;
    client.propose_loan_restructure(&borrower, &loan_id, &new_deadline, &500);

    client.accept_restructure(&admin, &loan_id);

    let stored = client.get_restructure(&loan_id).expect("proposal kept");
    assert_eq!(stored.status, 1, "accepted proposal must be marked accepted");

    // A second accept of the same proposal must fail.
    let replay = client.try_accept_restructure(&admin, &loan_id);
    assert!(replay.is_err(), "replaying an accepted restructure must fail");
}

// ── #1862: reduced_payment_amount is applied to the loan on accept ──

#[test]
fn accepted_restructure_applies_reduced_payment_amount() {
    let env = Env::default();
    let (contract_id, admin, _token) = setup(&env);
    let client = LendingContractClient::new(&env, &contract_id);

    let (borrower, loan_id) = active_loan(&env, &client, 1_000);
    let new_deadline = env.ledger().timestamp() + 10_000;
    client.propose_loan_restructure(&borrower, &loan_id, &new_deadline, &600);
    client.accept_restructure(&admin, &loan_id);

    let loan = client.get_loan(&borrower).unwrap();
    assert_eq!(loan.amount, 600, "amount due must be the restructured payment");
    assert_eq!(loan.deadline, new_deadline);
}

#[test]
fn repay_after_restructure_settles_against_reduced_amount() {
    let env = Env::default();
    let (contract_id, admin, _token) = setup(&env);
    let client = LendingContractClient::new(&env, &contract_id);

    let (borrower, loan_id) = active_loan(&env, &client, 1_000);
    let new_deadline = env.ledger().timestamp() + 10_000;
    client.propose_loan_restructure(&borrower, &loan_id, &new_deadline, &600);
    client.accept_restructure(&admin, &loan_id);

    client.repay(&borrower);

    let loan = client.get_loan(&borrower).unwrap();
    assert_eq!(loan.status, lending::LoanStatus::Repaid);
    assert_eq!(loan.amount, 600);
}

#[test]
fn restructure_rejects_amount_above_loan_amount() {
    let env = Env::default();
    let (contract_id, admin, _token) = setup(&env);
    let client = LendingContractClient::new(&env, &contract_id);

    let (borrower, loan_id) = active_loan(&env, &client, 1_000);
    let new_deadline = env.ledger().timestamp() + 10_000;
    client.propose_loan_restructure(&borrower, &loan_id, &new_deadline, &1_001);

    assert!(client.try_accept_restructure(&admin, &loan_id).is_err());
    assert_eq!(client.get_loan(&borrower).unwrap().amount, 1_000);
}

#[test]
fn restructure_rejects_zero_payment_amount() {
    let env = Env::default();
    let (contract_id, admin, _token) = setup(&env);
    let client = LendingContractClient::new(&env, &contract_id);

    let (borrower, loan_id) = active_loan(&env, &client, 1_000);
    let new_deadline = env.ledger().timestamp() + 10_000;
    client.propose_loan_restructure(&borrower, &loan_id, &new_deadline, &0);

    assert!(client.try_accept_restructure(&admin, &loan_id).is_err());
    assert_eq!(client.get_loan(&borrower).unwrap().amount, 1_000);
}

// ── #1863: proposals bind to the active loan and forbearance blocks slashing ──

#[test]
fn restructure_proposal_must_match_active_loan() {
    let env = Env::default();
    let (contract_id, _admin, _token) = setup(&env);
    let client = LendingContractClient::new(&env, &contract_id);

    let (borrower, loan_id) = active_loan(&env, &client, 1_000);
    let new_deadline = env.ledger().timestamp() + 10_000;
    let wrong_loan_id = loan_id + 999;

    assert!(client
        .try_propose_loan_restructure(&borrower, &wrong_loan_id, &new_deadline, &500)
        .is_err());
    assert!(client.get_restructure(&wrong_loan_id).is_none());
}

#[test]
fn accepted_restructure_blocks_slash_during_forbearance() {
    let env = Env::default();
    let (contract_id, admin, _token) = setup(&env);
    let client = LendingContractClient::new(&env, &contract_id);

    let (borrower, loan_id) = active_loan(&env, &client, 1_000);
    let new_deadline = env.ledger().timestamp() + 10_000;
    client.propose_loan_restructure(&borrower, &loan_id, &new_deadline, &600);
    client.accept_restructure(&admin, &loan_id);

    assert!(client.try_slash(&admin, &borrower).is_err());
    assert_eq!(
        client.get_loan(&borrower).unwrap().status,
        lending::LoanStatus::Active
    );
}

#[test]
fn slash_allowed_after_forbearance_expires() {
    let env = Env::default();
    let (contract_id, admin, _token) = setup(&env);
    let client = LendingContractClient::new(&env, &contract_id);

    let (borrower, loan_id) = active_loan(&env, &client, 1_000);
    let new_deadline = env.ledger().timestamp() + 10_000;
    client.propose_loan_restructure(&borrower, &loan_id, &new_deadline, &600);
    client.accept_restructure(&admin, &loan_id);

    let forbearance_end = client.get_restructure(&loan_id).unwrap().forbearance_end;
    env.ledger().with_mut(|li| li.timestamp = forbearance_end);

    client.slash(&admin, &borrower);
    assert_eq!(
        client.get_loan(&borrower).unwrap().status,
        lending::LoanStatus::Defaulted
    );
}

#[test]
fn slash_allowed_when_restructure_only_proposed() {
    let env = Env::default();
    let (contract_id, admin, _token) = setup(&env);
    let client = LendingContractClient::new(&env, &contract_id);

    let (borrower, loan_id) = active_loan(&env, &client, 1_000);
    let new_deadline = env.ledger().timestamp() + 10_000;
    client.propose_loan_restructure(&borrower, &loan_id, &new_deadline, &600);

    client.slash(&admin, &borrower);
    assert_eq!(
        client.get_loan(&borrower).unwrap().status,
        lending::LoanStatus::Defaulted
    );
}
