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
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ClaimStatus {
    Pending = 0,
    Paid = 1,
    Rejected = 2,
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
    Stats,
    NextPolicyId,
    NextClaimId,
    Shares(Address),
    Policy(u64),
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

fn load_stats(env: &Env) -> PoolStats {
    env.storage()
        .instance()
        .get(&DataKey::Stats)
        .unwrap_or(PoolStats {
            total_assets: 0,
            total_shares: 0,
            reserved: 0,
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
    /// Sets the admin, the pool token, the premium rate (basis points of coverage) and the
    /// policy term in seconds.
    pub fn initialize(env: Env, admin: Address, token: Address, premium_bps: u32, term_secs: u64) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, ContractError::AlreadyInitialized);
        }
        if i128::from(premium_bps) > BPS_DENOMINATOR {
            panic_with_error!(&env, ContractError::InvalidPremiumRate);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage()
            .instance()
            .set(&DataKey::PremiumBps, &premium_bps);
        env.storage().instance().set(&DataKey::TermSecs, &term_secs);
    }

    /// Deposits `amount` tokens and returns the shares minted for the provider.
    pub fn provide_capital(env: Env, provider: Address, amount: i128) -> i128 {
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
    pub fn buy_policy(env: Env, lender: Address, asset_id: u64, coverage: i128) -> u64 {
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
        let premium = mul_div(&env, coverage, i128::from(premium_bps), BPS_DENOMINATOR);

        if premium > 0 {
            token_client(&env).transfer(&lender, &env.current_contract_address(), &premium);
        }

        let mut stats = load_stats(&env);
        stats.total_assets += premium;
        stats.premiums_collected += premium;
        save_stats(&env, &stats);

        let id = next_id(&env, DataKey::NextPolicyId);
        let policy = Policy {
            id,
            lender: lender.clone(),
            asset_id,
            remaining_coverage: coverage,
            premium_paid: premium,
            expires_at: env.ledger().timestamp() + term,
        };
        save_policy(&env, &policy);

        env.events()
            .publish((symbol_short!("ins_pol"),), (id, lender, coverage, premium));
        id
    }

    /// Files a claim against a policy. Only the policy holder can file, and only before expiry.
    pub fn file_claim(env: Env, lender: Address, policy_id: u64, amount: i128) -> u64 {
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
        if amount > policy.remaining_coverage {
            panic_with_error!(&env, ContractError::ExceedsCoverage);
        }

        let id = next_id(&env, DataKey::NextClaimId);
        save_claim(
            &env,
            &Claim {
                id,
                policy_id,
                claimant: lender.clone(),
                amount,
                paid: 0,
                status: ClaimStatus::Pending,
                filed_at: env.ledger().timestamp(),
            },
        );

        let mut stats = load_stats(&env);
        stats.reserved += amount;
        stats.claims_filed += 1;
        save_stats(&env, &stats);

        env.events()
            .publish((symbol_short!("ins_clm"),), (id, policy_id, amount));
        id
    }

    /// Approves or rejects a pending claim. Admin only.
    ///
    /// Approval pays `min(claim amount, pool assets)`. The payout comes off the policy's
    /// remaining coverage, and the claim's reservation is released.
    pub fn settle_claim(env: Env, admin: Address, claim_id: u64, approve: bool) -> i128 {
        require_stored_admin(&env, &admin);
        let mut claim = load_claim(&env, claim_id);
        if claim.status != ClaimStatus::Pending {
            panic_with_error!(&env, ContractError::ClaimNotPending);
        }

        let mut stats = load_stats(&env);
        stats.reserved -= claim.amount;

        if !approve {
            claim.status = ClaimStatus::Rejected;
            stats.claims_rejected += 1;
            save_stats(&env, &stats);
            save_claim(&env, &claim);
            env.events().publish((symbol_short!("ins_rej"),), claim_id);
            return 0;
        }

        let payout = claim.amount.min(stats.total_assets);
        stats.total_assets -= payout;
        stats.total_payouts += payout;
        stats.claims_paid += 1;
        save_stats(&env, &stats);

        let mut policy = load_policy(&env, claim.policy_id);
        policy.remaining_coverage -= payout;
        save_policy(&env, &policy);

        claim.status = ClaimStatus::Paid;
        claim.paid = payout;
        save_claim(&env, &claim);

        if payout > 0 {
            token_client(&env).transfer(&env.current_contract_address(), &claim.claimant, &payout);
        }
        env.events()
            .publish((symbol_short!("ins_pay"),), (claim_id, payout));
        payout
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
