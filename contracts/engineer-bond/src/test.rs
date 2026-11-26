#![cfg(test)]

use crate::{
    DisputeOutcome, EngineerBond, EngineerBondClient, EventKind, DISPUTE_TIMEOUT_SECS, MAX_HISTORY,
    MAX_REASON_LEN, RECOVERY_STREAK,
};
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

fn funded_engineer(ctx: &Ctx, amount: i128) -> Address {
    let engineer = Address::generate(ctx.env);
    ctx.minter.mint(&engineer, &amount);
    engineer
}

fn test_reason(env: &Env) -> String {
    String::from_str(env, "missed milestone")
}

fn test_evidence(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[7u8; 32])
}

fn open_test_dispute(ctx: &Ctx, eng: &Address) -> u32 {
    ctx.client.open_dispute(
        &ctx.admin,
        eng,
        &test_reason(ctx.env),
        &test_evidence(ctx.env),
    )
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

    ctx.client.slash(&ctx.admin, &eng, &300, &None);

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
    ctx.client.slash(&ctx.admin, &eng, &201, &None);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn withdraw_after_slash_requires_recovery_streak() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    ctx.client.slash(&ctx.admin, &eng, &100, &None);
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
    ctx.client.slash(&ctx.admin, &eng, &100, &None);
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
    ctx.client
        .resolve_dispute(&ctx.admin, &eng, &DisputeOutcome::Dismissed);
    assert!(ctx.client.is_bonded(&eng));
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")]
fn resolve_without_dispute_is_rejected() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    ctx.client
        .resolve_dispute(&ctx.admin, &eng, &DisputeOutcome::Dismissed);
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
    ctx.client.slash(&intruder, &eng, &100, &None);
}

#[test]
fn history_records_bond_and_performance_events() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    ctx.client.record_performance(&ctx.admin, &eng, &false);
    ctx.client.slash(&ctx.admin, &eng, &50, &None);

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
fn upheld_dispute_records_outcome_and_links_slash() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);

    let id = open_test_dispute(&ctx, &eng);
    assert_eq!(id, 1);
    let open = ctx.client.get_dispute(&eng, &id);
    assert_eq!(open.outcome, DisputeOutcome::Pending);
    assert_eq!(open.slashed_total, 0);

    // Slashing is refused while the dispute is open, so resolve it before slashing.
    ctx.client
        .resolve_dispute(&ctx.admin, &eng, &DisputeOutcome::Upheld);
    ctx.client.slash(&ctx.admin, &eng, &120, &Some(id));
    ctx.client.slash(&ctx.admin, &eng, &30, &None);

    let resolved = ctx.client.get_dispute(&eng, &id);
    assert_eq!(resolved.outcome, DisputeOutcome::Upheld);
    // Only the slash linked to the dispute is counted against it.
    assert_eq!(resolved.slashed_total, 120);
    assert_eq!(ctx.client.get_bond(&eng).slashed_total, 150);
    assert!(!ctx.client.get_bond(&eng).dispute_open);
}

#[test]
fn dismissed_dispute_records_outcome_without_slash() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);

    let id = open_test_dispute(&ctx, &eng);
    ctx.client
        .resolve_dispute(&ctx.admin, &eng, &DisputeOutcome::Dismissed);

    let dispute = ctx.client.get_dispute(&eng, &id);
    assert_eq!(dispute.outcome, DisputeOutcome::Dismissed);
    assert_eq!(dispute.slashed_total, 0);
    assert_eq!(ctx.client.get_bond(&eng).slashed_total, 0);
    assert!(ctx.client.is_bonded(&eng));
}

#[test]
fn each_dispute_gets_its_own_identifier_and_slash_link() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);

    let first = open_test_dispute(&ctx, &eng);
    ctx.client
        .resolve_dispute(&ctx.admin, &eng, &DisputeOutcome::Dismissed);
    let second = open_test_dispute(&ctx, &eng);
    ctx.client
        .resolve_dispute(&ctx.admin, &eng, &DisputeOutcome::Upheld);
    assert_eq!((first, second), (1, 2));

    ctx.client.slash(&ctx.admin, &eng, &50, &Some(second));
    assert_eq!(ctx.client.get_dispute(&eng, &first).slashed_total, 0);
    assert_eq!(ctx.client.get_dispute(&eng, &second).slashed_total, 50);
    assert_eq!(
        ctx.client.get_dispute(&eng, &first).outcome,
        DisputeOutcome::Dismissed
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #11)")]
fn slash_linked_to_unknown_dispute_is_rejected() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    ctx.client.slash(&ctx.admin, &eng, &10, &Some(1));
}

#[test]
fn history_truncation_is_counted_and_keeps_newest_entries() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    // Posted is entry 0; each post_bond adds one entry, so this overflows MAX_HISTORY.
    let extra = 5u32;
    ctx.client.post_bond(&eng, &1);
    for _ in 0..(MAX_HISTORY + extra - 1) {
        ctx.client.record_performance(&ctx.admin, &eng, &true);
    }
    ctx.client.record_performance(&ctx.admin, &eng, &false);

    let history = ctx.client.get_history(&eng);
    assert_eq!(history.len(), MAX_HISTORY);
    assert_eq!(ctx.client.get_history_dropped(&eng), extra + 1);
    // The oldest surviving entry is the one that was (extra + 1) positions from the start.
    assert_eq!(
        history.get(MAX_HISTORY - 1).unwrap().kind,
        EventKind::PoorJob
    );
    assert_eq!(history.get(0).unwrap().kind, EventKind::GoodJob);
}

#[test]
fn history_below_limit_drops_nothing() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &100);
    ctx.client.record_performance(&ctx.admin, &eng, &true);
    assert_eq!(ctx.client.get_history(&eng).len(), 2);
    assert_eq!(ctx.client.get_history_dropped(&eng), 0);
}

/// Small deterministic xorshift generator. Each run is reproducible from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const ENGINEERS: usize = 3;
const ENGINEER_FUNDS: i128 = 10_000;

/// Property: for every engineer, deposits equal staked + slashed + withdrawn, and the
/// contract, treasury and engineer balances together conserve the minted supply.
/// Random sequences of stake, withdraw, slash, performance and dispute calls are run
/// under several seeds, and the invariants are checked after every step.
#[test]
fn accounting_invariants_hold_under_random_operation_sequences() {
    for seed in 1..=6u64 {
        run_random_sequence(seed, 200);
    }
}

fn run_random_sequence(seed: u64, steps: u32) {
    let env = Env::default();
    let ctx = setup(&env, 50);
    let engineers: [Address; ENGINEERS] =
        core::array::from_fn(|_| funded_engineer(&ctx, ENGINEER_FUNDS));
    let minted = ENGINEER_FUNDS * ENGINEERS as i128;
    let mut deposited = [0i128; ENGINEERS];
    let mut withdrawn = [0i128; ENGINEERS];
    let mut rng = Rng(seed);

    for step in 0..steps {
        let i = rng.below(ENGINEERS as u64) as usize;
        let eng = &engineers[i];
        match rng.below(6) {
            0 => {
                let amount = 1 + rng.below(300) as i128;
                if ctx.client.try_post_bond(eng, &amount).is_ok() {
                    deposited[i] += amount;
                }
            }
            1 => {
                let amount = 1 + rng.below(300) as i128;
                if ctx.client.try_withdraw_bond(eng, &amount).is_ok() {
                    withdrawn[i] += amount;
                }
            }
            2 => {
                let amount = 1 + rng.below(200) as i128;
                let _ = ctx.client.try_slash(&ctx.admin, eng, &amount, &None);
            }
            3 => {
                let good = rng.below(2) == 0;
                let _ = ctx.client.try_record_performance(&ctx.admin, eng, &good);
            }
            4 => {
                let _ = ctx.client.try_open_dispute(&ctx.admin, eng, &String::from_str(&env, "random"), &BytesN::from_array(&env, &[1u8; 32]));
            }
            _ => {
                let outcome = if rng.below(2) == 0 {
                    DisputeOutcome::Upheld
                } else {
                    DisputeOutcome::Dismissed
                };
                let _ = ctx.client.try_resolve_dispute(&ctx.admin, eng, &outcome);
            }
        }
        assert_accounting(&ctx, &engineers, &deposited, &withdrawn, minted, seed, step);
    }
}

fn assert_accounting(
    ctx: &Ctx,
    engineers: &[Address; ENGINEERS],
    deposited: &[i128; ENGINEERS],
    withdrawn: &[i128; ENGINEERS],
    minted: i128,
    seed: u64,
    step: u32,
) {
    let mut total_staked = 0i128;
    let mut total_slashed = 0i128;
    let mut total_held_by_engineers = 0i128;

    for i in 0..ENGINEERS {
        let bond = ctx.client.get_bond(&engineers[i]);
        assert!(
            bond.staked >= 0,
            "negative stake (seed {seed}, step {step})"
        );
        assert!(
            bond.slashed_total >= 0,
            "negative slash total (seed {seed}, step {step})"
        );
        assert_eq!(
            deposited[i],
            bond.staked + bond.slashed_total + withdrawn[i],
            "per-engineer accounting (seed {seed}, step {step})"
        );
        total_staked += bond.staked;
        total_slashed += bond.slashed_total;
        total_held_by_engineers += ctx.token.balance(&engineers[i]);
    }

    assert_eq!(
        ctx.token.balance(&ctx.contract_id),
        total_staked,
        "contract holds exactly the total stake (seed {seed}, step {step})"
    );
    assert_eq!(
        ctx.token.balance(&ctx.treasury),
        total_slashed,
        "treasury holds exactly the total slashed (seed {seed}, step {step})"
    );
    assert_eq!(
        total_held_by_engineers + total_staked + total_slashed,
        minted,
        "supply is conserved (seed {seed}, step {step})"
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn slash_is_blocked_while_dispute_is_open() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    open_test_dispute(&ctx, &eng);
    ctx.client.slash(&ctx.admin, &eng, &100, &None);
}

#[test]
fn slash_succeeds_after_dispute_is_resolved() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    open_test_dispute(&ctx, &eng);
    ctx.client
        .resolve_dispute(&ctx.admin, &eng, &DisputeOutcome::Dismissed);
    ctx.client.slash(&ctx.admin, &eng, &100, &None);
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
    let id = ctx
        .client
        .open_dispute(&ctx.admin, &eng, &reason, &evidence);

    let record = ctx.client.get_dispute(&eng, &id);
    assert_eq!(record.opened_by, ctx.admin);
    assert_eq!(record.reason, reason);
    assert_eq!(record.evidence_hash, evidence);
}

#[test]
#[should_panic(expected = "Error(Contract, #11)")]
fn get_dispute_unknown_id_is_rejected() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    ctx.client.get_dispute(&eng, &1);
}

#[test]
fn dispute_record_survives_resolution() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    let reason = String::from_str(&env, "audit trail");
    let evidence = BytesN::from_array(&env, &[3u8; 32]);
    let id = ctx
        .client
        .open_dispute(&ctx.admin, &eng, &reason, &evidence);
    ctx.client
        .resolve_dispute(&ctx.admin, &eng, &DisputeOutcome::Upheld);

    let record = ctx.client.get_dispute(&eng, &id);
    assert_eq!(record.outcome, DisputeOutcome::Upheld);
    assert_eq!(record.reason, reason);
    assert_eq!(record.evidence_hash, evidence);
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")]
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

#[test]
fn open_dispute_accepts_reason_at_max_length() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    let reason = String::from_str(&env, &"r".repeat(MAX_REASON_LEN as usize));
    let id = ctx
        .client
        .open_dispute(&ctx.admin, &eng, &reason, &test_evidence(&env));

    assert_eq!(ctx.client.get_dispute(&eng, &id).reason, reason);
}

#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn open_dispute_rejects_reason_over_max_length() {
    let env = Env::default();
    let ctx = setup(&env, 0);
    let eng = funded_engineer(&ctx, 1_000);
    ctx.client.post_bond(&eng, &500);
    ctx.client.open_dispute(
        &ctx.admin,
        &eng,
        &String::from_str(&env, &"r".repeat(MAX_REASON_LEN as usize + 1)),
        &test_evidence(&env),
    );
}
