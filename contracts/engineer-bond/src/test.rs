#![cfg(test)]

use crate::{EngineerBond, EngineerBondClient, EventKind, DISPUTE_TIMEOUT_SECS, RECOVERY_STREAK};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::{Client as TokenClient, StellarAssetClient},
    Address, BytesN, Env, String,
};

struct Ctx<'a> {
    client: EngineerBondClient<'a>,
    token: TokenClient<'a>,
    minter: StellarAssetClient<'a>,
    admin: Address,
    treasury: Address,
    contract_id: Address,
    env: &'a Env,
}

fn setup<'a>(env: &'a Env, min_bond: i128) -> Ctx<'a> {
    env.mock_all_auths();

    let admin = Address::generate(env);
    let treasury = Address::generate(env);
    let token_admin = Address::generate(env);
    let token_id = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let contract_id = env.register(EngineerBond, ());
    let client = EngineerBondClient::new(env, &contract_id);
    client.initialize(&admin, &token_id, &treasury, &min_bond);

    Ctx {
        client,
        token: TokenClient::new(env, &token_id),
        minter: StellarAssetClient::new(env, &token_id),
        admin,
        treasury,
        contract_id,
        env,
    }
}

fn test_reason(env: &Env) -> String {
    String::from_str(env, "missed milestone")
}

fn test_evidence(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[7u8; 32])
}

fn open_test_dispute(ctx: &Ctx, eng: &Address) {
    ctx.client.open_dispute(
        &ctx.admin,
        eng,
        &test_reason(ctx.env),
        &test_evidence(ctx.env),
    );
}

fn funded_engineer(ctx: &Ctx, amount: i128) -> Address {
    let engineer = Address::generate(ctx.env);
    ctx.minter.mint(&engineer, &amount);
    engineer
}

#[test]
fn post_bond_locks_tokens_and_makes_engineer_bonded() {
    let env = Env::default();
    let ctx = setup(&env, 100);
    let eng = funded_engineer(&ctx, 1_000);

    assert!(!ctx.client.is_bonded(&eng));
    ctx.client.post_bond(&eng, &500);

    assert_eq!(ctx.token.balance(&eng), 500);
    assert_eq!(ctx.token.balance(&ctx.contract_id), 500);
    assert_eq!(ctx.client.get_bond(&eng).staked, 500);
    assert!(ctx.client.is_bonded(&eng));
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn post_bond_rejects_zero_amount() {
    let env = Env::default();
    let ctx = setup(&env, 100);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &0);
}

#[test]
fn slash_moves_funds_to_treasury_and_resets_streak() {
    let env = Env::default();
    let ctx = setup(&env, 100);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &800);
    ctx.client.record_performance(&ctx.admin, &eng, &true);
    ctx.client.record_performance(&ctx.admin, &eng, &true);

    ctx.client.slash(&ctx.admin, &eng, &300);

    let bond = ctx.client.get_bond(&eng);
    assert_eq!(bond.staked, 500);
    assert_eq!(bond.slashed_total, 300);
    assert_eq!(bond.good_streak, 0);
    assert_eq!(ctx.token.balance(&ctx.treasury), 300);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn slash_cannot_exceed_bond() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &200);
    ctx.client.slash(&ctx.admin, &eng, &201);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn withdraw_after_slash_requires_recovery_streak() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    ctx.client.slash(&ctx.admin, &eng, &100);
    for _ in 0..(RECOVERY_STREAK - 1) {
        ctx.client.record_performance(&ctx.admin, &eng, &true);
    }
    ctx.client.withdraw_bond(&eng, &100);
}

#[test]
fn withdraw_after_recovery_streak_succeeds() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    ctx.client.slash(&ctx.admin, &eng, &100);
    for _ in 0..RECOVERY_STREAK {
        ctx.client.record_performance(&ctx.admin, &eng, &true);
    }

    ctx.client.withdraw_bond(&eng, &400);
    assert_eq!(ctx.client.get_bond(&eng).staked, 0);
    // 1000 minted - 500 posted + 400 withdrawn
    assert_eq!(ctx.token.balance(&eng), 900);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn dispute_blocks_withdrawal() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    open_test_dispute(&ctx, &eng);
    assert!(!ctx.client.is_bonded(&eng));
    ctx.client.withdraw_bond(&eng, &100);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")]
fn cannot_open_two_disputes() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    open_test_dispute(&ctx, &eng);
    open_test_dispute(&ctx, &eng);
}

#[test]
fn resolving_dispute_restores_bonded_status() {
    let env = Env::default();
    let ctx = setup(&env, 100);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    open_test_dispute(&ctx, &eng);
    ctx.client.resolve_dispute(&ctx.admin, &eng);
    assert!(ctx.client.is_bonded(&eng));
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn slash_is_blocked_while_dispute_is_open() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    open_test_dispute(&ctx, &eng);
    ctx.client.slash(&ctx.admin, &eng, &100);
}

#[test]
fn slash_succeeds_after_dispute_is_resolved() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    open_test_dispute(&ctx, &eng);
    ctx.client.resolve_dispute(&ctx.admin, &eng);
    ctx.client.slash(&ctx.admin, &eng, &100);
    assert_eq!(ctx.client.get_bond(&eng).staked, 400);
    assert_eq!(ctx.token.balance(&ctx.treasury), 100);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn withdrawal_blocked_just_before_dispute_deadline() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    env.ledger().set_timestamp(1_000);
    open_test_dispute(&ctx, &eng);
    env.ledger().set_timestamp(1_000 + DISPUTE_TIMEOUT_SECS - 1);
    ctx.client.withdraw_bond(&eng, &100);
}

#[test]
fn withdrawal_allowed_after_dispute_deadline() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    env.ledger().set_timestamp(1_000);
    open_test_dispute(&ctx, &eng);
    env.ledger().set_timestamp(1_000 + DISPUTE_TIMEOUT_SECS);

    ctx.client.withdraw_bond(&eng, &500);

    let bond = ctx.client.get_bond(&eng);
    assert_eq!(bond.staked, 0);
    assert!(!bond.dispute_open);
    assert_eq!(bond.dispute_opened_at, 0);
    assert_eq!(ctx.token.balance(&eng), 1_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")]
fn resolve_without_dispute_is_rejected() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    ctx.client.resolve_dispute(&ctx.admin, &eng);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn withdrawal_cannot_leave_dust_below_minimum() {
    let env = Env::default();
    let ctx = setup(&env, 100);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &150);
    ctx.client.withdraw_bond(&eng, &100);
}

#[test]
fn full_withdrawal_is_allowed_even_below_minimum() {
    let env = Env::default();
    let ctx = setup(&env, 100);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &150);
    ctx.client.withdraw_bond(&eng, &150);
    assert_eq!(ctx.client.get_bond(&eng).staked, 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn non_admin_cannot_slash() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    let intruder = Address::generate(&env);
    ctx.client.slash(&intruder, &eng, &100);
}

#[test]
fn history_records_bond_and_performance_events() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    ctx.client.record_performance(&ctx.admin, &eng, &false);
    ctx.client.slash(&ctx.admin, &eng, &50);

    let history = ctx.client.get_history(&eng);
    let kinds: [EventKind; 3] = [
        history.get(0).unwrap().kind,
        history.get(1).unwrap().kind,
        history.get(2).unwrap().kind,
    ];
    assert_eq!(
        kinds,
        [EventKind::Posted, EventKind::PoorJob, EventKind::Slashed]
    );
    assert_eq!(history.get(2).unwrap().amount, 50);
}

#[test]
fn record_performance_rejects_engineer_without_bond() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = Address::generate(&env);
    assert!(ctx
        .client
        .try_record_performance(&ctx.admin, &eng, &true)
        .is_err());
    let exists = env.as_contract(&ctx.contract_id, || {
        env.storage()
            .persistent()
            .has(&crate::DataKey::Bond(eng.clone()))
    });
    assert!(!exists, "no bond record may be created");
}

#[test]
fn open_dispute_rejects_engineer_without_bond() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = Address::generate(&env);
    assert!(ctx
        .client
        .try_open_dispute(&ctx.admin, &eng, &test_reason(&env), &test_evidence(&env))
        .is_err());
    let exists = env.as_contract(&ctx.contract_id, || {
        env.storage()
            .persistent()
            .has(&crate::DataKey::Bond(eng.clone()))
    });
    assert!(!exists, "no bond record may be created");
}

#[test]
fn open_dispute_stores_reason_evidence_and_actor() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    let reason = String::from_str(&env, "delivered wrong artifact");
    let evidence = BytesN::from_array(&env, &[9u8; 32]);
    ctx.client
        .open_dispute(&ctx.admin, &eng, &reason, &evidence);

    let record = ctx.client.get_dispute(&eng).expect("dispute record");
    assert_eq!(record.opened_by, ctx.admin);
    assert_eq!(record.reason, reason);
    assert_eq!(record.evidence_hash, evidence);
}

#[test]
fn get_dispute_is_none_without_dispute() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    assert!(ctx.client.get_dispute(&eng).is_none());
}

#[test]
fn dispute_record_survives_resolution() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    let reason = String::from_str(&env, "audit trail");
    let evidence = BytesN::from_array(&env, &[3u8; 32]);
    ctx.client
        .open_dispute(&ctx.admin, &eng, &reason, &evidence);
    ctx.client.resolve_dispute(&ctx.admin, &eng);

    let record = ctx.client.get_dispute(&eng).expect("dispute record kept");
    assert_eq!(record.reason, reason);
    assert_eq!(record.evidence_hash, evidence);
}

#[test]
#[should_panic(expected = "Error(Contract, #12)")]
fn open_dispute_rejects_empty_reason() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    ctx.client.open_dispute(
        &ctx.admin,
        &eng,
        &String::from_str(&env, ""),
        &test_evidence(&env),
    );
}
