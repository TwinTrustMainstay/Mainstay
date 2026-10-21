#![cfg(test)]

use crate::{ClaimStatus, InsurancePool, InsurancePoolClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env,
};

const PREMIUM_BPS: u32 = 500; // 5%
const TERM_SECS: u64 = 1_000;

struct Ctx<'a> {
    client: InsurancePoolClient<'a>,
    token: TokenClient<'a>,
    minter: StellarAssetClient<'a>,
    admin: Address,
    contract_id: Address,
}

fn setup<'a>(env: &'a Env) -> Ctx<'a> {
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 1_000);

    let admin = Address::generate(env);
    let token_admin = Address::generate(env);
    let token_id = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let contract_id = env.register(InsurancePool, ());
    let client = InsurancePoolClient::new(env, &contract_id);
    client.initialize(&admin, &token_id, &PREMIUM_BPS, &TERM_SECS);

    Ctx {
        client,
        token: TokenClient::new(env, &token_id),
        minter: StellarAssetClient::new(env, &token_id),
        admin,
        contract_id,
    }
}

fn funded(env: &Env, ctx: &Ctx, amount: i128) -> Address {
    let a = Address::generate(env);
    ctx.minter.mint(&a, &amount);
    a
}

#[test]
fn first_deposit_mints_one_share_per_token() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);

    let shares = ctx.client.provide_capital(&provider, &5_000);
    assert_eq!(shares, 5_000);
    assert_eq!(ctx.client.get_shares(&provider), 5_000);
    assert_eq!(ctx.token.balance(&ctx.contract_id), 5_000);
}

#[test]
fn later_deposits_are_priced_at_current_share_value() {
    let env = Env::default();
    let ctx = setup(&env);
    let first = funded(&env, &ctx, 10_000);
    let second = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&first, &1_000);

    // Premium income doubles the value of each share held by the first provider.
    let lender = funded(&env, &ctx, 100_000);
    ctx.client.buy_policy(&lender, &1, &20_000); // premium = 1_000
    let shares = ctx.client.provide_capital(&second, &2_000);

    // 2_000 tokens buy 2_000 * 1_000 / 2_000 = 1_000 shares when the pool holds 2_000 for 1_000 shares.
    assert_eq!(shares, 1_000);
}

#[test]
fn buy_policy_charges_premium_and_grows_pool() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&provider, &5_000);

    let lender = funded(&env, &ctx, 10_000);
    assert_eq!(ctx.client.quote_premium(&1_000), 50);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);

    let policy = ctx.client.get_policy(&policy_id);
    assert_eq!(policy.premium_paid, 50);
    assert_eq!(policy.remaining_coverage, 1_000);
    assert_eq!(policy.expires_at, 1_000 + TERM_SECS);

    let stats = ctx.client.get_stats();
    assert_eq!(stats.premiums_collected, 50);
    assert_eq!(stats.total_assets, 5_050);
    assert_eq!(ctx.token.balance(&lender), 9_950);
}

#[test]
fn approved_claim_pays_lender_and_reduces_coverage() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&provider, &5_000);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);

    let claim_id = ctx.client.file_claim(&lender, &policy_id, &400);
    assert_eq!(ctx.client.get_stats().reserved, 400);

    let paid = ctx.client.settle_claim(&ctx.admin, &claim_id, &true);
    assert_eq!(paid, 400);

    let claim = ctx.client.get_claim(&claim_id);
    assert_eq!(claim.status, ClaimStatus::Paid);
    assert_eq!(claim.paid, 400);
    assert_eq!(ctx.client.get_policy(&policy_id).remaining_coverage, 600);

    let stats = ctx.client.get_stats();
    assert_eq!(stats.reserved, 0);
    assert_eq!(stats.total_payouts, 400);
    assert_eq!(stats.claims_paid, 1);
    assert_eq!(ctx.token.balance(&lender), 10_000 - 50 + 400);
}

#[test]
fn payout_is_capped_by_pool_assets() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&provider, &100);
    let lender = funded(&env, &ctx, 10_000);
    // Coverage 1_000 at 5% premium = 50 added to pool, so 150 total.
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);

    let claim_id = ctx.client.file_claim(&lender, &policy_id, &1_000);
    let paid = ctx.client.settle_claim(&ctx.admin, &claim_id, &true);
    assert_eq!(paid, 150);
    assert_eq!(ctx.client.get_stats().total_assets, 0);
}

#[test]
fn rejected_claim_pays_nothing_and_releases_reservation() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&provider, &5_000);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    let claim_id = ctx.client.file_claim(&lender, &policy_id, &300);

    assert_eq!(ctx.client.settle_claim(&ctx.admin, &claim_id, &false), 0);
    assert_eq!(
        ctx.client.get_claim(&claim_id).status,
        ClaimStatus::Rejected
    );
    let stats = ctx.client.get_stats();
    assert_eq!(stats.reserved, 0);
    assert_eq!(stats.claims_rejected, 1);
    assert_eq!(ctx.client.get_policy(&policy_id).remaining_coverage, 1_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn pending_claim_reserves_funds_against_withdrawal() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    let shares = ctx.client.provide_capital(&provider, &1_000);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    // Reserve 1_000 of the 1_050 pool, leaving only 50 free.
    ctx.client.file_claim(&lender, &policy_id, &1_000);
    ctx.client.withdraw_capital(&provider, &shares);
}

#[test]
fn withdrawal_of_free_funds_returns_pro_rata_value() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    let shares = ctx.client.provide_capital(&provider, &1_000);
    let lender = funded(&env, &ctx, 10_000);
    ctx.client.buy_policy(&lender, &7, &1_000); // pool: 1_050 for 1_000 shares

    let paid = ctx.client.withdraw_capital(&provider, &shares);
    assert_eq!(paid, 1_050);
    assert_eq!(ctx.client.get_shares(&provider), 0);
    assert_eq!(ctx.client.get_stats().total_shares, 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn cannot_withdraw_more_shares_than_held() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&provider, &1_000);
    ctx.client.withdraw_capital(&provider, &1_001);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")]
fn only_policy_holder_can_file_claim() {
    let env = Env::default();
    let ctx = setup(&env);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    let other = Address::generate(&env);
    ctx.client.file_claim(&other, &policy_id, &100);
}

#[test]
#[should_panic(expected = "Error(Contract, #11)")]
fn claim_cannot_exceed_coverage() {
    let env = Env::default();
    let ctx = setup(&env);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    ctx.client.file_claim(&lender, &policy_id, &1_001);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")]
fn claim_after_expiry_is_rejected() {
    let env = Env::default();
    let ctx = setup(&env);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    env.ledger()
        .with_mut(|l| l.timestamp = 1_000 + TERM_SECS + 1);
    ctx.client.file_claim(&lender, &policy_id, &100);
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn claim_cannot_be_settled_twice() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&provider, &5_000);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    let claim_id = ctx.client.file_claim(&lender, &policy_id, &100);
    ctx.client.settle_claim(&ctx.admin, &claim_id, &true);
    ctx.client.settle_claim(&ctx.admin, &claim_id, &true);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn non_admin_cannot_settle_claim() {
    let env = Env::default();
    let ctx = setup(&env);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    let claim_id = ctx.client.file_claim(&lender, &policy_id, &100);
    let intruder = Address::generate(&env);
    ctx.client.settle_claim(&intruder, &claim_id, &true);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn premium_rate_above_100_percent_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let contract_id = env.register(InsurancePool, ());
    let client = InsurancePoolClient::new(&env, &contract_id);
    client.initialize(&admin, &token_id, &10_001, &TERM_SECS);
}
