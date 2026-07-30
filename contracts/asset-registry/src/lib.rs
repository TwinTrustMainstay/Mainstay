#![no_std]
use shared::error::SharedContractError;
use shared::validation::{require_non_empty_vec, require_string_length};
use shared::{extend_persistent_ttl, TTL_THRESHOLD, TTL_TARGET};
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, log, panic_with_error, symbol_short,
    Address, Bytes, BytesN, Env, String, Symbol, Vec,
};

pub use shared::error::SharedContractError as SharedError;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ContractError {
    AssetNotFound = 1,
    /// Same owner attempted to register an asset with identical metadata.
    DuplicateAsset = 2,
    UnauthorizedAdmin = 3,
    UnauthorizedOwner = 4,
    NotInitialized = 5,
    AdminAlreadyInitialized = 6,
    Paused = 7,
    InvalidAssetType = 8,
    PendingAdminAlreadyExists = 9,
    TypeInUse = 10,
    EmptyMetadata = 11,
    SameOwner = 12,
    TimelockNotExpired = 13,
    ProposalNotFound = 14,
    AssetDecommissioned = 15,
    /// A pending (non-executed) deregister proposal already exists for this asset.
    /// A new proposal cannot overwrite it; wait for the timelock to expire and execute,
    /// or allow the existing proposal to lapse before re-proposing.
    ProposalAlreadyExists = 16,
    /// Asset has already been deprecated and cannot be deprecated again.
    AssetAlreadyDeprecated = 17,
    /// The batch exceeds the maximum allowed size.
    BatchTooLarge = 18,
}

impl From<SharedContractError> for ContractError {
    fn from(e: SharedContractError) -> Self {
        match e {
            SharedContractError::NotInitialized => ContractError::NotInitialized,
            SharedContractError::AlreadyInitialized => ContractError::AdminAlreadyInitialized,
            SharedContractError::UnauthorizedAdmin => ContractError::UnauthorizedAdmin,
            SharedContractError::Paused => ContractError::Paused,
            SharedContractError::TimelockNotExpired => ContractError::TimelockNotExpired,
            SharedContractError::ProposalNotFound => ContractError::ProposalNotFound,
            SharedContractError::PendingAdminAlreadyExists => ContractError::PendingAdminAlreadyExists,
        }
    }
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Asset {
    pub asset_id: u64,
    pub asset_type: Symbol,
    pub metadata: String,
    /// Unique physical serial number of the asset (e.g. manufacturer plate number).
    /// Used as the primary deduplication key so the same machine cannot be registered
    /// twice even if its metadata description differs.
    pub serial_number: String,
    pub owner: Address,
    pub registered_at: u64,
    pub metadata_updated_at: u64,
    /// Incremented on every successful call to `update_asset_metadata`.
    /// Starts at 0 when the asset is first registered.
    pub metadata_version: u32,
}

/// A single entry in the metadata change history for an asset.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetadataHistoryEntry {
    pub version: u32,
    pub old_hash: BytesN<32>,
    pub new_hash: BytesN<32>,
    pub updated_at: u64,
    /// Soft lifecycle status set by the owner. Defaults to `Active` on registration.
    pub deprecation_status: DeprecationStatus,
    /// Whether this asset is currently locked as collateral under a lien.
    /// While `true`, ownership transfers are blocked.
    pub is_locked: bool,
    /// The lending contract address that placed the lien, if any.
    pub lender: Option<Address>,
    /// The loan ID associated with the lien, used to verify the correct loan
    /// releases the lock on repayment.
    pub loan_id: Option<u64>,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DeprecationStatus {
    Active = 0,
    Deprecated = 1,
    Decommissioned = 2,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetInput {
    pub asset_type: Symbol,
    pub metadata: String,
    pub serial_number: String,
}

/// Paginated result for `get_assets_by_type_paginated`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetTypePage {
    /// Asset IDs for the requested page.
    pub assets: Vec<u64>,
    /// Total number of assets of this type across all pages.
    pub total: u32,
}

/// Paginated result for `get_assets_by_owner_paginated`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnerPage {
    /// Asset IDs for the requested page.
    pub assets: Vec<u64>,
    /// Total number of assets owned by this address across all pages.
    pub total: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelockProposal {
    pub proposed_at: u64,
    pub executed: bool,
}

/// A pending multi-signature ownership transfer awaiting acceptance by `new_owner`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingTransfer {
    pub new_owner: Address,
    pub initiated_at: u64,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AssetStatus {
    Active = 0,
    Decommissioned = 1,
    UnderMaintenance = 2,
}

/// Storage key enum for indexed lookups.
#[contracttype]
pub enum DataKey {
    /// Maps a keyword category (arbitrary bytes) to the list of asset IDs tagged with it.
    AssetsByCategory(Bytes),
    /// Maps an owner address to the list of asset IDs they own.
    AssetsByOwner(Address),
}

/// Filter criteria for [`AssetRegistry::search_assets`].
///
/// All fields are optional; omitting a field means "no constraint on that dimension".
#[contracttype]
#[derive(Clone, Debug)]
pub struct SearchFilter {
    /// Return only assets whose `asset_type` matches this value exactly.
    pub asset_type: Option<Symbol>,
    /// Return only assets whose `metadata` field contains this substring (case-sensitive).
    pub manufacturer: Option<String>,
    /// Return only assets registered at least this many months ago (1 month ≈ 30 days).
    pub min_age_months: Option<u32>,
    /// Return only assets registered at most this many months ago (1 month ≈ 30 days).
    pub max_age_months: Option<u32>,
    /// How to sort the results.  Defaults to no particular order when `None`.
    pub sort: Option<SortOrder>,
    /// Required when `sort` is [`SortOrder::ByCollateralScore`].
    pub lifecycle_contract: Option<Address>,
}

/// Sorting options for [`AssetRegistry::search_assets`].
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SortOrder {
    /// Sort by on-chain collateral score (descending, highest first).
    /// Requires `SearchFilter::lifecycle_contract` to be set.
    ByCollateralScore = 0,
    /// Sort by most-recent metadata update timestamp (descending, newest first).
    ByMaintenanceDate = 1,
}

/// Result page returned by [`AssetRegistry::search_assets`].
#[contracttype]
#[derive(Clone, Debug)]
pub struct SearchPage {
    /// Matched assets (up to 100).
    pub assets: Vec<Asset>,
    /// Total number of assets that matched the filter (before the 100-result cap).
    pub total: u32,
}

const ASSET_COUNT: Symbol = symbol_short!("A_COUNT");
const PAUSED_KEY: Symbol = symbol_short!("PAUSED");
const TIMELOCK_DELAY_SECS: u64 = 48 * 60 * 60;
/// Default window for a proposed new owner to accept an ownership transfer.
const TRANSFER_TIMEOUT_SECS: u64 = 7 * 24 * 60 * 60;

const ADMIN_KEY: Symbol = symbol_short!("ADMIN");
const ASSET_TYPE_PREFIX: Symbol = symbol_short!("AST_TYPE");
const PENDING_ADMIN_KEY: Symbol = symbol_short!("PADMIN");
const DECOMM_PREFIX: Symbol = symbol_short!("DECOMM");
const LIFECYCLE_KEY: Symbol = symbol_short!("LIFECYCLE");

/// Storage key for the authorized lending contract address.
/// Only the contract stored under this key may call `lock_asset_as_collateral`
/// and `unlock_asset_from_collateral`.
const LENDING_CONTRACT_KEY: Symbol = symbol_short!("LEND_CTR");

/// Maximum number of assets that may be registered in a single batch call.
const MAX_BATCH_SIZE: u32 = 50;

pub const DEREG_TOPIC: Symbol = symbol_short!("DEREG");
pub const ADD_TYPE_TOPIC: Symbol = symbol_short!("ADD_TYPE");
pub const RM_TYPE_TOPIC: Symbol = symbol_short!("RM_TYPE");

fn asset_key(id: u64) -> (Symbol, u64) {
    (symbol_short!("ASSET"), id)
}

fn metadata_history_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("META_HIS"), asset_id)
}

fn timelock_key(op: Symbol, asset_id: u64) -> (Symbol, Symbol, u64) {
    (symbol_short!("TL_PROP"), op, asset_id)
}

fn require_timelock_ready(env: &Env, op: Symbol, asset_id: u64) {
    let key = timelock_key(op, asset_id);
    let mut proposal: TimelockProposal = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::ProposalNotFound));
    if proposal.executed {
        panic_with_error!(env, ContractError::ProposalNotFound);
    }
    // #795: Compare using ledger timestamp (Unix seconds), NOT ledger sequence number.
    // TIMELOCK_DELAY_SECS is expressed in seconds; env.ledger().timestamp() returns
    // Unix epoch seconds — they are directly comparable.  env.ledger().sequence()
    // returns the ledger number (currently ~30M on mainnet) and must NOT be used here:
    // the comparison would be either instant (delay << sequence) or centuries long.
    if env
        .ledger()
        .timestamp()
        .saturating_sub(proposal.proposed_at)
        < TIMELOCK_DELAY_SECS
    {
        panic_with_error!(env, ContractError::TimelockNotExpired);
    }
    proposal.executed = true;
    env.storage().persistent().set(&key, &proposal);
    extend_persistent_ttl(&env, &key);
}

/// Global timelock key for admin-level operations (e.g., upgrade).
fn global_timelock_key(op: Symbol) -> (Symbol, Symbol) {
    (symbol_short!("TL_GLOB"), op)
}

fn require_global_timelock_ready(env: &Env, op: Symbol) {
    let key = global_timelock_key(op);
    let mut proposal: TimelockProposal = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::ProposalNotFound));
    if proposal.executed {
        panic_with_error!(env, ContractError::ProposalNotFound);
    }
    // #795: Compare using ledger timestamp (Unix seconds), NOT ledger sequence number.
    // TIMELOCK_DELAY_SECS is expressed in seconds; env.ledger().timestamp() returns
    // Unix epoch seconds — they are directly comparable.  env.ledger().sequence()
    // returns the ledger number and must NOT be used here.
    if env
        .ledger()
        .timestamp()
        .saturating_sub(proposal.proposed_at)
        < TIMELOCK_DELAY_SECS
    {
        panic_with_error!(env, ContractError::TimelockNotExpired);
    }
    proposal.executed = true;
    env.storage().persistent().set(&key, &proposal);
    extend_persistent_ttl(&env, &key);
}

/// Decommissioned flag key: asset_id → bool.
fn decommissioned_key(asset_id: u64) -> (Symbol, u64) {
    (DECOMM_PREFIX, asset_id)
}

/// Deduplication key: (owner, asset_type, sha256(metadata)) → existing asset_id.
/// asset_type is included so same owner+metadata with different type is not erroneously deduplicated.
fn dedup_key(
    owner: &Address,
    asset_type: &Symbol,
    hash: &BytesN<32>,
) -> (Symbol, Address, Symbol, BytesN<32>) {
    (
        symbol_short!("DEDUP"),
        owner.clone(),
        asset_type.clone(),
        hash.clone(),
    )
}

/// Serial-number dedup key: sha256(serial_number) → existing asset_id.
/// Prevents the same physical machine from being registered twice regardless of metadata.
fn serial_dedup_key(hash: &BytesN<32>) -> (Symbol, BytesN<32>) {
    (symbol_short!("SN_DEDUP"), hash.clone())
}

/// Owner index key: owner → Vec<u64> of asset IDs.
fn owner_index_key(owner: &Address) -> DataKey {
    DataKey::AssetsByOwner(owner.clone())
}

/// Asset type allowlist key: asset_type → bool.
fn asset_type_key(asset_type: &Symbol) -> (Symbol, Symbol) {
    (ASSET_TYPE_PREFIX, asset_type.clone())
}

/// Asset type count key: asset_type → u64 (number of registered assets of this type).
fn type_count_key(asset_type: &Symbol) -> (Symbol, Symbol) {
    (symbol_short!("AST_CNT"), asset_type.clone())
}

fn type_count_inc(env: &Env, asset_type: &Symbol) {
    let key = type_count_key(asset_type);
    let count: u64 = env.storage().persistent().get(&key).unwrap_or(0);
    env.storage().persistent().set(&key, &(count + 1));
    extend_persistent_ttl(&env, &key);
}

fn type_count_dec(env: &Env, asset_type: &Symbol) {
    let key = type_count_key(asset_type);
    let count: u64 = env.storage().persistent().get(&key).unwrap_or(0);
    if count > 0 {
        env.storage().persistent().set(&key, &(count - 1));
        extend_persistent_ttl(&env, &key);
    }
}

/// Type-to-assets index key: asset_type → Vec<u64> of asset IDs.
fn type_assets_key(asset_type: &Symbol) -> (Symbol, Symbol) {
    (symbol_short!("TYP_IDX"), asset_type.clone())
}

fn type_assets_add(env: &Env, asset_type: &Symbol, asset_id: u64) {
    let key = type_assets_key(asset_type);
    let mut ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    ids.push_back(asset_id);
    env.storage().persistent().set(&key, &ids);
    extend_persistent_ttl(&env, &key);
}

fn type_assets_remove(env: &Env, asset_type: &Symbol, asset_id: u64) {
    let key = type_assets_key(asset_type);
    let ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    let mut updated: Vec<u64> = Vec::new(env);
    for id in ids.iter() {
        if id != asset_id {
            updated.push_back(id);
        }
    }
    env.storage().persistent().set(&key, &updated);
    extend_persistent_ttl(&env, &key);
}

/// Append an asset ID to the owner's index.
fn owner_index_add(env: &Env, owner: &Address, asset_id: u64) {
    let key = owner_index_key(owner);
    let mut ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    ids.push_back(asset_id);
    env.storage().persistent().set(&key, &ids);
    extend_persistent_ttl(&env, &key);
}

/// Remove an asset ID from the owner's index.
fn owner_index_remove(env: &Env, owner: &Address, asset_id: u64) {
    let key = owner_index_key(owner);
    if !env.storage().persistent().has(&key) {
        log!(
            env,
            "owner index missing during remove",
            owner.clone(),
            asset_id
        );
        env.events()
            .publish((symbol_short!("IDX_MISS"), owner.clone()), asset_id);
        return;
    }
    let ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    let mut updated: Vec<u64> = Vec::new(env);
    for id in ids.iter() {
        if id != asset_id {
            updated.push_back(id);
        }
    }
    if updated.is_empty() {
        env.storage().persistent().remove(&key);
    } else {
        env.storage().persistent().set(&key, &updated);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
    }
}

/// Category index key: category bytes → Vec<u64> of asset IDs.
fn category_assets_key(category: &Bytes) -> DataKey {
    DataKey::AssetsByCategory(category.clone())
}

/// Reverse index key: asset_id → Vec<Bytes> of categories the asset belongs to.
fn asset_categories_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("AST_CATS"), asset_id)
}

fn category_assets_add(env: &Env, category: &Bytes, asset_id: u64) {
    let key = category_assets_key(category);
    let mut ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    ids.push_back(asset_id);
    env.storage().persistent().set(&key, &ids);
    extend_persistent_ttl(&env, &key);
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
}

fn category_assets_remove(env: &Env, category: &Bytes, asset_id: u64) {
    let key = category_assets_key(category);
    let ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    let mut updated: Vec<u64> = Vec::new(env);
    for id in ids.iter() {
        if id != asset_id {
            updated.push_back(id);
        }
    }
    if updated.is_empty() {
        env.storage().persistent().remove(&key);
    } else {
        env.storage().persistent().set(&key, &updated);
        extend_persistent_ttl(&env, &key);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
    }
}

fn asset_categories_add(env: &Env, asset_id: u64, category: &Bytes) {
    let key = asset_categories_key(asset_id);
    let mut cats: Vec<Bytes> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    for existing in cats.iter() {
        if existing == *category {
            return;
        }
    }
    cats.push_back(category.clone());
    env.storage().persistent().set(&key, &cats);
    extend_persistent_ttl(&env, &key);
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
}

fn asset_categories_remove_all(env: &Env, asset_id: u64) {
    let key = asset_categories_key(asset_id);
    let cats: Vec<Bytes> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    for cat in cats.iter() {
        category_assets_remove(env, &cat, asset_id);
    }
    env.storage().persistent().remove(&key);
}

fn is_paused(env: &Env) -> bool {
    env.storage().persistent().get(&PAUSED_KEY).unwrap_or(false)
}

fn ensure_not_paused(env: &Env) {
    if is_paused(env) {
        panic_with_error!(env, ContractError::Paused);
    }
}

/// Validate that every character in a Symbol is alphanumeric or underscore
/// (`[A-Za-z0-9_]`). Panics with [`ContractError::InvalidAssetType`] otherwise.
///
/// Soroban Symbol XDR layout: 4-byte type tag + 4-byte big-endian length + raw ASCII chars.
/// We skip the 8-byte header and inspect the remaining bytes directly.
fn validate_asset_type_symbol(env: &Env, asset_type: &Symbol) {
    let xdr_bytes = asset_type.clone().to_xdr(env);
    // XDR header is 8 bytes (4-byte discriminant + 4-byte length).
    let header_len: u32 = 8;
    let total = xdr_bytes.len();
    if total <= header_len {
        // Empty symbol — treat as invalid.
        panic_with_error!(env, ContractError::InvalidAssetType);
    }
    for i in header_len..total {
        let b = xdr_bytes.get(i).unwrap_or(0);
        let valid = (b >= b'A' && b <= b'Z')
            || (b >= b'a' && b <= b'z')
            || (b >= b'0' && b <= b'9')
            || b == b'_';
        if !valid {
            panic_with_error!(env, ContractError::InvalidAssetType);
        }
    }
}

#[contract]
pub struct AssetRegistry;

#[contractimpl]
impl AssetRegistry {
    /// Propose a timelocked deregistration for an asset.
    /// This is the first step in removing an asset from the registry.
    ///
    /// Timelock semantics: after proposing, the caller must wait
    /// `TIMELOCK_DELAY_SECS` (48 hours) before calling
    /// [`execute_deregister_asset`]. A proposal cannot be re-proposed while
    /// a pending (non-executed) proposal already exists for the same asset —
    /// doing so would reset the clock and allow indefinite delay.
    ///
    /// # Arguments
    /// * `caller` - The address initiating the proposal (owner or admin)
    /// * `asset_id` - The unique identifier of the asset to deregister
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    /// - [`ContractError::UnauthorizedOwner`] if the caller is not the asset owner or admin
    /// - [`ContractError::ProposalAlreadyExists`] if a pending proposal already exists
    pub fn propose_deregister_asset(env: Env, caller: Address, asset_id: u64) {
        ensure_not_paused(&env);
        let asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));
        let admin = Self::get_admin(env.clone());
        if caller == admin {
            admin.require_auth();
        } else if caller == asset.owner {
            asset.owner.require_auth();
        } else {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }
        let key = timelock_key(DEREG_TOPIC, asset_id);
        // Block re-proposal if a pending proposal already exists to prevent
        // the owner from resetting the timelock clock indefinitely.
        if let Some(existing) = env.storage().persistent().get::<_, TimelockProposal>(&key) {
            if !existing.executed {
                panic_with_error!(&env, ContractError::ProposalAlreadyExists);
            }
        }
        env.storage().persistent().set(
            &key,
            &TimelockProposal {
                proposed_at: env.ledger().timestamp(),
                executed: false,
            },
        );
        extend_persistent_ttl(&env, &key);
    }

    /// Execute a previously proposed asset deregistration after the timelock expires.
    ///
    /// # Arguments
    /// * `caller` - The address completing the deregistration
    /// * `asset_id` - The unique identifier of the asset to deregister
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    /// - [`ContractError::UnauthorizedOwner`] if the caller is not the asset owner or admin
    /// - [`ContractError::TimelockNotReady`] if the proposal timelock has not yet matured
    pub fn execute_deregister_asset(env: Env, caller: Address, asset_id: u64) {
        require_timelock_ready(&env, DEREG_TOPIC, asset_id);
        Self::deregister_asset(env, caller, asset_id);
    }

    /// Register a new asset with the given type, metadata, and owner.
    ///
    /// # Arguments
    /// * `asset_type` - A Symbol representing the type of asset (e.g., "GENSET", "TURBINE")
    /// * `metadata` - String containing asset metadata and specifications
    /// * `owner` - Address of the asset owner
    ///
    /// # Returns
    /// The unique asset ID assigned to the registered asset
    ///
    /// # Panics
    /// - [`ContractError::DuplicateAsset`] if the same owner tries to register identical metadata
    /// - [`ContractError::InvalidAssetType`] if the asset type is not in the allowlist
    pub fn register_asset(
        env: Env,
        asset_type: Symbol,
        metadata: String,
        serial_number: String,
        owner: Address,
    ) -> u64 {
        ensure_not_paused(&env);
        owner.require_auth();

        require_string_length(&metadata, "metadata", 256);
        require_string_length(&serial_number, "serial_number", 64);

        // Validate asset_type contains only alphanumeric + underscore characters.
        validate_asset_type_symbol(&env, &asset_type);

        // Validate asset type against allowlist
        if !Self::is_valid_asset_type(env.clone(), asset_type.clone()) {
            panic_with_error!(&env, ContractError::InvalidAssetType);
        }

        // Deduplication by serial number: same physical machine cannot be registered twice.
        let sn_bytes = serial_number.clone().to_xdr(&env);
        let sn_hash: BytesN<32> = env.crypto().sha256(&sn_bytes).into();
        let sdk = serial_dedup_key(&sn_hash);
        if env.storage().persistent().has(&sdk) {
            panic_with_error!(&env, ContractError::DuplicateAsset);
        }

        // Secondary dedup: same owner + same metadata hash.
        let meta_bytes = metadata.clone().to_xdr(&env);
        let meta_hash: BytesN<32> = env.crypto().sha256(&meta_bytes).into();
        let dk = dedup_key(&owner, &asset_type, &meta_hash);
        if env.storage().persistent().has(&dk) {
            panic_with_error!(&env, ContractError::DuplicateAsset);
        }

        let id: u64 = env.storage().persistent().get(&ASSET_COUNT).unwrap_or(0) + 1;
        let asset = Asset {
            asset_id: id,
            asset_type: asset_type.clone(),
            metadata,
            serial_number,
            owner: owner.clone(),
            registered_at: env.ledger().timestamp(),
            metadata_updated_at: env.ledger().timestamp(),
            metadata_version: 0,
            deprecation_status: DeprecationStatus::Active,
            is_locked: false,
            lender: None,
            loan_id: None,
        };
        env.storage().persistent().set(&asset_key(id), &asset);
        extend_persistent_ttl(&env, &asset_key(id));
        env.storage().persistent().set(&ASSET_COUNT, &id);
        extend_persistent_ttl(&env, &ASSET_COUNT);
        env.storage().persistent().set(&dk, &id);
        extend_persistent_ttl(&env, &dk);
        env.storage().persistent().set(&sdk, &id);
        extend_persistent_ttl(&env, &sdk);
        env.storage()
            .persistent()
            .extend_ttl(&ASSET_COUNT, TTL_THRESHOLD, TTL_TARGET);
        env.storage().persistent().set(&dk, &id);
        env.storage()
            .persistent()
            .extend_ttl(&dk, TTL_THRESHOLD, TTL_TARGET);
        env.storage().persistent().set(&sdk, &id);
        env.storage()
            .persistent()
            .extend_ttl(&sdk, TTL_THRESHOLD, TTL_TARGET);

        // Update owner index
        owner_index_add(&env, &owner, id);

        // Increment type count
        type_count_inc(&env, &asset_type);

        // Update type-to-assets index
        type_assets_add(&env, &asset_type, id);

        // Emit asset registration event
        env.events().publish(
            (symbol_short!("reg_asset"),),
            (id, owner.clone(), env.ledger().timestamp()),
        );

        id
    }

    /// Register multiple assets in a single transaction.
    ///
    /// # Arguments
    /// * `owner` - Address of the asset owner
    /// * `assets` - Vec of AssetInput structs
    ///
    /// # Returns
    /// Vec of assigned asset IDs
    pub fn batch_register_assets(env: Env, owner: Address, assets: Vec<AssetInput>) -> Vec<u64> {
        ensure_not_paused(&env);
        owner.require_auth();
        require_non_empty_vec(&assets, "assets");

        if assets.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, ContractError::BatchTooLarge);
        }

        let mut ids: Vec<u64> = Vec::new(&env);
        // Track (asset_type, meta_hash) pairs to detect in-batch duplicates
        let mut batch_type_meta: Vec<(Symbol, BytesN<32>)> = Vec::new(&env);
        let mut batch_sn_hashes: Vec<BytesN<32>> = Vec::new(&env);

        let mut next_id: u64 = env.storage().persistent().get(&ASSET_COUNT).unwrap_or(0);

        for asset_in in assets.iter() {
            require_string_length(&asset_in.metadata, "metadata", 256);
            require_string_length(&asset_in.serial_number, "serial_number", 64);
            if !Self::is_valid_asset_type(env.clone(), asset_in.asset_type.clone()) {
                panic_with_error!(&env, ContractError::InvalidAssetType);
            }

            // Serial-number dedup (global)
            let sn_bytes = asset_in.serial_number.clone().to_xdr(&env);
            let sn_hash: BytesN<32> = env.crypto().sha256(&sn_bytes).into();
            if env.storage().persistent().has(&serial_dedup_key(&sn_hash)) {
                panic_with_error!(&env, ContractError::DuplicateAsset);
            }
            for seen in batch_sn_hashes.iter() {
                if seen == sn_hash {
                    panic_with_error!(&env, ContractError::DuplicateAsset);
                }
            }
            batch_sn_hashes.push_back(sn_hash.clone());

            let meta_bytes = asset_in.metadata.clone().to_xdr(&env);
            let meta_hash: BytesN<32> = env.crypto().sha256(&meta_bytes).into();

            if env
                .storage()
                .persistent()
                .has(&dedup_key(&owner, &asset_in.asset_type, &meta_hash))
            {
                panic_with_error!(&env, ContractError::DuplicateAsset);
            }

            for (seen_type, seen_hash) in batch_type_meta.iter() {
                if seen_type == asset_in.asset_type && seen_hash == meta_hash {
                    panic_with_error!(&env, ContractError::DuplicateAsset);
                }
            }
            batch_type_meta.push_back((asset_in.asset_type.clone(), meta_hash.clone()));

            next_id += 1;
            let id = next_id;
            let asset = Asset {
                asset_id: id,
                asset_type: asset_in.asset_type.clone(),
                metadata: asset_in.metadata.clone(),
                serial_number: asset_in.serial_number.clone(),
                owner: owner.clone(),
                registered_at: env.ledger().timestamp(),
                metadata_updated_at: env.ledger().timestamp(),
                metadata_version: 0,
                deprecation_status: DeprecationStatus::Active,
                is_locked: false,
                lender: None,
                loan_id: None,
            };

            env.storage().persistent().set(&asset_key(id), &asset);
            extend_persistent_ttl(&env, &asset_key(id));
            env.storage()
                .persistent()
                .set(&dedup_key(&owner, &asset_in.asset_type, &meta_hash), &id);
            extend_persistent_ttl(&env, &dedup_key(&owner, &asset_in.asset_type, &meta_hash));
            env.storage().persistent().set(&serial_dedup_key(&sn_hash), &id);
            extend_persistent_ttl(&env, &serial_dedup_key(&sn_hash));
            env.storage().persistent().extend_ttl(
                &dedup_key(&owner, &asset_in.asset_type, &meta_hash),
                TTL_THRESHOLD,
                TTL_TARGET,
            );
            env.storage()
                .persistent()
                .set(&serial_dedup_key(&sn_hash), &id);
            env.storage().persistent().extend_ttl(
                &serial_dedup_key(&sn_hash),
                TTL_THRESHOLD,
                TTL_TARGET,
            );

            owner_index_add(&env, &owner, id);

            // Increment type count
            type_count_inc(&env, &asset_in.asset_type);

            // Update type-to-assets index
            type_assets_add(&env, &asset_in.asset_type, id);

            env.events().publish(
                (symbol_short!("REG_AST"), id),
                (
                    asset_in.asset_type.clone(),
                    owner.clone(),
                    env.ledger().timestamp(),
                ),
            );

            ids.push_back(id);
        }

        if next_id > env.storage().persistent().get(&ASSET_COUNT).unwrap_or(0) {
            env.storage().persistent().set(&ASSET_COUNT, &next_id);
            extend_persistent_ttl(&env, &ASSET_COUNT);
        }

        // Ensure owner index TTL is extended after all batch writes
        if !ids.is_empty() {
            extend_persistent_ttl(&env, &owner_index_key(&owner));
        }

        // Emit batch registration event
        if !ids.is_empty() {
            env.events().publish(
                (symbol_short!("BATCH_REG"), owner.clone()),
                (ids.clone(), env.ledger().timestamp()),
            );
        }

        ids
    }

    /// Retrieve an asset by its unique ID.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset to retrieve
    ///
    /// # Returns
    /// The complete Asset struct containing all asset information
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if no asset exists with the given ID
    pub fn get_asset(env: Env, asset_id: u64) -> Asset {
        let key = asset_key(asset_id);
        let asset: Asset = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));
        // Extend TTL on read to prevent stale data after TTL expiry
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
        asset
    }

    /// Check whether an asset with the given ID is present in the registry.
    ///
    /// This is a lightweight existence check that reads a single persistent storage
    /// entry and does **not** verify the asset's deprecation or decommission status.
    /// Use [`get_asset`] if you need the full asset record, or [`asset_status`] if
    /// you need operational state.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset to check
    ///
    /// # Returns
    /// `true` if a record for `asset_id` exists in persistent storage; `false` otherwise
    pub fn asset_exists(env: Env, asset_id: u64) -> bool {
        env.storage().persistent().has(&asset_key(asset_id))
    }

    /// Returns the status of an asset (Active, Decommissioned, or UnderMaintenance).
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    ///
    /// # Returns
    /// AssetStatus enum: Active if normal, Decommissioned if marked as such,
    /// UnderMaintenance if the asset is marked as under maintenance
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if no asset exists with the given ID
    pub fn asset_status(env: Env, asset_id: u64) -> AssetStatus {
        // Verify asset exists
        if !Self::asset_exists(env.clone(), asset_id) {
            panic_with_error!(&env, ContractError::AssetNotFound);
        }

        // Check if asset is decommissioned
        let decomm_key = decommissioned_key(asset_id);
        let is_decommissioned: bool = env.storage().persistent().get(&decomm_key).unwrap_or(false);

        if is_decommissioned {
            // Extend TTL on read
            env.storage()
                .persistent()
                .extend_ttl(&decomm_key, TTL_THRESHOLD, TTL_TARGET);
            return AssetStatus::Decommissioned;
        }

        // Check if asset is under maintenance
        let maint_key = (symbol_short!("U_MAINT"), asset_id);
        let is_under_maintenance: bool =
            env.storage().persistent().get(&maint_key).unwrap_or(false);

        if is_under_maintenance {
            // Extend TTL on read
            env.storage()
                .persistent()
                .extend_ttl(&maint_key, TTL_THRESHOLD, TTL_TARGET);
            return AssetStatus::UnderMaintenance;
        }

        // For Active status, extend TTL on the asset itself
        env.storage()
            .persistent()
            .extend_ttl(&asset_key(asset_id), TTL_THRESHOLD, TTL_TARGET);

        AssetStatus::Active
    }

    /// Return all asset IDs currently owned by the given address.
    ///
    /// Uses the owner-to-assets index maintained by [`register_asset`] and
    /// [`transfer_asset`]. The list is updated on every registration and transfer
    /// so it reflects the owner's current portfolio.
    ///
    /// For owners with large portfolios that may exceed return-data limits, prefer
    /// the paginated variant [`get_assets_by_owner_paginated`].
    ///
    /// # Arguments
    /// * `owner` - The address of the asset owner to query
    ///
    /// # Returns
    /// A `Vec<u64>` of asset IDs owned by `owner` (empty vec if none)
    pub fn get_assets_by_owner(env: Env, owner: Address) -> Vec<u64> {
        let key = owner_index_key(&owner);
        let ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(&env));
        if env.storage().persistent().has(&key) {
            extend_persistent_ttl(&env, &key);
        }
        ids
    }

    /// Returns a paginated list of asset IDs owned by the given address.
    ///
    /// # Arguments
    /// * `owner` - The address of the asset owner
    /// * `page` - Zero-based page index
    /// * `page_size` - Number of asset IDs to return per page
    ///
    /// # Returns
    /// Vec containing the requested page of asset IDs
    pub fn get_assets_by_owner_page(
        env: Env,
        owner: Address,
        page: u32,
        page_size: u32,
    ) -> Vec<u64> {
        let key = owner_index_key(&owner);
        let all_assets: Vec<u64> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(&env));

        if env.storage().persistent().has(&key) {
            extend_persistent_ttl(&env, &key);
        }

        if page_size == 0 {
            return Vec::new(&env);
        }

        let len = all_assets.len();
        let offset = match page.checked_mul(page_size) {
            Some(offset) => offset,
            None => return Vec::new(&env),
        };
        if offset >= len {
            return Vec::new(&env);
        }

        let end = offset.checked_add(page_size).unwrap_or(len).min(len);
        let mut page_assets = Vec::new(&env);
        for i in offset..end {
            page_assets.push_back(all_assets.get(i).unwrap());
        }
        page_assets
    }

    /// Returns a page of asset IDs for the given owner together with the total count.
    ///
    /// # Arguments
    /// * `owner` - The address of the asset owner
    /// * `page` - Zero-based page index
    /// * `page_size` - Maximum number of asset IDs per page (capped at 100)
    ///
    /// # Returns
    /// `OwnerPage` containing the requested slice and the total asset count for this owner
    pub fn get_assets_by_owner_paginated(
        env: Env,
        owner: Address,
        page: u32,
        page_size: u32,
    ) -> OwnerPage {
        const MAX_PAGE_SIZE: u32 = 100;
        let page_size = page_size.min(MAX_PAGE_SIZE);

        let key = owner_index_key(&owner);
        let all: Vec<u64> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(&env));
        if env.storage().persistent().has(&key) {
            extend_persistent_ttl(&env, &key);
            env.storage()
                .persistent()
                .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
        }

        let total = all.len();

        if page_size == 0 {
            return OwnerPage {
                assets: Vec::new(&env),
                total,
            };
        }

        let offset = match page.checked_mul(page_size) {
            Some(o) => o,
            None => {
                return OwnerPage {
                    assets: Vec::new(&env),
                    total,
                }
            }
        };

        if offset >= total {
            return OwnerPage {
                assets: Vec::new(&env),
                total,
            };
        }

        let end = (offset + page_size).min(total);
        let mut assets = Vec::new(&env);
        for i in offset..end {
            assets.push_back(all.get(i).unwrap());
        }

        OwnerPage { assets, total }
    }

    /// Get the total count of registered assets in the system.
    ///
    /// # Returns
    /// The total number of assets that have been registered
    pub fn asset_count(env: Env) -> u64 {
        env.storage().persistent().get(&ASSET_COUNT).unwrap_or(0)
    }

    /// Get the total count of registered assets.
    ///
    /// # Returns
    /// The total number of assets that have been registered
    pub fn get_asset_count(env: Env) -> u64 {
        env.storage().persistent().get(&ASSET_COUNT).unwrap_or(0)
    }

    /// Return all asset IDs that have been registered with the given type symbol.
    ///
    /// Uses the type-to-assets index maintained by [`register_asset`] and updated
    /// on registration and deregistration. The returned list may include deprecated or
    /// decommissioned assets; callers that need only active assets should filter by
    /// [`asset_status`] after retrieval.
    ///
    /// For large fleets, prefer the paginated variant [`get_assets_by_type_paginated`]
    /// to avoid exceeding Soroban's return-data limits.
    ///
    /// # Arguments
    /// * `asset_type` - The symbol representing the asset type (e.g., `symbol_short!("GENSET")`)
    ///
    /// # Returns
    /// A `Vec<u64>` of asset IDs of the requested type (empty vec if none)
    /// Get the total number of registered assets.
    /// Useful for analytics dashboards and DeFi protocol integrations.
    ///
    /// # Returns
    /// The total number of assets that have ever been registered
    pub fn get_total_asset_count(env: Env) -> u64 {
        env.storage().persistent().get(&ASSET_COUNT).unwrap_or(0)
    }

    /// Returns all asset IDs of the given type.
    pub fn get_assets_by_type(env: Env, asset_type: Symbol) -> Vec<u64> {
        let key = type_assets_key(&asset_type);
        let ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(&env));
        if env.storage().persistent().has(&key) {
            extend_persistent_ttl(&env, &key);
        }
        ids
    }

    /// Returns a paginated list of asset IDs of the given type.
    ///
    /// # Arguments
    /// * `asset_type` - The asset type symbol to query
    /// * `offset` - Starting index for pagination
    /// * `limit` - Maximum number of asset IDs to return
    pub fn get_assets_by_type_page(
        env: Env,
        asset_type: Symbol,
        offset: u32,
        limit: u32,
    ) -> Vec<u64> {
        let all: Vec<u64> = env
            .storage()
            .persistent()
            .get(&type_assets_key(&asset_type))
            .unwrap_or_else(|| Vec::new(&env));
        let len = all.len();
        if offset >= len || limit == 0 {
            return Vec::new(&env);
        }
        let end = (offset + limit).min(len);
        let mut page = Vec::new(&env);
        for i in offset..end {
            page.push_back(all.get(i).unwrap());
        }
        page
    }

    /// Returns a page of asset IDs for the given type together with the total count.
    /// Designed for large fleets where returning the full list would exceed Soroban's
    /// return data limits.
    ///
    /// # Arguments
    /// * `asset_type` - The asset type symbol to query
    /// * `page` - Zero-based page index
    /// * `page_size` - Maximum number of asset IDs per page (capped at 100)
    ///
    /// # Returns
    /// `AssetTypePage` containing the requested slice and the total asset count
    pub fn get_assets_by_type_paginated(
        env: Env,
        asset_type: Symbol,
        page: u32,
        page_size: u32,
    ) -> AssetTypePage {
        const MAX_PAGE_SIZE: u32 = 100;
        let page_size = page_size.min(MAX_PAGE_SIZE);

        let key = type_assets_key(&asset_type);
        let all: Vec<u64> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(&env));
        if env.storage().persistent().has(&key) {
            extend_persistent_ttl(&env, &key);
        }

        let total = all.len();

        if page_size == 0 {
            return AssetTypePage {
                assets: Vec::new(&env),
                total,
            };
        }

        let offset = match page.checked_mul(page_size) {
            Some(o) => o,
            None => {
                return AssetTypePage {
                    assets: Vec::new(&env),
                    total,
                }
            }
        };

        if offset >= total {
            return AssetTypePage {
                assets: Vec::new(&env),
                total,
            };
        }

        let end = (offset + page_size).min(total);
        let mut assets = Vec::new(&env);
        for i in offset..end {
            assets.push_back(all.get(i).unwrap());
        }

        AssetTypePage { assets, total }
    }

    /// Returns all asset IDs tagged with the given category keyword.
    ///
    /// Categories are arbitrary byte strings (e.g. manufacturer name, geographic region)
    /// assigned to assets via [`set_asset_category`]. An empty vec is returned when no
    /// assets have been tagged with the given category.
    pub fn get_assets_by_category(env: Env, category: Bytes) -> Vec<u64> {
        let key = category_assets_key(&category);
        let ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(&env));
        if env.storage().persistent().has(&key) {
            extend_persistent_ttl(&env, &key);
        }
        ids
    }

    /// Tag an asset with a keyword category for later retrieval via [`get_assets_by_category`].
    ///
    /// Only the asset owner or the contract admin may tag an asset. Tagging an asset with
    /// a category it already has is a no-op. A single asset may carry multiple categories.
    ///
    /// # Arguments
    /// * `caller` - The address initiating the tag (owner or admin)
    /// * `asset_id` - The unique identifier of the asset to tag
    /// * `category` - Arbitrary byte keyword (e.g. `b"Caterpillar"`, `b"NorthAmerica"`)
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    /// - [`ContractError::UnauthorizedOwner`] if caller is neither owner nor admin
    pub fn set_asset_category(env: Env, caller: Address, asset_id: u64, category: Bytes) {
        ensure_not_paused(&env);
        let asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));
        let admin = Self::get_admin(env.clone());
        if caller == admin {
            admin.require_auth();
        } else if caller == asset.owner {
            asset.owner.require_auth();
        } else {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        asset_categories_add(&env, asset_id, &category);
        category_assets_add(&env, &category, asset_id);

        env.events()
            .publish((symbol_short!("TAG_ASSET"), asset_id), (caller, category));
    }

    /// Initialize the admin address for the contract.
    /// This function should be called once immediately after deployment.
    ///
    /// # Arguments
    /// * `deployer` - The address of the contract deployer; must sign this transaction.
    /// * `admin` - The address that will have administrative privileges
    ///
    /// # Panics
    /// - [`ContractError::AdminAlreadyInitialized`] if admin has already been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if deployer is not the transaction invoker
    pub fn initialize_admin(env: Env, deployer: Address, admin: Address) {
        // Soroban SDK removed `env.invoker()`; rely on `require_auth` to enforce
        // the deployer's signature instead, which is the standard pattern.
        deployer.require_auth();
        if env.storage().instance().has(&ADMIN_KEY) {
            panic_with_error!(&env, ContractError::AdminAlreadyInitialized);
        }
        env.storage().instance().set(&ADMIN_KEY, &admin);
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_TARGET);
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("INIT_ADM")),
            (admin, env.ledger().timestamp()),
        );
    }

    /// Get the current admin address of the contract.
    ///
    /// # Returns
    /// The address of the current administrator
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if the admin has not been initialized
    pub fn get_admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&ADMIN_KEY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized))
    }

    /// Set the lifecycle contract address for cross-contract notifications.
    /// Only the admin can set this.
    ///
    /// # Arguments
    /// * `admin` - The administrator making the update
    /// * `lifecycle_addr` - The address of the lifecycle contract
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    pub fn set_lifecycle_contract(env: Env, admin: Address, lifecycle_addr: Address) {
        admin.require_auth();
        let stored_admin: Address = Self::get_admin(env.clone());
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        env.storage().instance().set(&LIFECYCLE_KEY, &lifecycle_addr);
        env.storage().instance().extend_ttl(518400, 518400);
    }

    /// Get the configured lifecycle contract address.
    ///
    /// # Returns
    /// The address of the lifecycle contract, or panics if not set
    pub fn get_lifecycle_contract(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&LIFECYCLE_KEY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized))
    }

    /// Propose a new admin address (step 1 of 2-step transfer).
    /// Only the current admin can propose a new admin.
    ///
    /// # Arguments
    /// * `admin` - The current admin address
    /// * `new_admin` - The address to propose as the new admin
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the current admin
    /// - [`ContractError::PendingAdminAlreadyExists`] if a pending admin already exists
    pub fn propose_admin(env: Env, admin: Address, new_admin: Address) {
        admin.require_auth();
        let stored_admin: Address = Self::get_admin(env.clone());
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        if env.storage().instance().has(&PENDING_ADMIN_KEY) {
            panic_with_error!(&env, ContractError::PendingAdminAlreadyExists);
        }
        env.storage().instance().set(&PENDING_ADMIN_KEY, &new_admin);
        env.storage().instance().extend_ttl(518400, 518400);
        env.events().publish(
            (symbol_short!("PROP_ADM"),),
            (admin.clone(), new_admin.clone()),
        );
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("PROP_ADM")),
            (admin, env.ledger().timestamp(), new_admin),
        );
    }

    /// Accept the admin transfer (step 2 of 2-step transfer).
    /// Only the pending admin can accept and become the new admin.
    ///
    /// # Arguments
    /// * `new_admin` - The pending admin address
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if no pending admin exists
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the pending admin
    pub fn accept_admin(env: Env, new_admin: Address) {
        new_admin.require_auth();
        let pending_admin: Address = env
            .storage()
            .instance()
            .get(&PENDING_ADMIN_KEY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if pending_admin != new_admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        env.storage().instance().set(&ADMIN_KEY, &pending_admin);
        env.storage().instance().remove(&PENDING_ADMIN_KEY);
        env.storage().instance().extend_ttl(518400, 518400);
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("ADMIN_SET")),
            (pending_admin.clone(), env.ledger().timestamp()),
        );
        env.events()
            .publish((symbol_short!("ADMIN_SET"),), (pending_admin,));
    }

    /// Admin-only function to pause the contract.
    ///
    /// # Arguments
    /// * `admin` - The address that must match the stored admin
    pub fn pause(env: Env, admin: Address) {
        admin.require_auth();
        let stored_admin: Address = Self::get_admin(env.clone());
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        env.storage().persistent().set(&PAUSED_KEY, &true);
        extend_persistent_ttl(&env, &PAUSED_KEY);
        env.events()
            .publish((symbol_short!("PAUSED"),), (admin.clone(),));
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("PAUSED")),
            (admin, env.ledger().timestamp()),
        );
    }

    /// Admin-only function to unpause the contract.
    ///
    /// # Arguments
    /// * `admin` - The address that must match the stored admin
    pub fn unpause(env: Env, admin: Address) {
        admin.require_auth();
        let stored_admin: Address = Self::get_admin(env.clone());
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        env.storage().persistent().set(&PAUSED_KEY, &false);
        extend_persistent_ttl(&env, &PAUSED_KEY);
        env.events()
            .publish((symbol_short!("UNPAUSED"),), (admin.clone(),));
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("UNPAUSED")),
            (admin, env.ledger().timestamp()),
        );
    }

    /// Check if the contract is currently paused.
    ///
    /// # Returns
    /// `true` if paused; `false` otherwise
    pub fn is_paused(env: Env) -> bool {
        is_paused(&env)
    }

    /// Admin-only function to deregister (remove) an asset from the registry.
    /// This permanently removes the asset and all associated data.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset to deregister
    ///
    /// # Behavior
    /// If the dedup key has already expired from storage, the remove operation
    /// is a no-op. This allows the same owner to re-register the same metadata
    /// after the dedup key has naturally expired.
    ///
    /// # Lifecycle Data
    /// Maintenance history, collateral score, score history, and last-update timestamp
    /// stored in the lifecycle contract are **not** removed by this call. They remain
    /// readable by anyone who knows the asset ID and continue to consume storage until
    /// they expire or are explicitly removed. After deregistering, call
    /// `lifecycle::purge_asset_data(admin, asset_id)` to reclaim that storage.
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if no asset exists with the given ID
    /// - [`ContractError::UnauthorizedOwner`] if caller is neither the admin nor the asset owner
    pub fn deregister_asset(env: Env, caller: Address, asset_id: u64) {
        ensure_not_paused(&env);

        let asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));

        let admin = Self::get_admin(env.clone());
        if caller == admin {
            admin.require_auth();
        } else if caller == asset.owner {
            asset.owner.require_auth();
        } else {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        // Remove asset storage
        env.storage().persistent().remove(&asset_key(asset_id));

        // Remove deduplication key
        let dk = dedup_key(
            &asset.owner,
            &asset.asset_type,
            &env.crypto().sha256(&asset.metadata.to_xdr(&env)).into(),
        );
        env.storage().persistent().remove(&dk);

        // Remove from owner index
        owner_index_remove(&env, &asset.owner, asset_id);

        // Decrement type count
        type_count_dec(&env, &asset.asset_type);

        // Remove from type-to-assets index
        type_assets_remove(&env, &asset.asset_type, asset_id);

        // Remove from all category indexes
        asset_categories_remove_all(&env, asset_id);

        // Emit deregistration event
        env.events().publish(
            (DEREG_TOPIC, asset_id),
            (asset.asset_type.clone(), asset.owner.clone()),
        );
    }

    /// Owner-only function to update the metadata of an existing asset.
    /// This is typically used after refurbishment or specification changes.
    /// Removes the old deduplication key and registers a new one.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset to update
    /// * `owner` - The current owner of the asset (must match stored owner)
    /// * `new_metadata` - The new metadata string to assign to the asset
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if no asset exists with the given ID
    /// - [`ContractError::UnauthorizedOwner`] if caller is not the asset owner
    /// - [`ContractError::DuplicateAsset`] if new metadata already exists for this owner
    pub fn update_asset_metadata(env: Env, asset_id: u64, owner: Address, new_metadata: String) {
        ensure_not_paused(&env);
        owner.require_auth();
        require_string_length(&new_metadata, "metadata", 256);

        let mut asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));

        if asset.owner != owner {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        if new_metadata == asset.metadata {
            return;
        }

        // Remove old dedup key
        let old_hash: BytesN<32> = env.crypto().sha256(&asset.metadata.to_xdr(&env)).into();
        env.storage()
            .persistent()
            .remove(&dedup_key(&owner, &asset.asset_type, &old_hash));

        // Reject if new metadata is a duplicate for this owner
        let new_hash: BytesN<32> = env
            .crypto()
            .sha256(&new_metadata.clone().to_xdr(&env))
            .into();
        let new_dk = dedup_key(&owner, &asset.asset_type, &new_hash);
        if env.storage().persistent().has(&new_dk) {
            panic_with_error!(&env, ContractError::DuplicateAsset);
        }

        // Append history entry before updating the asset
        let history_key = metadata_history_key(asset_id);
        let mut history: Vec<MetadataHistoryEntry> = env
            .storage()
            .persistent()
            .get(&history_key)
            .unwrap_or_else(|| Vec::new(&env));
        let new_version = asset.metadata_version + 1;
        history.push_back(MetadataHistoryEntry {
            version: new_version,
            old_hash: old_hash.clone(),
            new_hash: new_hash.clone(),
            updated_at: env.ledger().timestamp(),
        });
        env.storage().persistent().set(&history_key, &history);
        env.storage()
            .persistent()
            .extend_ttl(&history_key, TTL_THRESHOLD, TTL_TARGET);

        // Store new dedup key and updated asset
        env.storage().persistent().set(&new_dk, &asset_id);
        extend_persistent_ttl(&env, &new_dk);
        asset.metadata = new_metadata.clone();
        asset.metadata_updated_at = env.ledger().timestamp();
        asset.metadata_version = new_version;
        env.storage().persistent().set(&asset_key(asset_id), &asset);
        extend_persistent_ttl(&env, &asset_key(asset_id));

        env.events().publish(
            (symbol_short!("UPD_META"), asset_id),
            (owner, old_hash, new_hash, new_version, env.ledger().timestamp()),
        );
    }

    /// Returns the full metadata change history for an asset, ordered oldest-first.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    ///
    /// # Returns
    /// `Vec<MetadataHistoryEntry>` — empty if no updates have been made
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if no asset exists with the given ID
    pub fn get_metadata_history(env: Env, asset_id: u64) -> Vec<MetadataHistoryEntry> {
        if !Self::asset_exists(env.clone(), asset_id) {
            panic_with_error!(&env, ContractError::AssetNotFound);
        }
        let key = metadata_history_key(asset_id);
        let history: Vec<MetadataHistoryEntry> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(&env));
        if env.storage().persistent().has(&key) {
            env.storage()
                .persistent()
                .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
        }
        history
    }

    /// Transfer ownership of an asset from the current owner to a new owner.
    /// Only the current owner can initiate the transfer.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset to transfer
    /// * `current_owner` - The current owner of the asset (must match stored owner)
    /// * `new_owner` - The address of the new asset owner
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if no asset exists with the given ID
    /// - [`ContractError::UnauthorizedOwner`] if caller is not the current owner
    pub fn transfer_asset(env: Env, asset_id: u64, current_owner: Address, new_owner: Address) {
        ensure_not_paused(&env);
        current_owner.require_auth();

        let mut asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));

        if asset.owner != current_owner {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        if current_owner == new_owner {
            panic_with_error!(&env, ContractError::SameOwner);
        }

        // Block transfers while the asset is locked as collateral under a lien.
        if asset.is_locked {
            panic_with_error!(&env, ContractError::AssetLocked);
        }

        // Move dedup key to new owner
        let hash: BytesN<32> = env
            .crypto()
            .sha256(&asset.metadata.clone().to_xdr(&env))
            .into();
        env.storage()
            .persistent()
            .remove(&dedup_key(&current_owner, &asset.asset_type, &hash));
        env.storage()
            .persistent()
            .set(&dedup_key(&new_owner, &asset.asset_type, &hash), &asset_id);
        extend_persistent_ttl(&env, &dedup_key(&new_owner, &asset.asset_type, &hash));

        // Move owner index entry
        owner_index_remove(&env, &current_owner, asset_id);
        owner_index_add(&env, &new_owner, asset_id);

        asset.owner = new_owner.clone();
        env.storage().persistent().set(&asset_key(asset_id), &asset);
        extend_persistent_ttl(&env, &asset_key(asset_id));

        env.events().publish(
            (symbol_short!("TRANSFER"), asset_id),
            (current_owner, new_owner, env.ledger().timestamp()),
        );
    }

    /// Initiate a multi-signature ownership transfer (step 1 of 2).
    /// Only the current owner can initiate. The proposed `new_owner` has
    /// `TRANSFER_TIMEOUT_SECS` (7 days) to accept via [`Self::accept_ownership_transfer`]
    /// before the proposal expires.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset to transfer
    /// * `new_owner` - The address proposed as the new owner
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if no asset exists with the given ID
    /// - [`ContractError::SameOwner`] if `new_owner` is already the current owner
    /// - [`ContractError::TransferAlreadyPending`] if an unexpired transfer is already pending
    pub fn initiate_ownership_transfer(env: Env, asset_id: u64, new_owner: Address) {
        ensure_not_paused(&env);

        let asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));

        asset.owner.require_auth();

        if asset.owner == new_owner {
            panic_with_error!(&env, ContractError::SameOwner);
        }

        let key = pending_transfer_key(asset_id);
        if let Some(existing) = env.storage().persistent().get::<_, PendingTransfer>(&key) {
            if env.ledger().timestamp().saturating_sub(existing.initiated_at) < TRANSFER_TIMEOUT_SECS {
                panic_with_error!(&env, ContractError::TransferAlreadyPending);
            }
        }

        let pending = PendingTransfer {
            new_owner: new_owner.clone(),
            initiated_at: env.ledger().timestamp(),
        };
        env.storage().persistent().set(&key, &pending);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);

        env.events().publish(
            (symbol_short!("OWN_INIT"), asset_id),
            (asset.owner, new_owner, env.ledger().timestamp()),
        );
    }

    /// Accept a pending ownership transfer (step 2 of 2). Only the proposed new owner
    /// can accept, and only within `TRANSFER_TIMEOUT_SECS` (7 days) of initiation.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset whose transfer is being accepted
    ///
    /// # Panics
    /// - [`ContractError::NoPendingTransfer`] if no transfer is pending for this asset
    /// - [`ContractError::TransferExpired`] if the acceptance window has elapsed
    /// - [`ContractError::AssetNotFound`] if no asset exists with the given ID
    pub fn accept_ownership_transfer(env: Env, asset_id: u64) {
        ensure_not_paused(&env);

        let key = pending_transfer_key(asset_id);
        let pending: PendingTransfer = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NoPendingTransfer));

        if env.ledger().timestamp().saturating_sub(pending.initiated_at) >= TRANSFER_TIMEOUT_SECS {
            env.storage().persistent().remove(&key);
            panic_with_error!(&env, ContractError::TransferExpired);
        }

        pending.new_owner.require_auth();

        let mut asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));

        let current_owner = asset.owner.clone();
        let new_owner = pending.new_owner.clone();

        // Move dedup key to new owner
        let hash: BytesN<32> = env
            .crypto()
            .sha256(&asset.metadata.clone().to_xdr(&env))
            .into();
        env.storage()
            .persistent()
            .remove(&dedup_key(&current_owner, &asset.asset_type, &hash));
        env.storage()
            .persistent()
            .set(&dedup_key(&new_owner, &asset.asset_type, &hash), &asset_id);
        env.storage().persistent().extend_ttl(
            &dedup_key(&new_owner, &asset.asset_type, &hash),
            TTL_THRESHOLD,
            TTL_TARGET,
        );

        // Move owner index entry
        owner_index_remove(&env, &current_owner, asset_id);
        owner_index_add(&env, &new_owner, asset_id);

        asset.owner = new_owner.clone();
        env.storage().persistent().set(&asset_key(asset_id), &asset);
        env.storage()
            .persistent()
            .extend_ttl(&asset_key(asset_id), TTL_THRESHOLD, TTL_TARGET);

        // Notify lifecycle contract to clear engineer authorizations for the asset
        if let Ok(lifecycle_addr) = env.storage().instance().get::<_, Address>(&LIFECYCLE_KEY) {
            let lifecycle_client = lifecycle::LifecycleClient::new(&env, &lifecycle_addr);
            lifecycle_client.transfer_notify(&asset_id, &new_owner);
        }
        env.storage().persistent().remove(&key);

        env.events().publish(
            (symbol_short!("OWN_DONE"), asset_id),
            (current_owner, new_owner, env.ledger().timestamp()),
        );
    }

    /// Admin-only function to decommission an asset.
    /// Sets the decommissioned flag and resets the collateral score to 0.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored admin
    /// * `asset_id` - The unique identifier of the asset to decommission
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::AssetNotFound`] if no asset exists with the given ID
    pub fn decommission_asset(env: Env, admin: Address, asset_id: u64) {
        ensure_not_paused(&env);
        admin.require_auth();

        let stored_admin: Address = Self::get_admin(env.clone());
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        // Verify asset exists
        if !Self::asset_exists(env.clone(), asset_id) {
            panic_with_error!(&env, ContractError::AssetNotFound);
        }

        // Set decommissioned flag
        let decomm_key = decommissioned_key(asset_id);
        env.storage().persistent().set(&decomm_key, &true);
        extend_persistent_ttl(&env, &decomm_key);
        env.storage()
            .persistent()
            .extend_ttl(&decomm_key, TTL_THRESHOLD, TTL_TARGET);

        // Clear the under_maintenance flag when decommissioning
        let maint_key = (symbol_short!("U_MAINT"), asset_id);
        env.storage().persistent().remove(&maint_key);

        // Emit decommission event with asset_id and ledger sequence
        let ledger_seq = env.ledger().sequence();
        env.events()
            .publish((symbol_short!("DECOMM"), asset_id), ledger_seq);
    }

    /// Owner-only function to mark an asset as deprecated.
    ///
    /// Deprecation is a soft, reversible signal from the asset owner indicating
    /// the machinery has reached end-of-life. A deprecated asset remains in the
    /// registry (preserving its maintenance audit trail) but returns a collateral
    /// score of 0 so it cannot be used as DeFi collateral.
    ///
    /// Unlike deregistration (which permanently removes the asset) or decommissioning
    /// (which is admin-only), deprecation is a self-service owner action.
    ///
    /// # Arguments
    /// * `owner` - The current owner of the asset (must match stored owner)
    /// * `asset_id` - The unique identifier of the asset to deprecate
    /// * `reason` - A human-readable explanation for the deprecation
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if no asset exists with the given ID
    /// - [`ContractError::UnauthorizedOwner`] if caller is not the asset owner
    /// - [`ContractError::AssetAlreadyDeprecated`] if asset is already deprecated or decommissioned
    pub fn deprecate_asset(env: Env, owner: Address, asset_id: u64, reason: String) {
        ensure_not_paused(&env);
        owner.require_auth();

        let mut asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));

        if asset.owner != owner {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        if asset.deprecation_status != DeprecationStatus::Active {
            panic_with_error!(&env, ContractError::AssetAlreadyDeprecated);
        }

        asset.deprecation_status = DeprecationStatus::Deprecated;
        env.storage().persistent().set(&asset_key(asset_id), &asset);
        extend_persistent_ttl(&env, &asset_key(asset_id));

        // Store reason separately to avoid bloating the core Asset struct on reads.
        let reason_key = (symbol_short!("DEP_RSN"), asset_id);
        env.storage().persistent().set(&reason_key, &reason);
        extend_persistent_ttl(&env, &reason_key);

        env.events().publish(
            (symbol_short!("DEPRECATD"), asset_id),
            (symbol_short!("DEPRCATED"), asset_id),
            (symbol_short!("DEPR"), asset_id),
            (owner, reason, env.ledger().timestamp()),
        );
    }

    /// Propose a WASM upgrade for the asset registry contract.
    /// Must be followed by `execute_upgrade` after the timelock delay.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored admin
    /// * `new_wasm_hash` - The hash of the new WASM to deploy
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if the admin has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    pub fn propose_upgrade(env: Env, admin: Address, new_wasm_hash: BytesN<32>) {
        ensure_not_paused(&env);
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&ADMIN_KEY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        env.storage().instance().extend_ttl(518400, 518400);

        let tl_key = global_timelock_key(symbol_short!("UPGRADE"));
        env.storage().persistent().set(
            &tl_key,
            &TimelockProposal {
                proposed_at: env.ledger().timestamp(),
                executed: false,
            },
        );
        extend_persistent_ttl(&env, &tl_key);
        env.storage()
            .persistent()
            .set(&symbol_short!("PEND_UPG"), &new_wasm_hash);
        extend_persistent_ttl(&env, &symbol_short!("PEND_UPG"));
        env.storage().persistent().extend_ttl(
            &symbol_short!("PEND_UPG"),
            TTL_THRESHOLD,
            TTL_TARGET,
        );

        env.events().publish(
            (symbol_short!("PROP_UPG"), admin.clone()),
            (new_wasm_hash, env.ledger().timestamp()),
        );
    }

    /// Execute a previously proposed WASM upgrade after the timelock delay has expired.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored admin
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if the admin has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::ProposalNotFound`] if no upgrade was proposed or already executed
    /// - [`ContractError::TimelockNotExpired`] if the delay has not elapsed
    pub fn execute_upgrade(env: Env, admin: Address) {
        ensure_not_paused(&env);
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&ADMIN_KEY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        require_global_timelock_ready(&env, symbol_short!("UPGRADE"));

        let new_wasm_hash: BytesN<32> = env
            .storage()
            .persistent()
            .get(&symbol_short!("PEND_UPG"))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::ProposalNotFound));
        env.storage()
            .persistent()
            .remove(&symbol_short!("PEND_UPG"));

        env.storage().instance().extend_ttl(518400, 518400);

        env.events().publish(
            (symbol_short!("UPGRADE"), admin.clone()),
            new_wasm_hash.clone(),
        );
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("UPGRADE")),
            (admin, env.ledger().timestamp(), new_wasm_hash.clone()),
        );

        #[cfg(not(test))]
        {
            env.deployer().update_current_contract_wasm(new_wasm_hash);
        }
    }

    /// Admin-only function to allow a new asset type symbol.
    ///
    /// # Arguments
    /// * `admin` - The address that must match the stored admin
    /// * `asset_type` - The symbol of the new asset type to allow
    pub fn add_asset_type(env: Env, admin: Address, asset_type: Symbol) {
        admin.require_auth();
        let stored_admin: Address = Self::get_admin(env.clone());
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        env.storage()
            .persistent()
            .set(&asset_type_key(&asset_type), &true);
        extend_persistent_ttl(&env, &asset_type_key(&asset_type));
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("ADD_TYPE")),
            (admin, env.ledger().timestamp(), asset_type.clone()),
        );
        env.events().publish((ADD_TYPE_TOPIC,), (asset_type,));
    }

    /// Admin-only function to remove an asset type from the allowlist.
    /// Removal is blocked if any registered assets of this type still exist.
    ///
    /// # Arguments
    /// * `admin` - The address that must match the stored admin
    /// * `asset_type` - The symbol of the asset type to remove
    ///
    /// # Panics
    /// - [`ContractError::TypeInUse`] if one or more assets of this type are still registered
    pub fn remove_asset_type(env: Env, admin: Address, asset_type: Symbol) {
        admin.require_auth();
        let stored_admin: Address = Self::get_admin(env.clone());
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        let count: u64 = env
            .storage()
            .persistent()
            .get(&type_count_key(&asset_type))
            .unwrap_or(0);
        if count > 0 {
            panic_with_error!(&env, ContractError::TypeInUse);
        }
        env.storage()
            .persistent()
            .remove(&asset_type_key(&asset_type));
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("RM_TYPE")),
            (admin, env.ledger().timestamp(), asset_type.clone()),
        );
        env.events().publish((RM_TYPE_TOPIC,), (asset_type,));
    }

    /// Check if an asset type is valid (exists in the allowlist).
    ///
    /// # Arguments
    /// * `asset_type` - The symbol of the asset type to check
    ///
    /// # Returns
    /// `true` if valid; `false` otherwise
    pub fn is_valid_asset_type(env: Env, asset_type: Symbol) -> bool {
        env.storage()
            .persistent()
            .get(&asset_type_key(&asset_type))
            .unwrap_or(false)
    }

    /// Get the lifecycle score for an asset by cross-calling the Lifecycle contract.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    /// * `lifecycle_contract` - The address of the Lifecycle contract
    ///
    /// # Returns
    /// The collateral score (u32) for the asset
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    pub fn get_lifecycle_score(env: Env, asset_id: u64, lifecycle_contract: Address) -> u32 {
        // Verify asset exists in this registry
        if !Self::asset_exists(env.clone(), asset_id) {
            panic_with_error!(&env, ContractError::AssetNotFound);
        }

        // Cross-call the Lifecycle contract to get the collateral score
        // Using invoke_contract to avoid circular dependency
        let args = soroban_sdk::vec![
            &env,
            soroban_sdk::IntoVal::<Env, soroban_sdk::Val>::into_val(&asset_id, &env)
        ];
        let score: u32 = env.invoke_contract(
            &lifecycle_contract,
            &Symbol::new(&env, "get_collateral_score"),
            args,
        );
        score
    }

    /// Decommission an asset and notify the lifecycle contract to freeze the score.
    ///
    /// This combines the registry-side decommission flag with a cross-contract call
    /// to the lifecycle contract so the collateral score is captured at decommission
    /// time and no longer decays. Lenders will see the final verified state.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored admin
    /// * `asset_id` - The unique identifier of the asset to decommission
    /// * `lifecycle_contract` - Address of the lifecycle contract to notify
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::AssetNotFound`] if no asset exists with the given ID
    pub fn decommission_asset_notify(
        env: Env,
        admin: Address,
        asset_id: u64,
        lifecycle_contract: Address,
    ) {
        ensure_not_paused(&env);
        admin.require_auth();

        let stored_admin: Address = Self::get_admin(env.clone());
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        if !Self::asset_exists(env.clone(), asset_id) {
            panic_with_error!(&env, ContractError::AssetNotFound);
        }

        let decomm_key = decommissioned_key(asset_id);
        env.storage().persistent().set(&decomm_key, &true);
        extend_persistent_ttl(&env, &decomm_key);

        let maint_key = (symbol_short!("U_MAINT"), asset_id);
        env.storage().persistent().remove(&maint_key);

        let ledger_seq = env.ledger().sequence();
        env.events()
            .publish((symbol_short!("DECOMM"), asset_id), ledger_seq);

        // Notify lifecycle to freeze the collateral score at its current value.
        let args = soroban_sdk::vec![
            &env,
            soroban_sdk::IntoVal::<Env, soroban_sdk::Val>::into_val(&asset_id, &env)
        ];
        env.invoke_contract::<()>(
            &lifecycle_contract,
            &Symbol::new(&env, "decommission_notify"),
            args,
        );
    }

    /// Search assets with optional metadata filtering and sorting.
    ///
    /// Scans all registered assets and returns those that match every supplied
    /// constraint.  At most **100** matching assets are returned; `SearchPage::total`
    /// always reflects the full match count before the cap is applied.
    ///
    /// # Arguments
    /// * `filter.asset_type`       – exact `asset_type` match (optional)
    /// * `filter.manufacturer`     – substring present in `metadata` (optional)
    /// * `filter.min_age_months`   – asset registered ≥ N months ago (optional)
    /// * `filter.max_age_months`   – asset registered ≤ N months ago (optional)
    /// * `filter.sort`             – sort order (optional)
    /// * `filter.lifecycle_contract` – required when sort = `ByCollateralScore`
    pub fn search_assets(env: Env, filter: SearchFilter) -> SearchPage {
        const MAX_RESULTS: u32 = 100;
        const SECS_PER_MONTH: u64 = 30 * 86_400;

        let total_assets: u64 = env
            .storage()
            .persistent()
            .get(&ASSET_COUNT)
            .unwrap_or(0);

        let now = env.ledger().timestamp();

        let mut matched: Vec<Asset> = Vec::new(&env);
        let mut total_matched: u32 = 0;

        for id in 1..=total_assets {
            let key = asset_key(id);
            let asset: Asset = match env.storage().persistent().get(&key) {
                Some(a) => a,
                None => continue,
            };

            // --- filter: asset_type ---
            if let Some(ref ft) = filter.asset_type {
                if asset.asset_type != *ft {
                    continue;
                }
            }

            // --- filter: manufacturer (substring of metadata) ---
            if let Some(ref needle) = filter.manufacturer {
                if !string_contains(&env, &asset.metadata, needle) {
                    continue;
                }
            }

            // --- filter: age ---
            let age_secs = now.saturating_sub(asset.registered_at);
            let age_months = (age_secs / SECS_PER_MONTH) as u32;
            if let Some(min) = filter.min_age_months {
                if age_months < min {
                    continue;
                }
            }
            if let Some(max) = filter.max_age_months {
                if age_months > max {
                    continue;
                }
            }

            total_matched += 1;
            if matched.len() < MAX_RESULTS {
                matched.push_back(asset);
            }
        }

        // --- sort ---
        if let Some(sort) = filter.sort {
            match sort {
                SortOrder::ByCollateralScore => {
                    if let Some(lc) = filter.lifecycle_contract {
                        // Fetch scores then sort descending.
                        let mut pairs: Vec<(u32, Asset)> = Vec::new(&env);
                        for i in 0..matched.len() {
                            let asset = matched.get(i).unwrap();
                            let args = soroban_sdk::vec![
                                &env,
                                soroban_sdk::IntoVal::<Env, soroban_sdk::Val>::into_val(
                                    &asset.asset_id,
                                    &env,
                                )
                            ];
                            let score: u32 = env.invoke_contract(
                                &lc,
                                &Symbol::new(&env, "get_collateral_score"),
                                args,
                            );
                            pairs.push_back((score, asset));
                        }
                        // Insertion sort descending by score (results ≤ 100, cost acceptable).
                        let n = pairs.len();
                        for i in 1..n {
                            let mut j = i;
                            while j > 0 {
                                let a = pairs.get(j - 1).unwrap().0;
                                let b = pairs.get(j).unwrap().0;
                                if a >= b {
                                    break;
                                }
                                // swap j-1 and j
                                let tmp_a = pairs.get(j - 1).unwrap();
                                let tmp_b = pairs.get(j).unwrap();
                                pairs.set(j - 1, tmp_b);
                                pairs.set(j, tmp_a);
                                j -= 1;
                            }
                        }
                        matched = Vec::new(&env);
                        for i in 0..n {
                            matched.push_back(pairs.get(i).unwrap().1);
                        }
                    }
                }
                SortOrder::ByMaintenanceDate => {
                    // Sort by metadata_updated_at descending (most recently updated first).
                    let n = matched.len();
                    for i in 1..n {
                        let mut j = i;
                        while j > 0 {
                            let a = matched.get(j - 1).unwrap().metadata_updated_at;
                            let b = matched.get(j).unwrap().metadata_updated_at;
                            if a >= b {
                                break;
                            }
                            let tmp_a = matched.get(j - 1).unwrap();
                            let tmp_b = matched.get(j).unwrap();
                            matched.set(j - 1, tmp_b);
                            matched.set(j, tmp_a);
                            j -= 1;
                        }
                    }
                }
            }
        }

        SearchPage { assets: matched, total: total_matched }
    }
}

/// Returns `true` if `haystack` contains `needle` as a substring (byte-level, UTF-8 safe).
fn string_contains(env: &Env, haystack: &String, needle: &String) -> bool {
    use soroban_sdk::xdr::ToXdr;
    // XDR encodes a string as: 4-byte big-endian length + UTF-8 bytes (+ padding).
    // We skip the first 4 bytes to obtain raw UTF-8.
    let h_xdr = haystack.to_xdr(env);
    let n_xdr = needle.to_xdr(env);
    let h_len = h_xdr.len();
    let n_len = n_xdr.len();
    if n_len <= 4 || h_len < n_len {
        // needle is empty after the 4-byte header → trivially true;
        // or haystack shorter than needle → false.
        return n_len <= 4;
    }
    // Raw byte lengths (subtract 4-byte XDR prefix; ignore padding since UTF-8 is before padding).
    // We work on raw Bytes indices.
    let h_data_len = h_len - 4;
    let n_data_len = n_len - 4;
    if h_data_len < n_data_len {
        return false;
    }
    // Naive O(h*n) scan — acceptable: metadata ≤ 256 bytes.
    'outer: for start in 0..=(h_data_len - n_data_len) {
        for k in 0..n_data_len {
            if h_xdr.get(4 + start + k) != n_xdr.get(4 + k) {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

// Minimal client interface for cross-contract call to Lifecycle
mod lifecycle {
    use soroban_sdk::{contractclient, Address, Env, Symbol, String};

    #[allow(dead_code)]
    #[contractclient(name = "LifecycleClient")]
    pub trait Lifecycle {
        fn transfer_notify(env: Env, asset_id: u64, new_owner: Address);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
}
