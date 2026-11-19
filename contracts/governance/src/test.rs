use super::*;
use soroban_sdk::{
    symbol_short, testutils::Address as _, testutils::Events, testutils::Ledger, Address, Env,
    IntoVal, String,
};

const T0: u64 = 10_000;
const PERIOD: u64 = 1_000;

fn config() -> GovernanceConfig {
    GovernanceConfig {
        voting_period_secs: PERIOD,
        quorum_bps: 2_000,
        approval_threshold_bps: 5_000,
        min_proposal_shares: 10,
    }
}

struct Ctx<'a> {
    env: &'a Env,
    client: GovernanceClient<'a>,
    admin: Address,
}

fn setup(env: &Env) -> Ctx<'_> {
    env.mock_all_auths();
    env.ledger().with_mut(|li| li.timestamp = T0);
    let admin = Address::generate(env);
    let id = env.register(Governance, ());
    let client = GovernanceClient::new(env, &id);
    client.initialize(&admin, &config());
    Ctx { env, client, admin }
}

fn holder(ctx: &Ctx, shares: u64) -> Address {
    let addr = Address::generate(ctx.env);
    ctx.client.mint_shares(&ctx.admin, &addr, &shares);
    addr
}

fn contract_err(code: ContractError) -> soroban_sdk::Error {
    soroban_sdk::Error::from_contract_error(code as u32)
}

fn param_kind(value: u64) -> ProposalKind {
    ProposalKind::Parameter(symbol_short!("FEE_BPS"), value)
}

fn title(env: &Env) -> String {
    String::from_str(env, "Adjust fee")
}

fn at(env: &Env, ts: u64) {
    env.ledger().with_mut(|li| li.timestamp = ts);
}

#[test]
fn mint_and_transfer_shares() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 100);
    let b = Address::generate(&env);
    assert_eq!(ctx.client.get_total_shares(), 100);

    ctx.client.transfer_shares(&a, &b, &40);
    assert_eq!(ctx.client.get_shares(&a), 60);
    assert_eq!(ctx.client.get_shares(&b), 40);
    assert_eq!(
        ctx.client.try_transfer_shares(&b, &a, &41),
        Err(Ok(contract_err(ContractError::InsufficientShares)))
    );
}

#[test]
fn non_admin_cannot_mint_or_fund() {
    let env = Env::default();
    let ctx = setup(&env);
    let other = Address::generate(&env);
    let who = Address::generate(&env);
    assert_eq!(
        ctx.client.try_mint_shares(&other, &who, &5),
        Err(Ok(contract_err(ContractError::UnauthorizedAdmin)))
    );
    assert_eq!(
        ctx.client.try_fund_treasury(&other, &5),
        Err(Ok(contract_err(ContractError::UnauthorizedAdmin)))
    );
}

#[test]
fn transfer_emits_from_to_amount_event() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 100);
    let b = Address::generate(&env);

    ctx.client.transfer_shares(&a, &b, &40);

    let events = env.events().all();
    let (_contract, topics, data) = events.last().unwrap();
    assert_eq!(
        topics,
        soroban_sdk::Vec::from_array(&env, [TRANSFER_TOPIC.into_val(&env)])
    );
    let payload: (Address, Address, u64) = data.into_val(&env);
    assert_eq!(payload, (a, b, 40u64));
}

#[test]
fn fund_treasury_emits_funder_and_amount_event() {
    let env = Env::default();
    let ctx = setup(&env);

    ctx.client.fund_treasury(&ctx.admin, &500);

    let events = env.events().all();
    let (_contract, topics, data) = events.last().unwrap();
    assert_eq!(
        topics,
        soroban_sdk::Vec::from_array(&env, [FUND_TOPIC.into_val(&env)])
    );
    let payload: (Address, u64) = data.into_val(&env);
    assert_eq!(payload, (ctx.admin.clone(), 500u64));
}

#[test]
fn active_proposals_per_address_are_capped() {
    let env = Env::default();
    let ctx = setup(&env);
    let proposer = holder(&ctx, 50);
    let other = holder(&ctx, 50);

    let mut ids = Vec::new(&env);
    for _ in 0..MAX_ACTIVE_PROPOSALS_PER_ADDRESS {
        ids.push_back(
            ctx.client
                .create_proposal(&proposer, &title(&env), &param_kind(1)),
        );
    }
    assert_eq!(
        ctx.client
            .try_create_proposal(&proposer, &title(&env), &param_kind(1)),
        Err(Ok(contract_err(ContractError::ProposalLimitReached)))
    );
    // The cap is per address: another holder is unaffected.
    ctx.client
        .create_proposal(&other, &title(&env), &param_kind(1));

    // Finalizing one of the proposer's proposals frees a slot.
    at(&env, T0 + PERIOD);
    ctx.client.finalize_proposal(&ids.get(0).unwrap());
    ctx.client
        .create_proposal(&proposer, &title(&env), &param_kind(1));
    assert_eq!(
        ctx.client
            .try_create_proposal(&proposer, &title(&env), &param_kind(1)),
        Err(Ok(contract_err(ContractError::ProposalLimitReached)))
    );
}

#[test]
fn invalid_config_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let id = env.register(Governance, ());
    let client = GovernanceClient::new(&env, &id);
    let mut bad = config();
    bad.quorum_bps = 10_001;
    assert_eq!(
        client.try_initialize(&admin, &bad),
        Err(Ok(contract_err(ContractError::InvalidConfig)))
    );
}

#[test]
fn proposer_needs_minimum_shares() {
    let env = Env::default();
    let ctx = setup(&env);
    let small = holder(&ctx, 9);
    assert_eq!(
        ctx.client
            .try_create_proposal(&small, &title(&env), &param_kind(25)),
        Err(Ok(contract_err(ContractError::InsufficientShares)))
    );
}

#[test]
fn invalid_proposals_are_rejected() {
    let env = Env::default();
    let ctx = setup(&env);
    let p = holder(&ctx, 100);
    assert_eq!(
        ctx.client
            .try_create_proposal(&p, &String::from_str(&env, ""), &param_kind(1)),
        Err(Ok(contract_err(ContractError::InvalidProposal)))
    );
    let recipient = Address::generate(&env);
    assert_eq!(
        ctx.client
            .try_create_proposal(&p, &title(&env), &ProposalKind::Funding(recipient, 0)),
        Err(Ok(contract_err(ContractError::InvalidProposal)))
    );
}

#[test]
fn passing_parameter_proposal_is_executed() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 60);
    let b = holder(&ctx, 30);
    let _c = holder(&ctx, 10);

    let id = ctx
        .client
        .create_proposal(&a, &title(&env), &param_kind(25));
    ctx.client.vote(&a, &id, &true);
    ctx.client.vote(&b, &id, &false);

    at(&env, T0 + PERIOD);
    assert_eq!(ctx.client.finalize_proposal(&id), ProposalStatus::Passed);
    ctx.client.execute_proposal(&id);

    assert_eq!(
        ctx.client.get_parameter(&symbol_short!("FEE_BPS")),
        Some(25)
    );
    let proposal = ctx.client.get_proposal(&id);
    assert_eq!(proposal.status, ProposalStatus::Executed);
    assert_eq!(proposal.executed_at, T0 + PERIOD);
    assert_eq!(
        ctx.client.try_execute_proposal(&id),
        Err(Ok(contract_err(ContractError::AlreadyExecuted)))
    );
}

#[test]
fn majority_against_rejects_and_blocks_execution() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 10);
    let b = holder(&ctx, 60);
    let _c = holder(&ctx, 30);

    let id = ctx
        .client
        .create_proposal(&a, &title(&env), &param_kind(99));
    ctx.client.vote(&a, &id, &true);
    ctx.client.vote(&b, &id, &false);

    at(&env, T0 + PERIOD);
    assert_eq!(ctx.client.finalize_proposal(&id), ProposalStatus::Rejected);
    assert_eq!(
        ctx.client.try_execute_proposal(&id),
        Err(Ok(contract_err(ContractError::ProposalNotPassed)))
    );
    assert_eq!(ctx.client.get_parameter(&symbol_short!("FEE_BPS")), None);
}

#[test]
fn turnout_below_quorum_fails_regardless_of_approval() {
    let env = Env::default();
    let ctx = setup(&env);
    let proposer = holder(&ctx, 10);
    let _big = holder(&ctx, 90);
    // Only 10 of 100 shares vote (quorum is 20%).
    let id = ctx
        .client
        .create_proposal(&proposer, &title(&env), &param_kind(1));
    ctx.client.vote(&proposer, &id, &true);

    at(&env, T0 + PERIOD);
    assert_eq!(ctx.client.finalize_proposal(&id), ProposalStatus::NoQuorum);
}

#[test]
fn double_vote_is_rejected() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 50);
    let id = ctx.client.create_proposal(&a, &title(&env), &param_kind(1));
    ctx.client.vote(&a, &id, &true);
    assert_eq!(
        ctx.client.try_vote(&a, &id, &false),
        Err(Ok(contract_err(ContractError::AlreadyVoted)))
    );
}

#[test]
fn voted_shares_are_locked_until_voting_ends() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 50);
    let other = Address::generate(&env);
    let id = ctx.client.create_proposal(&a, &title(&env), &param_kind(1));
    ctx.client.vote(&a, &id, &true);

    assert_eq!(
        ctx.client.try_transfer_shares(&a, &other, &50),
        Err(Ok(contract_err(ContractError::SharesLocked)))
    );
    at(&env, T0 + PERIOD);
    ctx.client.transfer_shares(&a, &other, &50);
    assert_eq!(ctx.client.get_shares(&other), 50);
}

#[test]
fn vote_after_period_is_rejected() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 50);
    let b = holder(&ctx, 50);
    let id = ctx.client.create_proposal(&a, &title(&env), &param_kind(1));
    at(&env, T0 + PERIOD);
    assert_eq!(
        ctx.client.try_vote(&b, &id, &true),
        Err(Ok(contract_err(ContractError::VotingClosed)))
    );
}

#[test]
fn zero_share_holder_cannot_vote() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 50);
    let outsider = Address::generate(&env);
    let id = ctx.client.create_proposal(&a, &title(&env), &param_kind(1));
    assert_eq!(
        ctx.client.try_vote(&outsider, &id, &true),
        Err(Ok(contract_err(ContractError::InsufficientShares)))
    );
}

#[test]
fn finalize_waits_for_voting_period_and_runs_once() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 50);
    let id = ctx.client.create_proposal(&a, &title(&env), &param_kind(1));
    ctx.client.vote(&a, &id, &true);

    assert_eq!(
        ctx.client.try_finalize_proposal(&id),
        Err(Ok(contract_err(ContractError::VotingStillOpen)))
    );
    at(&env, T0 + PERIOD);
    ctx.client.finalize_proposal(&id);
    assert_eq!(
        ctx.client.try_finalize_proposal(&id),
        Err(Ok(contract_err(ContractError::AlreadyFinalized)))
    );
}

#[test]
fn vote_history_records_each_vote_in_order() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 60);
    let b = holder(&ctx, 40);
    let id = ctx.client.create_proposal(&a, &title(&env), &param_kind(1));
    ctx.client.vote(&a, &id, &true);
    ctx.client.vote(&b, &id, &false);

    let votes = ctx.client.get_votes(&id);
    assert_eq!(votes.len(), 2);
    assert_eq!(votes.get(0).unwrap().voter, a);
    assert_eq!(votes.get(0).unwrap().weight, 60);
    assert!(votes.get(0).unwrap().support);
    assert_eq!(votes.get(1).unwrap().voter, b);
    assert!(!votes.get(1).unwrap().support);
    assert_eq!(ctx.client.get_proposal_count(), 1);
}

#[test]
fn vote_cap_is_enforced() {
    let env = Env::default();
    let ctx = setup(&env);
    let proposer = holder(&ctx, 50);
    let id = ctx
        .client
        .create_proposal(&proposer, &title(&env), &param_kind(1));
    for _ in 0..MAX_VOTES_PER_PROPOSAL {
        let v = holder(&ctx, 1);
        ctx.client.vote(&v, &id, &true);
    }
    let extra = holder(&ctx, 1);
    assert_eq!(
        ctx.client.try_vote(&extra, &id, &true),
        Err(Ok(contract_err(ContractError::TooManyVotes)))
    );
}

#[test]
fn funding_grant_debits_treasury_and_checks_budget_at_execution() {
    let env = Env::default();
    let ctx = setup(&env);
    ctx.client.fund_treasury(&ctx.admin, &500);
    let a = holder(&ctx, 100);
    let recipient = Address::generate(&env);

    let first = ctx.client.create_proposal(
        &a,
        &title(&env),
        &ProposalKind::Funding(recipient.clone(), 300),
    );
    ctx.client.vote(&a, &first, &true);
    let second =
        ctx.client
            .create_proposal(&a, &title(&env), &ProposalKind::Funding(recipient, 300));
    ctx.client.vote(&a, &second, &true);

    at(&env, T0 + PERIOD);
    ctx.client.finalize_proposal(&first);
    ctx.client.finalize_proposal(&second);

    ctx.client.execute_proposal(&first);
    assert_eq!(ctx.client.get_treasury_balance(), 200);
    assert_eq!(
        ctx.client.try_execute_proposal(&second),
        Err(Ok(contract_err(ContractError::InsufficientBudget)))
    );
    assert_eq!(ctx.client.get_treasury_balance(), 200);
}

#[test]
fn execute_requires_finalized_pass() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 50);
    let id = ctx.client.create_proposal(&a, &title(&env), &param_kind(1));
    assert_eq!(
        ctx.client.try_execute_proposal(&id),
        Err(Ok(contract_err(ContractError::ProposalNotPassed)))
    );
}

mod outcome_properties {
    use super::*;
    use proptest::prelude::*;

    const MAX_COUNT: u64 = u32::MAX as u64;

    fn bps() -> impl Strategy<Value = u32> {
        1u32..=10_000
    }

    proptest! {
        /// Additional yes votes never turn a Passed proposal into Rejected (or NoQuorum).
        #[test]
        fn extra_yes_votes_never_unpass(
            votes_for in 0..=MAX_COUNT,
            votes_against in 0..=MAX_COUNT,
            extra_yes in 0..=MAX_COUNT,
            total in 0..=MAX_COUNT,
            quorum_bps in bps(),
            approval_bps in bps(),
        ) {
            let before = outcome(votes_for, votes_against, total, quorum_bps, approval_bps);
            if before == ProposalStatus::Passed {
                let after = outcome(
                    votes_for + extra_yes,
                    votes_against,
                    total,
                    quorum_bps,
                    approval_bps,
                );
                prop_assert_eq!(after, ProposalStatus::Passed);
            }
        }

        /// Turnout exactly at `ceil(total * quorum / 10_000)` meets quorum; one vote fewer does not.
        #[test]
        fn quorum_holds_exactly_at_boundary(
            total in 0..=MAX_COUNT,
            quorum_bps in bps(),
            approval_bps in bps(),
        ) {
            let bps_total = BPS as u128;
            let needed = ((total as u128) * (quorum_bps as u128)).div_ceil(bps_total).max(1) as u64;

            // All votes in favour, so approval is never the reason for the result.
            prop_assert_ne!(
                outcome(needed, 0, total, quorum_bps, approval_bps),
                ProposalStatus::NoQuorum
            );
            if needed > 1 {
                prop_assert_eq!(
                    outcome(needed - 1, 0, total, quorum_bps, approval_bps),
                    ProposalStatus::NoQuorum
                );
            }
        }

        /// With zero total shares, quorum is met by any vote at all and never by none.
        #[test]
        fn zero_total_shares(
            votes_for in 0..=MAX_COUNT,
            votes_against in 0..=MAX_COUNT,
            quorum_bps in bps(),
            approval_bps in bps(),
        ) {
            let status = outcome(votes_for, votes_against, 0, quorum_bps, approval_bps);
            if votes_for + votes_against == 0 {
                prop_assert_eq!(status, ProposalStatus::NoQuorum);
            } else {
                prop_assert!(
                    status == ProposalStatus::Passed || status == ProposalStatus::Rejected
                );
            }
        }
    }
}
