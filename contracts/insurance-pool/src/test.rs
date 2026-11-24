#![cfg(test)]

use crate::{
    ClaimStatus, DataKey, InsurancePool, InsurancePoolClient, PoolStats, MAX_TERM_SECS,
    MIN_TERM_SECS,
};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Events, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env, IntoVal, Val,
};

const PREMIUM_BPS: u32 = 500; // 5%
const TERM_SECS: u64 = 1_000;
const MAX_EXPOSURE_BPS: u32 = 100_000; // 10x total assets, so the cap rarely binds in other tests

struct Ctx<'a> {
    client: InsurancePoolClient<'a>,
    token: TokenClient<'a>,
    minter: StellarAssetClient<'a>,
    admin: Address,
    contract_id: Address,
}

fn setup<'a>(env: &'a Env) -> Ctx<'a> {
    setup_with_cap(env, MAX_EXPOSURE_BPS)
}

fn setup_with_cap<'a>(env: &'a Env, max_exposure_bps: u32) -> Ctx<'a> {
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 1_000);

    let admin = Address::generate(env);
    let token_admin = Address::generate(env);
    let token_id = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let contract_id = env.register(InsurancePool, ());
    let client = InsurancePoolClient::new(env, &contract_id);
    client.initialize(
        &admin,
        &token_id,
        &PREMIUM_BPS,
        &TERM_SECS,
        &max_exposure_bps,
    );

    Ctx {
        client,
        token: TokenClient::new(env, &token_id),
        minter: StellarAssetClient::new(env, &token_id),
        admin,
        contract_id,
    }
}

/// Overwrites the pool's `total_assets` in contract storage. Test-only: simulates an
/// insufficient-asset state that the public API cannot reach.
fn force_total_assets(env: &Env, ctx: &Ctx, total_assets: i128) {
    env.as_contract(&ctx.contract_id, || {
        let mut stats: PoolStats = env.storage().instance().get(&DataKey::Stats).unwrap();
        stats.total_assets = total_assets;
        env.storage().instance().set(&DataKey::Stats, &stats);
    });
}

/// Deposits `amount` from a fresh provider so claims have assets to draw on.
fn fund_pool(env: &Env, ctx: &Ctx, amount: i128) {
    let provider = funded(env, ctx, amount);
    ctx.client.provide_capital(&provider, &amount);
}

/// Deposits capital so policy purchases fit under the exposure cap.
fn seed_capital(env: &Env, ctx: &Ctx) {
    let provider = funded(env, ctx, 1_000_000);
    ctx.client.provide_capital(&provider, &1_000_000);
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
    ctx.client.provide_capital(&provider, &1_000);
    let lender = funded(&env, &ctx, 10_000);
    // Coverage 1_000 at 5% premium = 50 added to pool, so 1_050 total, all of it free.
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    let claim_id = ctx.client.file_claim(&lender, &policy_id, &1_000);

    // Reservations are always backed by assets, so the pool cannot fall below a pending
    // claim through the public API. Force the assets down to exercise the defensive cap.
    force_total_assets(&env, &ctx, 150);

    let paid = ctx.client.settle_claim(&ctx.admin, &claim_id, &true);
    assert_eq!(paid, 150);
    assert_eq!(ctx.client.get_stats().total_assets, 0);

    let claim = ctx.client.get_claim(&claim_id);
    assert_eq!(claim.status, ClaimStatus::PartiallyPaid);
    assert_eq!(claim.paid, 150);
    assert_eq!(claim.shortfall, 850);
    assert_eq!(ctx.client.get_stats().claims_paid, 1);
}

#[test]
fn full_payout_records_no_shortfall() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&provider, &5_000);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    let claim_id = ctx.client.file_claim(&lender, &policy_id, &400);
    ctx.client.settle_claim(&ctx.admin, &claim_id, &true);

    let claim = ctx.client.get_claim(&claim_id);
    assert_eq!(claim.status, ClaimStatus::Paid);
    assert_eq!(claim.shortfall, 0);
}

#[test]
fn partial_payout_emits_shortfall_event() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&provider, &1_000);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    let claim_id = ctx.client.file_claim(&lender, &policy_id, &1_000);
    force_total_assets(&env, &ctx, 150); // defensive path, see payout_is_capped_by_pool_assets
    ctx.client.settle_claim(&ctx.admin, &claim_id, &true);

    let shortfall_topic: Val = symbol_short!("ins_shrt").into_val(&env);
    let emitted = env.events().all().iter().any(|(_, topics, _)| {
        topics.len() == 1 && topics.get(0).unwrap().shallow_eq(&shortfall_topic)
    });
    assert!(
        emitted,
        "expected an ins_shrt event for the unpaid remainder"
    );
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
#[should_panic(expected = "Error(Contract, #7)")]
fn claim_beyond_free_capital_is_rejected() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&provider, &1_000);
    let lender = funded(&env, &ctx, 10_000);
    // Pool holds 1_050. The first claim reserves 1_000, leaving 50 free.
    let first = ctx.client.buy_policy(&lender, &7, &1_000);
    ctx.client.file_claim(&lender, &first, &1_000);
    // The second premium brings the pool to 1_100. Only 100 is unreserved, so 101 is rejected.
    let second = ctx.client.buy_policy(&lender, &8, &1_000);
    ctx.client.file_claim(&lender, &second, &101);
}

#[test]
fn claim_within_free_capital_reserves_funds() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&provider, &1_000);
    let lender = funded(&env, &ctx, 10_000);
    let first = ctx.client.buy_policy(&lender, &7, &1_000);
    ctx.client.file_claim(&lender, &first, &1_000);
    let second = ctx.client.buy_policy(&lender, &8, &1_000);
    // 100 is unreserved after the second premium, so exactly 100 fits.
    ctx.client.file_claim(&lender, &second, &100);

    // Reservations may use every free token, so reserved can equal total assets exactly.
    let stats = ctx.client.get_stats();
    assert_eq!(stats.reserved, 1_100);
    assert_eq!(stats.total_assets, 1_100);
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
    fund_pool(&env, &ctx, 1_000);
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
    fund_pool(&env, &ctx, 1_000);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    ctx.client.file_claim(&lender, &policy_id, &1_001);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")]
fn claim_after_expiry_is_rejected() {
    let env = Env::default();
    let ctx = setup(&env);
    fund_pool(&env, &ctx, 1_000);
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
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&provider, &1_000);
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
    client.initialize(&admin, &token_id, &10_001, &TERM_SECS, &MAX_EXPOSURE_BPS);
}

#[test]
#[should_panic(expected = "Error(Contract, #18)")]
fn buy_policy_rejects_coverage_above_exposure_cap() {
    let env = Env::default();
    // Cap outstanding coverage at 100% of total assets.
    let ctx = setup_with_cap(&env, 10_000);
    fund_pool(&env, &ctx, 1_000);
    let lender = funded(&env, &ctx, 10_000);
    // Pool is 1_000 before the first premium. Coverage 1_000 brings it to 1_050, and the cap
    // is 1_050 with the premium counted.
    ctx.client.buy_policy(&lender, &7, &1_000);
    // Second policy: premium 55 gives total assets 1_105, so the cap is 1_105. Exposure of
    // 2_100 is over it.
    ctx.client.buy_policy(&lender, &8, &1_100);
}

#[test]
fn buy_policy_accepts_coverage_up_to_exposure_cap() {
    let env = Env::default();
    let ctx = setup_with_cap(&env, 10_000);
    fund_pool(&env, &ctx, 1_000);
    let lender = funded(&env, &ctx, 10_000);
    ctx.client.buy_policy(&lender, &7, &1_000);
    // Coverage 50 gives total assets 1_052 with its premium. Exposure 1_050 fits under that.
    ctx.client.buy_policy(&lender, &8, &50);

    assert_eq!(ctx.client.get_stats().total_exposure, 1_050);
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")]
fn zero_exposure_cap_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let contract_id = env.register(InsurancePool, ());
    let client = InsurancePoolClient::new(&env, &contract_id);
    client.initialize(&admin, &token_id, &PREMIUM_BPS, &TERM_SECS, &0);
}

#[test]
fn expired_policy_release_frees_exposure_capacity() {
    let env = Env::default();
    let ctx = setup_with_cap(&env, 10_000);
    fund_pool(&env, &ctx, 1_000);
    let lender = funded(&env, &ctx, 10_000);
    let first = ctx.client.buy_policy(&lender, &7, &1_000);
    assert_eq!(ctx.client.get_stats().total_exposure, 1_000);

    env.ledger()
        .with_mut(|l| l.timestamp = 1_000 + TERM_SECS + 1);
    assert_eq!(ctx.client.release_expired_policy(&first), 1_000);
    assert_eq!(ctx.client.get_stats().total_exposure, 0);
    assert!(ctx.client.get_policy(&first).released);

    // Capacity is back: a fresh 1_000 policy fits under the cap again.
    ctx.client.buy_policy(&lender, &8, &1_000);
    assert_eq!(ctx.client.get_stats().total_exposure, 1_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #19)")]
fn cannot_release_policy_before_expiry() {
    let env = Env::default();
    let ctx = setup(&env);
    seed_capital(&env, &ctx);
    fund_pool(&env, &ctx, 1_000);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    ctx.client.release_expired_policy(&policy_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #20)")]
fn policy_cannot_be_released_twice() {
    let env = Env::default();
    let ctx = setup(&env);
    seed_capital(&env, &ctx);
    fund_pool(&env, &ctx, 1_000);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    env.ledger()
        .with_mut(|l| l.timestamp = 1_000 + TERM_SECS + 1);
    ctx.client.release_expired_policy(&policy_id);
    ctx.client.release_expired_policy(&policy_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #11)")]
fn pending_claims_cannot_collectively_exceed_coverage() {
    let env = Env::default();
    let ctx = setup(&env);
    seed_capital(&env, &ctx);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    ctx.client.file_claim(&lender, &policy_id, &600);
    // 600 is still pending, so 600 more would total 1_200 against 1_000 of coverage.
    ctx.client.file_claim(&lender, &policy_id, &600);
}

#[test]
fn rejected_claim_frees_its_coverage_for_new_claims() {
    let env = Env::default();
    let ctx = setup(&env);
    seed_capital(&env, &ctx);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    let first = ctx.client.file_claim(&lender, &policy_id, &600);
    ctx.client.settle_claim(&ctx.admin, &first, &false);

    // The rejected claim no longer reserves coverage, so this fits exactly.
    ctx.client.file_claim(&lender, &policy_id, &1_000);
}

#[test]
fn settled_claims_leave_only_unpaid_pending_exposure() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&provider, &5_000);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    let paid = ctx.client.file_claim(&lender, &policy_id, &400);
    ctx.client.settle_claim(&ctx.admin, &paid, &true);

    // Paid claim reduced coverage to 600 and nothing is pending, so a 600 claim fits.
    ctx.client.file_claim(&lender, &policy_id, &600);
}

#[test]
fn claim_filed_at_expiry_boundary_settles_after_expiry() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&provider, &5_000);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    let expires_at = ctx.client.get_policy(&policy_id).expires_at;

    // Filing at exactly expires_at is still inside the term.
    env.ledger().with_mut(|l| l.timestamp = expires_at);
    let claim_id = ctx.client.file_claim(&lender, &policy_id, &400);

    // Settlement after expiry is allowed for a claim filed in time.
    env.ledger().with_mut(|l| l.timestamp = expires_at + 10_000);
    assert_eq!(ctx.client.settle_claim(&ctx.admin, &claim_id, &true), 400);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")]
fn claim_filed_one_second_after_expiry_cannot_be_filed() {
    let env = Env::default();
    let ctx = setup(&env);
    seed_capital(&env, &ctx);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    let expires_at = ctx.client.get_policy(&policy_id).expires_at;
    env.ledger().with_mut(|l| l.timestamp = expires_at + 1);
    ctx.client.file_claim(&lender, &policy_id, &100);
}

#[test]
#[should_panic(expected = "Error(Contract, #15)")]
fn pause_blocks_capital_deposits() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.pause(&ctx.admin);
    ctx.client.provide_capital(&provider, &1_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #15)")]
fn pause_blocks_withdrawals() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&provider, &1_000);
    ctx.client.pause(&ctx.admin);
    ctx.client.withdraw_capital(&provider, &100);
}

#[test]
#[should_panic(expected = "Error(Contract, #15)")]
fn pause_blocks_policy_purchases() {
    let env = Env::default();
    let ctx = setup(&env);
    let lender = funded(&env, &ctx, 10_000);
    ctx.client.pause(&ctx.admin);
    ctx.client.buy_policy(&lender, &7, &1_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #15)")]
fn pause_blocks_claim_filing() {
    let env = Env::default();
    let ctx = setup(&env);
    seed_capital(&env, &ctx);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    ctx.client.pause(&ctx.admin);
    ctx.client.file_claim(&lender, &policy_id, &100);
}

#[test]
fn settlement_stays_open_while_paused() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.provide_capital(&provider, &5_000);
    let lender = funded(&env, &ctx, 10_000);
    let policy_id = ctx.client.buy_policy(&lender, &7, &1_000);
    let claim_id = ctx.client.file_claim(&lender, &policy_id, &400);
    ctx.client.pause(&ctx.admin);
    assert_eq!(ctx.client.settle_claim(&ctx.admin, &claim_id, &true), 400);
}

#[test]
fn unpause_restores_operations() {
    let env = Env::default();
    let ctx = setup(&env);
    let provider = funded(&env, &ctx, 10_000);
    ctx.client.pause(&ctx.admin);
    assert!(ctx.client.is_paused());
    ctx.client.unpause(&ctx.admin);
    assert!(!ctx.client.is_paused());
    assert_eq!(ctx.client.provide_capital(&provider, &1_000), 1_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn non_admin_cannot_pause() {
    let env = Env::default();
    let ctx = setup(&env);
    let intruder = Address::generate(&env);
    ctx.client.pause(&intruder);
}

#[test]
fn admin_can_update_premium_rate() {
    let env = Env::default();
    let ctx = setup(&env);
    let lender = funded(&env, &ctx, 10_000);
    ctx.client.set_premium_bps(&ctx.admin, &1_000); // 10%
    assert_eq!(ctx.client.quote_premium(&1_000), 100);
    ctx.client.buy_policy(&lender, &7, &1_000);
    assert_eq!(ctx.token.balance(&lender), 10_000 - 100);
}

#[test]
fn premium_rate_bounds_are_inclusive() {
    let env = Env::default();
    let ctx = setup(&env);
    ctx.client.set_premium_bps(&ctx.admin, &0);
    assert_eq!(ctx.client.quote_premium(&1_000), 0);
    ctx.client.set_premium_bps(&ctx.admin, &10_000);
    assert_eq!(ctx.client.quote_premium(&1_000), 1_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn premium_rate_above_maximum_is_rejected_on_update() {
    let env = Env::default();
    let ctx = setup(&env);
    ctx.client.set_premium_bps(&ctx.admin, &10_001);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn non_admin_cannot_update_premium_rate() {
    let env = Env::default();
    let ctx = setup(&env);
    let intruder = Address::generate(&env);
    ctx.client.set_premium_bps(&intruder, &100);
}

#[test]
fn term_bounds_are_inclusive_and_apply_to_new_policies() {
    let env = Env::default();
    let ctx = setup(&env);
    seed_capital(&env, &ctx);
    let lender = funded(&env, &ctx, 10_000);
    ctx.client.set_term_secs(&ctx.admin, &MIN_TERM_SECS);
    let short = ctx.client.buy_policy(&lender, &7, &100);
    assert_eq!(
        ctx.client.get_policy(&short).expires_at,
        1_000 + MIN_TERM_SECS
    );
    ctx.client.set_term_secs(&ctx.admin, &MAX_TERM_SECS);
    let long = ctx.client.buy_policy(&lender, &7, &100);
    assert_eq!(
        ctx.client.get_policy(&long).expires_at,
        1_000 + MAX_TERM_SECS
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn term_below_minimum_is_rejected() {
    let env = Env::default();
    let ctx = setup(&env);
    ctx.client.set_term_secs(&ctx.admin, &(MIN_TERM_SECS - 1));
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn term_above_maximum_is_rejected() {
    let env = Env::default();
    let ctx = setup(&env);
    ctx.client.set_term_secs(&ctx.admin, &(MAX_TERM_SECS + 1));
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn non_admin_cannot_update_term() {
    let env = Env::default();
    let ctx = setup(&env);
    let intruder = Address::generate(&env);
    ctx.client.set_term_secs(&intruder, &MIN_TERM_SECS);
}
