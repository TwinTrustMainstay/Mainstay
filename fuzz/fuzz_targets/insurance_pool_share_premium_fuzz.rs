//! Fuzz target for insurance-pool share, premium and withdrawal math.
//!
//! `provide_capital`, `withdraw_capital` and `buy_policy` all use the contract's
//! `mul_div` helper, which truncates toward zero. Truncation moves value between
//! individual providers and the pool, so this target drives a random sequence of
//! deposits, withdrawals and policy purchases with arbitrary amounts and rates,
//! and asserts the conservation invariants the rounding must preserve:
//!
//!   * **shares are conserved** — the sum of every provider's share balance
//!     equals `PoolStats::total_shares`;
//!   * **assets are conserved** — the pool's token balance equals
//!     `PoolStats::total_assets`;
//!   * **totals never go negative** — neither `total_shares` nor `total_assets`
//!     ever drops below zero;
//!   * **rounding favours the pool** — truncating division always rounds the
//!     provider's share of value down, so the per-share value
//!     `total_assets / total_shares` never decreases within a continuous epoch
//!     (a `total_shares == 0` boundary starts a fresh epoch).
//!
//! No claims are filed or settled, so `reserved` stays at zero and withdrawals
//! are bounded only by the rounding math itself. A failing assertion here would
//! mean the integer math leaks or destroys shares/assets, or rounds the wrong
//! way.
//!
//! # Usage
//!
//! ```bash
//! cd fuzz && cargo fuzz run insurance_pool_share_premium_fuzz -- -max_total_time=7200
//! ```

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

/// Cap on operations per fuzz iteration so each run is bounded.
const MAX_OPS: usize = 32;
/// Distinct capital providers / lenders; every op indexes into this set.
const ACTOR_COUNT: u8 = 4;
/// Minted to every actor up front, well above anything the bounded op stream can
/// spend, so deposits and premiums never fail for lack of token balance.
const ACTOR_BALANCE: i128 = 1_000_000_000_000; // 10^12
/// Outstanding-coverage cap in basis points (1000x of total assets) so the cap
/// rarely binds, leaving `buy_policy` to exercise premium math.
const MAX_EXPOSURE_BPS: u32 = 10_000_000;
/// Policy term kept at the contract minimum so policies stay live for the run.
const TERM_SECS: u64 = 3_600;
/// Asset id used for every policy purchase; the pool does not validate it.
const ASSET_ID: u64 = 1;

/// A single randomized operation against the pool.
#[derive(Arbitrary, Debug)]
struct Op {
    /// `0` = deposit, `1` = withdraw, `2` = buy policy (taken modulo 3).
    kind: u8,
    /// Index into the actor set (taken modulo `ACTOR_COUNT`).
    actor: u8,
    /// Deposit amount / withdrawal shares / coverage. Used directly when `>= 1`.
    amount: u32,
}

/// Structured fuzz input: a premium rate plus a stream of pool operations.
#[derive(Arbitrary, Debug)]
struct SharePremiumFuzzInput {
    /// Premium rate seed, remapped into the valid `[0, 10_000]` basis-point range.
    premium_bps: u32,
    /// Random operation stream; only the first `MAX_OPS` are applied.
    ops: Vec<Op>,
}

fuzz_target!(|input: SharePremiumFuzzInput| {
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        token::Client as TokenClient,
        Address, Env,
    };
    use insurance_pool::{InsurancePool, InsurancePoolClient};

    let env = Env::default();
    env.mock_all_auths();
    // The run issues many host calls per iteration; keep the budget unlimited so
    // an exhaustion panic can never masquerade as a conservation failure.
    env.cost_estimate().budget().reset_unlimited();
    env.ledger().with_mut(|l| l.timestamp = 1_000);

    // A Stellar asset contract whose token balance the pool must mirror exactly.
    let token_admin = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let admin = Address::generate(&env);
    let contract_id = env.register(InsurancePool, ());
    let pool = InsurancePoolClient::new(&env, &contract_id);
    let token = TokenClient::new(&env, &token_id);

    // Premium rate in basis points, inclusive of the [0, 10_000] bounds.
    let premium_bps = input.premium_bps % 10_001;
    pool.initialize(
        &admin,
        &token_id,
        &premium_bps,
        &TERM_SECS,
        &MAX_EXPOSURE_BPS,
    );

    // Fund a fixed set of actors that double as providers and lenders.
    let stellar = soroban_sdk::token::StellarAssetClient::new(&env, &token_id);
    let mut actors: std::vec::Vec<Address> = std::vec::Vec::with_capacity(ACTOR_COUNT as usize);
    for _ in 0..ACTOR_COUNT {
        let a = Address::generate(&env);
        stellar.mint(&a, &ACTOR_BALANCE);
        actors.push(a);
    }

    // Per-share value carried as a fraction (ta, ts). Truncation rounds the
    // provider's share down, so ta/ts is non-decreasing within an epoch; a zero
    // share count resets the epoch and stops comparison across the boundary.
    let mut prev_ta: i128 = 0;
    let mut prev_ts: i128 = 0;

    for (i, op) in input.ops.iter().enumerate().take(MAX_OPS) {
        let actor = actors[(op.actor % ACTOR_COUNT) as usize].clone();
        let amount = op.amount.max(1) as i128; // the contract rejects amounts <= 0

        match op.kind % 3 {
            0 => {
                // Deposit: amount -> shares via truncating mul_div. A deposit so
                // small it truncates to zero shares panics InvalidAmount before
                // any token moves, which try_ swallows as a clean no-op.
                let _ = pool.try_provide_capital(&actor, &amount);
            }
            1 => {
                // Withdraw a bounded slice of held shares so the fuzzer explores
                // both partial and full withdrawals.
                let held = pool.get_shares(&actor);
                if held <= 0 {
                    continue;
                }
                let shares = amount.min(held);
                let _ = pool.try_withdraw_capital(&actor, &shares);
            }
            2 => {
                // Buy coverage: premium = coverage * bps / 10_000, paid to the
                // pool and added to total_assets without minting shares.
                let _ = pool.try_buy_policy(&actor, &ASSET_ID, &amount);
            }
            _ => {}
        }

        // ── Conservation invariants ─────────────────────────────────────
        let stats = pool.get_stats();

        // (1) Shares are conserved: every provider balance sums to total_shares.
        let sum_shares: i128 = actors.iter().map(|a| pool.get_shares(a)).sum();
        assert_eq!(
            sum_shares, stats.total_shares,
            "op #{} (kind={}, actor={}, amt={}): \
             sum of provider shares {} != total_shares {}",
            i, op.kind, op.actor, op.amount, sum_shares, stats.total_shares,
        );

        // (2) Assets are conserved: the pool token balance equals total_assets.
        let balance = token.balance(&contract_id);
        assert_eq!(
            balance, stats.total_assets,
            "op #{} (kind={}, actor={}, amt={}): \
             token balance {} != total_assets {}",
            i, op.kind, op.actor, op.amount, balance, stats.total_assets,
        );

        // (3) Totals never go negative.
        assert!(
            stats.total_shares >= 0 && stats.total_assets >= 0,
            "op #{} (kind={}, actor={}, amt={}): \
             negative totals (shares={}, assets={})",
            i, op.kind, op.actor, op.amount, stats.total_shares, stats.total_assets,
        );

        // (4) Rounding favours the pool, so per-share value never decreases
        // within an epoch. Compare ta/ts >= prev_ta/prev_ts by cross
        // multiplication; all terms are non-negative and positive once shares > 0.
        if stats.total_shares == 0 {
            prev_ta = 0;
            prev_ts = 0;
        } else {
            if prev_ts > 0 {
                let lhs = stats.total_assets.checked_mul(prev_ts);
                let rhs = prev_ta.checked_mul(stats.total_shares);
                if let (Some(lhs), Some(rhs)) = (lhs, rhs) {
                    assert!(
                        lhs >= rhs,
                        "op #{} (kind={}, actor={}, amt={}): \
                         share value fell from {}/{} to {}/{}",
                        i, op.kind, op.actor, op.amount,
                        prev_ta, prev_ts, stats.total_assets, stats.total_shares,
                    );
                }
            }
            prev_ta = stats.total_assets;
            prev_ts = stats.total_shares;
        }
    }
});
