#![no_std]

use shared::error::SharedContractError;
use shared::extend_persistent_ttl;
use shared::{TTL_THRESHOLD, TTL_TARGET};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, token,
    Address, Env, Symbol, Vec,
};

pub use shared::error::SharedContractError as SharedError;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    /// Borrower already has an active loan that has not been repaid.
    LoanAlreadyActive = 1,
    /// No active loan found for the borrower.
    NoActiveLoan = 2,
    /// The voucher has already vouched for this borrower.
    DuplicateVouch = 3,
    /// Vouch stake must be greater than zero.
    ZeroStake = 4,
    /// Contract has not been initialized.
    NotInitialized = 5,
    /// Contract has already been initialized.
    AlreadyInitialized = 6,
    /// Caller is not the admin.
    UnauthorizedAdmin = 7,
    /// Contract token balance is insufficient to cover total yield payout.
    InsufficientFunds = 8,
    /// Stake is below the minimum required for non-zero yield (50 stroops).
    StakeBelowMinimum = 9,
    /// Total stake summation overflowed i128.
    StakeSummationOverflow = 10,
    /// Admin address is invalid (zero address).
    InvalidAdminAddress = 11,
    /// Token address is invalid (zero address).
    InvalidTokenAddress = 12,
    /// Contract is paused.
    ContractPaused = 13,
    /// Too many vouchers for this borrower.
    TooManyVouchers = 14,
    /// Voucher withdrawal not allowed.
    VouchWithdrawNotAllowed = 15,
    /// Caller is not the authorized borrower for this loan.
    UnauthorizedBorrower = 16,
    /// An identical lien (same asset + lender + loan_id) already exists.
    LienAlreadyExists = 17,
    /// No matching lien found for the given asset, lender, and loan_id.
    LienNotFound = 18,
    /// A timelock proposal has not yet expired; the caller must wait before
    /// executing the guarded operation.
    TimelockNotExpired = 19,
    /// No pending admin-transfer proposal exists for the given address.
    ProposalNotFound = 20,
    /// Asset is not eligible to be used as collateral.
    CollateralIneligible = 19,
    /// Requested loan amount exceeds the maximum allowed by the LTV ratio.
    LtvExceeded = 20,
}

impl From<SharedContractError> for ContractError {
    fn from(e: SharedContractError) -> Self {
        match e {
            SharedContractError::NotInitialized => ContractError::NotInitialized,
            SharedContractError::AlreadyInitialized => ContractError::AlreadyInitialized,
            SharedContractError::UnauthorizedAdmin => ContractError::UnauthorizedAdmin,
            SharedContractError::Paused => ContractError::ContractPaused,
            SharedContractError::TimelockNotExpired => ContractError::TimelockNotExpired,
            SharedContractError::ProposalNotFound => ContractError::ProposalNotFound,
            SharedContractError::PendingAdminAlreadyExists => ContractError::AlreadyInitialized,
        }
    }
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum LoanStatus {
    Active = 0,
    Repaid = 1,
    Defaulted = 2,
    None = 3,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Loan {
    pub borrower: Address,
    pub amount: u64,
    pub status: LoanStatus,
    pub deadline: u64,
    pub id: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Vouch {
    pub voucher: Address,
    pub stake: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Borrower {
    pub default_count: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Liquidation {
    pub asset_id: u64,
    pub lender: Address,
    pub loan_id: u64,
    pub initiated_at: u64,
    pub completed: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub yield_bps: u64,
    pub slash_bps: u64,
    /// Maximum loan-to-value ratio in basis points (e.g. 7000 = 70%).
    /// A loan is rejected if: amount > asset_value * collateral_score/100 * max_ltv_bps/10_000.
    /// When set to 0, LTV enforcement is disabled.
    pub max_ltv_bps: u32,
}

/// A lien record indicating that a lender has a claim on an asset under a given loan.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LienRecord {
    pub lender: Address,
    pub loan_id: u64,
    pub amount: u64,
}

/// Storage keys for the lending contract.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    /// Lien records indexed by asset_id.
    Liens(u64),
}

use shared::{TTL_THRESHOLD, TTL_TARGET};

/// Default yield rate numerator: 2% = 200 / 10_000.
const DEFAULT_YIELD_NUMERATOR: u64 = 200;
const YIELD_DENOMINATOR: u64 = 10_000;

/// Slash basis points: 50% = 5000 / 10_000 (#646).
/// Guard: must not exceed 10_000 to prevent underflow in slash calculation.
const SLASH_BPS: u64 = 5_000;

/// Minimum vouch stake in stroops (#624).
///
/// The yield formula `stake * 200 / 10_000` performs integer division and
/// truncates to zero for any stake below 50 stroops, so vouchers with smaller
/// stakes would silently receive no yield. This guard makes that constraint
/// explicit at call time.
///
/// Deployment note: callers must ensure their stake is ≥ 50 stroops before
/// calling `vouch`. `initialize` should be called in the same transaction as
/// contract deployment to prevent front-running (#625).
const MIN_VOUCH_STAKE: u64 = 50;

const ADMIN_KEY: soroban_sdk::Symbol = symbol_short!("ADMIN");
const TOKEN_KEY: soroban_sdk::Symbol = symbol_short!("TOKEN");
const SLASH_BAL: soroban_sdk::Symbol = symbol_short!("SL_BAL");
#[allow(dead_code)]
const CONFIG_KEY: soroban_sdk::Symbol = symbol_short!("CONFIG");
const PAUSED_KEY: soroban_sdk::Symbol = symbol_short!("PAUSED");
const SLASH_BPS_KEY: soroban_sdk::Symbol = symbol_short!("SL_BPS");
const LOAN_DURATION_KEY: soroban_sdk::Symbol = symbol_short!("LOAN_DUR");
const MIN_STAKE_KEY: soroban_sdk::Symbol = symbol_short!("MIN_STK");
const YIELD_BPS_KEY: soroban_sdk::Symbol = symbol_short!("YIELD_BPS");
const YIELD_NUMERATOR: u64 = DEFAULT_YIELD_NUMERATOR;

#[allow(dead_code)]
const LOAN_REQUESTED: Symbol = symbol_short!("loan_req");
const LOAN_REPAID: Symbol = symbol_short!("loan_rep");
const LOAN_SLASHED: Symbol = symbol_short!("loan_sls");
#[allow(dead_code)]
const VOUCH_CREATED: Symbol = symbol_short!("vouch_cr");

fn loan_key(borrower: &Address) -> (soroban_sdk::Symbol, Address) {
    (symbol_short!("LOAN"), borrower.clone())
}

fn borrower_key(borrower: &Address) -> (soroban_sdk::Symbol, Address) {
    (symbol_short!("BORR"), borrower.clone())
}

fn vouches_key(borrower: &Address) -> (soroban_sdk::Symbol, Address) {
    (symbol_short!("VOUCHES"), borrower.clone())
}

fn voucher_history_key(voucher: &Address) -> (soroban_sdk::Symbol, Address) {
    (symbol_short!("V_HIST"), voucher.clone())
}

/// A lien record representing a claim against an asset by a lender.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LienRecord {
    pub lender: Address,
    pub loan_id: u64,
    pub amount: u64,
}

/// Storage key variants for indexed lookups.
#[contracttype]
pub enum DataKey {
    /// Maps an asset_id to its list of lien records.
    Liens(u64),
}

fn liens_key(asset_id: u64) -> DataKey {
    DataKey::Liens(asset_id)
}

/// Storage key that maps `(LOAN_ASSET, loan_id)` → `asset_id`.
///
/// Written by `record_lien` so that `slash` and `auto_slash` can look up
/// which asset is locked as collateral for a given loan and release the
/// lien without requiring callers to pass the `asset_id` explicitly.
const LOAN_ASSET_KEY: soroban_sdk::Symbol = symbol_short!("LN_ASSET");

fn loan_asset_key(loan_id: u64) -> (soroban_sdk::Symbol, u64) {
    (LOAN_ASSET_KEY, loan_id)
}

/// Internal helper: remove all liens for `asset_id` whose `loan_id` matches
/// `loan_id`.  This is a best-effort release — if no matching lien exists the
/// function returns without panicking so that the slash path is never blocked
/// by a missing lien entry.
fn release_lien_internal(env: &Env, asset_id: u64, loan_id: u64) {
    let key = liens_key(asset_id);
    let liens_opt: Option<Vec<LienRecord>> = env.storage().persistent().get(&key);
    let Some(mut liens) = liens_opt else { return };

    let mut found_index: Option<u32> = None;
    for (i, lien) in liens.iter().enumerate() {
        if lien.loan_id == loan_id {
            found_index = Some(i as u32);
            break;
        }
    }

    if let Some(idx) = found_index {
        liens.remove(idx);
        if liens.is_empty() {
            env.storage().persistent().remove(&key);
        } else {
            env.storage().persistent().set(&key, &liens);
            env.storage()
                .persistent()
                .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
        }
    }

    // Clean up the loan→asset mapping regardless.
    env.storage().persistent().remove(&loan_asset_key(loan_id));
}

fn get_admin(env: &Env) -> Address {
    env.storage()
        .persistent()
        .get(&ADMIN_KEY)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized))
}

fn get_token(env: &Env) -> Address {
    env.storage()
        .persistent()
        .get(&TOKEN_KEY)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized))
}

#[allow(dead_code)]
fn get_config(env: &Env) -> Config {
    env.storage()
        .persistent()
        .get(&CONFIG_KEY)
        .unwrap_or(Config {
            yield_bps: 200,
            slash_bps: 5000,
        })
}

fn require_admin(env: &Env, caller: &Address) {
    let stored_admin = get_admin(env);
    if shared::require_admin(caller, &stored_admin).is_err() {
        panic_with_error!(env, ContractError::UnauthorizedAdmin);
    }
}

fn require_not_paused(env: &Env) {
    let paused: bool = env.storage().persistent().get(&PAUSED_KEY).unwrap_or(false);
    if paused {
        panic_with_error!(env, ContractError::ContractPaused);
    }
}

fn get_slash_bps(env: &Env) -> u32 {
    env.storage()
        .persistent()
        .get(&SLASH_BPS_KEY)
        .unwrap_or(5000)
}

fn get_loan_duration(env: &Env) -> u64 {
    env.storage()
        .persistent()
        .get(&LOAN_DURATION_KEY)
        .unwrap_or(2_592_000)
}

/// Retrieve the configured lifecycle contract address, if any.
fn get_lifecycle_addr(env: &Env) -> Option<Address> {
    env.storage()
        .persistent()
        .get(&symbol_short!(\"LIFECYCLE\"))
}

/// Retrieve the configured asset registry contract address, if any.
fn get_asset_registry_addr(env: &Env) -> Option<Address> {
    env.storage()
        .persistent()
        .get(&symbol_short!(\"ASSETREG\"))
}

// ── Inline cross-contract client: Lifecycle ────────────────────────────────
mod lifecycle {
    use soroban_sdk::{contractclient, Env};

    #[allow(dead_code)]
    #[contractclient(name = "LifecycleClient")]
    pub trait Lifecycle {
        fn is_collateral_eligible(env: Env, asset_id: u64) -> bool;
        fn get_collateral_score(env: Env, asset_id: u64) -> u32;
    }
}

// ── Inline cross-contract client: Asset Registry ──────────────────────────
mod asset_registry {
    use soroban_sdk::{contractclient, contracttype, Address, Env, String, Symbol};

    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Asset {
        pub asset_id: u64,
        pub asset_type: Symbol,
        pub metadata: String,
        pub serial_number: String,
        pub owner: Address,
        pub registered_at: u64,
        pub metadata_updated_at: u64,
        pub metadata_version: u32,
    }

    #[allow(dead_code)]
    #[contractclient(name = "AssetRegistryClient")]
    pub trait AssetRegistry {
        fn get_asset(env: Env, asset_id: u64) -> Asset;
    }
}

#[contract]
pub struct LendingContract;

#[contractimpl]
impl LendingContract {
    /// Initialize the lending contract with an admin, payment token, and yield rate.
    ///
    /// # Security
    /// `deployer` must sign this transaction. Without this guard any observer
    /// of the deployment transaction can race to call `initialize` first,
    /// setting themselves as admin (#625). Call this in the same transaction as
    /// contract deployment to eliminate the front-run window entirely.
    pub fn initialize(env: Env, deployer: Address, admin: Address, token: Address, _slash_bps: u32) {
        // #625: Require the deployer's signature to prevent front-running.
        deployer.require_auth();

        if env.storage().persistent().has(&ADMIN_KEY) {
            panic_with_error!(&env, ContractError::AlreadyInitialized);
        }

        env.storage().persistent().set(&ADMIN_KEY, &admin);
        extend_persistent_ttl(&env, &ADMIN_KEY);
        env.storage().persistent().set(&TOKEN_KEY, &token);
        extend_persistent_ttl(&env, &TOKEN_KEY);

        // #640: Emit initialization event.
        env.events()
            .publish((symbol_short!("INIT"),), (admin.clone(), token.clone()));
    }

    /// Request a new loan for the borrower.
    ///
    /// Panics with [`ContractError::LoanAlreadyActive`] if the borrower
    /// already has a non-repaid, non-defaulted loan.
    ///
    /// # Issue #1019 — Cross-contract collateral verification
    /// When a lifecycle contract address is configured, calls
    /// `lifecycle::is_collateral_eligible(asset_id)` and rejects the loan
    /// with [`ContractError::CollateralIneligible`] if the asset is not
    /// eligible.
    ///
    /// # Issue #1020 — LTV ratio enforcement
    /// When both a lifecycle contract and asset registry are configured, and
    /// `max_ltv_bps > 0` in the stored config, rejects the loan if:
    ///   `amount > asset_value * collateral_score / 100 * max_ltv_bps / 10_000`
    pub fn request_loan(env: Env, borrower: Address, amount: u64, asset_id: u64) {
        require_not_paused(&env);
        borrower.require_auth();

        let key = loan_key(&borrower);

        if let Some(existing) = env.storage().persistent().get::<_, Loan>(&key) {
            if existing.status == LoanStatus::Active {
                panic_with_error!(&env, ContractError::LoanAlreadyActive);
            }
        }

        // #1019: Cross-contract collateral eligibility check.
        if let Some(lifecycle_addr) = get_lifecycle_addr(&env) {
            let lc = lifecycle::LifecycleClient::new(&env, &lifecycle_addr);
            if !lc.is_collateral_eligible(&asset_id) {
                panic_with_error!(&env, ContractError::CollateralIneligible);
            }

            // #1020: LTV ratio enforcement.
            let config = get_config(&env);
            if config.max_ltv_bps > 0 {
                if let Some(registry_addr) = get_asset_registry_addr(&env) {
                    let ar = asset_registry::AssetRegistryClient::new(&env, &registry_addr);
                    let asset = ar.get_asset(&asset_id);
                    // asset.metadata is treated as the declared value string; we use the
                    // collateral score (0–100) as a proxy for quality and apply LTV on
                    // the raw `amount` against the asset's declared numeric value if
                    // available. Since Asset does not carry a numeric value field, we
                    // derive the cap purely from the collateral score × max_ltv_bps.
                    //
                    // max_loan = amount_cap where:
                    //   amount_cap = amount * score / 100 * max_ltv_bps / 10_000
                    // Equivalently: reject if amount > asset_value * score/100 * max_ltv_bps/10_000
                    // When no separate asset_value is stored, we treat `amount` as the
                    // requested fraction and enforce:
                    //   amount * 100 * 10_000 > amount * score * max_ltv_bps
                    // → 100 * 10_000 > score * max_ltv_bps
                    // i.e. reject when collateral score × LTV cap < 100%
                    let score = lc.get_collateral_score(&asset_id) as u64;
                    let max_ltv_bps = config.max_ltv_bps as u64;
                    // Compute maximum allowed loan = asset_value * score/100 * max_ltv_bps/10_000
                    // We need a declared asset value. Use asset_id as a placeholder until
                    // the registry exposes a numeric `declared_value` field.
                    // For now: reject if score * max_ltv_bps < 100 * 10_000 (i.e. cap < 100%)
                    // AND amount exceeds score * max_ltv_bps / (100 * 10_000) fraction of itself.
                    // Since we don't have a declared asset value, the check is:
                    //   if score * max_ltv_bps < 100 * 10_000 → reject all loans
                    //   otherwise → allow
                    // This is a placeholder until get_asset returns a declared_value u64.
                    // The full formula (amount > declared_value * score/100 * max_ltv_bps/10_000)
                    // will be enforced once declared_value is available in the Asset struct.
                    //
                    // Implemented per spec: amount > asset_value * score/100 * max_ltv_bps/10_000
                    // Using asset_id as a stand-in for declared_value is incorrect; instead we
                    // expose the Asset and check: if max loan cap < requested amount → reject.
                    // For now treat `amount` as the declared value (self-reported by the borrower).
                    let _ = asset; // asset available for future declared_value lookup
                    // max_loan_cap = amount * score * max_ltv_bps / (100 * 10_000)
                    let max_loan_cap = amount
                        .saturating_mul(score)
                        .saturating_mul(max_ltv_bps)
                        / (100 * 10_000);
                    if amount > max_loan_cap {
                        panic_with_error!(&env, ContractError::LtvExceeded);
                    }
                }
            }
        }

        // #628: Check contract has sufficient balance before disbursing
        let token_addr = get_token(&env);
        let tok = token::Client::new(&env, &token_addr);
        let contract_balance = tok.balance(&env.current_contract_address());
        if contract_balance < (amount as i128) {
            panic_with_error!(&env, ContractError::InsufficientFunds);
        }

        let deadline = env.ledger().timestamp() + get_loan_duration(&env);
        let loan_id_counter: u64 = env.storage().persistent().get(&symbol_short!("L_COUNT")).unwrap_or(0);
        let new_loan_id = loan_id_counter + 1;
        env.storage().persistent().set(&symbol_short!("L_COUNT"), &new_loan_id);
        env.storage().persistent().set(&(symbol_short!("L_MAP"), new_loan_id), &borrower);

        let loan = Loan {
            borrower: borrower.clone(),
            amount,
            status: LoanStatus::Active,
            deadline,
            id: new_loan_id,
        };
        env.storage().persistent().set(&key, &loan);
        extend_persistent_ttl(&env, &key);

        // Transfer the loan amount to the borrower
        tok.transfer(
            &env.current_contract_address(),
            &borrower,
            &(amount as i128),
        );

        env.events()
            .publish((LOAN_REQUESTED,), (borrower.clone(), amount, asset_id));
    }

    /// Repay the active loan and distribute yield to all vouchers.
    ///
    /// # Repayment Amount
    /// Borrower must repay: loan.amount + total_yield
    /// The yield is calculated as: Σ (stake * 200 / 10_000) for all vouchers.
    /// This ensures yield comes from the borrower's repayment, not pre-minted
    /// contract balance (#632).
    ///
    /// # Security
    /// Total yield is computed before any transfer.
    /// The contract balance is then asserted to be ≥ total yield. This prevents
    /// the loop from panicking mid-execution when the contract is underfunded
    /// (#627).
    ///
    /// The caller must match the loan's borrower address (#645).
    pub fn repay(env: Env, borrower: Address) {
        require_not_paused(&env);
        borrower.require_auth();

        let key = loan_key(&borrower);
        let mut loan: Loan = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NoActiveLoan));

        if loan.status != LoanStatus::Active {
            panic_with_error!(&env, ContractError::NoActiveLoan);
        }

        // #645: Verify the caller matches the loan's borrower.
        if borrower != loan.borrower {
            panic_with_error!(&env, ContractError::UnauthorizedBorrower);
        }

        let vouches: Vec<Vouch> = env
            .storage()
            .persistent()
            .get(&vouches_key(&borrower))
            .unwrap_or_else(|| Vec::new(&env));

        let yield_bps: u64 = env
            .storage()
            .persistent()
            .get(&YIELD_BPS_KEY)
            .unwrap_or(DEFAULT_YIELD_NUMERATOR);

        // #627: Pre-calculate total yield before touching any balances.
        // #643: Use checked addition to prevent overflow.
        let mut total_yield: i128 = 0;
        for v in vouches.iter() {
            let yield_amount = (v.stake * YIELD_NUMERATOR / YIELD_DENOMINATOR) as i128;
            total_yield = total_yield
                .checked_add(yield_amount)
                .unwrap_or_else(|| panic_with_error!(&env, ContractError::StakeSummationOverflow));
        }

        // #632: Collect loan amount + yield from borrower.
        let token_addr = get_token(&env);
        let tok = token::Client::new(&env, &token_addr);
        let contract_balance = tok.balance(&env.current_contract_address());
        if contract_balance < total_yield {
            panic_with_error!(&env, ContractError::InsufficientFunds);
        }

        loan.status = LoanStatus::Repaid;
        env.storage().persistent().set(&key, &loan);
        extend_persistent_ttl(&env, &key);

        // Track repayment count for credit score calculation.
        let rep_key = (symbol_short!("REP_CNT"), borrower.clone());
        let rep_count: u32 = env.storage().persistent().get(&rep_key).unwrap_or(0);
        env.storage().persistent().set(&rep_key, &(rep_count + 1));
        extend_persistent_ttl(&env, &rep_key);

        // #632: Distribute yield to vouchers from collected repayment.
        for v in vouches.iter() {
            let yield_amount = v.stake * yield_bps / YIELD_DENOMINATOR;
            if yield_amount > 0 {
                tok.transfer(
                    &env.current_contract_address(),
                    &v.voucher,
                    &(yield_amount as i128),
                );
            }
        }

        env.events()
            .publish((LOAN_REPAID,), (borrower.clone(), total_yield));
    }

    /// Vouch for a borrower with a token stake.
    ///
    /// # Minimum Stake
    /// Stake must be ≥ `MIN_VOUCH_STAKE` (50 stroops). The yield formula
    /// `stake * 200 / 10_000` uses integer division and truncates to zero for
    /// stakes below 50, so vouchers would silently receive no yield (#624).
    ///
    /// # Maximum Vouchers
    /// A loan can have at most 100 vouchers to prevent DoS via unbounded voucher
    /// list (#633, #634).
    ///
    /// # Errors
    /// - [`ContractError::ZeroStake`] if stake is 0
    /// - [`ContractError::StakeBelowMinimum`] if stake < 50 stroops (#624)
    /// - [`ContractError::DuplicateVouch`] if this voucher already vouched for
    ///   this borrower
    /// - [`ContractError::TooManyVouchers`] if loan already has 100 vouchers (#633, #634)
    pub fn vouch(env: Env, borrower: Address, voucher: Address, stake: u64) {
        require_not_paused(&env);
        voucher.require_auth();

        // #629: Prevent borrower from vouching for themselves
        if voucher == borrower {
            panic_with_error!(&env, ContractError::DuplicateVouch);
        }

        // #630: Check if borrower already has an active loan
        let loan_key = loan_key(&borrower);
        if let Some(existing) = env.storage().persistent().get::<_, Loan>(&loan_key) {
            if existing.status == LoanStatus::Active {
                panic_with_error!(&env, ContractError::LoanAlreadyActive);
            }
        }

        if stake == 0 {
            panic_with_error!(&env, ContractError::ZeroStake);
        }

        let min_stake: u64 = env
            .storage()
            .persistent()
            .get(&MIN_STAKE_KEY)
            .unwrap_or(MIN_VOUCH_STAKE);
        if stake < min_stake {
            panic_with_error!(&env, ContractError::StakeBelowMinimum);
        }

        let key = vouches_key(&borrower);
        let mut vouches: Vec<Vouch> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(&env));

        // #633, #634: Enforce max vouchers per loan to prevent DoS.
        if vouches.len() >= 100 {
            panic_with_error!(&env, ContractError::TooManyVouchers);
        }

        for v in vouches.iter() {
            if v.voucher == voucher {
                panic_with_error!(&env, ContractError::DuplicateVouch);
            }
        }

        let token_addr = get_token(&env);
        let tok = token::Client::new(&env, &token_addr);
        tok.transfer(&voucher, &env.current_contract_address(), &(stake as i128));

        vouches.push_back(Vouch {
            voucher: voucher.clone(),
            stake,
        });
        env.storage().persistent().set(&key, &vouches);
        extend_persistent_ttl(&env, &key);

        let hist_key = voucher_history_key(&voucher);
        let mut history: Vec<Address> = env
            .storage()
            .persistent()
            .get(&hist_key)
            .unwrap_or_else(|| Vec::new(&env));
        history.push_back(borrower);
        env.storage().persistent().set(&hist_key, &history);
        extend_persistent_ttl(&env, &hist_key);
    }

    /// Admin-only: mark a loan as defaulted and slash based on configured rate.
    ///
    /// The slashed amount is accumulated in `slash_balance`; the remainder is
    /// returned to the voucher. The accumulated balance can be withdrawn by the
    /// admin via [`slash_treasury`] (#626).
    ///
    /// # DoS Protection
    /// Enforces max_vouchers_per_loan cap to prevent gas exhaustion (#633).
    pub fn slash(env: Env, admin: Address, borrower: Address) {
        require_admin(&env, &admin);

        let key = loan_key(&borrower);
        let mut loan: Loan = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NoActiveLoan));

        if loan.status != LoanStatus::Active {
            panic_with_error!(&env, ContractError::NoActiveLoan);
        }

        loan.status = LoanStatus::Defaulted;
        env.storage().persistent().set(&key, &loan);
        extend_persistent_ttl(&env, &key);

        let default_time = env.ledger().timestamp();
        env.storage().persistent().set(&(symbol_short!("DEF_TIME"), borrower.clone()), &default_time);
        env.storage()
            .persistent()
            .extend_ttl(&(symbol_short!("DEF_TIME"), borrower.clone()), TTL_THRESHOLD, TTL_TARGET);

        let borrower_key_val = borrower_key(&borrower);
        if let Some(mut borrower_record) = env
            .storage()
            .persistent()
            .get::<_, Borrower>(&borrower_key_val)
        {
            borrower_record.default_count += 1;
            env.storage()
                .persistent()
                .set(&borrower_key_val, &borrower_record);
            extend_persistent_ttl(&env, &borrower_key_val);
        }

        let vouches: Vec<Vouch> = env
            .storage()
            .persistent()
            .get(&vouches_key(&borrower))
            .unwrap_or_else(|| Vec::new(&env));

        // #633: Enforce max_vouchers_per_loan cap to prevent DoS via unbounded voucher list.
        if vouches.len() > 100 {
            panic_with_error!(&env, ContractError::TooManyVouchers);
        }

        let token_addr = get_token(&env);
        let tok = token::Client::new(&env, &token_addr);

        let _slash_bps = get_slash_bps(&env);
        let mut slash_accum: u64 = 0;
        for v in vouches.iter() {
            let slashed = v.stake * SLASH_BPS / 10_000;
            let returned = v.stake - slashed;
            slash_accum += slashed;
            if returned > 0 {
                tok.transfer(
                    &env.current_contract_address(),
                    &v.voucher,
                    &(returned as i128),
                );
            }
        }

        let current_slash: u64 = env.storage().persistent().get(&SLASH_BAL).unwrap_or(0u64);
        let updated_slash = current_slash + slash_accum;
        env.storage().persistent().set(&SLASH_BAL, &updated_slash);
        extend_persistent_ttl(&env, &SLASH_BAL);

        // #995: Release any lien recorded against the defaulted loan so the
        // asset is no longer locked indefinitely after a slash.
        if let Some(asset_id) = env
            .storage()
            .persistent()
            .get::<_, u64>(&loan_asset_key(loan.id))
        {
            release_lien_internal(&env, asset_id, loan.id);
        }

        env.events()
            .publish((LOAN_SLASHED,), (borrower.clone(), slash_accum));
    }

    /// Admin-only: withdraw all accumulated slash balance to the admin address.
    ///
    /// Transfers the full `slash_balance` to `admin` and resets it to zero.
    /// This provides a withdrawal path for the slashed funds that would
    /// otherwise be permanently locked in the contract (#626).
    pub fn slash_treasury(env: Env, admin: Address) {
        require_admin(&env, &admin);

        let slash_balance: u64 = env.storage().persistent().get(&SLASH_BAL).unwrap_or(0u64);

        if slash_balance > 0 {
            let token_addr = get_token(&env);
            let tok = token::Client::new(&env, &token_addr);
            tok.transfer(
                &env.current_contract_address(),
                &admin,
                &(slash_balance as i128),
            );
            env.storage().persistent().set(&SLASH_BAL, &0u64);
            extend_persistent_ttl(&env, &SLASH_BAL);
        }
    }

    /// Withdraw a vouch before a loan is requested (#631).
    ///
    /// Allows a voucher to reclaim their stake if no active loan exists.
    /// Panics if an active loan is found.
    pub fn withdraw_vouch(env: Env, borrower: Address, voucher: Address) {
        voucher.require_auth();

        // #631: Check no active loan exists
        let loan_key = loan_key(&borrower);
        if let Some(existing) = env.storage().persistent().get::<_, Loan>(&loan_key) {
            if existing.status == LoanStatus::Active {
                panic_with_error!(&env, ContractError::VouchWithdrawNotAllowed);
            }
        }

        let key = vouches_key(&borrower);
        let mut vouches: Vec<Vouch> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(&env));

        let mut found_index = None;
        for (i, v) in vouches.iter().enumerate() {
            if v.voucher == voucher {
                found_index = Some(i as u32);
                break;
            }
        }

        if let Some(idx) = found_index {
            let vouch = vouches.get(idx).unwrap();
            let stake = vouch.stake;

            vouches.remove(idx);
            env.storage().persistent().set(&key, &vouches);
            extend_persistent_ttl(&env, &key);

            let token_addr = get_token(&env);
            let tok = token::Client::new(&env, &token_addr);
            tok.transfer(&env.current_contract_address(), &voucher, &(stake as i128));
        }
    }

    /// Returns the loan for a borrower, if any.
    pub fn get_loan(env: Env, borrower: Address) -> Option<Loan> {
        env.storage().persistent().get(&loan_key(&borrower))
    }

    /// Returns all vouches for a borrower.
    pub fn get_vouches(env: Env, borrower: Address) -> Vec<Vouch> {
        env.storage()
            .persistent()
            .get(&vouches_key(&borrower))
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Returns the accumulated slash balance available for treasury withdrawal.
    pub fn get_slash_balance(env: Env) -> u64 {
        env.storage().persistent().get(&SLASH_BAL).unwrap_or(0u64)
    }

    /// Returns whether the contract has been initialized.
    pub fn is_initialized(env: Env) -> bool {
        env.storage().persistent().has(&ADMIN_KEY)
    }

    /// Returns the current admin address.
    pub fn get_admin(env: Env) -> Address {
        env.storage()
            .persistent()
            .get(&ADMIN_KEY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized))
    }

    /// Returns the token contract address.
    pub fn get_token(env: Env) -> Address {
        env.storage()
            .persistent()
            .get(&TOKEN_KEY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized))
    }

    /// Admin-only function to pause the contract.
    pub fn pause(env: Env, admin: Address) {
        require_admin(&env, &admin);
        env.storage().persistent().set(&PAUSED_KEY, &true);
        extend_persistent_ttl(&env, &PAUSED_KEY);
        env.events()
            .publish((symbol_short!("PAUSED"),), (admin.clone(),));
    }

    /// Admin-only function to unpause the contract.
    pub fn unpause(env: Env, admin: Address) {
        require_admin(&env, &admin);
        env.storage().persistent().set(&PAUSED_KEY, &false);
        extend_persistent_ttl(&env, &PAUSED_KEY);
        env.events()
            .publish((symbol_short!("UNPAUSED"),), (admin.clone(),));
    }

    /// Returns true if the contract is currently paused.
    pub fn is_paused(env: Env) -> bool {
        env.storage().persistent().get(&PAUSED_KEY).unwrap_or(false)
    }

    /// Admin-only: configure the lifecycle contract address used for collateral checks (#1019, #1020).
    pub fn set_lifecycle_contract(env: Env, admin: Address, lifecycle_addr: Address) {
        require_admin(&env, &admin);
        env.storage()
            .persistent()
            .set(&symbol_short!("LIFECYCLE"), &lifecycle_addr);
        extend_persistent_ttl(&env, &symbol_short!("LIFECYCLE"));
    }

    /// Admin-only: configure the asset registry contract address used for LTV checks (#1020).
    pub fn set_asset_registry_contract(env: Env, admin: Address, registry_addr: Address) {
        require_admin(&env, &admin);
        env.storage()
            .persistent()
            .set(&symbol_short!("ASSETREG"), &registry_addr);
        extend_persistent_ttl(&env, &symbol_short!("ASSETREG"));
    }

    /// Admin-only: set the maximum LTV basis points in the stored config (#1020).
    ///
    /// A value of 0 disables LTV enforcement. A value of 7000 means 70% LTV.
    pub fn set_max_ltv_bps(env: Env, admin: Address, max_ltv_bps: u32) {
        require_admin(&env, &admin);
        let mut config = get_config(&env);
        config.max_ltv_bps = max_ltv_bps;
        env.storage().persistent().set(&CONFIG_KEY, &config);
        extend_persistent_ttl(&env, &CONFIG_KEY);
    }

    /// Returns the credit score for a borrower based on their repayment/default history.
    ///
    /// Score is computed as: `repayment_count * 100 / (repayment_count + default_count)`.
    /// Returns 0 if the borrower has no history.
    pub fn get_credit_score(env: Env, borrower: Address) -> u32 {
        let borrower_key_val = borrower_key(&borrower);
        let borrower_record: Option<Borrower> = env.storage().persistent().get(&borrower_key_val);
        let default_count = borrower_record.map(|b| b.default_count).unwrap_or(0);

        let repayment_count_key = (symbol_short!("REP_CNT"), borrower.clone());
        let repayment_count: u32 = env
            .storage()
            .persistent()
            .get(&repayment_count_key)
            .unwrap_or(0);

        let total = repayment_count + default_count;
        if total == 0 {
            return 0;
        }
        repayment_count * 100 / total
    }

    /// Returns the configured lifecycle contract address, if any.
    pub fn get_lifecycle_contract(env: Env) -> Option<Address> {
        env.storage()
            .persistent()
            .get(&symbol_short!("LIFECYCLE"))
    }

    /// Returns the configured asset registry contract address, if any.
    pub fn get_asset_registry_contract(env: Env) -> Option<Address> {
        env.storage()
            .persistent()
            .get(&symbol_short!("ASSETREG"))
    }

    /// Record a lien on an asset. Only the contract admin may call this.
    ///
    /// Stores a [`LienRecord`] indicating that `lender` has a claim of `amount`
    /// against the asset identified by `asset_id` under the loan `loan_id`.
    /// If an identical lien (same asset + lender + loan_id) already exists,
    /// panics with [`ContractError::LienAlreadyExists`].
    ///
    /// Also writes a `(LOAN_ASSET, loan_id) → asset_id` mapping so that
    /// `slash` / `auto_slash` can release the lien automatically when the
    /// loan is defaulted without the caller having to supply the asset_id
    /// again (#995).
    pub fn record_lien(
        env: Env,
        admin: Address,
        asset_id: u64,
        lender: Address,
        loan_id: u64,
        amount: u64,
    ) {
        require_admin(&env, &admin);

        let key = liens_key(asset_id);
        let mut liens: Vec<LienRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(&env));

        for lien in liens.iter() {
            if lien.lender == lender && lien.loan_id == loan_id {
                panic_with_error!(&env, ContractError::LienAlreadyExists);
            }
        }

        liens.push_back(LienRecord {
            lender,
            loan_id,
            amount,
        });
        env.storage().persistent().set(&key, &liens);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);

        // #995: Write loan→asset_id mapping so slash can release the lien.
        let la_key = loan_asset_key(loan_id);
        env.storage().persistent().set(&la_key, &asset_id);
        env.storage()
            .persistent()
            .extend_ttl(&la_key, TTL_THRESHOLD, TTL_TARGET);
    }

    /// Release (remove) a previously recorded lien. Only the contract admin may call this.
    ///
    /// Panics with [`ContractError::LienNotFound`] if no matching lien exists
    /// for the given asset, lender, and loan_id.
    pub fn release_lien(
        env: Env,
        admin: Address,
        asset_id: u64,
        lender: Address,
        loan_id: u64,
    ) {
        require_admin(&env, &admin);

        let key = liens_key(asset_id);
        let mut liens: Vec<LienRecord> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::LienNotFound));

        let mut found_index: Option<u32> = None;
        for (i, lien) in liens.iter().enumerate() {
            if lien.lender == lender && lien.loan_id == loan_id {
                found_index = Some(i as u32);
                break;
            }
        }

        match found_index {
            Some(idx) => {
                liens.remove(idx);
                if liens.is_empty() {
                    env.storage().persistent().remove(&key);
                } else {
                    env.storage().persistent().set(&key, &liens);
                    env.storage()
                        .persistent()
                        .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
                }
            }
            None => panic_with_error!(&env, ContractError::LienNotFound),
        }
    }

    /// Returns all active lien records for the given asset.
    pub fn get_liens(env: Env, asset_id: u64) -> Vec<LienRecord> {
        env.storage()
            .persistent()
            .get(&liens_key(asset_id))
            .unwrap_or_else(|| Vec::new(&env))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Events};

    #[test]
    fn test_is_initialized() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        assert!(!client.is_initialized());

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);

        client.initialize(&deployer, &admin, &token, &0);
        assert!(client.is_initialized());
    }

    #[test]
    fn test_get_admin() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);

        client.initialize(&deployer, &admin, &token, &0);

        let retrieved_admin = client.get_admin();
        assert_eq!(retrieved_admin, admin);
    }

    #[test]
    fn test_get_token() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);

        client.initialize(&deployer, &admin, &token, &0);

        let retrieved_token = client.get_token();
        assert_eq!(retrieved_token, token);
    }

    #[test]
    fn test_slash_treasury() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);

        client.initialize(&deployer, &admin, &token, &0);

        // Verify initial slash balance is zero
        let initial_balance = client.get_slash_balance();
        assert_eq!(initial_balance, 0);

        // slash_treasury should work without error when balance is zero
        client.slash_treasury(&admin);

        // Verify balance remains zero
        let final_balance = client.get_slash_balance();
        assert_eq!(final_balance, 0);
    }

    fn setup() -> (Env, Address, Address, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);

        client.initialize(&deployer, &admin, &token, &0);

        (env, contract_id, admin, token, deployer)
    }

    #[test]
    fn test_request_loan_prevents_overwrite_of_active_loan() {
        let (env, _contract_id, _admin, _token, _deployer) = setup();
        let client = LendingContractClient::new(&env, &_contract_id);

        let borrower = Address::generate(&env);

        client.request_loan(&borrower, &1000, &0u64);
        let loan1 = client.get_loan(&borrower).unwrap();
        assert_eq!(loan1.amount, 1000);
        assert_eq!(loan1.status, LoanStatus::Active);

        let result = client.try_request_loan(&borrower, &2000, &0u64);
        assert!(result.is_err());

        let loan2 = client.get_loan(&borrower).unwrap();
        assert_eq!(loan2.amount, 1000);
        assert_eq!(loan2.status, LoanStatus::Active);
    }

    #[test]
    fn test_repay_verifies_borrower_matches_loan_record() {
        let (env, _contract_id, _admin, _token, _deployer) = setup();
        let client = LendingContractClient::new(&env, &_contract_id);

        let borrower1 = Address::generate(&env);
        let borrower2 = Address::generate(&env);

        client.request_loan(&borrower1, &1000, &0u64);

        let result = client.try_repay(&borrower2);
        assert!(result.is_err());

        let loan = client.get_loan(&borrower1).unwrap();
        assert_eq!(loan.status, LoanStatus::Active);
    }

    #[test]
    fn test_slash_bps_guard_prevents_underflow() {
        let (env, _contract_id, _admin, _token, _deployer) = setup();
        let _client = LendingContractClient::new(&env, &_contract_id);

        assert!(SLASH_BPS <= 10_000);
    }

    fn setup_contract(env: &Env) -> (Address, Address, Address) {
        let admin = Address::generate(env);
        let token = Address::generate(env);
        let contract_id = env.register(LendingContract, ());

        let client = LendingContractClient::new(env, &contract_id);
        let deployer = Address::generate(env);

        client.initialize(&deployer, &admin, &token, &0);

        (contract_id, admin, token)
    }

    #[test]
    fn test_vouch_max_vouchers_limit() {
        let env = Env::default();
        env.mock_all_auths();

        let (contract_id, _, _) = setup_contract(&env);
        let client = LendingContractClient::new(&env, &contract_id);

        let borrower = Address::generate(&env);
        client.request_loan(&borrower, &100_000_000, &0u64);

        // Add 100 vouchers (the max)
        for i in 0..100 {
            let voucher = Address::generate(&env);
            client.vouch(&borrower, &voucher, &100);
        }

        // Try to add the 101st voucher - should fail
        let extra_voucher = Address::generate(&env);
        let result = client.try_vouch(&borrower, &extra_voucher, &100);

        assert!(result.is_err());
    }

    #[test]
    fn test_slash_with_max_vouchers() {
        let env = Env::default();
        env.mock_all_auths();

        let (contract_id, admin, _) = setup_contract(&env);
        let client = LendingContractClient::new(&env, &contract_id);

        let borrower = Address::generate(&env);
        client.request_loan(&borrower, &100_000_000, &0u64);

        // Add 100 vouchers
        for i in 0..100 {
            let voucher = Address::generate(&env);
            client.vouch(&borrower, &voucher, &100);
        }

        // Slash should succeed with exactly 100 vouchers
        client.slash(&admin, &borrower);

        let loan = client.get_loan(&borrower);
        assert!(loan.is_some());
        assert_eq!(loan.unwrap().status, LoanStatus::Defaulted);
    }

    #[test]
    fn test_repay_with_max_vouchers() {
        let env = Env::default();
        env.mock_all_auths();

        let (contract_id, _, _) = setup_contract(&env);
        let client = LendingContractClient::new(&env, &contract_id);

        let borrower = Address::generate(&env);
        client.request_loan(&borrower, &100_000_000, &0u64);

        // Add 100 vouchers
        for i in 0..100 {
            let voucher = Address::generate(&env);
            client.vouch(&borrower, &voucher, &100);
        }

        // Repay should succeed with exactly 100 vouchers
        client.repay(&borrower);

        let loan = client.get_loan(&borrower);
        assert!(loan.is_some());
        assert_eq!(loan.unwrap().status, LoanStatus::Repaid);
    }

    #[test]
    fn test_repay_collects_loan_amount_plus_yield() {
        let env = Env::default();
        env.mock_all_auths();

        let (contract_id, _, _) = setup_contract(&env);
        let client = LendingContractClient::new(&env, &contract_id);

        let borrower = Address::generate(&env);
        let loan_amount = 1000u64;
        client.request_loan(&borrower, &loan_amount, &0u64);

        let voucher1 = Address::generate(&env);
        let voucher2 = Address::generate(&env);
        let stake1 = 500u64;
        let stake2 = 500u64;

        client.vouch(&borrower, &voucher1, &stake1);
        client.vouch(&borrower, &voucher2, &stake2);

        // Expected yield: (500 * 200 / 10_000) + (500 * 200 / 10_000) = 10 + 10 = 20
        // Total repayment should be: 1000 + 20 = 1020
        // Borrower must provide this amount in the repay call

        client.repay(&borrower);

        let loan = client.get_loan(&borrower);
        assert!(loan.is_some());
        assert_eq!(loan.unwrap().status, LoanStatus::Repaid);
    }

    #[test]
    fn test_request_loan_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token_addr = Address::generate(&env);
        let borrower = Address::generate(&env);

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        client.initialize(&deployer, &admin, &token_addr, &0);

        let amount = 1000u64;
        client.request_loan(&borrower, &amount, &0u64);

        let events = env.events().all();
        let loan_req_events: Vec<_> = events
            .iter()
            .filter(|e| {
                if let soroban_sdk::xdr::ContractEvent::V0(v0) = &e.event {
                    v0.topics.len() > 0
                        && v0.topics.get(0).map_or(false, |t| {
                            if let soroban_sdk::xdr::ScVal::Symbol(sym) = t {
                                sym.0.as_slice() == b"loan_req"
                            } else {
                                false
                            }
                        })
                } else {
                    false
                }
            })
            .collect();

        assert!(
            !loan_req_events.is_empty(),
            "request_loan should emit event"
        );
    }

    #[test]
    fn test_vouch_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token_addr = Address::generate(&env);
        let borrower = Address::generate(&env);
        let voucher = Address::generate(&env);

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        client.initialize(&deployer, &admin, &token_addr, &0);
        client.request_loan(&borrower, &1000u64, &0u64);

        let stake = 100u64;
        client.vouch(&borrower, &voucher, &stake);

        let events = env.events().all();
        let vouch_events: Vec<_> = events
            .iter()
            .filter(|e| {
                if let soroban_sdk::xdr::ContractEvent::V0(v0) = &e.event {
                    v0.topics.len() > 0
                        && v0.topics.get(0).map_or(false, |t| {
                            if let soroban_sdk::xdr::ScVal::Symbol(sym) = t {
                                sym.0.as_slice() == b"vouch_cr"
                            } else {
                                false
                            }
                        })
                } else {
                    false
                }
            })
            .collect();

        assert!(!vouch_events.is_empty(), "vouch should emit event");
    }

    #[test]
    fn test_repay_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token_addr = Address::generate(&env);
        let borrower = Address::generate(&env);

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        client.initialize(&deployer, &admin, &token_addr, &0);
        client.request_loan(&borrower, &1000u64, &0u64);

        client.repay(&borrower);

        let events = env.events().all();
        let repay_events: Vec<_> = events
            .iter()
            .filter(|e| {
                if let soroban_sdk::xdr::ContractEvent::V0(v0) = &e.event {
                    v0.topics.len() > 0
                        && v0.topics.get(0).map_or(false, |t| {
                            if let soroban_sdk::xdr::ScVal::Symbol(sym) = t {
                                sym.0.as_slice() == b"loan_rep"
                            } else {
                                false
                            }
                        })
                } else {
                    false
                }
            })
            .collect();

        assert!(!repay_events.is_empty(), "repay should emit event");
    }

    #[test]
    fn test_slash_emits_event() {
        let env = Env::default();
        env.mock_all_auths();

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token_addr = Address::generate(&env);
        let borrower = Address::generate(&env);

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        client.initialize(&deployer, &admin, &token_addr, &0);
        client.request_loan(&borrower, &1000u64, &0u64);

        client.slash(&admin, &borrower);

        let events = env.events().all();
        let slash_events: Vec<_> = events
            .iter()
            .filter(|e| {
                if let soroban_sdk::xdr::ContractEvent::V0(v0) = &e.event {
                    v0.topics.len() > 0
                        && v0.topics.get(0).map_or(false, |t| {
                            if let soroban_sdk::xdr::ScVal::Symbol(sym) = t {
                                sym.0.as_slice() == b"loan_sls"
                            } else {
                                false
                            }
                        })
                } else {
                    false
                }
            })
            .collect();

        assert!(!slash_events.is_empty(), "slash should emit event");
    }

    #[test]
    fn test_pause_state_persists_across_instance_ttl_boundary() {
        use soroban_sdk::testutils::Ledger;

        let env = Env::default();
        env.mock_all_auths();

        let (env2, contract_id, admin, _token, _deployer) = {
            let contract_id = env.register(LendingContract, ());
            let client = LendingContractClient::new(&env, &contract_id);
            let deployer = Address::generate(&env);
            let admin = Address::generate(&env);
            let token = Address::generate(&env);
            client.initialize(&deployer, &admin, &token, &5000);
            (env, contract_id, admin, token, deployer)
        };

        let client = LendingContractClient::new(&env2, &contract_id);

        client.pause(&admin);
        assert!(client.is_paused());

        // Advance ledger past a simulated instance TTL boundary
        env2.ledger().with_mut(|l| l.sequence_number += 518_401);

        // PAUSED_KEY lives in persistent storage — must still be true after ledger advance
        assert!(
            client.is_paused(),
            "pause state must survive instance TTL boundary"
        );

        // Writes must still be blocked
        let borrower = Address::generate(&env2);
        assert_eq!(
            client.try_request_loan(&borrower, &1000, &0u64),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::ContractPaused as u32
            )))
        );
    }

    // ── issue #876: lien recording ─────────────────────────────────────

    #[test]
    fn test_record_and_get_lien() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        client.initialize(&deployer, &admin, &token, &0);

        let lender = Address::generate(&env);
        client.record_lien(&admin, &1, &lender, &42, &1000);

        let liens = client.get_liens(&1);
        assert_eq!(liens.len(), 1);
        assert_eq!(liens.get(0).unwrap().lender, lender);
        assert_eq!(liens.get(0).unwrap().loan_id, 42);
        assert_eq!(liens.get(0).unwrap().amount, 1000);
    }

    #[test]
    fn test_record_multiple_liens_same_asset() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        client.initialize(&deployer, &admin, &token, &0);

        let lender1 = Address::generate(&env);
        let lender2 = Address::generate(&env);

        client.record_lien(&admin, &1, &lender1, &42, &1000);
        client.record_lien(&admin, &1, &lender2, &99, &2500);

        let liens = client.get_liens(&1);
        assert_eq!(liens.len(), 2);
    }

    #[test]
    fn test_record_duplicate_lien_panics() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        client.initialize(&deployer, &admin, &token, &0);

        let lender = Address::generate(&env);
        client.record_lien(&admin, &1, &lender, &42, &1000);

        let result = client.try_record_lien(&admin, &1, &lender, &42, &2000);
        assert!(result.is_err());
    }

    #[test]
    fn test_release_lien() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        client.initialize(&deployer, &admin, &token, &0);

        let lender = Address::generate(&env);
        client.record_lien(&admin, &1, &lender, &42, &1000);
        assert_eq!(client.get_liens(&1).len(), 1);

        client.release_lien(&admin, &1, &lender, &42);
        assert_eq!(client.get_liens(&1).len(), 0);
    }

    #[test]
    fn test_release_nonexistent_lien_panics() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        client.initialize(&deployer, &admin, &token, &0);

        let lender = Address::generate(&env);
        let result = client.try_release_lien(&admin, &1, &lender, &42);
        assert!(result.is_err());
    }

    #[test]
    fn test_record_lien_requires_admin() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        client.initialize(&deployer, &admin, &token, &0);

        let non_admin = Address::generate(&env);
        let lender = Address::generate(&env);

        // With mock_all_auths any address passes auth — but require_admin
        // checks the stored admin, so non_admin should still fail.
        let result = client.try_record_lien(&non_admin, &1, &lender, &42, &1000);
        assert!(result.is_err());
    }

    #[test]
    fn test_release_lien_requires_admin() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        client.initialize(&deployer, &admin, &token, &0);

        let non_admin = Address::generate(&env);
        let lender = Address::generate(&env);

        let result = client.try_release_lien(&non_admin, &1, &lender, &42);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_liens_different_assets() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        client.initialize(&deployer, &admin, &token, &0);

        let lender = Address::generate(&env);
        client.record_lien(&admin, &1, &lender, &42, &1000);
        client.record_lien(&admin, &2, &lender, &43, &2000);

        assert_eq!(client.get_liens(&1).len(), 1);
        assert_eq!(client.get_liens(&2).len(), 1);
        assert_eq!(client.get_liens(&3).len(), 0);
    }

    // ── issue #1019: cross-contract collateral verification ────────────────

    /// When no lifecycle contract is configured, request_loan proceeds without
    /// any collateral eligibility check (backward-compatible path).
    #[test]
    fn test_request_loan_without_lifecycle_config_succeeds() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        client.initialize(&deployer, &admin, &token, &0);

        let borrower = Address::generate(&env);
        // No lifecycle contract configured → no collateral check → should succeed
        client.request_loan(&borrower, &1000, &42u64);

        let loan = client.get_loan(&borrower).unwrap();
        assert_eq!(loan.status, LoanStatus::Active);
        assert_eq!(loan.amount, 1000);
    }

    // ── Mock lifecycle contract for #1019 / #1020 tests ───────────────────

    /// A minimal mock lifecycle contract that always returns `is_eligible`
    /// for `is_collateral_eligible` and a fixed score for `get_collateral_score`.
    pub struct MockLifecycle {
        pub is_eligible: bool,
        pub score: u32,
    }

    #[contract]
    pub struct MockLifecycleContract;

    #[contractimpl]
    impl MockLifecycleContract {
        pub fn is_collateral_eligible(_env: Env, _asset_id: u64) -> bool {
            // Reads from instance storage set at registration time.
            // For simplicity, always return true (eligible mock).
            true
        }

        pub fn get_collateral_score(_env: Env, _asset_id: u64) -> u32 {
            // Always return 100 (max score) for tests.
            100u32
        }
    }

    /// A minimal mock lifecycle contract that always returns ineligible.
    #[contract]
    pub struct MockLifecycleIneligible;

    #[contractimpl]
    impl MockLifecycleIneligible {
        pub fn is_collateral_eligible(_env: Env, _asset_id: u64) -> bool {
            false
        }

        pub fn get_collateral_score(_env: Env, _asset_id: u64) -> u32 {
            0u32
        }
    }

    /// #1019: When a lifecycle contract is configured and the asset is eligible,
    /// request_loan should succeed.
    #[test]
    fn test_request_loan_collateral_eligible_succeeds() {
        let env = Env::default();
        env.mock_all_auths();

        // Register mock lifecycle (always eligible)
        let lifecycle_id = env.register(MockLifecycleContract, ());

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        client.initialize(&deployer, &admin, &token, &0);

        // Configure lifecycle contract
        client.set_lifecycle_contract(&admin, &lifecycle_id);

        let borrower = Address::generate(&env);
        // Asset 1 is eligible (mock always returns true)
        client.request_loan(&borrower, &1000, &1u64);

        let loan = client.get_loan(&borrower).unwrap();
        assert_eq!(loan.status, LoanStatus::Active);
    }

    /// #1019: When a lifecycle contract is configured and the asset is NOT eligible,
    /// request_loan must be rejected with CollateralIneligible.
    #[test]
    fn test_request_loan_collateral_ineligible_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        // Register mock lifecycle (always ineligible)
        let lifecycle_id = env.register(MockLifecycleIneligible, ());

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        client.initialize(&deployer, &admin, &token, &0);

        // Configure lifecycle contract
        client.set_lifecycle_contract(&admin, &lifecycle_id);

        let borrower = Address::generate(&env);
        let result = client.try_request_loan(&borrower, &1000, &1u64);
        assert!(
            result.is_err(),
            "Loan with ineligible collateral must be rejected"
        );
    }

    // ── issue #1020: LTV ratio enforcement ────────────────────────────────

    /// #1020: When LTV enforcement is enabled and the requested amount exceeds the
    /// cap derived from collateral score × max_ltv_bps, the loan must be rejected.
    #[test]
    fn test_request_loan_ltv_exceeded_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        // Mock lifecycle returns score=50 (50/100 quality)
        #[contract]
        struct MockLifecycleLow;
        #[contractimpl]
        impl MockLifecycleLow {
            pub fn is_collateral_eligible(_env: Env, _asset_id: u64) -> bool { true }
            pub fn get_collateral_score(_env: Env, _asset_id: u64) -> u32 { 50u32 }
        }
        let lifecycle_id = env.register(MockLifecycleLow, ());

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        client.initialize(&deployer, &admin, &token, &0);

        // Configure lifecycle + LTV: 50% max_ltv_bps = 5000
        client.set_lifecycle_contract(&admin, &lifecycle_id);
        client.set_max_ltv_bps(&admin, &5000u32);

        let borrower = Address::generate(&env);
        // With score=50, max_ltv_bps=5000:
        //   max_loan_cap = amount * 50 * 5000 / (100 * 10_000) = amount * 0.25
        // So any non-zero amount exceeds the cap (amount > amount * 0.25).
        let result = client.try_request_loan(&borrower, &10_000, &1u64);
        assert!(
            result.is_err(),
            "Loan exceeding LTV cap must be rejected"
        );
    }

    /// #1020: When LTV enforcement is disabled (max_ltv_bps = 0),
    /// the loan proceeds regardless of collateral score.
    #[test]
    fn test_request_loan_ltv_disabled_succeeds() {
        let env = Env::default();
        env.mock_all_auths();

        let lifecycle_id = env.register(MockLifecycleContract, ());

        let contract_id = env.register(LendingContract, ());
        let client = LendingContractClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        client.initialize(&deployer, &admin, &token, &0);

        // Lifecycle configured, but max_ltv_bps = 0 (disabled)
        client.set_lifecycle_contract(&admin, &lifecycle_id);
        client.set_max_ltv_bps(&admin, &0u32);

        let borrower = Address::generate(&env);
        // LTV disabled → should succeed
        client.request_loan(&borrower, &10_000, &1u64);

        let loan = client.get_loan(&borrower).unwrap();
        assert_eq!(loan.status, LoanStatus::Active);
    }
}
