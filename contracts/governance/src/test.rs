use super::*;
use soroban_sdk::{
    symbol_short, testutils::Address as _, testutils::Events as _, testutils::Ledger, Address, Env,
    IntoVal, String, Symbol, TryFromVal,
};

const T0: u64 = 10_000;
const PERIOD: u64 = 1_000;

fn config() -> GovernanceConfig {
    GovernanceConfig {
        voting_period_secs: PERIOD,
        quorum_bps: 2_000,
        approval_threshold_bps: 5_000,
        min_proposal_shares: 10,
        execution_delay_secs: 0,
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
fn parameter_proposals_must_match_schema() {
    let env = Env::default();
    let ctx = setup(&env);
    let p = holder(&ctx, 100);

    // Unknown name is rejected at creation.
    assert_eq!(
        ctx.client.try_create_proposal(
            &p,
            &title(&env),
            &ProposalKind::Parameter(symbol_short!("NOPE"), 1),
        ),
        Err(Ok(contract_err(ContractError::UnknownParameter)))
    );
    // Values outside the FEE_BPS bounds (0..=1_000) are rejected at creation.
    assert_eq!(
        ctx.client
            .try_create_proposal(&p, &title(&env), &param_kind(1_001)),
        Err(Ok(contract_err(ContractError::ParameterOutOfRange)))
    );
    // Boundary values are accepted.
    ctx.client.create_proposal(&p, &title(&env), &param_kind(0));
    ctx.client
        .create_proposal(&p, &title(&env), &param_kind(1_000));
    assert_eq!(ctx.client.get_proposal_count(), 2);
}

#[test]
fn parameter_schema_bounds_are_looked_up_by_name() {
    let env = Env::default();
    assert_eq!(
        param_bounds(&env, &symbol_short!("FEE_BPS")),
        Some((0, 1_000))
    );
    assert_eq!(
        param_bounds(&env, &symbol_short!("HEARTBEAT")),
        Some((60, 86_400))
    );
    assert_eq!(param_bounds(&env, &symbol_short!("NOPE")), None);
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
    // Voters must hold shares when the proposal is created to carry any weight.
    let mut voters: Vec<Address> = Vec::new(&env);
    for _ in 0..=MAX_VOTES_PER_PROPOSAL {
        voters.push_back(holder(&ctx, 1));
    }
    let id = ctx
        .client
        .create_proposal(&proposer, &title(&env), &param_kind(1));
    for i in 0..MAX_VOTES_PER_PROPOSAL {
        ctx.client.vote(&voters.get(i).unwrap(), &id, &true);
    }
    let extra = voters.get(MAX_VOTES_PER_PROPOSAL).unwrap();
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
fn funding_execution_debits_treasury_ledger_and_emits_recipient() {
    let env = Env::default();
    let ctx = setup(&env);
    ctx.client.fund_treasury(&ctx.admin, &500);
    let a = holder(&ctx, 100);
    let recipient = Address::generate(&env);
    let id = ctx.client.create_proposal(
        &a,
        &title(&env),
        &ProposalKind::Funding(recipient.clone(), 120),
    );
    ctx.client.vote(&a, &id, &true);
    at(&env, T0 + PERIOD);
    assert_eq!(ctx.client.finalize_proposal(&id), ProposalStatus::Passed);

    // Balances before execution: nothing has been debited yet.
    assert_eq!(ctx.client.get_treasury_balance(), 500);
    assert_eq!(ctx.client.get_shares(&recipient), 0);

    ctx.client.execute_proposal(&id);

    // The GOV_FUND event carries the recipient and amount for off-chain payout.
    let fund_event = env
        .events()
        .all()
        .iter()
        .find(|(c, _, _)| *c == ctx.client.address);
    let (_, topics, data) = fund_event.expect("GOV_FUND event emitted");
    assert_eq!(topics.len(), 2);
    assert_eq!(
        Symbol::try_from_val(&env, &topics.get(0).unwrap()).unwrap(),
        symbol_short!("GOV_FUND")
    );
    assert_eq!(
        u64::try_from_val(&env, &topics.get(1).unwrap()).unwrap(),
        id
    );
    assert_eq!(
        <(Address, u64)>::try_from_val(&env, &data).unwrap(),
        (recipient.clone(), 120u64)
    );

    // Ledger is debited by exactly the grant amount.
    assert_eq!(ctx.client.get_treasury_balance(), 380);
    // Accounting-only: the recipient's voting shares are untouched.
    assert_eq!(ctx.client.get_shares(&recipient), 0);
    assert_eq!(ctx.client.get_total_shares(), 100);
    // The grant is recorded in the proposal as executed.
    assert_eq!(
        ctx.client.get_proposal(&id).status,
        ProposalStatus::Executed
    );
}

#[test]
fn execution_waits_for_delay_after_finalization() {
    const DELAY: u64 = 500;
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|li| li.timestamp = T0);
    let admin = Address::generate(&env);
    let id_contract = env.register(Governance, ());
    let client = GovernanceClient::new(&env, &id_contract);
    let mut cfg = config();
    cfg.execution_delay_secs = DELAY;
    client.initialize(&admin, &cfg);
    let ctx = Ctx {
        env: &env,
        client,
        admin,
    };

    let a = holder(&ctx, 100);
    let id = ctx
        .client
        .create_proposal(&a, &title(&env), &param_kind(25));
    ctx.client.vote(&a, &id, &true);
    at(&env, T0 + PERIOD);
    assert_eq!(ctx.client.finalize_proposal(&id), ProposalStatus::Passed);
    assert_eq!(ctx.client.get_proposal(&id).finalized_at, T0 + PERIOD);

    // Early execution is rejected and leaves the parameter untouched.
    assert_eq!(
        ctx.client.try_execute_proposal(&id),
        Err(Ok(contract_err(ContractError::ExecutionDelayActive)))
    );
    at(&env, T0 + PERIOD + DELAY - 1);
    assert_eq!(
        ctx.client.try_execute_proposal(&id),
        Err(Ok(contract_err(ContractError::ExecutionDelayActive)))
    );
    assert_eq!(ctx.client.get_parameter(&symbol_short!("FEE_BPS")), None);

    // Once the delay has elapsed, execution succeeds.
    at(&env, T0 + PERIOD + DELAY);
    ctx.client.execute_proposal(&id);
    assert_eq!(
        ctx.client.get_parameter(&symbol_short!("FEE_BPS")),
        Some(25)
    );
}

#[test]
fn config_proposal_updates_governance_settings() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 100);
    let mut next = config();
    next.voting_period_secs = 2 * PERIOD;
    next.quorum_bps = 5_000;
    next.approval_threshold_bps = 6_000;
    next.execution_delay_secs = 77;

    let id = ctx
        .client
        .create_proposal(&a, &title(&env), &ProposalKind::Config(next.clone()));
    ctx.client.vote(&a, &id, &true);
    at(&env, T0 + PERIOD);
    assert_eq!(ctx.client.finalize_proposal(&id), ProposalStatus::Passed);
    ctx.client.execute_proposal(&id);

    assert_eq!(ctx.client.get_config(), next);
}

#[test]
fn config_proposal_rejects_invalid_settings() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 100);

    let mut zero_period = config();
    zero_period.voting_period_secs = 0;
    assert_eq!(
        ctx.client
            .try_create_proposal(&a, &title(&env), &ProposalKind::Config(zero_period)),
        Err(Ok(contract_err(ContractError::InvalidConfig)))
    );

    let mut over_quorum = config();
    over_quorum.quorum_bps = 10_001;
    assert_eq!(
        ctx.client
            .try_create_proposal(&a, &title(&env), &ProposalKind::Config(over_quorum)),
        Err(Ok(contract_err(ContractError::InvalidConfig)))
    );

    // Rejected proposals do not change the stored configuration.
    assert_eq!(ctx.client.get_config(), config());
    assert_eq!(ctx.client.get_proposal_count(), 0);
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

#[test]
fn shares_received_after_creation_carry_no_vote_weight() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 100);
    let id = ctx.client.create_proposal(&a, &title(&env), &param_kind(1));

    // Shares transferred in after creation are not part of the snapshot.
    let late = Address::generate(&env);
    ctx.client.transfer_shares(&a, &late, &60);
    assert_eq!(
        ctx.client.try_vote(&late, &id, &true),
        Err(Ok(contract_err(ContractError::InsufficientShares)))
    );

    // The original holder votes with the snapshot balance, not the reduced live one.
    ctx.client.vote(&a, &id, &true);
    assert_eq!(ctx.client.get_proposal(&id).votes_for, 100);
    assert_eq!(ctx.client.get_votes(&id).get(0).unwrap().weight, 100);
}

#[test]
fn inbound_transfer_after_creation_does_not_change_vote_weight() {
    let env = Env::default();
    let ctx = setup(&env);
    let voter = holder(&ctx, 50);
    let id = ctx
        .client
        .create_proposal(&voter, &title(&env), &param_kind(1));

    // A third party sends a share to the voter after creation. The vote still
    // counts only the snapshot balance, and the voter cannot be locked out.
    let dust_sender = holder(&ctx, 1);
    ctx.client.transfer_shares(&dust_sender, &voter, &1);
    ctx.client.vote(&voter, &id, &true);
    assert_eq!(ctx.client.get_proposal(&id).votes_for, 50);
}

#[test]
fn shares_sent_after_creation_still_count_once_for_the_sender() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 100);
    let b = Address::generate(&env);
    let id = ctx.client.create_proposal(&a, &title(&env), &param_kind(1));

    // `a` sends 60 to `b` after creation. Both sides together carry the
    // snapshot total exactly once: `a` votes 100, `b` gets no weight.
    ctx.client.transfer_shares(&a, &b, &60);
    ctx.client.vote(&a, &id, &true);
    assert_eq!(
        ctx.client.try_vote(&b, &id, &true),
        Err(Ok(contract_err(ContractError::InsufficientShares)))
    );
    assert_eq!(ctx.client.get_proposal(&id).total_shares_snapshot, 100);
    assert_eq!(ctx.client.get_proposal(&id).votes_for, 100);
}

#[test]
fn snapshot_weight_spans_multiple_changes_after_creation() {
    let env = Env::default();
    let ctx = setup(&env);
    let a = holder(&ctx, 40);
    let id = ctx.client.create_proposal(&a, &title(&env), &param_kind(1));
    // Several changes to the same holder after creation; weight must still be 40.
    let b = Address::generate(&env);
    ctx.client.mint_shares(&ctx.admin, &a, &10);
    ctx.client.transfer_shares(&a, &b, &20);
    ctx.client.mint_shares(&ctx.admin, &a, &5);
    ctx.client.vote(&a, &id, &false);
    assert_eq!(ctx.client.get_proposal(&id).votes_against, 40);
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
