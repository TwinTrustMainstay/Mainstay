#![no_std]

//! Engineer performance bonds (issue #1735).
//!
//! Engineers lock tokens (typically XLM through the native Stellar Asset Contract) as a
//! performance bond. The admin records job outcomes, opens disputes and slashes bonds
//! when work is poor. Slashed tokens go to a treasury address.
//!
//! Recovery rules:
//! - A poor outcome resets the engineer's good-job streak to zero.
//! - Once an engineer has been slashed, withdrawals are blocked until the streak reaches
//!   [`RECOVERY_STREAK`]. Good performance is what restores withdrawal rights.
//! - An open dispute blocks withdrawal and makes the engineer not bonded.

#[cfg(test)]
mod test;

use shared::{extend_persistent_ttl, require_admin};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, token,
    Address, Env, Vec,
};

/// Consecutive good-job outcomes an engineer must reach after a slash before withdrawing.
pub const RECOVERY_STREAK: u32 = 3;

/// Maximum history entries kept per engineer. Older entries are dropped first.
pub const MAX_HISTORY: u32 = 50;

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
    /// Requested amount exceeds the engineer's bonded balance.
    InsufficientBond = 5,
    /// Withdrawal would leave a non-zero bond below the minimum.
    BelowMinimumBond = 6,
    /// Withdrawal is blocked while a dispute is open.
    DisputeOpen = 7,
    /// Engineer was slashed and has not yet recovered (good-job streak too short).
    RecoveryIncomplete = 8,
    /// A dispute is already open for this engineer.
    DisputeAlreadyOpen = 9,
    /// No dispute is open for this engineer.
    NoOpenDispute = 10,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum EventKind {
    Posted = 0,
    Slashed = 1,
    Withdrawn = 2,
    GoodJob = 3,
    PoorJob = 4,
    DisputeOpened = 5,
    DisputeResolved = 6,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bond {
    pub staked: i128,
    pub slashed_total: i128,
    pub good_streak: u32,
    pub dispute_open: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BondEvent {
    pub kind: EventKind,
    pub amount: i128,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Token,
    Treasury,
    MinBond,
    Bond(Address),
    History(Address),
}

#[contract]
pub struct EngineerBond;

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

fn load_bond(env: &Env, engineer: &Address) -> Bond {
    env.storage()
        .persistent()
        .get(&DataKey::Bond(engineer.clone()))
        .unwrap_or(Bond {
            staked: 0,
            slashed_total: 0,
            good_streak: 0,
            dispute_open: false,
        })
}

fn save_bond(env: &Env, engineer: &Address, bond: &Bond) {
    let key = DataKey::Bond(engineer.clone());
    env.storage().persistent().set(&key, bond);
    extend_persistent_ttl(env, key);
}

fn record(env: &Env, engineer: &Address, kind: EventKind, amount: i128) {
    let key = DataKey::History(engineer.clone());
    let mut history: Vec<BondEvent> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    if history.len() >= MAX_HISTORY {
        history.remove(0);
    }
    history.push_back(BondEvent {
        kind,
        amount,
        timestamp: env.ledger().timestamp(),
    });
    env.storage().persistent().set(&key, &history);
    extend_persistent_ttl(env, key);
}

#[contractimpl]
impl EngineerBond {
    /// Sets the admin, bond token, treasury (slash destination) and minimum bond.
    pub fn initialize(env: Env, admin: Address, token: Address, treasury: Address, min_bond: i128) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, ContractError::AlreadyInitialized);
        }
        if min_bond < 0 {
            panic_with_error!(&env, ContractError::InvalidAmount);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::Treasury, &treasury);
        env.storage().instance().set(&DataKey::MinBond, &min_bond);
    }

    /// Locks `amount` tokens from the engineer as a performance bond.
    pub fn post_bond(env: Env, engineer: Address, amount: i128) {
        engineer.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, ContractError::InvalidAmount);
        }

        token_client(&env).transfer(&engineer, &env.current_contract_address(), &amount);

        let mut bond = load_bond(&env, &engineer);
        bond.staked += amount;
        save_bond(&env, &engineer, &bond);
        record(&env, &engineer, EventKind::Posted, amount);

        env.events()
            .publish((symbol_short!("bond_post"),), (engineer, amount));
    }

    /// Records the outcome of a job. A good job extends the streak, a poor one resets it.
    pub fn record_performance(env: Env, admin: Address, engineer: Address, good: bool) {
        require_stored_admin(&env, &admin);
        let mut bond = load_bond(&env, &engineer);
        if good {
            bond.good_streak = bond.good_streak.saturating_add(1);
            record(&env, &engineer, EventKind::GoodJob, 0);
        } else {
            bond.good_streak = 0;
            record(&env, &engineer, EventKind::PoorJob, 0);
        }
        save_bond(&env, &engineer, &bond);
    }

    /// Opens a dispute against the engineer. Blocks withdrawal until resolved.
    pub fn open_dispute(env: Env, admin: Address, engineer: Address) {
        require_stored_admin(&env, &admin);
        let mut bond = load_bond(&env, &engineer);
        if bond.dispute_open {
            panic_with_error!(&env, ContractError::DisputeAlreadyOpen);
        }
        bond.dispute_open = true;
        save_bond(&env, &engineer, &bond);
        record(&env, &engineer, EventKind::DisputeOpened, 0);
    }

    /// Closes an open dispute without slashing.
    pub fn resolve_dispute(env: Env, admin: Address, engineer: Address) {
        require_stored_admin(&env, &admin);
        let mut bond = load_bond(&env, &engineer);
        if !bond.dispute_open {
            panic_with_error!(&env, ContractError::NoOpenDispute);
        }
        bond.dispute_open = false;
        save_bond(&env, &engineer, &bond);
        record(&env, &engineer, EventKind::DisputeResolved, 0);
    }

    /// Slashes `amount` from the engineer's bond and transfers it to the treasury.
    /// A slash resets the good-job streak, so recovery must be earned again.
    pub fn slash(env: Env, admin: Address, engineer: Address, amount: i128) {
        require_stored_admin(&env, &admin);
        if amount <= 0 {
            panic_with_error!(&env, ContractError::InvalidAmount);
        }
        let mut bond = load_bond(&env, &engineer);
        if amount > bond.staked {
            panic_with_error!(&env, ContractError::InsufficientBond);
        }

        bond.staked -= amount;
        bond.slashed_total += amount;
        bond.good_streak = 0;
        save_bond(&env, &engineer, &bond);

        let treasury: Address = env
            .storage()
            .instance()
            .get(&DataKey::Treasury)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        token_client(&env).transfer(&env.current_contract_address(), &treasury, &amount);
        record(&env, &engineer, EventKind::Slashed, amount);

        env.events()
            .publish((symbol_short!("bond_slsh"),), (engineer, amount));
    }

    /// Withdraws `amount` of the engineer's bond.
    ///
    /// Blocked while a dispute is open. After any slash, requires a good-job streak of at
    /// least [`RECOVERY_STREAK`]. A non-zero remaining bond must stay at or above the minimum.
    pub fn withdraw_bond(env: Env, engineer: Address, amount: i128) {
        engineer.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, ContractError::InvalidAmount);
        }
        let mut bond = load_bond(&env, &engineer);
        if bond.dispute_open {
            panic_with_error!(&env, ContractError::DisputeOpen);
        }
        if amount > bond.staked {
            panic_with_error!(&env, ContractError::InsufficientBond);
        }
        if bond.slashed_total > 0 && bond.good_streak < RECOVERY_STREAK {
            panic_with_error!(&env, ContractError::RecoveryIncomplete);
        }
        let remaining = bond.staked - amount;
        if remaining > 0 && remaining < min_bond(&env) {
            panic_with_error!(&env, ContractError::BelowMinimumBond);
        }

        bond.staked = remaining;
        save_bond(&env, &engineer, &bond);
        token_client(&env).transfer(&env.current_contract_address(), &engineer, &amount);
        record(&env, &engineer, EventKind::Withdrawn, amount);

        env.events()
            .publish((symbol_short!("bond_wdrw"),), (engineer, amount));
    }

    /// True when the engineer's bond meets the minimum and no dispute is open.
    pub fn is_bonded(env: Env, engineer: Address) -> bool {
        let bond = load_bond(&env, &engineer);
        !bond.dispute_open && bond.staked > 0 && bond.staked >= min_bond(&env)
    }

    pub fn get_bond(env: Env, engineer: Address) -> Bond {
        load_bond(&env, &engineer)
    }

    pub fn get_history(env: Env, engineer: Address) -> Vec<BondEvent> {
        env.storage()
            .persistent()
            .get(&DataKey::History(engineer))
            .unwrap_or_else(|| Vec::new(&env))
    }
}

fn min_bond(env: &Env) -> i128 {
    env.storage().instance().get(&DataKey::MinBond).unwrap_or(0)
}
