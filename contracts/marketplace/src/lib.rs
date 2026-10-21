#![no_std]

//! Maintenance marketplace: dynamic pricing (issue #1738) and competitive
//! maintenance auctions (issue #1739).
//!
//! * **Dynamic pricing**: the admin records supply (available engineers) and
//!   demand (pending tasks). The contract derives a price multiplier from the
//!   imbalance, moves toward it by at most `max_step_bps` per update, and keeps
//!   the surge within `[min_multiplier_bps, max_multiplier_bps]`. Price history
//!   is kept (bounded) and an elasticity estimate is derived from it.
//! * **Auctions**: the admin opens an auction for a maintenance task. Credentialed
//!   engineers bid, either openly or via a sealed commit/reveal. The admin-free
//!   `finalize_auction` picks a winner by price or reputation. Awards can be
//!   disputed within a window and the admin resolves the dispute.

mod auction;
mod pricing;

use shared::require_admin;
use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, panic_with_error,
    symbol_short, Address, BytesN, Env, String, Symbol, Vec,
};

pub use auction::{
    commitment, select_winner, Auction, AuctionParams, AuctionStatus, Bid, WinnerCriterion,
    DISPUTE_WINDOW_SECS, MAX_BIDS_PER_AUCTION,
};

/// Maximum number of price observations retained.
pub const MAX_PRICE_HISTORY: u32 = 100;

const BPS: u32 = 10_000;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ContractError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    UnauthorizedAdmin = 3,
    /// Pricing configuration is internally inconsistent.
    InvalidPricingConfig = 4,
    /// A price update was attempted before `min_update_interval_secs` elapsed.
    PriceUpdateTooSoon = 5,
    AuctionNotFound = 6,
    /// The auction is not open for this action (already finalized or disputed).
    AuctionNotOpen = 7,
    /// Bidding or commit window has closed.
    BiddingClosed = 8,
    /// Reveal window is not currently open.
    RevealWindowClosed = 9,
    /// Price is zero or above the auction ceiling.
    InvalidBid = 10,
    /// Engineer does not hold a valid or grace-period credential.
    NotCredentialed = 11,
    /// Engineer has already bid or committed on this auction.
    AlreadyBid = 12,
    /// No sealed commitment exists for this engineer on this auction.
    NoCommitment = 13,
    /// Revealed price and salt do not match the commitment.
    RevealMismatch = 14,
    /// The auction's bidding or reveal window has not ended yet.
    AuctionStillOpen = 15,
    /// Auction configuration is invalid (e.g. end time in the past).
    InvalidAuctionConfig = 16,
    /// Action requires an awarded auction.
    NotAwarded = 17,
    /// The dispute window for this award has closed.
    DisputeWindowClosed = 18,
    /// Caller is not a bidder on this auction.
    NotABidder = 19,
    /// Action requires an auction in the disputed state.
    NotDisputed = 20,
    /// Auction is in the other mode: use `place_bid` for open auctions, `commit_bid` for sealed.
    AuctionModeMismatch = 21,
    /// The auction already has the maximum number of bids.
    BidLimitReached = 22,
}

/// Credential status as reported by the engineer registry. Variants must match
/// `engineer-registry`'s `CredentialStatus` to decode correctly.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CredentialStatus {
    Valid = 0,
    GracePeriod = 1,
    HardExpired = 2,
    Revoked = 3,
    NotFound = 4,
    Suspended = 5,
    Expired = 6,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PricingConfig {
    /// Price at a 1.0x multiplier, in the marketplace's price unit.
    pub base_price: u64,
    /// Lowest multiplier allowed (bps). Must be at most 10_000.
    pub min_multiplier_bps: u32,
    /// Surge cap (bps). Must be at least 10_000.
    pub max_multiplier_bps: u32,
    /// Largest multiplier change allowed per update (bps). Must be non-zero.
    pub max_step_bps: u32,
    /// How strongly an imbalance moves the target multiplier (10_000 = 1:1).
    pub sensitivity_bps: u32,
    /// Minimum seconds between price updates.
    pub min_update_interval_secs: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarketState {
    pub timestamp: u64,
    pub supply: u32,
    pub demand: u32,
    pub multiplier_bps: u32,
    pub price: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum DataKey {
    Admin,
    Registry,
    Pricing,
    PriceHistory,
    NextAuctionId,
    Auction(u64),
    Bids(u64),
    Commit(u64, Address),
}

/// Minimal client for the engineer registry. Only the methods used here are declared.
#[allow(dead_code)]
#[contractclient(name = "EngineerRegistryClient")]
pub trait EngineerRegistry {
    fn verify_engineer(
        env: Env,
        engineer: Address,
        required_specialization: Option<Symbol>,
    ) -> CredentialStatus;
    fn get_reputation(env: Env, engineer: Address) -> u32;
}

const PRICE_UPDATE_TOPIC: Symbol = symbol_short!("PRC_UPD");
const AUCTION_NEW_TOPIC: Symbol = symbol_short!("AUC_NEW");
const BID_TOPIC: Symbol = symbol_short!("AUC_BID");
const COMMIT_TOPIC: Symbol = symbol_short!("AUC_COMM");
const REVEAL_TOPIC: Symbol = symbol_short!("AUC_REV");
const FINALIZE_TOPIC: Symbol = symbol_short!("AUC_FIN");
const DISPUTE_TOPIC: Symbol = symbol_short!("AUC_DISP");
const RESOLVE_TOPIC: Symbol = symbol_short!("AUC_RSLV");

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

fn load_pricing(env: &Env) -> PricingConfig {
    env.storage()
        .instance()
        .get(&DataKey::Pricing)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized))
}

fn load_history(env: &Env) -> Vec<MarketState> {
    env.storage()
        .persistent()
        .get(&DataKey::PriceHistory)
        .unwrap_or_else(|| Vec::new(env))
}

fn current_price(env: &Env) -> u64 {
    let cfg = load_pricing(env);
    match load_history(env).last() {
        Some(state) => state.price,
        None => cfg.base_price,
    }
}

fn load_auction(env: &Env, auction_id: u64) -> Auction {
    env.storage()
        .persistent()
        .get(&DataKey::Auction(auction_id))
        .unwrap_or_else(|| panic_with_error!(env, ContractError::AuctionNotFound))
}

fn store_auction(env: &Env, auction: &Auction) {
    let key = DataKey::Auction(auction.id);
    env.storage().persistent().set(&key, auction);
    env.storage()
        .persistent()
        .extend_ttl(&key, shared::TTL_THRESHOLD, shared::TTL_TARGET);
}

fn load_bids(env: &Env, auction_id: u64) -> Vec<Bid> {
    env.storage()
        .persistent()
        .get(&DataKey::Bids(auction_id))
        .unwrap_or_else(|| Vec::new(env))
}

fn store_bids(env: &Env, auction_id: u64, bids: &Vec<Bid>) {
    let key = DataKey::Bids(auction_id);
    env.storage().persistent().set(&key, bids);
    env.storage()
        .persistent()
        .extend_ttl(&key, shared::TTL_THRESHOLD, shared::TTL_TARGET);
}

/// Panics unless `engineer` holds a valid or grace-period credential.
fn ensure_credentialed(env: &Env, engineer: &Address) {
    let registry: Address = env
        .storage()
        .instance()
        .get(&DataKey::Registry)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized));
    let status = EngineerRegistryClient::new(env, &registry).verify_engineer(engineer, &None);
    if !matches!(
        status,
        CredentialStatus::Valid | CredentialStatus::GracePeriod
    ) {
        panic_with_error!(env, ContractError::NotCredentialed);
    }
}

fn reputation_of(env: &Env, engineer: &Address) -> u32 {
    let registry: Address = env
        .storage()
        .instance()
        .get(&DataKey::Registry)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized));
    EngineerRegistryClient::new(env, &registry).get_reputation(engineer)
}

#[contract]
pub struct Marketplace;

#[contractimpl]
impl Marketplace {
    /// One-time setup.
    ///
    /// # Arguments
    /// * `admin` - Address allowed to record market state, create auctions, and resolve disputes
    /// * `engineer_registry` - Address of the engineer registry used for credential and reputation checks
    /// * `pricing` - Initial pricing configuration
    ///
    /// # Panics
    /// - [`ContractError::AlreadyInitialized`] if called twice
    /// - [`ContractError::InvalidPricingConfig`] if `pricing` is inconsistent
    pub fn initialize(
        env: Env,
        admin: Address,
        engineer_registry: Address,
        pricing: PricingConfig,
    ) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, ContractError::AlreadyInitialized);
        }
        if !pricing::is_valid_config(&pricing) {
            panic_with_error!(&env, ContractError::InvalidPricingConfig);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::Registry, &engineer_registry);
        env.storage().instance().set(&DataKey::Pricing, &pricing);
        env.storage().instance().set(&DataKey::NextAuctionId, &1u64);
        env.storage()
            .instance()
            .extend_ttl(shared::TTL_THRESHOLD, shared::TTL_TARGET);
    }

    /// Admin-only: replace the pricing configuration. Existing history is kept.
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if `admin` is not the stored admin
    /// - [`ContractError::InvalidPricingConfig`] if `pricing` is inconsistent
    pub fn update_pricing_config(env: Env, admin: Address, pricing: PricingConfig) {
        require_admin_auth(&env, &admin);
        if !pricing::is_valid_config(&pricing) {
            panic_with_error!(&env, ContractError::InvalidPricingConfig);
        }
        env.storage().instance().set(&DataKey::Pricing, &pricing);
    }

    /// Admin-only: record the current supply (available engineers) and demand
    /// (pending tasks), and move the price toward the implied target.
    ///
    /// The multiplier moves by at most `max_step_bps` per call and is clamped to
    /// the configured surge range. Calls closer together than
    /// `min_update_interval_secs` are rejected.
    ///
    /// # Returns
    /// The new price.
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if `admin` is not the stored admin
    /// - [`ContractError::PriceUpdateTooSoon`] if the update interval has not elapsed
    pub fn record_market_state(env: Env, admin: Address, supply: u32, demand: u32) -> u64 {
        require_admin_auth(&env, &admin);
        let cfg = load_pricing(&env);
        let mut history = load_history(&env);
        let now = env.ledger().timestamp();

        let current_multiplier = match history.last() {
            Some(last) => {
                if now.saturating_sub(last.timestamp) < cfg.min_update_interval_secs {
                    panic_with_error!(&env, ContractError::PriceUpdateTooSoon);
                }
                last.multiplier_bps
            }
            None => BPS,
        };

        let target = pricing::target_multiplier_bps(&cfg, supply, demand);
        let multiplier = pricing::step_towards(current_multiplier, target, cfg.max_step_bps);
        let price = pricing::price_for(cfg.base_price, multiplier);

        if history.len() >= MAX_PRICE_HISTORY {
            history.pop_front();
        }
        history.push_back(MarketState {
            timestamp: now,
            supply,
            demand,
            multiplier_bps: multiplier,
            price,
        });
        env.storage()
            .persistent()
            .set(&DataKey::PriceHistory, &history);
        env.storage().persistent().extend_ttl(
            &DataKey::PriceHistory,
            shared::TTL_THRESHOLD,
            shared::TTL_TARGET,
        );

        env.events().publish(
            (PRICE_UPDATE_TOPIC,),
            (price, multiplier, supply, demand, now),
        );
        price
    }

    /// Current dynamic price. Returns the base price before any market state is recorded.
    pub fn get_price(env: Env) -> u64 {
        current_price(&env)
    }

    /// Recorded price observations, oldest first, capped at [`MAX_PRICE_HISTORY`].
    pub fn get_price_history(env: Env) -> Vec<MarketState> {
        load_history(&env)
    }

    /// Estimated price elasticity of demand between the last two observations, in bps.
    ///
    /// Negative means demand fell as price rose. Returns `0` when fewer than two
    /// observations exist or the estimate is undefined.
    pub fn get_price_elasticity(env: Env) -> i64 {
        let history = load_history(&env);
        let len = history.len();
        if len < 2 {
            return 0;
        }
        let prev = history.get(len - 2).unwrap();
        let curr = history.get(len - 1).unwrap();
        pricing::elasticity_bps(&prev, &curr)
    }

    /// Admin-only: open an auction for a maintenance task.
    ///
    /// For open auctions `reveal_end` is ignored and bidding closes at `bid_end`.
    /// For sealed auctions `reveal_end` must be after `bid_end`.
    ///
    /// # Returns
    /// The new auction id.
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if `admin` is not the stored admin
    /// - [`ContractError::InvalidAuctionConfig`] if a deadline is in the past, the
    ///   reveal window is empty, or the ceiling is zero
    pub fn create_auction(env: Env, admin: Address, params: AuctionParams) -> u64 {
        require_admin_auth(&env, &admin);
        let now = env.ledger().timestamp();
        if params.bid_end <= now {
            panic_with_error!(&env, ContractError::InvalidAuctionConfig);
        }
        let reveal_end = if params.sealed {
            if params.reveal_end <= params.bid_end {
                panic_with_error!(&env, ContractError::InvalidAuctionConfig);
            }
            params.reveal_end
        } else {
            params.bid_end
        };
        let ceiling_price = params.ceiling_price.unwrap_or_else(|| current_price(&env));
        if ceiling_price == 0 {
            panic_with_error!(&env, ContractError::InvalidAuctionConfig);
        }

        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextAuctionId)
            .unwrap_or(1);
        env.storage()
            .instance()
            .set(&DataKey::NextAuctionId, &(id + 1));

        let auction = Auction {
            id,
            asset_id: params.asset_id,
            task_type: params.task_type,
            ceiling_price,
            sealed: params.sealed,
            criterion: params.criterion,
            bid_end: params.bid_end,
            reveal_end,
            status: AuctionStatus::Open,
            winner: None,
            winning_price: 0,
            finalized_at: 0,
        };
        store_auction(&env, &auction);
        env.events().publish(
            (AUCTION_NEW_TOPIC, id),
            (
                params.asset_id,
                ceiling_price,
                params.sealed,
                params.bid_end,
                reveal_end,
            ),
        );
        id
    }

    /// Place a public bid on an open auction.
    ///
    /// # Panics
    /// - [`ContractError::AuctionNotFound`] if the auction does not exist
    /// - [`ContractError::AuctionModeMismatch`] if the auction is sealed
    /// - [`ContractError::AuctionNotOpen`] if the auction is no longer open
    /// - [`ContractError::BiddingClosed`] if `bid_end` has passed
    /// - [`ContractError::InvalidBid`] if `price` is zero or above the ceiling
    /// - [`ContractError::NotCredentialed`] if the engineer has no valid credential
    /// - [`ContractError::AlreadyBid`] if the engineer already bid
    /// - [`ContractError::BidLimitReached`] if the auction is full
    pub fn place_bid(env: Env, engineer: Address, auction_id: u64, price: u64) {
        engineer.require_auth();
        let auction = load_auction(&env, auction_id);
        if auction.sealed {
            panic_with_error!(&env, ContractError::AuctionModeMismatch);
        }
        if auction.status != AuctionStatus::Open {
            panic_with_error!(&env, ContractError::AuctionNotOpen);
        }
        let now = env.ledger().timestamp();
        if now > auction.bid_end {
            panic_with_error!(&env, ContractError::BiddingClosed);
        }
        if price == 0 || price > auction.ceiling_price {
            panic_with_error!(&env, ContractError::InvalidBid);
        }
        ensure_credentialed(&env, &engineer);

        let mut bids = load_bids(&env, auction_id);
        if auction::find_bid(&bids, &engineer).is_some() {
            panic_with_error!(&env, ContractError::AlreadyBid);
        }
        if bids.len() >= MAX_BIDS_PER_AUCTION {
            panic_with_error!(&env, ContractError::BidLimitReached);
        }
        let reputation = reputation_of(&env, &engineer);
        bids.push_back(Bid {
            engineer: engineer.clone(),
            price,
            reputation,
            placed_at: now,
        });
        store_bids(&env, auction_id, &bids);
        env.events()
            .publish((BID_TOPIC, auction_id), (engineer, price, reputation, now));
    }

    /// Commit to a sealed bid. The price is hidden until [`reveal_bid`].
    ///
    /// `commitment` must be produced with [`commitment`]: `sha256(auction_id || engineer || price || salt)`.
    ///
    /// # Panics
    /// - [`ContractError::AuctionNotFound`] if the auction does not exist
    /// - [`ContractError::AuctionModeMismatch`] if the auction is open
    /// - [`ContractError::AuctionNotOpen`] if the auction is no longer open
    /// - [`ContractError::BiddingClosed`] if `bid_end` has passed
    /// - [`ContractError::NotCredentialed`] if the engineer has no valid credential
    /// - [`ContractError::AlreadyBid`] if the engineer already committed
    pub fn commit_bid(env: Env, engineer: Address, auction_id: u64, commitment: BytesN<32>) {
        engineer.require_auth();
        let auction = load_auction(&env, auction_id);
        if !auction.sealed {
            panic_with_error!(&env, ContractError::AuctionModeMismatch);
        }
        if auction.status != AuctionStatus::Open {
            panic_with_error!(&env, ContractError::AuctionNotOpen);
        }
        if env.ledger().timestamp() > auction.bid_end {
            panic_with_error!(&env, ContractError::BiddingClosed);
        }
        ensure_credentialed(&env, &engineer);

        let key = DataKey::Commit(auction_id, engineer.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, ContractError::AlreadyBid);
        }
        env.storage().persistent().set(&key, &commitment);
        env.storage()
            .persistent()
            .extend_ttl(&key, shared::TTL_THRESHOLD, shared::TTL_TARGET);
        env.events()
            .publish((COMMIT_TOPIC, auction_id), (engineer,));
    }

    /// Reveal a sealed bid during the reveal window.
    ///
    /// A commitment with no reveal by `reveal_end` is ignored at finalization.
    /// The bidder forfeits that bid.
    ///
    /// # Panics
    /// - [`ContractError::AuctionNotFound`] if the auction does not exist
    /// - [`ContractError::AuctionModeMismatch`] if the auction is open
    /// - [`ContractError::AuctionNotOpen`] if the auction is no longer open
    /// - [`ContractError::RevealWindowClosed`] if the reveal window is not active
    /// - [`ContractError::NoCommitment`] if the engineer has no commitment
    /// - [`ContractError::RevealMismatch`] if price and salt do not match the commitment
    /// - [`ContractError::InvalidBid`] if `price` is zero or above the ceiling
    /// - [`ContractError::BidLimitReached`] if the auction is full
    pub fn reveal_bid(env: Env, engineer: Address, auction_id: u64, price: u64, salt: BytesN<32>) {
        engineer.require_auth();
        let auction = load_auction(&env, auction_id);
        if !auction.sealed {
            panic_with_error!(&env, ContractError::AuctionModeMismatch);
        }
        if auction.status != AuctionStatus::Open {
            panic_with_error!(&env, ContractError::AuctionNotOpen);
        }
        let now = env.ledger().timestamp();
        if now <= auction.bid_end || now > auction.reveal_end {
            panic_with_error!(&env, ContractError::RevealWindowClosed);
        }

        let key = DataKey::Commit(auction_id, engineer.clone());
        let stored: BytesN<32> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NoCommitment));
        if commitment(&env, auction_id, &engineer, price, &salt) != stored {
            panic_with_error!(&env, ContractError::RevealMismatch);
        }
        if price == 0 || price > auction.ceiling_price {
            panic_with_error!(&env, ContractError::InvalidBid);
        }

        let mut bids = load_bids(&env, auction_id);
        if bids.len() >= MAX_BIDS_PER_AUCTION {
            panic_with_error!(&env, ContractError::BidLimitReached);
        }
        let reputation = reputation_of(&env, &engineer);
        bids.push_back(Bid {
            engineer: engineer.clone(),
            price,
            reputation,
            placed_at: now,
        });
        store_bids(&env, auction_id, &bids);
        // Remove the commitment so the same reveal cannot be submitted twice.
        env.storage().persistent().remove(&key);
        env.events()
            .publish((REVEAL_TOPIC, auction_id), (engineer, price, reputation));
    }

    /// Close an auction after its bidding (open) or reveal (sealed) window and pick a winner.
    ///
    /// Callable by anyone. The auction becomes `Awarded` if there was at least one
    /// valid bid, otherwise `Unawarded`.
    ///
    /// # Panics
    /// - [`ContractError::AuctionNotFound`] if the auction does not exist
    /// - [`ContractError::AuctionNotOpen`] if the auction was already finalized
    /// - [`ContractError::AuctionStillOpen`] if the relevant window has not ended
    pub fn finalize_auction(env: Env, auction_id: u64) {
        let mut auction = load_auction(&env, auction_id);
        if auction.status != AuctionStatus::Open {
            panic_with_error!(&env, ContractError::AuctionNotOpen);
        }
        let now = env.ledger().timestamp();
        if now <= auction.reveal_end {
            panic_with_error!(&env, ContractError::AuctionStillOpen);
        }

        let bids = load_bids(&env, auction_id);
        auction.finalized_at = now;
        match select_winner(&bids, auction.criterion) {
            Some(idx) => {
                let bid = bids.get(idx).unwrap();
                auction.status = AuctionStatus::Awarded;
                auction.winner = Some(bid.engineer.clone());
                auction.winning_price = bid.price;
            }
            None => {
                auction.status = AuctionStatus::Unawarded;
            }
        }
        let winner_event = auction.winner.clone();
        let winning_price = auction.winning_price;
        store_auction(&env, &auction);
        env.events().publish(
            (FINALIZE_TOPIC, auction_id),
            (winner_event, winning_price, now),
        );
    }

    /// Dispute an awarded auction. Only a bidder on this auction may dispute, and
    /// only within [`DISPUTE_WINDOW_SECS`] of finalization.
    ///
    /// # Panics
    /// - [`ContractError::AuctionNotFound`] if the auction does not exist
    /// - [`ContractError::NotAwarded`] if the auction is not in the awarded state
    /// - [`ContractError::DisputeWindowClosed`] if the window has passed
    /// - [`ContractError::NotABidder`] if the caller did not bid
    pub fn open_dispute(env: Env, bidder: Address, auction_id: u64, reason: String) {
        bidder.require_auth();
        let mut auction = load_auction(&env, auction_id);
        if auction.status != AuctionStatus::Awarded {
            panic_with_error!(&env, ContractError::NotAwarded);
        }
        let now = env.ledger().timestamp();
        if now > auction.finalized_at.saturating_add(DISPUTE_WINDOW_SECS) {
            panic_with_error!(&env, ContractError::DisputeWindowClosed);
        }
        let bids = load_bids(&env, auction_id);
        if auction::find_bid(&bids, &bidder).is_none() {
            panic_with_error!(&env, ContractError::NotABidder);
        }
        auction.status = AuctionStatus::Disputed;
        store_auction(&env, &auction);
        env.events()
            .publish((DISPUTE_TOPIC, auction_id), (bidder, reason, now));
    }

    /// Admin-only: resolve a disputed auction by naming the final winner. The winner
    /// must be a bidder on this auction. Naming the original winner upholds the award.
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if `admin` is not the stored admin
    /// - [`ContractError::AuctionNotFound`] if the auction does not exist
    /// - [`ContractError::NotDisputed`] if the auction is not disputed
    /// - [`ContractError::NotABidder`] if `final_winner` did not bid
    pub fn resolve_dispute(env: Env, admin: Address, auction_id: u64, final_winner: Address) {
        require_admin_auth(&env, &admin);
        let mut auction = load_auction(&env, auction_id);
        if auction.status != AuctionStatus::Disputed {
            panic_with_error!(&env, ContractError::NotDisputed);
        }
        let bids = load_bids(&env, auction_id);
        let idx = auction::find_bid(&bids, &final_winner)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotABidder));
        let bid = bids.get(idx).unwrap();
        auction.status = AuctionStatus::Resolved;
        auction.winner = Some(final_winner.clone());
        auction.winning_price = bid.price;
        store_auction(&env, &auction);
        env.events().publish(
            (RESOLVE_TOPIC, auction_id),
            (admin, final_winner, bid.price),
        );
    }

    /// Get an auction by id.
    ///
    /// # Panics
    /// - [`ContractError::AuctionNotFound`] if the auction does not exist
    pub fn get_auction(env: Env, auction_id: u64) -> Auction {
        load_auction(&env, auction_id)
    }

    /// Bid history for an auction, in placement order. For sealed auctions this
    /// contains only revealed bids.
    pub fn get_bids(env: Env, auction_id: u64) -> Vec<Bid> {
        load_bids(&env, auction_id)
    }
}

#[cfg(test)]
mod test;
