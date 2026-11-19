//! Tests for syndicate creation validation (#1865).

#![cfg(test)]

use lending::{ContractError, LendingContract, LendingContractClient};
use soroban_sdk::{testutils::Address as _, vec, Address, Env, Vec};

fn setup_contract(env: &Env) -> LendingContractClient {
    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(env, &contract_id);

    let deployer = Address::generate(env);
    let admin = Address::generate(env);
    let token_id = env.register_stellar_asset_contract(Address::generate(env));

    env.mock_all_auths();
    client.initialize(&deployer, &admin, &token_id, &0);
    client
}

fn lenders(env: &Env, n: usize) -> Vec<Address> {
    let mut v = Vec::new(env);
    for _ in 0..n {
        v.push_back(Address::generate(env));
    }
    v
}

#[test]
fn test_create_syndicate_stores_valid_syndicate() {
    let env = Env::default();
    let client = setup_contract(&env);
    let admin = client.get_admin();

    let l = lenders(&env, 2);
    client.create_syndicate(&admin, &7, &l, &vec![&env, 60u64, 40u64], &2);

    let stored = client.get_syndicate(&7).unwrap();
    assert_eq!(stored.total_shares, 100);
    assert_eq!(stored.quorum_required, 2);
}

#[test]
fn test_create_syndicate_rejects_existing_loan_and_keeps_original() {
    let env = Env::default();
    let client = setup_contract(&env);
    let admin = client.get_admin();

    let original = lenders(&env, 2);
    client.create_syndicate(&admin, &7, &original, &vec![&env, 60u64, 40u64], &1);

    let replacement = lenders(&env, 3);
    let err = client
        .try_create_syndicate(&admin, &7, &replacement, &vec![&env, 1u64, 1u64, 1u64], &3)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::SyndicateAlreadyExists.into());

    let stored = client.get_syndicate(&7).unwrap();
    assert_eq!(stored.lenders, original);
    assert_eq!(stored.total_shares, 100);
}

#[test]
fn test_create_syndicate_rejects_length_mismatch() {
    let env = Env::default();
    let client = setup_contract(&env);
    let admin = client.get_admin();

    let err = client
        .try_create_syndicate(&admin, &1, &lenders(&env, 2), &vec![&env, 50u64], &1)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::SyndicateLengthMismatch.into());
}

#[test]
fn test_create_syndicate_rejects_out_of_range_quorum() {
    let env = Env::default();
    let client = setup_contract(&env);
    let admin = client.get_admin();

    let zero = client
        .try_create_syndicate(&admin, &1, &lenders(&env, 2), &vec![&env, 1u64, 1u64], &0)
        .unwrap_err()
        .unwrap();
    assert_eq!(zero, ContractError::InvalidQuorum.into());

    let too_high = client
        .try_create_syndicate(&admin, &1, &lenders(&env, 2), &vec![&env, 1u64, 1u64], &3)
        .unwrap_err()
        .unwrap();
    assert_eq!(too_high, ContractError::InvalidQuorum.into());
}

#[test]
fn test_create_syndicate_rejects_zero_share() {
    let env = Env::default();
    let client = setup_contract(&env);
    let admin = client.get_admin();

    let err = client
        .try_create_syndicate(&admin, &1, &lenders(&env, 2), &vec![&env, 10u64, 0u64], &1)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::ZeroSyndicateShare.into());
}
