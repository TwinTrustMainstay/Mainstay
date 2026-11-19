use super::*;
use soroban_sdk::{symbol_short, testutils::Address as _, testutils::Ledger, Address, Env, String};

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
