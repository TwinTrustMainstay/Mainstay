//! Maintenance auction types and pure helpers.
//!
//! An auction is either *open* (bids are public and placed directly) or *sealed*
//! (bidders first commit to a hash of their price and a salt, then reveal during a
//! second window). Sealed bids stop a bidder from undercutting a price they can
//! see, at the cost of an extra transaction per bidder.

use soroban_sdk::{Address, Bytes, BytesN, Env, Symbol, Vec};

use soroban_sdk::xdr::ToXdr;

/// Hard cap on bids per auction, bounding the cost of winner selection.
pub const MAX_BIDS_PER_AUCTION: u32 = 100;

/// How long after finalization an awarded auction can be disputed (3 days).
pub const DISPUTE_WINDOW_SECS: u64 = 3 * 24 * 60 * 60;

#[soroban_sdk::contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WinnerCriterion {
    /// Lowest price wins. Ties go to the earliest bid.
    LowestPrice = 0,
    /// Highest reputation at bid time wins. Ties go to the lower price, then the earliest bid.
    HighestReputation = 1,
}

#[soroban_sdk::contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuctionStatus {
    /// Accepting bids or commitments, or waiting to be finalized.
    Open = 0,
    /// Finalized with no valid bids.
    Unawarded = 1,
    /// Finalized with a winner. Can be disputed within the dispute window.
    Awarded = 2,
    /// A bidder disputed the award. Waiting for admin resolution.
    Disputed = 3,
    /// Dispute resolved by the admin. The winner is final.
    Resolved = 4,
}

#[soroban_sdk::contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Auction {
    pub id: u64,
    pub asset_id: u64,
    pub task_type: Symbol,
    pub ceiling_price: u64,
    pub sealed: bool,
    pub criterion: WinnerCriterion,
    /// End of the bidding (or commit) window.
    pub bid_end: u64,
    /// End of the reveal window. Equal to `bid_end` for open auctions.
    pub reveal_end: u64,
    pub status: AuctionStatus,
    pub winner: Option<Address>,
    /// Price the winner will be paid. `0` until an award is made.
    pub winning_price: u64,
    /// Ledger timestamp at which the auction was finalized. `0` while open.
    pub finalized_at: u64,
}

/// Parameters for creating an auction. Bundled to keep the entry point signature small.
#[soroban_sdk::contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuctionParams {
    pub asset_id: u64,
    pub task_type: Symbol,
    /// Maximum acceptable price. `None` uses the current dynamic price.
    pub ceiling_price: Option<u64>,
    pub sealed: bool,
    pub criterion: WinnerCriterion,
    pub bid_end: u64,
    /// Required for sealed auctions (must be after `bid_end`). Ignored for open auctions.
    pub reveal_end: u64,
}

#[soroban_sdk::contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bid {
    pub engineer: Address,
    pub price: u64,
    /// Reputation snapshot taken when the bid was placed or revealed.
    pub reputation: u32,
    pub placed_at: u64,
}

/// Commitment to a sealed bid: `sha256(auction_id || engineer || price || salt)`.
///
/// Binding the engineer and auction id prevents a bidder from copying another
/// bidder's commitment from the ledger and revealing it as their own.
pub fn commitment(
    env: &Env,
    auction_id: u64,
    engineer: &Address,
    price: u64,
    salt: &BytesN<32>,
) -> BytesN<32> {
    let mut preimage = Bytes::new(env);
    preimage.append(&Bytes::from_slice(env, &auction_id.to_be_bytes()));
    preimage.append(&engineer.clone().to_xdr(env));
    preimage.append(&Bytes::from_slice(env, &price.to_be_bytes()));
    preimage.append(&Bytes::from_slice(env, &salt.to_array()));
    env.crypto().sha256(&preimage).to_bytes()
}

/// Index of the winning bid under `criterion`, or `None` if there are no bids.
pub fn select_winner(bids: &Vec<Bid>, criterion: WinnerCriterion) -> Option<u32> {
    let mut best: Option<(u32, Bid)> = None;
    for (idx, bid) in bids.iter().enumerate() {
        let idx = idx as u32;
        let better = match &best {
            None => true,
            Some((_, current)) => match criterion {
                WinnerCriterion::LowestPrice => bid.price < current.price,
                WinnerCriterion::HighestReputation => {
                    bid.reputation > current.reputation
                        || (bid.reputation == current.reputation && bid.price < current.price)
                }
            },
        };
        if better {
            best = Some((idx, bid));
        }
    }
    best.map(|(idx, _)| idx)
}

/// Index of the bid placed by `engineer`, if any.
pub fn find_bid(bids: &Vec<Bid>, engineer: &Address) -> Option<u32> {
    bids.iter()
        .position(|b| b.engineer == *engineer)
        .map(|i| i as u32)
}
