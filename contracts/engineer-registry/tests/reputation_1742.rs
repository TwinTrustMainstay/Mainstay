//! Tests for #1742: engineer reputation decay, quality bonuses and caps.
//!
//! Verifies that:
//! - Inactivity decay is applied before a delta is added, so decay is not lost on write.
//! - `award_quality_bonus` adds points after applying pending decay.
//! - Reputation is capped at 1000 for both deltas and bonuses.
//! - Bonuses are admin-only and bounded to `1..=50`.

use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, BytesN, Env,
};

const NINETY_DAYS: u64 = 90 * 86_400;

struct Ctx<'a> {
    env: Env,
    client: EngineerRegistryClient<'a>,
    admin: Address,
    engineer: Address,
}

fn setup<'a>() -> Ctx<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let deployer = Address::generate(&env);
    let contract_id = env.register(EngineerRegistry, (&deployer,));
    let client = EngineerRegistryClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize_admin(&deployer, &admin);

    let issuer = Address::generate(&env);
    client.add_trusted_issuer(&admin, &issuer);

    let engineer = Address::generate(&env);
    client.register_engineer(
        &engineer,
        &BytesN::from_array(&env, &[7u8; 32]),
        &issuer,
        &31_536_000,
        &None,
    );

    Ctx {
        env,
        client,
        admin,
        engineer,
    }
}

fn advance(env: &Env, secs: u64) {
    env.ledger().set_timestamp(env.ledger().timestamp() + secs);
}

#[test]
fn test_update_reputation_preserves_pending_decay() {
    let ctx = setup();
    ctx.client.update_reputation(&ctx.engineer, &500);

    // One full inactivity interval: 5% decay → 500 becomes 475.
    advance(&ctx.env, NINETY_DAYS);
    assert_eq!(ctx.client.get_reputation(&ctx.engineer), 475);

    // Adding 100 must start from the decayed 475, not the stored 500.
    ctx.client.update_reputation(&ctx.engineer, &100);
    assert_eq!(ctx.client.get_reputation(&ctx.engineer), 575);
}

#[test]
fn test_quality_bonus_adds_points() {
    let ctx = setup();
    ctx.client.update_reputation(&ctx.engineer, &200);

    ctx.client
        .award_quality_bonus(&ctx.admin, &ctx.engineer, &25);
    assert_eq!(ctx.client.get_reputation(&ctx.engineer), 225);
}

#[test]
fn test_quality_bonus_applies_pending_decay_first() {
    let ctx = setup();
    ctx.client.update_reputation(&ctx.engineer, &500);

    // Two intervals: 500 * (1 - 0.10) = 450, then +10 bonus = 460.
    advance(&ctx.env, 2 * NINETY_DAYS);
    ctx.client
        .award_quality_bonus(&ctx.admin, &ctx.engineer, &10);
    assert_eq!(ctx.client.get_reputation(&ctx.engineer), 460);
}

#[test]
fn test_quality_bonus_capped_at_max_reputation() {
    let ctx = setup();
    ctx.client.update_reputation(&ctx.engineer, &990);

    ctx.client
        .award_quality_bonus(&ctx.admin, &ctx.engineer, &50);
    assert_eq!(ctx.client.get_reputation(&ctx.engineer), 1000);
}

#[test]
fn test_update_reputation_capped_at_max_reputation() {
    let ctx = setup();
    ctx.client.update_reputation(&ctx.engineer, &1500);
    assert_eq!(ctx.client.get_reputation(&ctx.engineer), 1000);
}

#[test]
fn test_quality_bonus_accepts_maximum_bonus() {
    let ctx = setup();
    ctx.client
        .award_quality_bonus(&ctx.admin, &ctx.engineer, &50);
    assert_eq!(ctx.client.get_reputation(&ctx.engineer), 50);
}

#[test]
fn test_quality_bonus_rejects_zero() {
    let ctx = setup();
    assert!(ctx
        .client
        .try_award_quality_bonus(&ctx.admin, &ctx.engineer, &0)
        .is_err());
    assert_eq!(ctx.client.get_reputation(&ctx.engineer), 0);
}

#[test]
fn test_quality_bonus_rejects_above_maximum() {
    let ctx = setup();
    assert!(ctx
        .client
        .try_award_quality_bonus(&ctx.admin, &ctx.engineer, &51)
        .is_err());
    assert_eq!(ctx.client.get_reputation(&ctx.engineer), 0);
}

#[test]
fn test_quality_bonus_rejects_non_admin() {
    let ctx = setup();
    let impostor = Address::generate(&ctx.env);
    assert!(ctx
        .client
        .try_award_quality_bonus(&impostor, &ctx.engineer, &10)
        .is_err());
    assert_eq!(ctx.client.get_reputation(&ctx.engineer), 0);
}

#[test]
fn test_quality_bonus_rejects_unknown_engineer() {
    let ctx = setup();
    let unknown = Address::generate(&ctx.env);
    assert!(ctx
        .client
        .try_award_quality_bonus(&ctx.admin, &unknown, &10)
        .is_err());
}
