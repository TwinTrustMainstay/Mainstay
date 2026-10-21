#![no_std]

//! Asset tokenization as NFTs (issue #1736).
//!
//! Each registered asset can be minted as exactly one NFT. The NFT owner is the asset owner,
//! so transferring the NFT transfers the asset. Metadata (URI, collateral score, maintenance
//! count) is kept in sync by the admin. Listed NFTs can be bought for a payment token, and
//! every mint, transfer and sale is recorded in the token's trading history.

#[cfg(test)]
mod test;

use shared::{extend_persistent_ttl, require_admin};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, token,
    Address, Env, String, Vec,
};

/// Maximum trading history entries kept per NFT. Oldest entries are dropped first.
pub const MAX_HISTORY: u32 = 100;

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
    /// No NFT exists with the given id.
    TokenNotFound = 4,
    /// The asset already has an NFT.
    AssetAlreadyTokenized = 5,
    /// Caller does not own the NFT.
    NotTokenOwner = 6,
    /// Price must be greater than zero.
    InvalidPrice = 7,
    /// The NFT is listed for sale and must be delisted before transfer.
    TokenListed = 8,
    /// The NFT is not listed for sale.
    NotListed = 9,
    /// The owner cannot buy their own NFT.
    CannotBuyOwnToken = 10,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum TradeKind {
    Mint = 0,
    Transfer = 1,
    Sale = 2,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetToken {
    pub id: u64,
    pub asset_id: u64,
    pub owner: Address,
    pub uri: String,
    pub collateral_score: u32,
    pub maintenance_count: u32,
    /// Ledger timestamp of the last metadata sync.
    pub synced_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Listing {
    pub token_id: u64,
    pub seller: Address,
    pub price: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Trade {
    pub kind: TradeKind,
    /// `None` for mints.
    pub from: Option<Address>,
    pub to: Address,
    /// Sale price in the payment token; `0` for mints and plain transfers.
    pub price: i128,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    PaymentToken,
    NextId,
    Token(u64),
    TokenByAsset(u64),
    Listing(u64),
    History(u64),
}

#[contract]
pub struct AssetNft;

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

fn load_token(env: &Env, id: u64) -> AssetToken {
    env.storage()
        .persistent()
        .get(&DataKey::Token(id))
        .unwrap_or_else(|| panic_with_error!(env, ContractError::TokenNotFound))
}

fn save_token(env: &Env, token: &AssetToken) {
    let key = DataKey::Token(token.id);
    env.storage().persistent().set(&key, token);
    extend_persistent_ttl(env, key);
}

fn record_trade(env: &Env, token_id: u64, trade: Trade) {
    let key = DataKey::History(token_id);
    let mut history: Vec<Trade> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    if history.len() >= MAX_HISTORY {
        history.remove(0);
    }
    history.push_back(trade);
    env.storage().persistent().set(&key, &history);
    extend_persistent_ttl(env, key);
}

/// Moves ownership of an NFT, which moves the asset with it. Callers must ensure the
/// NFT is not listed.
fn move_token(env: &Env, token: &mut AssetToken, to: &Address) {
    token.owner = to.clone();
    save_token(env, token);
}

#[contractimpl]
impl AssetNft {
    /// Sets the admin (asset registry bridge) and the token used for NFT sales.
    pub fn initialize(env: Env, admin: Address, payment_token: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, ContractError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::PaymentToken, &payment_token);
        env.storage().instance().set(&DataKey::NextId, &1u64);
    }

    /// Mints the NFT for an asset to `owner`. Admin only. One NFT per asset. Returns the token id.
    pub fn mint(env: Env, admin: Address, asset_id: u64, owner: Address, uri: String) -> u64 {
        require_stored_admin(&env, &admin);
        if env
            .storage()
            .persistent()
            .has(&DataKey::TokenByAsset(asset_id))
        {
            panic_with_error!(&env, ContractError::AssetAlreadyTokenized);
        }

        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(1);
        let now = env.ledger().timestamp();
        let token = AssetToken {
            id,
            asset_id,
            owner: owner.clone(),
            uri,
            collateral_score: 0,
            maintenance_count: 0,
            synced_at: now,
        };
        save_token(&env, &token);

        let asset_key = DataKey::TokenByAsset(asset_id);
        env.storage().persistent().set(&asset_key, &id);
        extend_persistent_ttl(&env, asset_key);
        env.storage().instance().set(&DataKey::NextId, &(id + 1));

        record_trade(
            &env,
            id,
            Trade {
                kind: TradeKind::Mint,
                from: None,
                to: owner.clone(),
                price: 0,
                timestamp: now,
            },
        );
        env.events()
            .publish((symbol_short!("nft_mint"),), (id, asset_id, owner));
        id
    }

    /// Transfers the NFT and the asset it represents. Fails while the NFT is listed.
    pub fn transfer(env: Env, from: Address, to: Address, token_id: u64) {
        from.require_auth();
        let mut token = load_token(&env, token_id);
        if token.owner != from {
            panic_with_error!(&env, ContractError::NotTokenOwner);
        }
        if env.storage().persistent().has(&DataKey::Listing(token_id)) {
            panic_with_error!(&env, ContractError::TokenListed);
        }

        move_token(&env, &mut token, &to);
        record_trade(
            &env,
            token_id,
            Trade {
                kind: TradeKind::Transfer,
                from: Some(from.clone()),
                to: to.clone(),
                price: 0,
                timestamp: env.ledger().timestamp(),
            },
        );
        env.events()
            .publish((symbol_short!("nft_xfer"),), (token_id, from, to));
    }

    /// Updates the NFT metadata from the asset registry. Admin only.
    pub fn sync_metadata(
        env: Env,
        admin: Address,
        token_id: u64,
        uri: String,
        collateral_score: u32,
        maintenance_count: u32,
    ) {
        require_stored_admin(&env, &admin);
        let mut token = load_token(&env, token_id);
        token.uri = uri;
        token.collateral_score = collateral_score;
        token.maintenance_count = maintenance_count;
        token.synced_at = env.ledger().timestamp();
        save_token(&env, &token);

        env.events()
            .publish((symbol_short!("nft_sync"),), (token_id, collateral_score));
    }

    /// Lists an NFT for sale at `price` payment-token units. Owner only.
    pub fn list_for_sale(env: Env, owner: Address, token_id: u64, price: i128) {
        owner.require_auth();
        if price <= 0 {
            panic_with_error!(&env, ContractError::InvalidPrice);
        }
        let token = load_token(&env, token_id);
        if token.owner != owner {
            panic_with_error!(&env, ContractError::NotTokenOwner);
        }

        let key = DataKey::Listing(token_id);
        env.storage().persistent().set(
            &key,
            &Listing {
                token_id,
                seller: owner,
                price,
            },
        );
        extend_persistent_ttl(&env, key);
    }

    /// Removes an active listing. Owner only.
    pub fn cancel_listing(env: Env, owner: Address, token_id: u64) {
        owner.require_auth();
        let listing = Self::get_listing(env.clone(), token_id)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotListed));
        if listing.seller != owner {
            panic_with_error!(&env, ContractError::NotTokenOwner);
        }
        env.storage()
            .persistent()
            .remove(&DataKey::Listing(token_id));
    }

    /// Buys a listed NFT. The price is paid to the seller and ownership transfers to the buyer.
    pub fn buy(env: Env, buyer: Address, token_id: u64) {
        buyer.require_auth();
        let listing = Self::get_listing(env.clone(), token_id)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotListed));
        if listing.seller == buyer {
            panic_with_error!(&env, ContractError::CannotBuyOwnToken);
        }

        let payment: Address = env
            .storage()
            .instance()
            .get(&DataKey::PaymentToken)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        token::Client::new(&env, &payment).transfer(&buyer, &listing.seller, &listing.price);

        env.storage()
            .persistent()
            .remove(&DataKey::Listing(token_id));
        let mut token = load_token(&env, token_id);
        move_token(&env, &mut token, &buyer);

        let now = env.ledger().timestamp();
        record_trade(
            &env,
            token_id,
            Trade {
                kind: TradeKind::Sale,
                from: Some(listing.seller.clone()),
                to: buyer.clone(),
                price: listing.price,
                timestamp: now,
            },
        );
        env.events().publish(
            (symbol_short!("nft_sale"),),
            (token_id, buyer, listing.price),
        );
    }

    pub fn get_token(env: Env, token_id: u64) -> AssetToken {
        load_token(&env, token_id)
    }

    /// Returns the current owner of the asset represented by `asset_id`, if it is tokenized.
    pub fn get_asset_owner(env: Env, asset_id: u64) -> Option<Address> {
        let token_id: Option<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::TokenByAsset(asset_id));
        token_id.map(|id| load_token(&env, id).owner)
    }

    pub fn get_token_for_asset(env: Env, asset_id: u64) -> Option<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::TokenByAsset(asset_id))
    }

    pub fn get_listing(env: Env, token_id: u64) -> Option<Listing> {
        env.storage().persistent().get(&DataKey::Listing(token_id))
    }

    pub fn get_history(env: Env, token_id: u64) -> Vec<Trade> {
        env.storage()
            .persistent()
            .get(&DataKey::History(token_id))
            .unwrap_or_else(|| Vec::new(&env))
    }
}
