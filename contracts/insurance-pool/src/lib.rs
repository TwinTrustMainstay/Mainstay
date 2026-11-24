#![no_std]

//! Insurance pool for collateral protection (issue #1737).
//!
//! Capital providers deposit tokens and receive pool shares. Lenders buy coverage policies
//! for an asset by paying a premium (basis points of coverage), which goes into the pool.
//! Lenders file claims against a policy, and the admin approves or rejects them. Approved
//! claims are paid from pool assets and reduce the policy's remaining coverage.
//!
//! Fund management: pending claims reserve their maximum payout, and providers can only
//! withdraw assets that are not reserved. Share value rises as premiums accrue and falls
//! as claims are paid.

#[cfg(test)]
mod test;

use shared::{extend_persistent_ttl, require_admin};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, token,
    Address, Env,
};

/// Basis points denominator (10_000 = 100%).
pub const BPS_DENOMINATOR: i128 = 10_000;

/// Shortest policy term accepted by `set_term_secs`: one hour.
pub const MIN_TERM_SECS: u64 = 3_600;

/// Longest policy term accepted by `set_term_secs`: 365 days.
pub const MAX_TERM_SECS: u64 = 365 * 24 * 3_600;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    /// Contract has not been initialized.
    NotInitialized = 1,
    /// Contract has already been initialized.
    AlreadyInitialized = 2,
    /// Caller is not the admin.
    UnauthorizedAdmin = 3,
    /// Amount must be greater than zero.
    InvalidAmount = 4,
    /// Premium rate must be between 0 and 10_000 basis points.
    InvalidPremiumRate = 5,
    /// Provider does not hold enough shares.
    InsufficientShares = 6,
    /// Requested amount exceeds pool assets that are not reserved for claims.
    InsufficientFreeFunds = 7,
    /// No policy exists with the given id.
    PolicyNotFound = 8,
    /// Caller is not the policy holder.
    NotPolicyHolder = 9,
    /// The policy term has ended.
    PolicyExpired = 10,
    /// Claim amount exceeds the policy's remaining coverage.
    ExceedsCoverage = 11,
    /// No claim exists with the given id.
    ClaimNotFound = 12,
    /// The claim has already been settled.
    ClaimNotPending = 13,
    /// Arithmetic overflowed.
    MathOverflow = 14,
    /// The pool is paused. Filing, purchases and withdrawals are blocked.
    Paused = 15,
    /// Policy term must be between MIN_TERM_SECS and MAX_TERM_SECS.
    InvalidTerm = 16,
    /// Maximum exposure must be greater than zero.
    InvalidExposureLimit = 17,
    /// Outstanding coverage would exceed `max_exposure_bps` of total assets.
    ExceedsExposureLimit = 18,
    /// The policy term has not ended, so its coverage cannot be released.
    PolicyNotExpired = 19,
    /// The policy's coverage has already been released.
    PolicyAlreadyReleased = 20,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ClaimStatus {
    Pending = 0,
    /// Approved and paid in full.
    Paid = 1,
    Rejected = 2,
    /// Approved, but the pool had too few assets to pay the full amount. The unpaid
    /// remainder is recorded in `Claim::shortfall`.
    PartiallyPaid = 3,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Policy {
    pub id: u64,
    pub lender: Address,
    pub asset_id: u64,
    /// Coverage still available for claims.
    pub remaining_coverage: i128,
    pub premium_paid: i128,
    pub expires_at: u64,
    /// Set once the policy has expired and its remaining coverage was removed from
    /// `PoolStats::total_exposure`.
    pub released: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Claim {
    pub id: u64,
    pub policy_id: u64,
    pub claimant: Address,
    pub amount: i128,
    /// Amount actually paid. `0` until paid.
    pub paid: i128,
    /// Approved amount that the pool could not pay: `amount - paid` once settled, `0` otherwise.
    pub shortfall: i128,
    pub status: ClaimStatus,
    pub filed_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PoolStats {
    /// Total tokens held for providers, including premiums.
    pub total_assets: i128,
    pub total_shares: i128,
    /// Maximum payouts owed on pending claims.
    pub reserved: i128,
    /// Coverage outstanding on policies not yet released: remaining coverage, net of payouts.
    pub total_exposure: i128,
    pub premiums_collected: i128,
    pub total_payouts: i128,
    pub claims_filed: u64,
    pub claims_paid: u64,
    pub claims_rejected: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Token,
    PremiumBps,
    TermSecs,
    MaxExposureBps,
    Paused,
    Stats,
    NextPolicyId,
    NextClaimId,
    Shares(Address),
    Policy(u64),
    /// Sum of pending claim amounts against a policy.
    PolicyReserved(u64),
    Claim(u64),
}

#[contract]
pub struct InsurancePool;

fn stored_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized))
}

fn require_stored_admin(env: &Env, admin: &Address) {
    if require_admin(admin, &stored_admin(env)).is_err() {
        panic_with_error!(env, ContractError::UnauthorizedAdmin);
    }
}

fn token_client(env: &Env) -> token::Client<'_> {
    let addr: Address = env
        .storage()
        .instance()
        .get(&DataKey::Token)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized));
    token::Client::new(env, &addr)
}

fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
}

fn ensure_not_paused(env: &Env) {
    if is_paused(env) {
        panic_with_error!(env, ContractError::Paused);
    }
}

fn load_stats(env: &Env) -> PoolStats {
    env.storage()
        .instance()
        .get(&DataKey::Stats)
        .unwrap_or(PoolStats {
            total_assets: 0,
            total_shares: 0,
            reserved: 0,
            total_exposure: 0,
            premiums_collected: 0,
            total_payouts: 0,
            claims_filed: 0,
            claims_paid: 0,
            claims_rejected: 0,
        })
}

fn save_stats(env: &Env, stats: &PoolStats) {
    env.storage().instance().set(&DataKey::Stats, stats);
}

fn load_policy(env: &Env, id: u64) -> Policy {
    env.storage()
        .persistent()
        .get(&DataKey::Policy(id))
        .unwrap_or_else(|| panic_with_error!(env, ContractError::PolicyNotFound))
}

fn save_policy(env: &Env, policy: &Policy) {
    let key = DataKey::Policy(policy.id);
    env.storage().persistent().set(&key, policy);
    extend_persistent_ttl(env, key);
}

fn policy_reserved(env: &Env, policy_id: u64) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::PolicyReserved(policy_id))
        .unwrap_or(0)
}

fn set_policy_reserved(env: &Env, policy_id: u64, amount: i128) {
    let key = DataKey::PolicyReserved(policy_id);
    env.storage().persistent().set(&key, &amount);
    extend_persistent_ttl(env, key);
}

fn load_claim(env: &Env, id: u64) -> Claim {
    env.storage()
        .persistent()
        .get(&DataKey::Claim(id))
        .unwrap_or_else(|| panic_with_error!(env, ContractError::ClaimNotFound))
}

fn save_claim(env: &Env, claim: &Claim) {
    let key = DataKey::Claim(claim.id);
    env.storage().persistent().set(&key, claim);
    extend_persistent_ttl(env, key);
}

fn max_exposure_bps(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::MaxExposureBps)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized))
}

fn mul_div(env: &Env, a: i128, b: i128, div: i128) -> i128 {
    a.checked_mul(b)
        .and_then(|v| v.checked_div(div))
        .unwrap_or_else(|| panic_with_error!(env, ContractError::MathOverflow))
}

fn next_id(env: &Env, key: DataKey) -> u64 {
    let id: u64 = env.storage().instance().get(&key).unwrap_or(1);
    env.storage().instance().set(&key, &(id + 1));
    id
}

#[contractimpl]
impl InsurancePool {
    /// Sets the admin, the pool token, the premium rate (basis points of coverage), the
    /// policy term in seconds, and the maximum outstanding coverage as basis points of total
    /// assets. Must be greater than zero.
    pub fn initialize(
        env: Env,
        admin: Address,
        token: Address,
        premium_bps: u32,
        term_secs: u64,
        max_exposure_bps: u32,
    ) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, ContractError::AlreadyInitialized);
        }
        if i128::from(premium_bps) > BPS_DENOMINATOR {
            panic_with_error!(&env, ContractError::InvalidPremiumRate);
        }
        if max_exposure_bps == 0 {
            panic_with_error!(&env, ContractError::InvalidExposureLimit);
        }
        env.storage()
            .instance()
            .set(&DataKey::MaxExposureBps, &max_exposure_bps);
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage()
            .instance()
            .set(&DataKey::PremiumBps, &premium_bps);
        env.storage().instance().set(&DataKey::TermSecs, &term_secs);
    }

    /// Deposits `amount` tokens and returns the shares minted for the provider.
    pub fn provide_capital(env: Env, provider: Address, amount: i128) -> i128 {
        ensure_not_paused(&env);
        provider.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, ContractError::InvalidAmount);
        }
        let mut stats = load_stats(&env);
        let shares = if stats.total_shares == 0 || stats.total_assets == 0 {
            amount
        } else {
            mul_div(&env, amount, stats.total_shares, stats.total_assets)
        };
        if shares <= 0 {
            panic_with_error!(&env, ContractError::InvalidAmount);
        }

        token_client(&env).transfer(&provider, &env.current_contract_address(), &amount);

        let key = DataKey::Shares(provider.clone());
        let held: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        env.storage().persistent().set(&key, &(held + shares));
        extend_persistent_ttl(&env, key);

        stats.total_assets += amount;
        stats.total_shares += shares;
        save_stats(&env, &stats);

        env.events()
            .publish((symbol_short!("ins_dep"),), (provider, amount, shares));
        shares
    }

    /// Burns `shares` and pays the provider their pro-rata value from unreserved assets.
    /// Returns the amount paid out.
    pub fn withdraw_capital(env: Env, provider: Address, shares: i128) -> i128 {
        ensure_not_paused(&env);
        provider.require_auth();
        if shares <= 0 {
            panic_with_error!(&env, ContractError::InvalidAmount);
        }
        let key = DataKey::Shares(provider.clone());
        let held: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        if shares > held {
            panic_with_error!(&env, ContractError::InsufficientShares);
        }

        let mut stats = load_stats(&env);
        let amount = mul_div(&env, shares, stats.total_assets, stats.total_shares);
        let free = stats.total_assets - stats.reserved;
        if amount > free {
            panic_with_error!(&env, ContractError::InsufficientFreeFunds);
        }

        env.storage().persistent().set(&key, &(held - shares));
        extend_persistent_ttl(&env, key);
        stats.total_assets -= amount;
        stats.total_shares -= shares;
        save_stats(&env, &stats);

        token_client(&env).transfer(&env.current_contract_address(), &provider, &amount);
        env.events()
            .publish((symbol_short!("ins_wdr"),), (provider, amount, shares));
        amount
    }

    /// Buys coverage for `asset_id` up to `coverage`. The premium is paid to the pool.
    /// Returns the policy id.
    ///
    /// Total outstanding coverage, including this policy, must not exceed `max_exposure_bps`
    /// of total assets after the premium is added.
    pub fn buy_policy(env: Env, lender: Address, asset_id: u64, coverage: i128) -> u64 {
        ensure_not_paused(&env);
        lender.require_auth();
        if coverage <= 0 {
            panic_with_error!(&env, ContractError::InvalidAmount);
        }
        let premium_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::PremiumBps)
            .unwrap_or(0);
        let term: u64 = env
            .storage()
            .instance()
            .get(&DataKey::TermSecs)
            .unwrap_or(0);
        let max_exposure_bps = max_exposure_bps(&env);
        let premium = mul_div(&env, coverage, i128::from(premium_bps), BPS_DENOMINATOR);

        let mut stats = load_stats(&env);
        let limit = mul_div(
            &env,
            stats.total_assets + premium,
            i128::from(max_exposure_bps),
            BPS_DENOMINATOR,
        );
        if stats.total_exposure + coverage > limit {
            panic_with_error!(&env, ContractError::ExceedsExposureLimit);
        }

        if premium > 0 {
            token_client(&env).transfer(&lender, &env.current_contract_address(), &premium);
        }

        stats.total_assets += premium;
        stats.premiums_collected += premium;
        stats.total_exposure += coverage;
        save_stats(&env, &stats);

        let id = next_id(&env, DataKey::NextPolicyId);
        let policy = Policy {
            id,
            lender: lender.clone(),
            asset_id,
            remaining_coverage: coverage,
            premium_paid: premium,
            expires_at: env.ledger().timestamp() + term,
            released: false,
        };
        save_policy(&env, &policy);

        env.events()
            .publish((symbol_short!("ins_pol"),), (id, lender, coverage, premium));
        id
    }

    /// Files a claim against a policy. Only the policy holder can file, and only before expiry.
    /// The claim amount must fit within the pool's free capital (`total_assets - reserved`),
    /// so reservations never exceed the assets that back them.
    pub fn file_claim(env: Env, lender: Address, policy_id: u64, amount: i128) -> u64 {
        ensure_not_paused(&env);
        lender.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, ContractError::InvalidAmount);
        }
        let policy = load_policy(&env, policy_id);
        if policy.lender != lender {
            panic_with_error!(&env, ContractError::NotPolicyHolder);
        }
        if env.ledger().timestamp() > policy.expires_at {
            panic_with_error!(&env, ContractError::PolicyExpired);
        }
        // Pending claims on the same policy share its coverage, so count them all.
        let reserved = policy_reserved(&env, policy_id);
        let exposure = reserved
            .checked_add(amount)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::MathOverflow));
        if exposure > policy.remaining_coverage {
            panic_with_error!(&env, ContractError::ExceedsCoverage);
        }
        let mut stats = load_stats(&env);
        if amount > stats.total_assets - stats.reserved {
            panic_with_error!(&env, ContractError::InsufficientFreeFunds);
        }
        set_policy_reserved(&env, policy_id, exposure);

        let id = next_id(&env, DataKey::NextClaimId);
        save_claim(
            &env,
            &Claim {
                id,
                policy_id,
                claimant: lender.clone(),
                amount,
                paid: 0,
                shortfall: 0,
                status: ClaimStatus::Pending,
                filed_at: env.ledger().timestamp(),
            },
        );

        stats.reserved += amount;
        stats.claims_filed += 1;
        save_stats(&env, &stats);

        env.events()
            .publish((symbol_short!("ins_clm"),), (id, policy_id, amount));
        id
    }

    /// Approves or rejects a pending claim. Admin only.
    ///
    /// Settlement rule: a claim filed on or before `expires_at` may be settled at any later
    /// time. There is no settlement deadline. A claim filed after expiry cannot exist, so
    /// settlement rejects any claim whose `filed_at` falls outside the policy term.
    ///
    /// Approval pays `min(claim amount, pool assets)`. The payout comes off the policy's
    /// remaining coverage, and the claim's reservation is released.
    ///
    /// If the payout is less than the claim amount, the claim is marked `PartiallyPaid`, the
    /// unpaid remainder is stored in `Claim::shortfall`, and a shortfall event is emitted.
    /// Approved claims, full or partial, count toward `PoolStats::claims_paid`.
    pub fn settle_claim(env: Env, admin: Address, claim_id: u64, approve: bool) -> i128 {
        require_stored_admin(&env, &admin);
        let mut claim = load_claim(&env, claim_id);
        if claim.status != ClaimStatus::Pending {
            panic_with_error!(&env, ContractError::ClaimNotPending);
        }
        let policy = load_policy(&env, claim.policy_id);
        if claim.filed_at > policy.expires_at {
            panic_with_error!(&env, ContractError::PolicyExpired);
        }

        let mut stats = load_stats(&env);
        stats.reserved -= claim.amount;
        set_policy_reserved(
            &env,
            claim.policy_id,
            policy_reserved(&env, claim.policy_id) - claim.amount,
        );

        if !approve {
            claim.status = ClaimStatus::Rejected;
            stats.claims_rejected += 1;
            save_stats(&env, &stats);
            save_claim(&env, &claim);
            env.events().publish((symbol_short!("ins_rej"),), claim_id);
            return 0;
        }

        let payout = claim.amount.min(stats.total_assets);
        let shortfall = claim.amount - payout;
        stats.total_assets -= payout;
        stats.total_payouts += payout;
        stats.claims_paid += 1;
        // A released policy's coverage already left `total_exposure`, so only live policies
        // give back the payout here.
        if !policy.released {
            stats.total_exposure -= payout;
        }
        save_stats(&env, &stats);

        let mut policy = policy;
        policy.remaining_coverage -= payout;
        save_policy(&env, &policy);

        claim.status = if shortfall > 0 {
            ClaimStatus::PartiallyPaid
        } else {
            ClaimStatus::Paid
        };
        claim.paid = payout;
        claim.shortfall = shortfall;
        save_claim(&env, &claim);

        if payout > 0 {
            token_client(&env).transfer(&env.current_contract_address(), &claim.claimant, &payout);
        }
        env.events()
            .publish((symbol_short!("ins_pay"),), (claim_id, payout));
        if shortfall > 0 {
            env.events()
                .publish((symbol_short!("ins_shrt"),), (claim_id, shortfall));
        }
        payout
    }

    /// Removes an expired policy's remaining coverage from `total_exposure`, freeing that
    /// capacity for new policies. Callable by anyone, once, after `expires_at`. Returns the
    /// coverage released.
    pub fn release_expired_policy(env: Env, policy_id: u64) -> i128 {
        let mut policy = load_policy(&env, policy_id);
        if env.ledger().timestamp() <= policy.expires_at {
            panic_with_error!(&env, ContractError::PolicyNotExpired);
        }
        if policy.released {
            panic_with_error!(&env, ContractError::PolicyAlreadyReleased);
        }
        let released = policy.remaining_coverage;
        let mut stats = load_stats(&env);
        stats.total_exposure -= released;
        save_stats(&env, &stats);

        policy.released = true;
        save_policy(&env, &policy);
        env.events()
            .publish((symbol_short!("ins_rel"),), (policy_id, released));
        released
    }

    /// Sets the premium rate in basis points for future policies. Admin only. Existing
    /// policies keep the premium they were bought with.
    pub fn set_premium_bps(env: Env, admin: Address, premium_bps: u32) {
        require_stored_admin(&env, &admin);
        if i128::from(premium_bps) > BPS_DENOMINATOR {
            panic_with_error!(&env, ContractError::InvalidPremiumRate);
        }
        let old: u32 = env
            .storage()
            .instance()
            .get(&DataKey::PremiumBps)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::PremiumBps, &premium_bps);
        env.events()
            .publish((symbol_short!("ins_prm"),), (admin, old, premium_bps));
    }

    /// Sets the policy term in seconds for future policies. Admin only. Must be between
    /// MIN_TERM_SECS and MAX_TERM_SECS. Existing policies keep their stored `expires_at`.
    pub fn set_term_secs(env: Env, admin: Address, term_secs: u64) {
        require_stored_admin(&env, &admin);
        if !(MIN_TERM_SECS..=MAX_TERM_SECS).contains(&term_secs) {
            panic_with_error!(&env, ContractError::InvalidTerm);
        }
        let old: u64 = env
            .storage()
            .instance()
            .get(&DataKey::TermSecs)
            .unwrap_or(0);
        env.storage().instance().set(&DataKey::TermSecs, &term_secs);
        env.events()
            .publish((symbol_short!("ins_trm"),), (admin, old, term_secs));
    }

    /// Halts filing, policy purchases and withdrawals. Settlement stays open so the admin can
    /// still resolve pending claims during an incident. Admin only.
    pub fn pause(env: Env, admin: Address) {
        require_stored_admin(&env, &admin);
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish(
            (symbol_short!("ins_pau"),),
            (admin, env.ledger().timestamp()),
        );
    }

    /// Resumes filing, policy purchases and withdrawals. Admin only.
    pub fn unpause(env: Env, admin: Address) {
        require_stored_admin(&env, &admin);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events().publish(
            (symbol_short!("ins_unp"),),
            (admin, env.ledger().timestamp()),
        );
    }

    /// Returns whether the pool is paused.
    pub fn is_paused(env: Env) -> bool {
        is_paused(&env)
    }

    /// Returns a provider's share balance.
    pub fn get_shares(env: Env, provider: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Shares(provider))
            .unwrap_or(0)
    }

    pub fn get_policy(env: Env, policy_id: u64) -> Policy {
        load_policy(&env, policy_id)
    }

    pub fn get_claim(env: Env, claim_id: u64) -> Claim {
        load_claim(&env, claim_id)
    }

    pub fn get_stats(env: Env) -> PoolStats {
        load_stats(&env)
    }

    /// Premium, in pool tokens, for buying `coverage` at the configured rate.
    pub fn quote_premium(env: Env, coverage: i128) -> i128 {
        let premium_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::PremiumBps)
            .unwrap_or(0);
        mul_div(&env, coverage, i128::from(premium_bps), BPS_DENOMINATOR)
    }
}
