//! Tests for reputation decay on update and admin quality bonuses (issue #1741).

use super::*;
use soroban_sdk::{testutils::Address as _, testutils::Ledger, Address, BytesN, Env};

const T0: u64 = 1_000;

fn setup<'a>(env: &'a Env) -> (EngineerRegistryClient<'a>, Address, Address, Address) {
    let deployer = Address::generate(env);
    let contract_id = env.register(EngineerRegistry, (&deployer,));
    let client = EngineerRegistryClient::new(env, &contract_id);
    let admin = Address::generate(env);
    client.initialize_admin(&deployer, &admin);
    let issuer = Address::generate(env);
    client.add_trusted_issuer(&admin, &issuer);
    let engineer = Address::generate(env);
    client.register_engineer(
        &engineer,
        &BytesN::from_array(env, &[7u8; 32]),
        &issuer,
        &31_536_000,
        &None,
    );
    (client, admin, issuer, engineer)
}

fn contract_err(code: ContractError) -> soroban_sdk::Error {
    soroban_sdk::Error::from_contract_error(code as u32)
}

#[test]
fn update_reputation_applies_decay_before_adding_delta() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let (client, _admin, _issuer, engineer) = setup(&env);

    client.update_reputation(&engineer, &500);
    assert_eq!(client.get_reputation(&engineer), 500);

    // Two full inactivity intervals at 5% each: 500 * 0.9 = 450 before the new delta.
    env.ledger()
        .set_timestamp(T0 + 2 * DEFAULT_DECAY_INTERVAL_SECS);
    assert_eq!(client.get_reputation(&engineer), 450);

    client.update_reputation(&engineer, &100);
    assert_eq!(client.get_reputation(&engineer), 550);
}

#[test]
fn quality_bonus_is_added_and_recorded() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let (client, admin, _issuer, engineer) = setup(&env);

    client.update_reputation(&engineer, &100);
    client.award_quality_bonus(&admin, &engineer, &20);
    assert_eq!(client.get_reputation(&engineer), 120);
}

#[test]
fn quality_bonus_is_capped_at_max_reputation() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let (client, admin, _issuer, engineer) = setup(&env);

    client.update_reputation(&engineer, &990);
    client.award_quality_bonus(&admin, &engineer, &MAX_QUALITY_BONUS);
    assert_eq!(client.get_reputation(&engineer), MAX_REPUTATION);
}

#[test]
fn quality_bonus_bounds_are_enforced() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let (client, admin, _issuer, engineer) = setup(&env);

    assert_eq!(
        client.try_award_quality_bonus(&admin, &engineer, &0),
        Err(Ok(contract_err(ContractError::InvalidReputationBonus)))
    );
    assert_eq!(
        client.try_award_quality_bonus(&admin, &engineer, &(MAX_QUALITY_BONUS + 1)),
        Err(Ok(contract_err(ContractError::InvalidReputationBonus)))
    );
    assert_eq!(client.get_reputation(&engineer), 0);
}

#[test]
fn quality_bonus_requires_admin() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let (client, _admin, _issuer, engineer) = setup(&env);

    let other = Address::generate(&env);
    assert_eq!(
        client.try_award_quality_bonus(&other, &engineer, &10),
        Err(Ok(contract_err(ContractError::UnauthorizedAdmin)))
    );
}

#[test]
fn quality_bonus_for_unknown_engineer_fails() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let (client, admin, _issuer, _engineer) = setup(&env);

    let unknown = Address::generate(&env);
    assert_eq!(
        client.try_award_quality_bonus(&admin, &unknown, &10),
        Err(Ok(contract_err(ContractError::EngineerNotFound)))
    );
}
