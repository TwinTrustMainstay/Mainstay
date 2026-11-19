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

// ── #1864: syndicate votes are recorded, de-duplicated, tallied and enforced ──

fn syndicate_setup(
    env: &Env,
    client: &LendingContractClient,
    admin: &Address,
    loan_id: u64,
    lenders: &[Address],
    quorum: u32,
) {
    let mut members = soroban_sdk::Vec::new(env);
    let mut shares = soroban_sdk::Vec::new(env);
    for l in lenders {
        members.push_back(l.clone());
        shares.push_back(1u64);
    }
    client.create_syndicate(admin, &loan_id, &members, &shares, &quorum);
}

#[test]
fn syndicate_vote_is_recorded_and_tallied() {
    let env = Env::default();
    let (contract_id, admin, _token) = setup(&env);
    let client = LendingContractClient::new(&env, &contract_id);

    let (_borrower, loan_id) = active_loan(&env, &client, 1_000);
    let l1 = Address::generate(&env);
    let l2 = Address::generate(&env);
    let l3 = Address::generate(&env);
    syndicate_setup(&env, &client, &admin, loan_id, &[l1.clone(), l2.clone(), l3.clone()], 2);
    let action = soroban_sdk::symbol_short!("RESTRUCT");

    client.syndicate_vote(&l1, &loan_id, &action, &true);
    client.syndicate_vote(&l2, &loan_id, &action, &false);
    client.syndicate_vote(&l3, &loan_id, &action, &true);

    // Only approving votes are tallied; the rejection is recorded but not counted.
    assert_eq!(client.get_syndicate_approvals(&loan_id, &action), 2);
}

#[test]
fn syndicate_lender_cannot_vote_twice() {
    let env = Env::default();
    let (contract_id, admin, _token) = setup(&env);
    let client = LendingContractClient::new(&env, &contract_id);

    let (_borrower, loan_id) = active_loan(&env, &client, 1_000);
    let l1 = Address::generate(&env);
    syndicate_setup(&env, &client, &admin, loan_id, &[l1.clone()], 1);
    let action = soroban_sdk::symbol_short!("RESTRUCT");

    client.syndicate_vote(&l1, &loan_id, &action, &true);
    assert!(client.try_syndicate_vote(&l1, &loan_id, &action, &true).is_err());
    assert!(client.try_syndicate_vote(&l1, &loan_id, &action, &false).is_err());

    assert_eq!(client.get_syndicate_approvals(&loan_id, &action), 1);
}

#[test]
fn non_member_cannot_vote_in_syndicate() {
    let env = Env::default();
    let (contract_id, admin, _token) = setup(&env);
    let client = LendingContractClient::new(&env, &contract_id);

    let (_borrower, loan_id) = active_loan(&env, &client, 1_000);
    let l1 = Address::generate(&env);
    let outsider = Address::generate(&env);
    syndicate_setup(&env, &client, &admin, loan_id, &[l1], 1);
    let action = soroban_sdk::symbol_short!("RESTRUCT");

    assert!(client
        .try_syndicate_vote(&outsider, &loan_id, &action, &true)
        .is_err());
    assert_eq!(client.get_syndicate_approvals(&loan_id, &action), 0);
}

#[test]
fn restructure_blocked_until_syndicate_quorum_reached() {
    let env = Env::default();
    let (contract_id, admin, _token) = setup(&env);
    let client = LendingContractClient::new(&env, &contract_id);

    let (borrower, loan_id) = active_loan(&env, &client, 1_000);
    let l1 = Address::generate(&env);
    let l2 = Address::generate(&env);
    let l3 = Address::generate(&env);
    syndicate_setup(&env, &client, &admin, loan_id, &[l1.clone(), l2.clone(), l3.clone()], 2);
    let action = soroban_sdk::symbol_short!("RESTRUCT");

    let new_deadline = env.ledger().timestamp() + 10_000;
    client.propose_loan_restructure(&borrower, &loan_id, &new_deadline, &600);

    // One approval and one rejection: quorum of 2 not reached.
    client.syndicate_vote(&l1, &loan_id, &action, &true);
    client.syndicate_vote(&l2, &loan_id, &action, &false);
    assert!(client.try_accept_restructure(&l1, &loan_id).is_err());

    // A second approval reaches quorum; a member can now accept.
    client.syndicate_vote(&l3, &loan_id, &action, &true);
    assert_eq!(client.get_syndicate_approvals(&loan_id, &action), 2);
    client.accept_restructure(&l1, &loan_id);
    assert_eq!(client.get_loan(&borrower).unwrap().deadline, new_deadline);
}
