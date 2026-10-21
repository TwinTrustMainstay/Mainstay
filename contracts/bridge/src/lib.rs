//! Cross-chain asset bridge for Mainstay (#1743, #1744).
//!
//! This contract is the Soroban (Stellar) side of the bridge. It provides:
//!
//! - **Chain registry** with per-chain constraints: enablement, min/max transfer
//!   amounts, required source-chain confirmations, and a rolling outflow window
//!   with a hard cap and an alert threshold for monitoring.
//! - **Outbound transfers** (`bridge_out`): tokens are locked here and a
//!   per-destination nonce is emitted in a `BR_OUT` event for relayers to pick up.
//! - **Inbound releases** (`attest_release`): an M-of-N relayer set attests to a
//!   transfer from a source chain. Once the threshold is reached the tokens are
//!   released. Each `(source_chain, nonce)` can execute only once.
//! - **Atomic swaps** (`create_swap` / `claim_swap` / `refund_swap`): hashed
//!   timelock contracts. The counterparty chain uses the same hashlock, and the
//!   preimage revealed on claim lets the other leg complete atomically.
//!
//! The EVM-side bridge contract is not part of this crate.

#![no_std]

use shared::{TTL_TARGET, TTL_THRESHOLD};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, token,
    Address, Bytes, BytesN, Env, Symbol, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    /// `initialize` was called a second time.
    AlreadyInitialized = 1,
    /// The contract has not been initialized.
    NotInitialized = 2,
    /// Caller is not the admin.
    UnauthorizedAdmin = 3,
    /// The contract is paused; new transfers are blocked.
    Paused = 4,
    /// No chain is registered under the given chain ID.
    ChainNotFound = 5,
    /// A chain is already registered under the given chain ID.
    ChainAlreadyExists = 6,
    /// The chain is registered but currently disabled.
    ChainDisabled = 7,
    /// Amount is zero, negative, or outside the chain's min/max range.
    AmountOutOfRange = 8,
    /// Destination address is empty.
    InvalidDestination = 9,
    /// The release for this `(source_chain, nonce)` has already executed.
    AlreadyProcessed = 10,
    /// Caller is not in the relayer set.
    NotRelayer = 11,
    /// This relayer has already attested to this release.
    AlreadyAttested = 12,
    /// Attestation parameters differ from earlier attestations for the same nonce.
    AttestationMismatch = 13,
    /// The attested source-chain confirmations are below the chain's requirement.
    InsufficientConfirmations = 14,
    /// Threshold is zero or greater than the relayer count.
    InvalidThreshold = 15,
    /// No swap exists with the given ID.
    SwapNotFound = 16,
    /// The preimage does not hash to the swap's hashlock.
    InvalidPreimage = 17,
    /// The swap's timelock has passed, so it can no longer be claimed.
    SwapExpired = 18,
    /// The swap's timelock has not passed, so it cannot be refunded yet.
    SwapNotExpired = 19,
    /// The swap was already claimed or refunded.
    SwapNotOpen = 20,
    /// This release would exceed the source chain's rolling outflow cap.
    WindowCapExceeded = 21,
    /// A chain configuration value failed validation.
    InvalidConfig = 22,
    /// The relayer is already in the relayer set.
    RelayerAlreadyExists = 23,
    /// Relayer list contains duplicates or is empty.
    InvalidRelayerSet = 24,
}

/// Maximum number of relayers in the set.
const MAX_RELAYERS: u32 = 20;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChainConfig {
    /// Human-readable chain name (for dashboards), e.g. `ETH`.
    pub name: Symbol,
    /// Whether transfers to or from this chain are accepted.
    pub enabled: bool,
    /// Smallest allowed transfer amount (inclusive). Must be > 0.
    pub min_amount: i128,
    /// Largest allowed transfer amount (inclusive). Must be ≥ `min_amount`.
    pub max_amount: i128,
    /// Source-chain confirmations relayers must observe before attesting.
    pub required_confirmations: u32,
    /// Length of the rolling outflow window in seconds. Must be > 0.
    pub window_secs: u64,
    /// Maximum total inbound release amount per window. Must be > 0.
    pub window_cap: i128,
    /// Window outflow level at which a `BR_ALERT` event is emitted.
    /// Must be > 0 and ≤ `window_cap`.
    pub alert_threshold: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowState {
    /// Ledger timestamp at which the current window started.
    pub start: u64,
    /// Total released from the source chain within the current window.
    pub outflow: i128,
}

/// Transfer details a relayer attests to for an inbound release.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseParams {
    pub token: Address,
    pub recipient: Address,
    pub amount: i128,
    /// Source-chain confirmations the relayer observed for this transfer.
    pub confirmations: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingRelease {
    pub token: Address,
    pub recipient: Address,
    pub amount: i128,
    /// Relayers that have attested, in the order they attested.
    pub approvals: Vec<Address>,
    /// `true` once the release has been paid out. Doubles as the replay guard.
    pub executed: bool,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwapStatus {
    Open = 0,
    Claimed = 1,
    Refunded = 2,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Swap {
    pub sender: Address,
    pub recipient: Address,
    pub token: Address,
    pub amount: i128,
    /// `sha256(preimage)`. Claiming requires revealing the preimage.
    pub hashlock: BytesN<32>,
    /// Ledger timestamp after which the swap can only be refunded.
    pub expiry: u64,
    pub status: SwapStatus,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Paused,
    NextSwapId,
    Relayers,
    Threshold,
    Chain(u32),
    OutNonce(u32),
    Pending(u32, u64),
    Window(u32),
    Swap(u64),
}

fn validate_chain_config(env: &Env, cfg: &ChainConfig) {
    if cfg.min_amount <= 0
        || cfg.max_amount < cfg.min_amount
        || cfg.window_secs == 0
        || cfg.window_cap <= 0
        || cfg.alert_threshold <= 0
        || cfg.alert_threshold > cfg.window_cap
    {
        panic_with_error!(env, ContractError::InvalidConfig);
    }
}

/// Extend the TTL of a persistent entry so active bridge state is not archived.
fn bump(env: &Env, key: &DataKey) {
    env.storage()
        .persistent()
        .extend_ttl(key, TTL_THRESHOLD, TTL_TARGET);
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

fn stored_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized))
}

fn require_admin(env: &Env, admin: &Address) {
    admin.require_auth();
    if stored_admin(env) != *admin {
        panic_with_error!(env, ContractError::UnauthorizedAdmin);
    }
}

fn load_chain(env: &Env, chain_id: u32) -> ChainConfig {
    let key = DataKey::Chain(chain_id);
    let cfg: ChainConfig = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::ChainNotFound));
    bump(env, &key);
    cfg
}

fn ensure_amount_in_range(env: &Env, cfg: &ChainConfig, amount: i128) {
    if amount < cfg.min_amount || amount > cfg.max_amount {
        panic_with_error!(env, ContractError::AmountOutOfRange);
    }
}

/// Add `amount` to the source chain's rolling outflow window. Resets the window
/// when it has expired, rejects the release if it would exceed the cap, and
/// emits `BR_ALERT` when the alert threshold is crossed.
fn account_window_outflow(env: &Env, source_chain: u32, cfg: &ChainConfig, amount: i128) {
    let now = env.ledger().timestamp();
    let key = DataKey::Window(source_chain);
    let mut window: WindowState = env.storage().persistent().get(&key).unwrap_or(WindowState {
        start: now,
        outflow: 0,
    });

    if now >= window.start.saturating_add(cfg.window_secs) {
        window = WindowState {
            start: now,
            outflow: 0,
        };
    }

    let new_outflow = window.outflow.checked_add(amount).unwrap_or(i128::MAX);
    if new_outflow > cfg.window_cap {
        panic_with_error!(env, ContractError::WindowCapExceeded);
    }
    if window.outflow < cfg.alert_threshold && new_outflow >= cfg.alert_threshold {
        env.events().publish(
            (symbol_short!("BR_ALERT"),),
            (source_chain, new_outflow, cfg.window_cap),
        );
    }

    window.outflow = new_outflow;
    env.storage().persistent().set(&key, &window);
    bump(env, &key);
}

#[contract]
pub struct Bridge;

#[contractimpl]
impl Bridge {
    /// Initialize the bridge with an admin. Can be called only once.
    ///
    /// # Panics
    /// - [`ContractError::AlreadyInitialized`] if the admin is already set
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, ContractError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage().instance().set(&DataKey::NextSwapId, &0u64);
    }

    /// Pause new transfers (`bridge_out`, `attest_release`, `create_swap`).
    /// Claims and refunds stay available so users can always reclaim funds.
    pub fn pause(env: Env, admin: Address) {
        require_admin(&env, &admin);
        env.storage().instance().set(&DataKey::Paused, &true);
    }

    /// Resume transfers after a pause.
    pub fn unpause(env: Env, admin: Address) {
        require_admin(&env, &admin);
        env.storage().instance().set(&DataKey::Paused, &false);
    }

    /// Replace the relayer set and the number of attestations required per release.
    ///
    /// # Panics
    /// - [`ContractError::InvalidRelayerSet`] if the list is empty, too large, or has duplicates
    /// - [`ContractError::InvalidThreshold`] if `threshold` is 0 or exceeds the relayer count
    pub fn set_relayers(env: Env, admin: Address, relayers: Vec<Address>, threshold: u32) {
        require_admin(&env, &admin);
        let count = relayers.len();
        if count == 0 || count > MAX_RELAYERS {
            panic_with_error!(&env, ContractError::InvalidRelayerSet);
        }
        for i in 0..count {
            for j in (i + 1)..count {
                if relayers.get(i).unwrap() == relayers.get(j).unwrap() {
                    panic_with_error!(&env, ContractError::InvalidRelayerSet);
                }
            }
        }
        if threshold == 0 || threshold > count {
            panic_with_error!(&env, ContractError::InvalidThreshold);
        }
        env.storage().instance().set(&DataKey::Relayers, &relayers);
        env.storage()
            .instance()
            .set(&DataKey::Threshold, &threshold);
    }

    /// Register a supported chain.
    ///
    /// # Panics
    /// - [`ContractError::ChainAlreadyExists`] if the chain ID is already registered
    /// - [`ContractError::InvalidConfig`] if the config fails validation
    pub fn add_chain(env: Env, admin: Address, chain_id: u32, config: ChainConfig) {
        require_admin(&env, &admin);
        validate_chain_config(&env, &config);
        let key = DataKey::Chain(chain_id);
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, ContractError::ChainAlreadyExists);
        }
        env.storage().persistent().set(&key, &config);
        bump(&env, &key);
    }

    /// Replace the constraints of a registered chain.
    ///
    /// # Panics
    /// - [`ContractError::ChainNotFound`] if the chain ID is not registered
    /// - [`ContractError::InvalidConfig`] if the config fails validation
    pub fn update_chain(env: Env, admin: Address, chain_id: u32, config: ChainConfig) {
        require_admin(&env, &admin);
        validate_chain_config(&env, &config);
        load_chain(&env, chain_id);
        let key = DataKey::Chain(chain_id);
        env.storage().persistent().set(&key, &config);
        bump(&env, &key);
    }

    /// Enable or disable a chain without changing its other constraints.
    pub fn set_chain_enabled(env: Env, admin: Address, chain_id: u32, enabled: bool) {
        require_admin(&env, &admin);
        let mut cfg = load_chain(&env, chain_id);
        cfg.enabled = enabled;
        let key = DataKey::Chain(chain_id);
        env.storage().persistent().set(&key, &cfg);
        bump(&env, &key);
    }

    /// Get a chain's current configuration.
    ///
    /// # Panics
    /// - [`ContractError::ChainNotFound`] if the chain ID is not registered
    pub fn get_chain(env: Env, chain_id: u32) -> ChainConfig {
        load_chain(&env, chain_id)
    }

    /// Lock tokens for transfer to `dest_chain`. Emits `BR_OUT` for relayers.
    ///
    /// Returns the outbound nonce for this destination chain (starts at 1).
    ///
    /// # Panics
    /// - [`ContractError::Paused`] if the bridge is paused
    /// - [`ContractError::ChainNotFound`] / [`ContractError::ChainDisabled`] for an unusable destination
    /// - [`ContractError::AmountOutOfRange`] if `amount` is outside the chain's limits
    /// - [`ContractError::InvalidDestination`] if `dest_address` is empty
    pub fn bridge_out(
        env: Env,
        sender: Address,
        token: Address,
        amount: i128,
        dest_chain: u32,
        dest_address: Bytes,
    ) -> u64 {
        ensure_not_paused(&env);
        sender.require_auth();
        let cfg = load_chain(&env, dest_chain);
        if !cfg.enabled {
            panic_with_error!(&env, ContractError::ChainDisabled);
        }
        ensure_amount_in_range(&env, &cfg, amount);
        if dest_address.is_empty() {
            panic_with_error!(&env, ContractError::InvalidDestination);
        }

        token::Client::new(&env, &token).transfer(
            &sender,
            &env.current_contract_address(),
            &amount,
        );

        let nonce_key = DataKey::OutNonce(dest_chain);
        let nonce: u64 = env.storage().persistent().get(&nonce_key).unwrap_or(0) + 1;
        env.storage().persistent().set(&nonce_key, &nonce);
        bump(&env, &nonce_key);

        env.events().publish(
            (symbol_short!("BR_OUT"),),
            (dest_chain, nonce, sender, token, amount, dest_address),
        );
        nonce
    }

    /// Relayer attestation for an inbound transfer from `source_chain`.
    ///
    /// Each relayer attests once per `(source_chain, nonce)`. All attestations must
    /// agree on `token`, `recipient` and `amount`. When the approval count reaches
    /// the threshold the tokens are released, and the release is marked executed
    /// so it cannot be replayed.
    ///
    /// # Panics
    /// - [`ContractError::Paused`] if the bridge is paused
    /// - [`ContractError::NotRelayer`] if the caller is not in the relayer set
    /// - [`ContractError::ChainNotFound`] / [`ContractError::ChainDisabled`] for an unusable source chain
    /// - [`ContractError::InsufficientConfirmations`] if `confirmations` is below the chain requirement
    /// - [`ContractError::AmountOutOfRange`] if `amount` is outside the chain's limits
    /// - [`ContractError::AlreadyProcessed`] if this release already executed
    /// - [`ContractError::AttestationMismatch`] if parameters differ from earlier attestations
    /// - [`ContractError::AlreadyAttested`] if this relayer already attested
    /// - [`ContractError::WindowCapExceeded`] if the release would breach the outflow cap
    pub fn attest_release(
        env: Env,
        relayer: Address,
        source_chain: u32,
        nonce: u64,
        params: ReleaseParams,
    ) {
        let ReleaseParams {
            token,
            recipient,
            amount,
            confirmations,
        } = params;
        ensure_not_paused(&env);
        relayer.require_auth();

        let relayers: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Relayers)
            .unwrap_or_else(|| Vec::new(&env));
        if !relayers.contains(&relayer) {
            panic_with_error!(&env, ContractError::NotRelayer);
        }

        let cfg = load_chain(&env, source_chain);
        if !cfg.enabled {
            panic_with_error!(&env, ContractError::ChainDisabled);
        }
        if confirmations < cfg.required_confirmations {
            panic_with_error!(&env, ContractError::InsufficientConfirmations);
        }
        ensure_amount_in_range(&env, &cfg, amount);

        let key = DataKey::Pending(source_chain, nonce);
        let mut pending: PendingRelease =
            match env.storage().persistent().get::<_, PendingRelease>(&key) {
                Some(existing) => {
                    if existing.executed {
                        panic_with_error!(&env, ContractError::AlreadyProcessed);
                    }
                    if existing.token != token
                        || existing.recipient != recipient
                        || existing.amount != amount
                    {
                        panic_with_error!(&env, ContractError::AttestationMismatch);
                    }
                    existing
                }
                None => PendingRelease {
                    token: token.clone(),
                    recipient: recipient.clone(),
                    amount,
                    approvals: Vec::new(&env),
                    executed: false,
                },
            };

        if pending.approvals.contains(&relayer) {
            panic_with_error!(&env, ContractError::AlreadyAttested);
        }
        pending.approvals.push_back(relayer.clone());

        let threshold: u32 = env
            .storage()
            .instance()
            .get(&DataKey::Threshold)
            .unwrap_or(0);
        if pending.approvals.len() >= threshold {
            account_window_outflow(&env, source_chain, &cfg, amount);
            pending.executed = true;
            token::Client::new(&env, &token).transfer(
                &env.current_contract_address(),
                &recipient,
                &amount,
            );
            env.events().publish(
                (symbol_short!("BR_REL"),),
                (
                    source_chain,
                    nonce,
                    recipient.clone(),
                    token.clone(),
                    amount,
                ),
            );
        }

        env.storage().persistent().set(&key, &pending);
        bump(&env, &key);
    }

    /// Look up a release by `(source_chain, nonce)`. Returns `None` if no relayer
    /// has attested yet.
    pub fn get_release(env: Env, source_chain: u32, nonce: u64) -> Option<PendingRelease> {
        env.storage()
            .persistent()
            .get(&DataKey::Pending(source_chain, nonce))
    }

    /// Current rolling outflow window for a source chain. Returns zeroed state if none.
    pub fn get_window(env: Env, source_chain: u32) -> WindowState {
        env.storage()
            .persistent()
            .get(&DataKey::Window(source_chain))
            .unwrap_or(WindowState {
                start: 0,
                outflow: 0,
            })
    }

    /// Latest outbound nonce issued for a destination chain (0 if none).
    pub fn get_outbound_nonce(env: Env, dest_chain: u32) -> u64 {
        env.storage()
            .persistent()
            .get(&DataKey::OutNonce(dest_chain))
            .unwrap_or(0)
    }

    /// Open a hashed-timelock swap. Locks `amount` of `token` until `expiry`.
    /// Returns the swap ID.
    ///
    /// # Panics
    /// - [`ContractError::Paused`] if the bridge is paused
    /// - [`ContractError::AmountOutOfRange`] if `amount` is not positive
    /// - [`ContractError::SwapExpired`] if `expiry` is not in the future
    pub fn create_swap(
        env: Env,
        sender: Address,
        recipient: Address,
        token: Address,
        amount: i128,
        hashlock: BytesN<32>,
        expiry: u64,
    ) -> u64 {
        ensure_not_paused(&env);
        sender.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, ContractError::AmountOutOfRange);
        }
        if expiry <= env.ledger().timestamp() {
            panic_with_error!(&env, ContractError::SwapExpired);
        }

        token::Client::new(&env, &token).transfer(
            &sender,
            &env.current_contract_address(),
            &amount,
        );

        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextSwapId)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::NextSwapId, &(id + 1));

        let swap = Swap {
            sender: sender.clone(),
            recipient,
            token,
            amount,
            hashlock: hashlock.clone(),
            expiry,
            status: SwapStatus::Open,
        };
        let key = DataKey::Swap(id);
        env.storage().persistent().set(&key, &swap);
        bump(&env, &key);

        env.events().publish(
            (symbol_short!("BR_SWP_N"),),
            (id, sender, hashlock, expiry, amount),
        );
        id
    }

    /// Claim an open swap by revealing a preimage whose SHA-256 matches the hashlock.
    /// Funds go to the swap's recipient. Anyone holding the preimage may submit the claim.
    ///
    /// # Panics
    /// - [`ContractError::SwapNotFound`] if the swap does not exist
    /// - [`ContractError::SwapNotOpen`] if the swap was already claimed or refunded
    /// - [`ContractError::SwapExpired`] if the timelock has passed
    /// - [`ContractError::InvalidPreimage`] if the preimage does not match the hashlock
    pub fn claim_swap(env: Env, swap_id: u64, preimage: Bytes) {
        let key = DataKey::Swap(swap_id);
        let mut swap: Swap = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::SwapNotFound));
        if swap.status != SwapStatus::Open {
            panic_with_error!(&env, ContractError::SwapNotOpen);
        }
        if env.ledger().timestamp() >= swap.expiry {
            panic_with_error!(&env, ContractError::SwapExpired);
        }
        let digest: BytesN<32> = env.crypto().sha256(&preimage).into();
        if digest != swap.hashlock {
            panic_with_error!(&env, ContractError::InvalidPreimage);
        }

        swap.status = SwapStatus::Claimed;
        env.storage().persistent().set(&key, &swap);
        bump(&env, &key);

        token::Client::new(&env, &swap.token).transfer(
            &env.current_contract_address(),
            &swap.recipient,
            &swap.amount,
        );
        env.events()
            .publish((symbol_short!("BR_SWP_C"),), (swap_id, preimage));
    }

    /// Refund an open swap to its sender after the timelock has passed.
    /// Anyone may trigger the refund; funds always go back to the sender.
    ///
    /// # Panics
    /// - [`ContractError::SwapNotFound`] if the swap does not exist
    /// - [`ContractError::SwapNotOpen`] if the swap was already claimed or refunded
    /// - [`ContractError::SwapNotExpired`] if the timelock has not passed
    pub fn refund_swap(env: Env, swap_id: u64) {
        let key = DataKey::Swap(swap_id);
        let mut swap: Swap = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::SwapNotFound));
        if swap.status != SwapStatus::Open {
            panic_with_error!(&env, ContractError::SwapNotOpen);
        }
        if env.ledger().timestamp() < swap.expiry {
            panic_with_error!(&env, ContractError::SwapNotExpired);
        }

        swap.status = SwapStatus::Refunded;
        env.storage().persistent().set(&key, &swap);
        bump(&env, &key);

        token::Client::new(&env, &swap.token).transfer(
            &env.current_contract_address(),
            &swap.sender,
            &swap.amount,
        );
        env.events()
            .publish((symbol_short!("BR_SWP_R"),), (swap_id,));
    }

    /// Get a swap by ID.
    ///
    /// # Panics
    /// - [`ContractError::SwapNotFound`] if the swap does not exist
    pub fn get_swap(env: Env, swap_id: u64) -> Swap {
        env.storage()
            .persistent()
            .get(&DataKey::Swap(swap_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::SwapNotFound))
    }
}
