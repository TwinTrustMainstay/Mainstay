#![no_std]

//! Decentralized governance for platform parameters (issue #1740).
//!
//! * **Voting shares**: the admin bootstraps holders with shares. Holders can
//!   transfer them. Voting weight equals the voter's share balance.
//! * **Proposals**: any holder with at least `min_proposal_shares` can propose a
//!   parameter change or a treasury funding grant.
//! * **Voting**: one vote per holder per proposal. A holder's balance is locked
//!   until that proposal's voting period ends, so shares cannot be voted, moved,
//!   and voted again.
//! * **Outcome**: a proposal passes when turnout meets `quorum_bps` of the share
//!   supply at creation and `approval_threshold_bps` of votes cast are in favour.
//! * **Execution**: a passed proposal can be executed once. Parameter changes are
//!   written to the on-chain parameter store. Funding grants are debited from the
//!   treasury budget ledger.
//!
//! The treasury is an accounting ledger. Actual token movement is out of scope.

use shared::require_admin;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    Env, String, Symbol, Vec,
};

/// Maximum votes recorded per proposal. Bounds history storage and the duplicate-vote scan.
pub const MAX_VOTES_PER_PROPOSAL: u32 = 500;
/// Maximum proposal title length in bytes.
pub const MAX_TITLE_LEN: u32 = 128;
const BPS: u64 = 10_000;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ContractError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    UnauthorizedAdmin = 3,
    /// Governance configuration is out of range.
    InvalidConfig = 4,
    /// Amount must be greater than zero.
    InvalidAmount = 5,
    /// Holder lacks the shares needed for the action.
    InsufficientShares = 6,
    /// Shares are locked by an active vote and cannot be transferred.
    SharesLocked = 7,
    ProposalNotFound = 8,
    /// Title, kind or parameter is invalid.
    InvalidProposal = 9,
    /// Voting is not open for this proposal.
    VotingClosed = 10,
    /// Holder has already voted on this proposal.
    AlreadyVoted = 11,
    /// The voting period has not ended.
    VotingStillOpen = 12,
    /// Proposal was already finalized.
    AlreadyFinalized = 13,
    /// Only passed proposals can be executed.
    ProposalNotPassed = 14,
    /// Proposal was already executed.
    AlreadyExecuted = 15,
    /// Treasury budget does not cover the funding grant.
    InsufficientBudget = 16,
    /// Proposal has reached `MAX_VOTES_PER_PROPOSAL`.
    TooManyVotes = 17,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GovernanceConfig {
    /// Seconds a proposal stays open for voting.
    pub voting_period_secs: u64,
    /// Minimum turnout as a share of total supply at creation (bps). 1..=10_000.
    pub quorum_bps: u32,
    /// Minimum share of votes cast in favour (bps). 1..=10_000.
    pub approval_threshold_bps: u32,
    /// Minimum shares a holder must own to create a proposal.
    pub min_proposal_shares: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProposalKind {
    /// Set a platform parameter to a new value.
    Parameter(Symbol, u64),
    /// Grant `amount` from the treasury budget to `recipient`.
    Funding(Address, u64),
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProposalStatus {
    /// Voting is open, or the period has ended but the outcome is not yet finalized.
    Active = 0,
    Passed = 1,
    Rejected = 2,
    /// Turnout did not reach quorum.
    NoQuorum = 3,
    Executed = 4,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Proposal {
    pub id: u64,
    pub proposer: Address,
    pub title: String,
    pub kind: ProposalKind,
    pub created_at: u64,
    pub voting_ends_at: u64,
    /// Total share supply when the proposal was created. Quorum is measured against this.
    pub total_shares_snapshot: u64,
    pub votes_for: u64,
    pub votes_against: u64,
    pub status: ProposalStatus,
    /// Timestamp at which the proposal was executed. `0` if not yet executed.
    pub executed_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoteRecord {
    pub voter: Address,
    pub support: bool,
    pub weight: u64,
    pub cast_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum DataKey {
    Admin,
    Config,
    TotalShares,
    Shares(Address),
    /// Ledger timestamp until which the holder's shares are locked by a vote.
    LockedUntil(Address),
    Treasury,
    NextProposalId,
    Proposal(u64),
    Votes(u64),
    Voted(u64, Address),
    Param(Symbol),
}

const PROPOSAL_TOPIC: Symbol = symbol_short!("GOV_PROP");
const VOTE_TOPIC: Symbol = symbol_short!("GOV_VOTE");
const FINALIZE_TOPIC: Symbol = symbol_short!("GOV_FIN");
const EXECUTE_TOPIC: Symbol = symbol_short!("GOV_EXEC");
const MINT_TOPIC: Symbol = symbol_short!("GOV_MINT");

fn require_admin_auth(env: &Env, admin: &Address) {
    let stored: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized));
    if require_admin(admin, &stored).is_err() {
        panic_with_error!(env, ContractError::UnauthorizedAdmin);
    }
}

fn load_config(env: &Env) -> GovernanceConfig {
    env.storage()
        .instance()
        .get(&DataKey::Config)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized))
}

fn balance_of(env: &Env, holder: &Address) -> u64 {
    env.storage()
        .persistent()
        .get(&DataKey::Shares(holder.clone()))
        .unwrap_or(0)
}

fn set_balance(env: &Env, holder: &Address, amount: u64) {
    let key = DataKey::Shares(holder.clone());
    if amount == 0 {
        env.storage().persistent().remove(&key);
    } else {
        env.storage().persistent().set(&key, &amount);
        env.storage()
            .persistent()
            .extend_ttl(&key, shared::TTL_THRESHOLD, shared::TTL_TARGET);
    }
}

fn total_shares(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::TotalShares)
        .unwrap_or(0)
}

fn load_proposal(env: &Env, id: u64) -> Proposal {
    env.storage()
        .persistent()
        .get(&DataKey::Proposal(id))
        .unwrap_or_else(|| panic_with_error!(env, ContractError::ProposalNotFound))
}

fn store_proposal(env: &Env, proposal: &Proposal) {
    let key = DataKey::Proposal(proposal.id);
    env.storage().persistent().set(&key, proposal);
    env.storage()
        .persistent()
        .extend_ttl(&key, shared::TTL_THRESHOLD, shared::TTL_TARGET);
}

/// Pure outcome rule, separated for unit testing.
///
/// Returns `NoQuorum` when `votes_cast` is below `quorum_bps` of `total_snapshot`,
/// `Passed` when the for-share meets the approval threshold, otherwise `Rejected`.
pub fn outcome(
    votes_for: u64,
    votes_against: u64,
    total_snapshot: u64,
    quorum_bps: u32,
    approval_threshold_bps: u32,
) -> ProposalStatus {
    let votes_cast = votes_for.saturating_add(votes_against);
    let quorum_met = (votes_cast as u128) * (BPS as u128)
        >= (total_snapshot as u128) * (quorum_bps as u128)
        && votes_cast > 0;
    if !quorum_met {
        return ProposalStatus::NoQuorum;
    }
    let approved = (votes_for as u128) * (BPS as u128)
        >= (votes_cast as u128) * (approval_threshold_bps as u128);
    if approved {
        ProposalStatus::Passed
    } else {
        ProposalStatus::Rejected
    }
}

#[contract]
pub struct Governance;

#[contractimpl]
impl Governance {
    /// One-time setup.
    ///
    /// # Panics
    /// - [`ContractError::AlreadyInitialized`] if called twice
    /// - [`ContractError::InvalidConfig`] if any setting is out of range
    pub fn initialize(env: Env, admin: Address, config: GovernanceConfig) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, ContractError::AlreadyInitialized);
        }
        if config.voting_period_secs == 0
            || config.quorum_bps == 0
            || config.quorum_bps as u64 > BPS
            || config.approval_threshold_bps == 0
            || config.approval_threshold_bps as u64 > BPS
        {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Config, &config);
        env.storage().instance().set(&DataKey::TotalShares, &0u64);
        env.storage().instance().set(&DataKey::Treasury, &0u64);
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &1u64);
        env.storage()
            .instance()
            .extend_ttl(shared::TTL_THRESHOLD, shared::TTL_TARGET);
    }

    /// Admin-only: issue voting shares to a holder (bootstrapping or grants).
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if `admin` is not the stored admin
    /// - [`ContractError::InvalidAmount`] if `amount` is zero
    pub fn mint_shares(env: Env, admin: Address, holder: Address, amount: u64) {
        require_admin_auth(&env, &admin);
        if amount == 0 {
            panic_with_error!(&env, ContractError::InvalidAmount);
        }
        set_balance(&env, &holder, balance_of(&env, &holder) + amount);
        env.storage()
            .instance()
            .set(&DataKey::TotalShares, &(total_shares(&env) + amount));
        env.events().publish((MINT_TOPIC, holder), (admin, amount));
    }

    /// Transfer voting shares. Shares locked by an active vote cannot be transferred.
    ///
    /// # Panics
    /// - [`ContractError::InvalidAmount`] if `amount` is zero
    /// - [`ContractError::SharesLocked`] if the sender's shares are locked by a vote
    /// - [`ContractError::InsufficientShares`] if the sender holds fewer than `amount`
    pub fn transfer_shares(env: Env, from: Address, to: Address, amount: u64) {
        from.require_auth();
        if amount == 0 {
            panic_with_error!(&env, ContractError::InvalidAmount);
        }
        let now = env.ledger().timestamp();
        let locked_until: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::LockedUntil(from.clone()))
            .unwrap_or(0);
        if now < locked_until {
            panic_with_error!(&env, ContractError::SharesLocked);
        }
        let from_balance = balance_of(&env, &from);
        if from_balance < amount {
            panic_with_error!(&env, ContractError::InsufficientShares);
        }
        set_balance(&env, &from, from_balance - amount);
        set_balance(&env, &to, balance_of(&env, &to) + amount);
    }

    /// Create a proposal. The proposer must hold at least `min_proposal_shares`.
    ///
    /// # Returns
    /// The new proposal id.
    ///
    /// # Panics
    /// - [`ContractError::InsufficientShares`] if the proposer is below the minimum (or holds none)
    /// - [`ContractError::InvalidProposal`] if the title is empty or longer than [`MAX_TITLE_LEN`],
    ///   or a funding amount is zero
    pub fn create_proposal(env: Env, proposer: Address, title: String, kind: ProposalKind) -> u64 {
        proposer.require_auth();
        let config = load_config(&env);
        let power = balance_of(&env, &proposer);
        if power == 0 || power < config.min_proposal_shares {
            panic_with_error!(&env, ContractError::InsufficientShares);
        }
        if title.is_empty() || title.len() > MAX_TITLE_LEN {
            panic_with_error!(&env, ContractError::InvalidProposal);
        }
        if let ProposalKind::Funding(_, 0) = kind {
            panic_with_error!(&env, ContractError::InvalidProposal);
        }

        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextProposalId)
            .unwrap_or(1);
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &(id + 1));

        let now = env.ledger().timestamp();
        let proposal = Proposal {
            id,
            proposer: proposer.clone(),
            title,
            kind,
            created_at: now,
            voting_ends_at: now + config.voting_period_secs,
            total_shares_snapshot: total_shares(&env),
            votes_for: 0,
            votes_against: 0,
            status: ProposalStatus::Active,
            executed_at: 0,
        };
        store_proposal(&env, &proposal);
        env.events()
            .publish((PROPOSAL_TOPIC, id), (proposer, proposal.voting_ends_at));
        id
    }

    /// Cast a vote. Weight is the voter's current share balance. The balance is
    /// locked until the voting period ends.
    ///
    /// # Panics
    /// - [`ContractError::ProposalNotFound`] if the proposal does not exist
    /// - [`ContractError::VotingClosed`] if the proposal is not active or the period has ended
    /// - [`ContractError::InsufficientShares`] if the voter holds no shares
    /// - [`ContractError::AlreadyVoted`] if the voter already voted on this proposal
    /// - [`ContractError::TooManyVotes`] if the proposal has reached its vote cap
    pub fn vote(env: Env, voter: Address, proposal_id: u64, support: bool) {
        voter.require_auth();
        let mut proposal = load_proposal(&env, proposal_id);
        let now = env.ledger().timestamp();
        if proposal.status != ProposalStatus::Active || now >= proposal.voting_ends_at {
            panic_with_error!(&env, ContractError::VotingClosed);
        }
        let weight = balance_of(&env, &voter);
        if weight == 0 {
            panic_with_error!(&env, ContractError::InsufficientShares);
        }
        let voted_key = DataKey::Voted(proposal_id, voter.clone());
        if env.storage().persistent().has(&voted_key) {
            panic_with_error!(&env, ContractError::AlreadyVoted);
        }
        let votes_key = DataKey::Votes(proposal_id);
        let mut votes: Vec<VoteRecord> = env
            .storage()
            .persistent()
            .get(&votes_key)
            .unwrap_or_else(|| Vec::new(&env));
        if votes.len() >= MAX_VOTES_PER_PROPOSAL {
            panic_with_error!(&env, ContractError::TooManyVotes);
        }

        votes.push_back(VoteRecord {
            voter: voter.clone(),
            support,
            weight,
            cast_at: now,
        });
        env.storage().persistent().set(&votes_key, &votes);
        env.storage().persistent().extend_ttl(
            &votes_key,
            shared::TTL_THRESHOLD,
            shared::TTL_TARGET,
        );
        env.storage().persistent().set(&voted_key, &true);
        env.storage().persistent().extend_ttl(
            &voted_key,
            shared::TTL_THRESHOLD,
            shared::TTL_TARGET,
        );

        // Lock the voter's balance until voting ends so the same shares cannot be
        // moved and voted again on this proposal.
        let lock_key = DataKey::LockedUntil(voter.clone());
        let existing: u64 = env.storage().persistent().get(&lock_key).unwrap_or(0);
        let lock_until = existing.max(proposal.voting_ends_at);
        env.storage().persistent().set(&lock_key, &lock_until);
        env.storage()
            .persistent()
            .extend_ttl(&lock_key, shared::TTL_THRESHOLD, shared::TTL_TARGET);

        if support {
            proposal.votes_for += weight;
        } else {
            proposal.votes_against += weight;
        }
        store_proposal(&env, &proposal);
        env.events()
            .publish((VOTE_TOPIC, proposal_id), (voter, support, weight));
    }

    /// Close voting and record the outcome. Callable by anyone after the voting period ends.
    ///
    /// # Panics
    /// - [`ContractError::ProposalNotFound`] if the proposal does not exist
    /// - [`ContractError::AlreadyFinalized`] if the outcome is already recorded
    /// - [`ContractError::VotingStillOpen`] if the voting period has not ended
    pub fn finalize_proposal(env: Env, proposal_id: u64) -> ProposalStatus {
        let mut proposal = load_proposal(&env, proposal_id);
        if proposal.status != ProposalStatus::Active {
            panic_with_error!(&env, ContractError::AlreadyFinalized);
        }
        if env.ledger().timestamp() < proposal.voting_ends_at {
            panic_with_error!(&env, ContractError::VotingStillOpen);
        }
        let config = load_config(&env);
        proposal.status = outcome(
            proposal.votes_for,
            proposal.votes_against,
            proposal.total_shares_snapshot,
            config.quorum_bps,
            config.approval_threshold_bps,
        );
        store_proposal(&env, &proposal);
        env.events()
            .publish((FINALIZE_TOPIC, proposal_id), (proposal.status,));
        proposal.status
    }

    /// Apply a passed proposal. Callable by anyone, once.
    ///
    /// # Panics
    /// - [`ContractError::ProposalNotFound`] if the proposal does not exist
    /// - [`ContractError::AlreadyExecuted`] if already executed
    /// - [`ContractError::ProposalNotPassed`] if the proposal has not passed
    /// - [`ContractError::InsufficientBudget`] if a funding grant exceeds the treasury balance
    pub fn execute_proposal(env: Env, proposal_id: u64) {
        let mut proposal = load_proposal(&env, proposal_id);
        if proposal.status == ProposalStatus::Executed {
            panic_with_error!(&env, ContractError::AlreadyExecuted);
        }
        if proposal.status != ProposalStatus::Passed {
            panic_with_error!(&env, ContractError::ProposalNotPassed);
        }
        match proposal.kind.clone() {
            ProposalKind::Parameter(name, value) => {
                env.storage()
                    .persistent()
                    .set(&DataKey::Param(name), &value);
            }
            ProposalKind::Funding(_, amount) => {
                let treasury: u64 = env
                    .storage()
                    .instance()
                    .get(&DataKey::Treasury)
                    .unwrap_or(0);
                if amount > treasury {
                    panic_with_error!(&env, ContractError::InsufficientBudget);
                }
                env.storage()
                    .instance()
                    .set(&DataKey::Treasury, &(treasury - amount));
            }
        }
        let now = env.ledger().timestamp();
        proposal.status = ProposalStatus::Executed;
        proposal.executed_at = now;
        store_proposal(&env, &proposal);
        env.events().publish((EXECUTE_TOPIC, proposal_id), (now,));
    }

    /// Admin-only: credit the treasury budget ledger. Funding grants draw from this balance.
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if `admin` is not the stored admin
    /// - [`ContractError::InvalidAmount`] if `amount` is zero
    pub fn fund_treasury(env: Env, admin: Address, amount: u64) {
        require_admin_auth(&env, &admin);
        if amount == 0 {
            panic_with_error!(&env, ContractError::InvalidAmount);
        }
        let treasury: u64 = env
            .storage()
            .instance()
            .get(&DataKey::Treasury)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::Treasury, &(treasury + amount));
    }

    /// Get a proposal by id.
    ///
    /// # Panics
    /// - [`ContractError::ProposalNotFound`] if the proposal does not exist
    pub fn get_proposal(env: Env, proposal_id: u64) -> Proposal {
        load_proposal(&env, proposal_id)
    }

    /// Votes cast on a proposal, in cast order.
    pub fn get_votes(env: Env, proposal_id: u64) -> Vec<VoteRecord> {
        env.storage()
            .persistent()
            .get(&DataKey::Votes(proposal_id))
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Number of proposals created so far.
    pub fn get_proposal_count(env: Env) -> u64 {
        let next: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextProposalId)
            .unwrap_or(1);
        next - 1
    }

    /// Voting shares held by `holder`.
    pub fn get_shares(env: Env, holder: Address) -> u64 {
        balance_of(&env, &holder)
    }

    /// Total voting shares in circulation.
    pub fn get_total_shares(env: Env) -> u64 {
        total_shares(&env)
    }

    /// Treasury budget remaining.
    pub fn get_treasury_balance(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::Treasury)
            .unwrap_or(0)
    }

    /// Value of a platform parameter set by an executed proposal.
    pub fn get_parameter(env: Env, name: Symbol) -> Option<u64> {
        env.storage().persistent().get(&DataKey::Param(name))
    }

    /// Current governance configuration.
    pub fn get_config(env: Env) -> GovernanceConfig {
        load_config(&env)
    }
}

#[cfg(test)]
mod test;
