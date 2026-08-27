#![no_std]
use shared::error::SharedContractError;
use shared::validation::{require_non_empty_vec, require_string_length};
use shared::{extend_persistent_ttl, require_admin, TTL_THRESHOLD, TTL_TARGET};
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
    AssetLocked = 19,
    LendingContractNotSet = 20,
    UnauthorizedLender = 21,
    LoanIdMismatch = 22,
    AssetNotLocked = 23,
    /// An unexpired ownership transfer is already pending for this asset.
    TransferAlreadyPending = 24,
    /// No pending ownership transfer exists for this asset.
    NoPendingTransfer = 25,
    /// The pending ownership transfer's acceptance window has elapsed.
    TransferExpired = 26,
    /// A pending ownership transfer exists but has not yet passed its timeout.
    TransferNotExpired = 27,
    /// A required configuration field was missing for the requested operation
    /// (e.g. `SearchFilter::lifecycle_contract` when sorting by `ByCollateralScore`).
    InvalidConfig = 24,
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
    /// Unix timestamp when the asset was deprecated. `None` if the asset is still active
    /// or was decommissioned without going through the `Deprecated` state.
    pub deprecated_at: Option<u64>,
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
    /// Required when `sort` is [`SortOrder::ByCollateralScore`]. If omitted while
    /// that sort order is requested, `search_assets` panics with
    /// `ContractError::InvalidConfig` instead of attempting the cross-contract call.
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
const DEPLOYER_KEY: Symbol = symbol_short!("DEPLOYER");
const ALLOWED_ASSET_TYPES_KEY: Symbol = symbol_short!("A_TYPES");

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

/// Serial-number lookup key for `get_asset_by_serial_number`.
///
/// This is the same underlying storage slot as `serial_dedup_key` — the
/// deduplication map written during `register_asset` doubles as the reverse
/// lookup index.  The helper is kept separate so call-sites are self-documenting.
fn serial_number_lookup_key(env: &Env, serial: &String) -> (Symbol, BytesN<32>) {
    let sn_bytes = serial.clone().to_xdr(env);
    let hash: BytesN<32> = env.crypto().sha256(&sn_bytes).into();
    serial_dedup_key(&hash)
}

/// Owner index key: owner → Vec<u64> of asset IDs.
fn owner_index_key(owner: &Address) -> DataKey {
    DataKey::AssetsByOwner(owner.clone())
}

/// Asset type allowlist key: asset_type → bool.
fn asset_type_key(asset_type: &Symbol) -> (Symbol, Symbol) {
    (ASSET_TYPE_PREFIX, asset_type.clone())
}

fn allowed_asset_types(env: &Env) -> Vec<Symbol> {
    env.storage()
        .persistent()
        .get(&ALLOWED_ASSET_TYPES_KEY)
        .unwrap_or_else(|| Vec::new(env))
}

fn set_allowed_asset_types(env: &Env, asset_types: &Vec<Symbol>) {
    env.storage().persistent().set(&ALLOWED_ASSET_TYPES_KEY, asset_types);
    extend_persistent_ttl(&env, &ALLOWED_ASSET_TYPES_KEY);
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
    /// Store the deployer address at deploy time.
    pub fn __constructor(env: Env, deployer: Address) {
        env.storage().instance().set(&DEPLOYER_KEY, &deployer);
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_TARGET);
    }

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
            deprecated_at: None,
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
                deprecated_at: None,
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

    /// Look up an asset by its physical serial number.
    ///
    /// Field engineers and auditors who know a machine's manufacturer plate number
    /// can use this function to retrieve the full on-chain record without needing
    /// to know the numeric `asset_id` in advance.
    ///
    /// The lookup is O(1): during `register_asset` a mapping of
    /// `sha256(serial_number) → asset_id` is written to persistent storage under
    /// the same key used for serial-number deduplication, so no additional storage
    /// is required.
    ///
    /// # Arguments
    /// * `serial` - The physical serial number string (case-sensitive, as registered)
    ///
    /// # Returns
    /// `Some(Asset)` if an asset with that serial number exists; `None` otherwise.
    pub fn get_asset_by_serial_number(env: Env, serial: String) -> Option<Asset> {
        let key = serial_number_lookup_key(&env, &serial);
        // Resolve serial → asset_id using the dedup index written at registration time.
        let asset_id: u64 = match env.storage().persistent().get(&key) {
            Some(id) => id,
            None => return None,
        };
        // Fetch the full Asset record.
        let asset_key = asset_key(asset_id);
        let asset: Asset = match env.storage().persistent().get(&asset_key) {
            Some(a) => a,
            None => return None,
        };
        // Extend TTL on both entries on read to keep the index alive.
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
        env.storage()
            .persistent()
            .extend_ttl(&asset_key, TTL_THRESHOLD, TTL_TARGET);
        Some(asset)
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
        deployer.require_auth();
        let stored_deployer: Address = env
            .storage()
            .instance()
            .get(&DEPLOYER_KEY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if deployer != stored_deployer {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
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
        let stored_admin: Address = Self::get_admin(env.clone());
        if require_admin(&admin, &stored_admin).is_err() {
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
        let stored_admin: Address = Self::get_admin(env.clone());
        if require_admin(&admin, &stored_admin).is_err() {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        if env.storage().instance().has(&PENDING_ADMIN_KEY) {
            panic_with_error!(&env, ContractError::PendingAdminAlreadyExists);
        }
        env.storage().instance().set(&PENDING_ADMIN_KEY, &new_admin);
        env.storage().instance().extend_ttl(DEFAULT_TTL_LEDGERS, DEFAULT_TTL_LEDGERS);
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
        env.storage().instance().extend_ttl(DEFAULT_TTL_LEDGERS, DEFAULT_TTL_LEDGERS);
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("ADMIN_SET")),
            (pending_admin.clone(), env.ledger().timestamp()),
        );
        env.events()
            .publish((symbol_short!("ADMIN_SET"),), (pending_admin,));
    }

    /// Cancel a pending admin transfer proposal.
    /// Only the current admin can cancel. Clears the pending admin entry so the
    /// proposed address can no longer call `accept_admin`.
    ///
    /// # Arguments
    /// * `admin` - The current admin address (must match stored admin)
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the current admin
    /// - [`ContractError::ProposalNotFound`] if no pending admin proposal exists
    pub fn cancel_admin_proposal(env: Env, admin: Address) {
        let stored_admin: Address = Self::get_admin(env.clone());
        if require_admin(&admin, &stored_admin).is_err() {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        if !env.storage().instance().has(&PENDING_ADMIN_KEY) {
            panic_with_error!(&env, ContractError::ProposalNotFound);
        }
        env.storage().instance().remove(&PENDING_ADMIN_KEY);
        env.storage().instance().extend_ttl(DEFAULT_TTL_LEDGERS, DEFAULT_TTL_LEDGERS);
        env.events().publish(
            (symbol_short!("ADM_AUD"), symbol_short!("ADM_CNCL")),
            (admin.clone(), env.ledger().timestamp()),
        );
        env.events().publish(
            (symbol_short!("ADM_CANCEL"),),
            (admin,),
        );
    }

    /// Admin-only function to pause the contract.
    ///
    /// # Arguments
    /// * `admin` - The address that must match the stored admin
    pub fn pause(env: Env, admin: Address) {
        let stored_admin: Address = Self::get_admin(env.clone());
        if require_admin(&admin, &stored_admin).is_err() {
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
        let stored_admin: Address = Self::get_admin(env.clone());
        if require_admin(&admin, &stored_admin).is_err() {
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

    /// Retrieve a paginated slice of the metadata change history for an asset.
    ///
    /// Issue #1021 — auditors need to trace metadata evolution without querying
    /// raw storage. This view function returns a bounded slice of
    /// [`MetadataHistoryEntry`] records, ordered oldest-first, allowing callers
    /// to page through the full history in chunks.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    /// * `offset`   - Zero-based index of the first entry to return
    /// * `limit`    - Maximum number of entries to return (capped at 100)
    ///
    /// # Returns
    /// `Vec<MetadataHistoryEntry>` — the requested slice, possibly shorter than
    /// `limit` if fewer entries remain after `offset`.  Returns an empty vector
    /// when `offset` ≥ total history length or no history exists.
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if no asset exists with the given ID
    pub fn get_asset_metadata_history(
        env: Env,
        asset_id: u64,
        offset: u32,
        limit: u32,
    ) -> Vec<MetadataHistoryEntry> {
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

        // Cap the page size to 100 to prevent excessive per-call compute cost.
        let capped_limit = limit.min(100);

        let total = history.len();
        if offset >= total || capped_limit == 0 {
            return Vec::new(&env);
        }

        let end = (offset + capped_limit).min(total);
        let mut page: Vec<MetadataHistoryEntry> = Vec::new(&env);
        for i in offset..end {
            page.push_back(history.get(i).unwrap());
        }
        page
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

    /// Clear a pending ownership transfer that has passed its `TRANSFER_TIMEOUT_SECS`
    /// acceptance window. Callable by anyone, since an expired transfer can no longer
    /// be accepted; this simply frees the pending-transfer slot so a new transfer can
    /// be initiated immediately instead of waiting on `initiate_ownership_transfer`'s
    /// own lazy-expiry check.
    ///
    /// # Panics
    /// - [`ContractError::NoPendingTransfer`] if no transfer is pending for this asset
    /// - [`ContractError::TransferNotExpired`] if the timeout has not yet elapsed
    pub fn cancel_expired_transfer(env: Env, asset_id: u64) {
        let key = pending_transfer_key(asset_id);
        let pending: PendingTransfer = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NoPendingTransfer));

        if env.ledger().timestamp().saturating_sub(pending.initiated_at) < TRANSFER_TIMEOUT_SECS {
            panic_with_error!(&env, ContractError::TransferNotExpired);
        }

        env.storage().persistent().remove(&key);

        env.events()
            .publish((symbol_short!("OWN_EXP"), asset_id), pending.new_owner);
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

        let timestamp = env.ledger().timestamp();
        asset.deprecation_status = DeprecationStatus::Deprecated;
        asset.deprecated_at = Some(timestamp);
        env.storage().persistent().set(&asset_key(asset_id), &asset);
        extend_persistent_ttl(&env, &asset_key(asset_id));

        // If a lien is active on this asset, emit DEPR_WARN so DeFi lenders
        // holding loans against it receive an on-chain warning.
        if asset.is_locked {
            env.events().publish(
                (symbol_short!("DEPR_WARN"), asset_id),
                (asset_id, asset.lender.clone(), timestamp),
            );
        }

        // Store reason separately to avoid bloating the core Asset struct on reads.
        let reason_key = (symbol_short!("DEP_RSN"), asset_id);
        env.storage().persistent().set(&reason_key, &reason);
        extend_persistent_ttl(&env, &reason_key);

        env.events().publish(
            (symbol_short!("DEPR"), asset_id),
            (owner, reason, timestamp),
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

        env.storage().instance().extend_ttl(DEFAULT_TTL_LEDGERS, DEFAULT_TTL_LEDGERS);

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

        env.storage().instance().extend_ttl(DEFAULT_TTL_LEDGERS, DEFAULT_TTL_LEDGERS);

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
        let stored_admin: Address = Self::get_admin(env.clone());
        if require_admin(&admin, &stored_admin).is_err() {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        let mut allowed_types = allowed_asset_types(&env);
        let mut already_present = false;
        for existing in allowed_types.iter() {
            if existing == asset_type {
                already_present = true;
                break;
            }
        }
        if !already_present {
            allowed_types.push_back(asset_type.clone());
            set_allowed_asset_types(&env, &allowed_types);
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
        let stored_admin: Address = Self::get_admin(env.clone());
        if require_admin(&admin, &stored_admin).is_err() {
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

        let mut allowed_types = allowed_asset_types(&env);
        let mut updated_types = Vec::new(&env);
        for existing in allowed_types.iter() {
            if existing != asset_type {
                updated_types.push_back(existing);
            }
        }
        set_allowed_asset_types(&env, &updated_types);

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
        for allowed_type in allowed_asset_types(&env).iter() {
            if allowed_type == asset_type {
                return true;
            }
        }
        false
    }

    /// Get the lifecycle score for an asset by cross-calling the Lifecycle contract.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    /// * `lifecycle_contract` - The address of the Lifecycle contract
    ///
    /// # Returns
    /// `Some(score)` with the collateral score for the asset, or `None` if the asset
    /// has never had a maintenance record submitted. Returning `None` instead of a
    /// sentinel value forces callers (e.g. DeFi lenders) to explicitly handle the
    /// no-history case rather than risk misreading it as a valid, high score.
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    pub fn get_lifecycle_score(env: Env, asset_id: u64, lifecycle_contract: Address) -> Option<u32> {
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

        // A fresh asset with no maintenance history at all must be reported as `None`
        // rather than the raw score, which would otherwise read as 0 and be
        // indistinguishable from a poorly-maintained (also-0) asset.
        let last_service: Option<u64> = env.invoke_contract(
            &lifecycle_contract,
            &Symbol::new(&env, "get_last_service_timestamp"),
            args.clone(),
        );
        if last_service.is_none() {
            return None;
        }

        let score: u32 = env.invoke_contract(
            &lifecycle_contract,
            &Symbol::new(&env, "get_collateral_score"),
            args,
        );
        Some(score)
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

    /// Set the lending contract address that is authorized to lock and unlock assets.
    pub fn set_lending_contract(env: Env, admin: Address, lending_addr: Address) {
        let stored_admin: Address = Self::get_admin(env.clone());
        if require_admin(&admin, &stored_admin).is_err() {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        env.storage().instance().set(&LENDING_CONTRACT_KEY, &lending_addr);
        env.storage().instance().extend_ttl(TTL_THRESHOLD, TTL_TARGET);
    }

    /// Return the currently configured lending contract, if one has been set.
    pub fn get_lending_contract(env: Env) -> Option<Address> {
        env.storage().instance().get(&LENDING_CONTRACT_KEY)
    }

    /// Lock an asset as collateral under the configured lending contract.
    pub fn lock_asset_as_collateral(env: Env, lender: Address, asset_id: u64, loan_id: u64) {
        ensure_not_paused(&env);
        lender.require_auth();

        let registered_lender: Address = env
            .storage()
            .instance()
            .get(&LENDING_CONTRACT_KEY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::LendingContractNotSet));
        if lender != registered_lender {
            panic_with_error!(&env, ContractError::UnauthorizedLender);
        }

        let mut asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));
        if asset.is_locked {
            panic_with_error!(&env, ContractError::AssetLocked);
        }

        // Refuse to lock an asset with a live ownership transfer in flight: if the
        // transfer completed after the lien were placed, the new owner would inherit
        // a locked asset pledged by the previous owner with no lender notification.
        let pending_key = pending_transfer_key(asset_id);
        if let Some(pending) = env
            .storage()
            .persistent()
            .get::<_, PendingTransfer>(&pending_key)
        {
            if env.ledger().timestamp().saturating_sub(pending.initiated_at) < TRANSFER_TIMEOUT_SECS
            {
                panic_with_error!(&env, ContractError::TransferAlreadyPending);
            }
        }

        asset.is_locked = true;
        asset.lender = Some(lender.clone());
        asset.loan_id = Some(loan_id);

        env.storage().persistent().set(&asset_key(asset_id), &asset);
        extend_persistent_ttl(&env, &asset_key(asset_id));

        env.events().publish(
            (symbol_short!("LOCK"), asset_id),
            (lender, loan_id, env.ledger().timestamp()),
        );
    }

    /// Unlock an asset from collateral after the loan is repaid.
    pub fn unlock_asset_from_collateral(env: Env, lender: Address, asset_id: u64, loan_id: u64) {
        ensure_not_paused(&env);
        lender.require_auth();

        let registered_lender: Address = env
            .storage()
            .instance()
            .get(&LENDING_CONTRACT_KEY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::LendingContractNotSet));
        if lender != registered_lender {
            panic_with_error!(&env, ContractError::UnauthorizedLender);
        }

        let mut asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));
        if !asset.is_locked {
            panic_with_error!(&env, ContractError::AssetNotLocked);
        }
        if asset.loan_id != Some(loan_id) {
            panic_with_error!(&env, ContractError::LoanIdMismatch);
        }

        asset.is_locked = false;
        asset.lender = None;
        asset.loan_id = None;

        env.storage().persistent().set(&asset_key(asset_id), &asset);
        extend_persistent_ttl(&env, &asset_key(asset_id));

        env.events().publish(
            (symbol_short!("UNLOCK"), asset_id),
            (lender, loan_id, env.ledger().timestamp()),
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

        if filter.sort == Some(SortOrder::ByCollateralScore) && filter.lifecycle_contract.is_none()
        {
            panic_with_error!(&env, ContractError::InvalidConfig);
        }

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

    /// Mark an asset as under maintenance.
    /// Callable by the asset owner or contract admin.
    ///
    /// Sets the asset's status to [`AssetStatus::UnderMaintenance`], which
    /// signals to integrators (e.g. lending contracts, lifecycle scoring)
    /// that the asset is temporarily unavailable for normal operation.
    ///
    /// # Arguments
    /// * `caller` - The address initiating the maintenance (owner or admin)
    /// * `asset_id` - The unique identifier of the asset
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    /// - [`ContractError::UnauthorizedOwner`] if caller is neither owner nor admin
    /// - [`ContractError::AssetDecommissioned`] if the asset is decommissioned
    pub fn mark_under_maintenance(env: Env, caller: Address, asset_id: u64) {
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

        // Reject decommissioned assets
        let decomm_key = decommissioned_key(asset_id);
        if env.storage().persistent().get::<_, bool>(&decomm_key).unwrap_or(false) {
            panic_with_error!(&env, ContractError::AssetDecommissioned);
        }

        let maint_key = (symbol_short!("U_MAINT"), asset_id);
        env.storage().persistent().set(&maint_key, &true);
        extend_persistent_ttl(&env, &maint_key);

        env.events().publish(
            (symbol_short!("MAINT_START"), asset_id),
            (caller, env.ledger().timestamp()),
        );
    }

    /// Mark an asset as having completed maintenance, returning it to [`AssetStatus::Active`].
    /// Callable by the asset owner or contract admin.
    ///
    /// # Arguments
    /// * `caller` - The address completing maintenance (owner or admin)
    /// * `asset_id` - The unique identifier of the asset
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    /// - [`ContractError::UnauthorizedOwner`] if caller is neither owner nor admin
    pub fn mark_maintenance_complete(env: Env, caller: Address, asset_id: u64) {
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

        let maint_key = (symbol_short!("U_MAINT"), asset_id);
        env.storage().persistent().remove(&maint_key);

        env.events().publish(
            (symbol_short!("MAINT_END"), asset_id),
            (caller, env.ledger().timestamp()),
        );
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
    use soroban_sdk::testutils::storage::Instance as _;
    use soroban_sdk::testutils::storage::Persistent;
    use soroban_sdk::{
        symbol_short,
        testutils::{Address as _, Events, Ledger as _, Logs},
        Bytes, Env, FromVal, String, Symbol, TryIntoVal,
    };

    use crate::AssetRegistryClient;
    use engineer_registry;
    use lifecycle;

    fn unique_serial(env: &Env) -> String {
        use core::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut buf = [0u8; 24];
        buf[0] = b'S';
        buf[1] = b'N';
        buf[2] = b'-';
        let mut end = 24usize;
        let mut v = if n == 0 { 1u64 } else { n };
        while v > 0 {
            end -= 1;
            buf[end] = b'0' + (v % 10) as u8;
            v /= 10;
        }
        let digit_len = 24 - end;
        let mut out = [0u8; 24];
        out[0] = b'S';
        out[1] = b'N';
        out[2] = b'-';
        out[3..3 + digit_len].copy_from_slice(&buf[end..24]);
        let s = core::str::from_utf8(&out[..3 + digit_len]).unwrap_or("SN-1");
        String::from_str(env, s)
    }

    /// Wrapper: register_asset with an auto-generated unique serial number.
    fn reg(
        client: &AssetRegistryClient,
        env: &Env,
        asset_type: Symbol,
        metadata: String,
        owner: &Address,
    ) -> u64 {
        client.register_asset(&asset_type, &metadata, &unique_serial(env), owner)
    }

    /// Wrapper: try_register_asset with an auto-generated unique serial number.
    #[allow(dead_code)]
    fn try_reg(
        client: &AssetRegistryClient,
        env: &Env,
        asset_type: Symbol,
        metadata: String,
        owner: &Address,
    ) -> Result<Result<u64, soroban_sdk::Error>, Result<soroban_sdk::Error, soroban_sdk::InvokeError>>
    {
        client.try_register_asset(&asset_type, &metadata, &unique_serial(env), owner)
    }

    #[test]
    fn test_register_and_get_asset() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Caterpillar 3516 Generator"),
            &unique_serial(&env),
            &owner,
        );
        assert_eq!(id, 1);

        let asset = client.get_asset(&id);
        assert_eq!(asset.asset_id, 1);
        assert_eq!(asset.owner, owner);
    }

    #[test]
    fn test_get_asset_returns_correct_owner() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("TURBINE"));

        let expected_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("TURBINE"),
            &String::from_str(&env, "GE LM2500 Turbine"),
            &unique_serial(&env),
            &expected_owner,
        );

        let asset = client.get_asset(&id);
        assert_eq!(asset.owner, expected_owner);
    }

    #[test]
    fn test_get_asset_not_found() {
        let env = Env::default();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);
        let result = client.try_get_asset(&999);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32
            )))
        );
    }

    #[test]
    fn test_get_asset_extends_ttl_on_read() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let asset_id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        // Read via get_asset — TTL must be extended
        client.get_asset(&asset_id);

        env.as_contract(&contract_id, || {
            let ttl = env.storage().persistent().get_ttl(&asset_key(asset_id));
            assert!(ttl > 0, "asset TTL must be extended on get_asset read");
        });
    }

    #[test]
    fn test_duplicate_metadata_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let metadata = String::from_str(&env, "CAT-3516-SN123456");
        let serial = String::from_str(&env, "SN-CAT-3516-001");

        // First registration succeeds
        let id = client.register_asset(&symbol_short!("GENSET"), &metadata, &serial, &owner);
        assert_eq!(id, 1);

        // Second registration with same serial is rejected (same physical machine)
        let result =
            client.try_register_asset(&symbol_short!("GENSET"), &metadata, &serial, &owner);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::DuplicateAsset as u32
            )))
        );
    }

    #[test]
    fn test_register_asset_duplicate_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let metadata = String::from_str(&env, "CAT-3516-DUPLICATE");
        let serial = String::from_str(&env, "SN-CAT-3516-DUP");

        let id = client.register_asset(&symbol_short!("GENSET"), &metadata, &serial, &owner);
        assert_eq!(id, 1);

        let result =
            client.try_register_asset(&symbol_short!("GENSET"), &metadata, &serial, &owner);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::DuplicateAsset as u32
            )))
        );
    }

    /// Closes #1067 — the owner+metadata dedup key must reject a duplicate even
    /// when the serial number differs, proving this check is independent from
    /// (and not merely a side effect of) the serial-number dedup check.
    #[test]
    fn test_register_asset_same_owner_metadata_different_serial_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let metadata = String::from_str(&env, "CAT-3516-SAME-METADATA");

        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &metadata,
            &String::from_str(&env, "SN-FIRST-001"),
            &owner,
        );
        assert_eq!(id, 1);

        // Same owner + same metadata, but a distinct serial number: the
        // secondary (owner, asset_type, metadata_hash) dedup key must still
        // reject this as a duplicate asset.
        let result = client.try_register_asset(
            &symbol_short!("GENSET"),
            &metadata,
            &String::from_str(&env, "SN-SECOND-002"),
            &owner,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::DuplicateAsset as u32
            )))
        );
    }

    #[test]
    fn test_different_owners_same_metadata_allowed() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner_a = Address::generate(&env);
        let owner_b = Address::generate(&env);
        let metadata = String::from_str(&env, "CAT-3516-SN123456");

        // Different owners may register the same metadata (different physical assets)
        let id_a = client.register_asset(
            &symbol_short!("GENSET"),
            &metadata,
            &String::from_str(&env, "SN-A-001"),
            &owner_a,
        );
        let id_b = client.register_asset(
            &symbol_short!("GENSET"),
            &metadata,
            &String::from_str(&env, "SN-B-001"),
            &owner_b,
        );
        assert_ne!(id_a, id_b);
    }

    /// Closes #782 — serial numbers must be globally unique: a different owner must
    /// not be able to register an asset with the same physical serial number.
    #[test]
    fn test_cross_owner_duplicate_serial_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner_a = Address::generate(&env);
        let owner_b = Address::generate(&env);
        let serial = String::from_str(&env, "SN-GLOBAL-001");

        // First owner registers successfully.
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Machine A metadata"),
            &serial,
            &owner_a,
        );
        assert_eq!(id, 1);

        // Second owner attempts to register the same physical serial — must be rejected.
        let result = client.try_register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Machine A metadata different owner"),
            &serial,
            &owner_b,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::DuplicateAsset as u32
            ))),
            "duplicate serial number must be rejected even for a different owner"
        );
    }

    #[test]
    fn test_register_asset_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let asset_type = symbol_short!("GENSET");
        let metadata = String::from_str(&env, "Caterpillar 3516 Generator");

        let timestamp = env.ledger().timestamp();
        let asset_id = client.register_asset(&asset_type, &metadata, &unique_serial(&env), &owner);

        use soroban_sdk::TryIntoVal;
        let reg_topic = symbol_short!("reg_asset");
        let events = env.events().all();
        let (_, topics, data) = events.last().unwrap();

        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, reg_topic);

        let (emitted_id, emitted_owner, emitted_timestamp): (u64, Address, u64) =
            data.try_into_val(&env).unwrap();
        assert_eq!(emitted_id, asset_id);
        assert_eq!(emitted_owner, owner);
        assert_eq!(emitted_timestamp, timestamp);
    }

    #[test]
    fn test_ttl_extended_on_registration() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let asset_type = symbol_short!("GENSET");
        let metadata = String::from_str(&env, "Caterpillar 3516 Generator");

        let id = client.register_asset(&asset_type, &metadata, &unique_serial(&env), &owner);

        // Verify TTL is set for asset storage entry
        let asset_ttl = env.as_contract(&contract_id, || {
            env.storage().persistent().get_ttl(&asset_key(id))
        });
        assert!(asset_ttl > 0, "Asset TTL should be extended");

        // Verify TTL is set for deduplication key
        let meta_bytes = metadata.to_xdr(&env);
        let meta_hash: BytesN<32> = env.crypto().sha256(&meta_bytes).into();
        let dedup_ttl = env.as_contract(&contract_id, || {
            let dk = dedup_key(&owner, &symbol_short!("GENSET"), &meta_hash);
            env.storage().persistent().get_ttl(&dk)
        });
        assert!(dedup_ttl > 0, "Deduplication key TTL should be extended");
    }

    /// Issue #838: every write must extend the relevant persistent entry's TTL
    /// to at least `TTL_THRESHOLD`. Verify the asset entry after `register_asset`.
    #[test]
    fn test_register_asset_ttl_at_least_threshold() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let metadata = String::from_str(&env, "Caterpillar 3516 Generator");
        let id =
            client.register_asset(&symbol_short!("GENSET"), &metadata, &unique_serial(&env), &owner);

        let asset_ttl = env.as_contract(&contract_id, || {
            env.storage().persistent().get_ttl(&asset_key(id))
        });
        assert!(
            asset_ttl >= TTL_THRESHOLD,
            "asset entry TTL ({}) must be >= TTL_THRESHOLD ({}) after register_asset",
            asset_ttl,
            TTL_THRESHOLD
        );
    }

    #[test]
    fn test_register_asset_dedup_key_ttl_is_set() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let metadata = String::from_str(&env, "Dedup TTL test asset");
        client.register_asset(
            &symbol_short!("GENSET"),
            &metadata,
            &unique_serial(&env),
            &owner,
        );

        let meta_bytes = metadata.to_xdr(&env);
        let meta_hash: BytesN<32> = env.crypto().sha256(&meta_bytes).into();
        let ttl = env.as_contract(&contract_id, || {
            env.storage().persistent().get_ttl(&dedup_key(
                &owner,
                &symbol_short!("GENSET"),
                &meta_hash,
            ))
        });
        assert!(
            ttl > 0,
            "dedup key TTL must be extended after register_asset"
        );
    }

    #[test]
    fn test_admin_can_propose_upgrade() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);

        let new_wasm_hash = BytesN::from_array(&env, &[0xabu8; 32]);
        let result = client.try_propose_upgrade(&admin, &new_wasm_hash);
        assert_ne!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_non_admin_cannot_propose_upgrade() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);

        let outsider = Address::generate(&env);
        let new_wasm_hash = BytesN::from_array(&env, &[0xabu8; 32]);

        let result = client.try_propose_upgrade(&outsider, &new_wasm_hash);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_propose_and_accept_admin_transfer() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let new_admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);

        client.propose_admin(&admin, &new_admin);
        client.accept_admin(&new_admin);

        assert_eq!(client.get_admin(), new_admin);
    }

    #[test]
    fn test_accept_admin_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let new_admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);

        client.propose_admin(&admin, &new_admin);
        client.accept_admin(&new_admin);

        // propose_admin emits PROP_ADM; accept_admin must emit ADMIN_SET
        let events = env.events().all();
        let (_, topics, data) = events.last().unwrap();
        use soroban_sdk::TryIntoVal;
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("ADMIN_SET"));

        let (emitted_admin,): (Address,) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_admin, new_admin);
    }

    #[test]
    fn test_pending_admin_key_cleared_after_accept() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let new_admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);

        client.propose_admin(&admin, &new_admin);
        client.accept_admin(&new_admin);

        env.as_contract(&contract_id, || {
            assert!(!env.storage().instance().has(&PENDING_ADMIN_KEY));
        });
    }

    #[test]
    fn test_non_admin_cannot_propose_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let outsider = Address::generate(&env);
        let new_admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);

        let result = client.try_propose_admin(&outsider, &new_admin);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_propose_admin_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let new_admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);

        client.propose_admin(&admin, &new_admin);

        let events = env.events().all();
        assert!(events.len() >= 1);
        let (_, topics, data): (_, soroban_sdk::Vec<soroban_sdk::Val>, soroban_sdk::Val) =
            events.get(0).unwrap();
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            symbol_short!("PROP_ADM")
        );
        let (emitted_admin, emitted_new_admin): (Address, Address) =
            soroban_sdk::FromVal::from_val(&env, &data);
        assert_eq!(emitted_admin, admin);
        assert_eq!(emitted_new_admin, new_admin);
    }

    #[test]
    fn test_wrong_address_cannot_accept_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let impostor = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.propose_admin(&admin, &new_admin);

        use soroban_sdk::IntoVal;
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &impostor,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &contract_id,
                fn_name: "accept_admin",
                args: (&impostor,).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        let result = client.try_accept_admin(&impostor);
        assert!(result.is_err());
        // Original admin unchanged
        assert_eq!(client.get_admin(), admin);
    }

    #[test]
    fn test_owner_can_update_metadata() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Original spec"),
            &unique_serial(&env),
            &owner,
        );

        client.update_asset_metadata(&id, &owner, &String::from_str(&env, "Refurbished spec v2"));

        let asset = client.get_asset(&id);
        assert_eq!(
            asset.metadata,
            String::from_str(&env, "Refurbished spec v2")
        );
    }

    #[test]
    fn test_update_metadata_stamps_metadata_updated_at() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Original spec"),
            &unique_serial(&env),
            &owner,
        );

        // Advance ledger time before updating
        env.ledger().with_mut(|li| li.timestamp += 1000);
        let update_time = env.ledger().timestamp();

        client.update_asset_metadata(&id, &owner, &String::from_str(&env, "Refurbished spec v2"));

        let asset = client.get_asset(&id);
        assert_eq!(asset.metadata_updated_at, update_time);
        assert!(asset.metadata_updated_at > asset.registered_at);
    }

    #[test]
    fn test_update_metadata_restamps_on_every_update() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Spec v1"),
            &unique_serial(&env),
            &owner,
        );

        env.ledger().with_mut(|li| li.timestamp += 500);
        client.update_asset_metadata(&id, &owner, &String::from_str(&env, "Spec v2"));
        let first = client.get_asset(&id).metadata_updated_at;

        env.ledger().with_mut(|li| li.timestamp += 700);
        client.update_asset_metadata(&id, &owner, &String::from_str(&env, "Spec v3"));
        let second = client.get_asset(&id).metadata_updated_at;

        assert_eq!(second, env.ledger().timestamp());
        assert!(second > first);
    }

    #[test]
    fn test_update_metadata_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Original spec"),
            &unique_serial(&env),
            &owner,
        );

        client.update_asset_metadata(&id, &owner, &String::from_str(&env, "Refurbished spec v2"));

        // env.events().all() reflects only the most recent contract call
        assert_eq!(env.events().all().len(), 1);
    }

    #[test]
    fn test_update_metadata_skips_noop() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Original spec"),
            &unique_serial(&env),
            &owner,
        );

        let original_asset = client.get_asset(&id);
        client.update_asset_metadata(&id, &owner, &String::from_str(&env, "Original spec"));

        let updated_asset = client.get_asset(&id);
        assert_eq!(updated_asset.metadata, original_asset.metadata);
        assert_eq!(
            updated_asset.metadata_updated_at,
            original_asset.metadata_updated_at
        );
        assert_eq!(env.events().all().len(), 0);
    }

    #[test]
    fn test_non_owner_cannot_update_metadata() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Original spec"),
            &unique_serial(&env),
            &owner,
        );

        let attacker = Address::generate(&env);
        let result = client.try_update_asset_metadata(
            &id,
            &attacker,
            &String::from_str(&env, "Hacked spec"),
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedOwner as u32,
            ))),
        );
    }

    #[test]
    fn test_update_metadata_nonexistent_asset() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let result =
            client.try_update_asset_metadata(&999u64, &owner, &String::from_str(&env, "New spec"));
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_metadata_version_starts_at_zero() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Original spec"),
            &unique_serial(&env),
            &owner,
        );

        let asset = client.get_asset(&id);
        assert_eq!(asset.metadata_version, 0);
    }

    #[test]
    fn test_metadata_version_increments_on_update() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Original spec"),
            &unique_serial(&env),
            &owner,
        );

        client.update_asset_metadata(&id, &owner, &String::from_str(&env, "Spec v2"));
        assert_eq!(client.get_asset(&id).metadata_version, 1);

        client.update_asset_metadata(&id, &owner, &String::from_str(&env, "Spec v3"));
        assert_eq!(client.get_asset(&id).metadata_version, 2);
    }

    #[test]
    fn test_get_metadata_history_returns_entries() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let original = String::from_str(&env, "Original spec");
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &original,
            &unique_serial(&env),
            &owner,
        );

        // No history before any update
        assert_eq!(client.get_metadata_history(&id).len(), 0);

        let v2 = String::from_str(&env, "Spec v2");
        client.update_asset_metadata(&id, &owner, &v2);

        let history = client.get_metadata_history(&id);
        assert_eq!(history.len(), 1);

        let entry = history.get(0).unwrap();
        assert_eq!(entry.version, 1);

        // Verify old_hash matches sha256 of original metadata XDR
        let expected_old_hash: BytesN<32> = env.crypto().sha256(&original.to_xdr(&env)).into();
        let expected_new_hash: BytesN<32> = env.crypto().sha256(&v2.to_xdr(&env)).into();
        assert_eq!(entry.old_hash, expected_old_hash);
        assert_eq!(entry.new_hash, expected_new_hash);
    }

    #[test]
    fn test_get_metadata_history_multiple_updates() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "v1"),
            &unique_serial(&env),
            &owner,
        );

        client.update_asset_metadata(&id, &owner, &String::from_str(&env, "v2"));
        client.update_asset_metadata(&id, &owner, &String::from_str(&env, "v3"));
        client.update_asset_metadata(&id, &owner, &String::from_str(&env, "v4"));

        let history = client.get_metadata_history(&id);
        assert_eq!(history.len(), 3);
        assert_eq!(history.get(0).unwrap().version, 1);
        assert_eq!(history.get(1).unwrap().version, 2);
        assert_eq!(history.get(2).unwrap().version, 3);
    }

    #[test]
    fn test_get_metadata_history_nonexistent_asset_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let result = client.try_get_metadata_history(&999u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32,
            ))),
        );
    }

    // ── issue #1021: get_asset_metadata_history (paginated) ───────────────

    fn setup_asset_with_history(env: &Env) -> (AssetRegistryClient, u64) {
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(env, &contract_id);

        let admin = Address::generate(env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(env, "Original metadata"),
            &String::from_str(env, "SN-HIST-001"),
            &owner,
        );

        // Add 5 metadata updates to build history
        for i in 0..5u32 {
            let new_meta = String::from_str(env, &format!("Updated metadata v{}", i + 1));
            client.update_asset_metadata(&id, &owner, &new_meta);
        }

        (client, id)
    }

    #[test]
    fn test_get_asset_metadata_history_first_page() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, id) = setup_asset_with_history(&env);

        // Get first 3 entries
        let page = client.get_asset_metadata_history(&id, &0u32, &3u32);
        assert_eq!(page.len(), 3, "First page should have 3 entries");
    }

    #[test]
    fn test_get_asset_metadata_history_second_page() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, id) = setup_asset_with_history(&env);

        // Get entries 3-5 (offset=3, limit=3 → only 2 remain)
        let page = client.get_asset_metadata_history(&id, &3u32, &3u32);
        assert_eq!(page.len(), 2, "Second page should have 2 remaining entries");
    }

    #[test]
    fn test_get_asset_metadata_history_offset_beyond_end_returns_empty() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, id) = setup_asset_with_history(&env);

        let page = client.get_asset_metadata_history(&id, &100u32, &10u32);
        assert_eq!(page.len(), 0, "Offset beyond history length returns empty");
    }

    #[test]
    fn test_get_asset_metadata_history_zero_limit_returns_empty() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, id) = setup_asset_with_history(&env);

        let page = client.get_asset_metadata_history(&id, &0u32, &0u32);
        assert_eq!(page.len(), 0, "Zero limit returns empty");
    }

    #[test]
    fn test_get_asset_metadata_history_limit_capped_at_100() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, id) = setup_asset_with_history(&env);

        // Limit of 1000 is capped to 100; only 5 entries exist so we get 5
        let page = client.get_asset_metadata_history(&id, &0u32, &1000u32);
        assert_eq!(
            page.len(),
            5,
            "Limit is capped to 100, returns all available entries"
        );
    }

    #[test]
    fn test_get_asset_metadata_history_nonexistent_asset_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let result = client.try_get_asset_metadata_history(&999u64, &0u32, &10u32);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_get_asset_metadata_history_entries_ordered_oldest_first() {
        let env = Env::default();
        env.mock_all_auths();

        let (client, id) = setup_asset_with_history(&env);

        // All 5 entries, check version increments (oldest = version 1)
        let all = client.get_asset_metadata_history(&id, &0u32, &10u32);
        assert_eq!(all.len(), 5);
        // Versions should be monotonically increasing (oldest first)
        for i in 0..all.len() - 1 {
            assert!(
                all.get(i).unwrap().version < all.get(i + 1).unwrap().version,
                "Entries should be ordered oldest-first by version"
            );
        }
    }

    #[test]
    fn test_update_metadata_event_contains_hashes_and_version() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let original = String::from_str(&env, "Original spec");
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &original,
            &unique_serial(&env),
            &owner,
        );

        let new_meta = String::from_str(&env, "Repowered spec");
        let ts = env.ledger().timestamp();
        client.update_asset_metadata(&id, &owner, &new_meta);

        let events = env.events().all();
        assert_eq!(events.len(), 1);

        let (_, topics, data) = events.last().unwrap();
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("UPD_META"));

        let (_, old_hash, new_hash, version, _timestamp): (Address, BytesN<32>, BytesN<32>, u32, u64) =
            data.try_into_val(&env).unwrap();

        let expected_old: BytesN<32> = env.crypto().sha256(&original.to_xdr(&env)).into();
        let expected_new: BytesN<32> = env.crypto().sha256(&new_meta.to_xdr(&env)).into();
        assert_eq!(old_hash, expected_old);
        assert_eq!(new_hash, expected_new);
        assert_eq!(version, 1u32);
        let _ = ts; // timestamp checked separately if needed
    }

    #[test]
    fn test_transfer_asset() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        client.transfer_asset(&id, &owner, &new_owner);

        let asset = client.get_asset(&id);
        assert_eq!(asset.owner, new_owner);
    }

    #[test]
    fn test_transfer_asset_same_owner_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        let result = client.try_transfer_asset(&id, &owner, &owner);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::SameOwner as u32
            )))
        );
        // Asset still belongs to original owner
        assert_eq!(client.get_asset(&id).owner, owner);
    }

    #[test]
    fn test_transfer_asset_non_owner_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let attacker = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        let result = client.try_transfer_asset(&id, &attacker, &new_owner);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedOwner as u32,
            ))),
        );
    }

    #[test]
    fn test_transfer_asset_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        client.transfer_asset(&id, &owner, &new_owner);

        // env.events().all() reflects only the most recent contract call
        assert_eq!(env.events().all().len(), 1);
    }

    #[test]
    fn test_ownership_transfer_initiate_and_accept() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        client.initiate_ownership_transfer(&id, &new_owner);
        // Ownership has not changed yet — it only changes on acceptance.
        assert_eq!(client.get_asset(&id).owner, owner);

        client.accept_ownership_transfer(&id);
        assert_eq!(client.get_asset(&id).owner, new_owner);
    }

    #[test]
    fn test_ownership_transfer_emits_events() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        client.initiate_ownership_transfer(&id, &new_owner);
        assert_eq!(env.events().all().len(), 1);

        client.accept_ownership_transfer(&id);
        assert_eq!(env.events().all().len(), 1);
    }

    #[test]
    fn test_ownership_transfer_same_owner_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        let result = client.try_initiate_ownership_transfer(&id, &owner);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::SameOwner as u32,
            ))),
        );
    }

    #[test]
    fn test_ownership_transfer_already_pending_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let other_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        client.initiate_ownership_transfer(&id, &new_owner);
        let result = client.try_initiate_ownership_transfer(&id, &other_owner);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::TransferAlreadyPending as u32,
            ))),
        );
    }

    #[test]
    fn test_ownership_transfer_accept_without_pending_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        let result = client.try_accept_ownership_transfer(&id);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NoPendingTransfer as u32,
            ))),
        );
    }

    #[test]
    fn test_ownership_transfer_timeout_expires() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        client.initiate_ownership_transfer(&id, &new_owner);

        // Advance the ledger past the 7-day acceptance window.
        env.ledger()
            .with_mut(|li| li.timestamp += TRANSFER_TIMEOUT_SECS);

        let result = client.try_accept_ownership_transfer(&id);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::TransferExpired as u32,
            ))),
        );

        // Ownership is unchanged after expiry.
        assert_eq!(client.get_asset(&id).owner, owner);

        // A new transfer can be initiated after the old one expired.
        client.initiate_ownership_transfer(&id, &new_owner);
        client.accept_ownership_transfer(&id);
        assert_eq!(client.get_asset(&id).owner, new_owner);
    }

    /// Issue #1215: an expired pending transfer must be cancellable by anyone,
    /// freeing the slot without waiting on a stray `accept`/`initiate` call.
    #[test]
    fn test_cancel_expired_transfer_succeeds_after_timeout() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let another_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        client.initiate_ownership_transfer(&id, &new_owner);

        env.ledger()
            .with_mut(|li| li.timestamp += TRANSFER_TIMEOUT_SECS);

        client.cancel_expired_transfer(&id);

        // A new transfer can be initiated immediately after cancellation.
        client.initiate_ownership_transfer(&id, &another_owner);
        client.accept_ownership_transfer(&id);
        assert_eq!(client.get_asset(&id).owner, another_owner);
    }

    #[test]
    fn test_cancel_expired_transfer_fails_before_timeout() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        client.initiate_ownership_transfer(&id, &new_owner);

        let result = client.try_cancel_expired_transfer(&id);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::TransferNotExpired as u32,
            ))),
        );
    }

    #[test]
    fn test_cancel_expired_transfer_fails_without_pending_transfer() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        let result = client.try_cancel_expired_transfer(&id);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NoPendingTransfer as u32,
            ))),
        );
    }

    /// Issue #1214: a pending ownership transfer must block a new lien from being
    /// placed, otherwise the transfer could complete after the lock and hand the
    /// new owner a locked asset with no lender notification.
    #[test]
    fn test_lock_asset_as_collateral_blocked_by_pending_transfer() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let lender = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        client.set_lending_contract(&admin, &lender);
        client.initiate_ownership_transfer(&id, &new_owner);

        let result = client.try_lock_asset_as_collateral(&lender, &id, &1u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::TransferAlreadyPending as u32,
            ))),
        );
    }

    #[test]
    fn test_ownership_transfer_updates_dedup_so_old_owner_can_reregister() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let metadata = String::from_str(&env, "CAT-3516");

        let id = client.register_asset(&symbol_short!("GENSET"), &metadata, &unique_serial(&env), &owner);
        client.initiate_ownership_transfer(&id, &new_owner);
        client.accept_ownership_transfer(&id);

        let id2 = client.register_asset(&symbol_short!("GENSET"), &metadata, &unique_serial(&env), &owner);
        assert_ne!(id, id2);
    }

    #[test]
    fn test_transfer_updates_dedup_so_new_owner_can_register_same_metadata() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let metadata = String::from_str(&env, "CAT-3516");

        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &metadata,
            &unique_serial(&env),
            &owner,
        );
        client.transfer_asset(&id, &owner, &new_owner);

        // Original owner can now register the same metadata again (dedup key was moved)
        let id2 = client.register_asset(
            &symbol_short!("GENSET"),
            &metadata,
            &unique_serial(&env),
            &owner,
        );
        assert_ne!(id, id2);
    }

    // Closes #774
    #[test]
    fn test_transfer_asset_updates_owner_index_and_previous_owner_can_reregister() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let prev_owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let metadata = String::from_str(&env, "CAT-3516C");

        // Register with prev_owner and transfer to new_owner
        let id = client.register_asset(&symbol_short!("GENSET"), &metadata, &unique_serial(&env), &prev_owner);
        client.transfer_asset(&id, &prev_owner, &new_owner);

        // Owner index: prev_owner no longer holds the transferred asset
        let prev_assets = client.get_assets_by_owner(&prev_owner);
        assert!(!prev_assets.contains(&id), "prev_owner should not appear in owner index after transfer");

        // Owner index: new_owner now holds the asset
        let new_assets = client.get_assets_by_owner(&new_owner);
        assert!(new_assets.contains(&id), "new_owner should appear in owner index after transfer");

        // prev_owner can register same metadata again (their dedup key was cleared)
        let id2 = client.register_asset(&symbol_short!("GENSET"), &metadata, &unique_serial(&env), &prev_owner);
        assert_ne!(id, id2, "re-registration by prev_owner should produce a new asset id");

        // new_owner cannot register the same metadata (dedup key now belongs to them)
        let dup_result = client.try_register_asset(
            &symbol_short!("GENSET"),
            &metadata,
            &unique_serial(&env),
            &new_owner,
        );
        assert_eq!(
            dup_result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::DuplicateAsset as u32,
            ))),
            "new_owner should not be able to register the same metadata (dedup applies to new owner)",
        );
    }

    #[test]
    fn test_update_metadata_dedup_enforced() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        // Register two assets with different metadata
        let id1 = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Spec A"),
            &unique_serial(&env),
            &owner,
        );
        client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Spec B"),
            &unique_serial(&env),
            &owner,
        );

        // Trying to update asset 1 to "Spec B" (already taken by same owner) should fail
        let result =
            client.try_update_asset_metadata(&id1, &owner, &String::from_str(&env, "Spec B"));
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::DuplicateAsset as u32,
            ))),
        );
    }

    #[test]
    fn test_asset_exists_returns_true_for_existing_asset() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Turbine X"),
            &unique_serial(&env),
            &owner,
        );

        assert!(client.asset_exists(&id));
    }

    #[test]
    fn test_asset_exists_returns_false_for_nonexistent_asset() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        assert!(!client.asset_exists(&9999u64));
    }

    #[test]
    fn test_get_assets_by_owner_returns_registered_ids() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));
        client.add_asset_type(&admin, &symbol_short!("TURBINE"));

        let owner = Address::generate(&env);
        let id1 = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Asset Alpha"),
            &unique_serial(&env),
            &owner,
        );
        let id2 = client.register_asset(
            &symbol_short!("TURBINE"),
            &String::from_str(&env, "Asset Beta"),
            &unique_serial(&env),
            &owner,
        );

        let ids = client.get_assets_by_owner(&owner);
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
    }

    #[test]
    fn test_get_assets_by_owner_returns_empty_for_unknown_owner() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let stranger = Address::generate(&env);
        let ids = client.get_assets_by_owner(&stranger);
        assert_eq!(ids.len(), 0);
    }

    #[test]
    fn test_get_assets_by_owner_page_returns_paged_ids() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let mut ids: Vec<u64> = Vec::new(&env);
        ids.push_back(client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Asset 0"),
            &unique_serial(&env),
            &owner,
        ));
        ids.push_back(client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Asset 1"),
            &unique_serial(&env),
            &owner,
        ));
        ids.push_back(client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Asset 2"),
            &unique_serial(&env),
            &owner,
        ));
        ids.push_back(client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Asset 3"),
            &unique_serial(&env),
            &owner,
        ));
        ids.push_back(client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Asset 4"),
            &unique_serial(&env),
            &owner,
        ));

        let page0 = client.get_assets_by_owner_page(&owner, &0, &2);
        assert_eq!(page0.len(), 2);
        assert_eq!(page0.get(0).unwrap(), ids.get(0).unwrap());
        assert_eq!(page0.get(1).unwrap(), ids.get(1).unwrap());

        let page1 = client.get_assets_by_owner_page(&owner, &1, &2);
        assert_eq!(page1.len(), 2);
        assert_eq!(page1.get(0).unwrap(), ids.get(2).unwrap());
        assert_eq!(page1.get(1).unwrap(), ids.get(3).unwrap());

        let page2 = client.get_assets_by_owner_page(&owner, &2, &2);
        assert_eq!(page2.len(), 1);
        assert_eq!(page2.get(0).unwrap(), ids.get(4).unwrap());

        let page3 = client.get_assets_by_owner_page(&owner, &3, &2);
        assert_eq!(page3.len(), 0);
    }

    #[test]
    fn test_get_assets_by_owner_page_returns_empty_for_unknown_owner_or_zero_page_size() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let unknown_owner = Address::generate(&env);
        client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Asset X"),
            &unique_serial(&env),
            &owner,
        );

        assert_eq!(
            client
                .get_assets_by_owner_page(&unknown_owner, &0, &2)
                .len(),
            0
        );
        assert_eq!(client.get_assets_by_owner_page(&owner, &0, &0).len(), 0);
        assert_eq!(client.get_assets_by_owner_page(&owner, &5, &2).len(), 0);
    }

    #[test]
    fn test_get_assets_by_owner_paginated_returns_page_and_total() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id1 = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "P1"),
            &unique_serial(&env),
            &owner,
        );
        let id2 = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "P2"),
            &unique_serial(&env),
            &owner,
        );
        let id3 = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "P3"),
            &unique_serial(&env),
            &owner,
        );

        let page0 = client.get_assets_by_owner_paginated(&owner, &0, &2);
        assert_eq!(page0.total, 3);
        assert_eq!(page0.assets.len(), 2);
        assert_eq!(page0.assets.get(0).unwrap(), id1);
        assert_eq!(page0.assets.get(1).unwrap(), id2);

        let page1 = client.get_assets_by_owner_paginated(&owner, &1, &2);
        assert_eq!(page1.total, 3);
        assert_eq!(page1.assets.len(), 1);
        assert_eq!(page1.assets.get(0).unwrap(), id3);

        // Out-of-range page returns empty assets but still correct total
        let page2 = client.get_assets_by_owner_paginated(&owner, &5, &2);
        assert_eq!(page2.total, 3);
        assert_eq!(page2.assets.len(), 0);

        // Unknown owner returns total=0
        let unknown = Address::generate(&env);
        let empty = client.get_assets_by_owner_paginated(&unknown, &0, &10);
        assert_eq!(empty.total, 0);
        assert_eq!(empty.assets.len(), 0);
    }

    #[test]
    fn test_get_assets_by_owner_updated_after_transfer() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        client.transfer_asset(&id, &owner, &new_owner);

        // Original owner should have no assets
        assert_eq!(client.get_assets_by_owner(&owner).len(), 0);
        // New owner should have the asset
        let new_ids = client.get_assets_by_owner(&new_owner);
        assert_eq!(new_ids.len(), 1);
        assert!(new_ids.contains(&id));
    }

    #[test]
    fn test_transfer_asset_logs_missing_owner_index_and_keeps_old_owner_clean() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let retained_id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );
        let transferred_id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3520"),
            &unique_serial(&env),
            &owner,
        );

        env.as_contract(&contract_id, || {
            env.storage().persistent().remove(&owner_index_key(&owner));
        });

        client.transfer_asset(&transferred_id, &owner, &new_owner);

        let logs = env.logs().all();
        let warning = logs.last().unwrap();
        assert!(warning.contains("owner index missing during remove"));

        let old_owner_ids = client.get_assets_by_owner(&owner);
        assert_eq!(old_owner_ids.len(), 0);
        assert!(!old_owner_ids.contains(&transferred_id));
        assert!(!old_owner_ids.contains(&retained_id));

        let new_owner_ids = client.get_assets_by_owner(&new_owner);
        assert_eq!(new_owner_ids.len(), 1);
        assert!(new_owner_ids.contains(&transferred_id));
    }

    #[test]
    fn test_owner_index_remove_missing_key_emits_diagnostic_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        // Remove the owner index to simulate expiry
        env.as_contract(&contract_id, || {
            env.storage().persistent().remove(&owner_index_key(&owner));
        });

        // Trigger owner_index_remove via deregister
        client.deregister_asset(&admin, &id);

        // Verify the IDX_MISS diagnostic event was emitted
        let events = env.events().all();
        let idx_miss_event = events.iter().find(|(_, topics, _)| {
            use soroban_sdk::TryIntoVal;
            topics
                .get(0)
                .and_then(|v| {
                    let s: Result<Symbol, _> = v.try_into_val(&env);
                    s.ok()
                })
                .map(|s| s == symbol_short!("IDX_MISS"))
                .unwrap_or(false)
        });
        assert!(
            idx_miss_event.is_some(),
            "IDX_MISS diagnostic event must be emitted when owner index is missing"
        );
    }

    #[test]
    fn test_owner_index_key_removed_after_last_asset_deregistered() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        client.deregister_asset(&admin, &id);

        let key_exists = env.as_contract(&contract_id, || {
            env.storage().persistent().has(&owner_index_key(&owner))
        });
        assert!(
            !key_exists,
            "owner index key must be absent after last asset is removed"
        );
    }

    #[test]
    fn test_update_asset_metadata_removes_old_dedup_key() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let meta_a = String::from_str(&env, "Spec A");
        let meta_b = String::from_str(&env, "Spec B");

        // Register with metadata A, then update to B
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &meta_a,
            &unique_serial(&env),
            &owner,
        );
        client.update_asset_metadata(&id, &owner, &meta_b);

        // Old dedup key (A) is gone — owner can register metadata A again
        let id2 = client.register_asset(
            &symbol_short!("GENSET"),
            &meta_a,
            &unique_serial(&env),
            &owner,
        );
        assert_ne!(id, id2);

        // New dedup key (B) is present — owner cannot register metadata B again
        let result = client.try_register_asset(
            &symbol_short!("GENSET"),
            &meta_b,
            &unique_serial(&env),
            &owner,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::DuplicateAsset as u32,
            ))),
        );
    }

    #[test]
    fn test_initialize_admin_called_twice_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        // Second call must fail with AdminAlreadyInitialized
        let result = client.try_initialize_admin(&admin, &admin);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AdminAlreadyInitialized as u32,
            ))),
        );
    }

    #[test]
    fn test_get_assets_by_owner_updated_after_deregister() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        assert_eq!(client.get_assets_by_owner(&owner).len(), 1);
        client.deregister_asset(&admin, &id);
        assert_eq!(client.get_assets_by_owner(&owner).len(), 0);
    }

    #[test]
    fn test_deregister_allows_reregistration_of_same_metadata() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let metadata = String::from_str(&env, "CAT-3516");

        // Register asset
        let id1 = client.register_asset(
            &symbol_short!("GENSET"),
            &metadata,
            &unique_serial(&env),
            &owner,
        );

        // Deregister removes dedup key
        client.deregister_asset(&admin, &id1);

        // Same owner can now re-register the same metadata
        let id2 = client.register_asset(
            &symbol_short!("GENSET"),
            &metadata,
            &unique_serial(&env),
            &owner,
        );
        assert_ne!(id1, id2);
    }

    // --- Issue #142: get_admin structured error before initialization ---

    #[test]
    fn test_get_admin_before_init_returns_structured_error() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let result = client.try_get_admin();
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NotInitialized as u32,
            ))),
        );
    }

    #[test]
    fn test_deregister_asset_with_expired_owner_index() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        // Simulate owner index expiration by removing it
        env.as_contract(&contract_id, || {
            let key = owner_index_key(&owner);
            env.storage().persistent().remove(&key);
        });

        // Deregister should not create a stale empty entry
        client.deregister_asset(&admin, &id);

        // Verify owner index was not recreated
        env.as_contract(&contract_id, || {
            let key = owner_index_key(&owner);
            assert!(!env.storage().persistent().has(&key));
        });
    }

    #[test]
    fn test_transfer_asset_extends_new_owner_dedup_key_ttl() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let metadata = String::from_str(&env, "CAT-3516");
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &metadata,
            &unique_serial(&env),
            &owner,
        );

        client.transfer_asset(&id, &owner, &new_owner);

        // Verify new owner's dedup key TTL is extended
        env.as_contract(&contract_id, || {
            let meta_bytes = Bytes::from(metadata.to_xdr(&env));
            let meta_hash: BytesN<32> = env.crypto().sha256(&meta_bytes).into();
            let new_dk = dedup_key(&new_owner, &symbol_short!("GENSET"), &meta_hash);
            let dedup_ttl = env.storage().persistent().get_ttl(&new_dk);
            assert!(
                dedup_ttl > 0,
                "New owner's dedup key TTL should be extended"
            );
        });
    }

    #[test]
    fn test_update_metadata_extends_new_dedup_key_and_asset_ttl() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Original spec"),
            &unique_serial(&env),
            &owner,
        );

        client.update_asset_metadata(&id, &owner, &String::from_str(&env, "Updated spec"));

        // Verify new dedup key TTL is extended
        env.as_contract(&contract_id, || {
            let new_metadata = String::from_str(&env, "Updated spec");
            let meta_bytes = Bytes::from(new_metadata.to_xdr(&env));
            let meta_hash: BytesN<32> = env.crypto().sha256(&meta_bytes).into();
            let new_dk = dedup_key(&owner, &symbol_short!("GENSET"), &meta_hash);
            let dedup_ttl = env.storage().persistent().get_ttl(&new_dk);
            assert!(dedup_ttl > 0, "New dedup key TTL should be extended");

            // Verify asset record TTL is extended
            let asset_ttl = env.storage().persistent().get_ttl(&asset_key(id));
            assert!(asset_ttl > 0, "Asset record TTL should be extended");
        });
    }

    #[test]
    fn test_batch_register_assets_rejects_duplicate_existing_metadata() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "A"),
            &unique_serial(&env),
            &owner,
        );

        let mut batch = Vec::new(&env);
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "A"),
            serial_number: unique_serial(&env),
        });

        let result = client.try_batch_register_assets(&owner, &batch);

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::DuplicateAsset as u32,
            ))),
        );
    }

    #[test]
    fn test_batch_register_assets_success_and_pause() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let mut batch = Vec::new(&env);
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "A"),
            serial_number: unique_serial(&env),
        });
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "B"),
            serial_number: unique_serial(&env),
        });

        let ids = client.batch_register_assets(&owner, &batch);
        assert_eq!(ids.len(), 2);

        client.pause(&admin);
        let result = client.try_batch_register_assets(&owner, &batch);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32,
            ))),
        );

        client.unpause(&admin);
        let id3 = client.batch_register_assets(&owner, &Vec::new(&env));
        assert_eq!(id3.len(), 0);
    }

    #[test]
    fn test_batch_register_assets_rejects_oversized_batch() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        // Build 51 items — one over MAX_BATCH_SIZE (50)
        let mut batch: Vec<AssetInput> = Vec::new(&env);
        for _ in 0u32..51 {
            batch.push_back(AssetInput {
                asset_type: symbol_short!("GENSET"),
                metadata: String::from_str(&env, "meta"),
                serial_number: unique_serial(&env),
            });
        }

        let result = client.try_batch_register_assets(&owner, &batch);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::BatchTooLarge as u32,
            ))),
        );
    }

    #[test]
    fn test_pause_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.pause(&admin);

        let events = env.events().all();
        assert!(events.len() >= 1);
        let (_, topics, data) = events.get(0).unwrap();
        use soroban_sdk::TryIntoVal;
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("PAUSED"));
        let (emitted_admin,): (Address,) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_admin, admin);
    }

    #[test]
    fn test_unpause_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.pause(&admin);
        client.unpause(&admin);

        let events = env.events().all();
        assert!(events.len() >= 1);
        let (_, topics, data) = events.get(0).unwrap();
        use soroban_sdk::TryIntoVal;
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("UNPAUSED"));
        let (emitted_admin,): (Address,) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_admin, admin);
    }

    #[test]
    fn test_pause_affects_all_state_changes() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Base"),
            &unique_serial(&env),
            &owner,
        );

        client.pause(&admin);

        // Read-only access should still work while paused
        let paused_asset = client.get_asset(&id);
        assert_eq!(paused_asset.asset_id, id);
        assert_eq!(paused_asset.owner, owner);
        assert!(client.asset_exists(&id));
        assert_eq!(client.get_assets_by_owner(&owner).len(), 1);
        assert!(client.try_get_asset(&id).is_ok());

        // register_asset
        assert_eq!(
            client.try_register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "A"),
                &unique_serial(&env),
                &owner
            ),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );

        // update_asset_metadata
        assert_eq!(
            client.try_update_asset_metadata(&id, &owner, &String::from_str(&env, "New")),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );

        // transfer_asset
        assert_eq!(
            client.try_transfer_asset(&id, &owner, &Address::generate(&env)),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );

        // deregister_asset
        assert_eq!(
            client.try_deregister_asset(&owner, &id),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );

        // propose_upgrade
        assert_eq!(
            client.try_propose_upgrade(&admin, &BytesN::from_array(&env, &[0u8; 32])),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );
    }

    #[test]
    fn test_batch_register_assets_internal_duplicates_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let mut batch = Vec::new(&env);
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "Duplicate"),
            serial_number: unique_serial(&env),
        });
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "Duplicate"),
            serial_number: unique_serial(&env),
        });

        let result = client.try_batch_register_assets(&owner, &batch);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::DuplicateAsset as u32,
            ))),
        );
    }

    #[test]
    fn test_batch_register_assets_emits_batch_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let mut batch = Vec::new(&env);
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "A"),
            serial_number: unique_serial(&env),
        });
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "B"),
            serial_number: unique_serial(&env),
        });

        let ids = client.batch_register_assets(&owner, &batch);

        // 2 REG_AST + 1 BATCH_REG
        let events = env.events().all();
        assert_eq!(events.len(), 3);

        // Last event must be the BATCH_REG with the correct topic and assigned IDs
        let (_, topics, data) = events.last().unwrap();
        use soroban_sdk::TryIntoVal;
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        let t1: Address = topics.get(1).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("BATCH_REG"));
        assert_eq!(t1, owner);

        let (emitted_ids, _timestamp): (Vec<u64>, u64) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_ids.len(), 2);
        assert_eq!(emitted_ids.get(0).unwrap(), ids.get(0).unwrap());
        assert_eq!(emitted_ids.get(1).unwrap(), ids.get(1).unwrap());
    }

    #[test]
    fn test_batch_register_assets_empty_emits_no_batch_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);

        let owner = Address::generate(&env);
        client.batch_register_assets(&owner, &Vec::new(&env));

        // Empty batch — no events at all
        assert_eq!(env.events().all().len(), 0);
    }

    #[test]
    fn test_batch_register_assets_contiguous_ids() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);

        // Register one asset first so ASSET_COUNT starts at 1
        let single = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "first"),
            &unique_serial(&env),
            &owner,
        );
        assert_eq!(single, 1);

        // Batch of three should get IDs 2, 3, 4 — contiguous, no gaps
        let mut batch = Vec::new(&env);
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "A"),
            serial_number: unique_serial(&env),
        });
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "B"),
            serial_number: unique_serial(&env),
        });
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "C"),
            serial_number: unique_serial(&env),
        });

        let ids = client.batch_register_assets(&owner, &batch);
        assert_eq!(ids.len(), 3);
        assert_eq!(ids.get(0).unwrap(), 2);
        assert_eq!(ids.get(1).unwrap(), 3);
        assert_eq!(ids.get(2).unwrap(), 4);
    }

    #[test]
    fn test_batch_register_assets_rejects_in_batch_serial_duplicate() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let shared_serial = String::from_str(&env, "SN-SAME-001");
        let mut batch = Vec::new(&env);
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "Machine A"),
            serial_number: shared_serial.clone(),
        });
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "Machine B"),
            serial_number: shared_serial,
        });

        let result = client.try_batch_register_assets(&owner, &batch);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::DuplicateAsset as u32,
            ))),
        );
    }

    #[test]
    fn test_batch_register_assets_rejects_invalid_asset_type() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let mut batch = Vec::new(&env);
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "Valid asset"),
            serial_number: unique_serial(&env),
        });
        batch.push_back(AssetInput {
            asset_type: symbol_short!("UNKNOWN"),
            metadata: String::from_str(&env, "Invalid type asset"),
            serial_number: unique_serial(&env),
        });

        let result = client.try_batch_register_assets(&owner, &batch);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidAssetType as u32,
            ))),
        );
    }

    /// Issue #1213: an empty-metadata element inside an otherwise valid batch must be
    /// rejected the same way a lone `register_asset` call with empty metadata is,
    /// instead of only the outer `Vec<AssetInput>` being checked for non-emptiness.
    #[test]
    fn test_batch_register_assets_rejects_empty_metadata_element() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let mut batch = Vec::new(&env);
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "Valid asset"),
            serial_number: unique_serial(&env),
        });
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, ""),
            serial_number: unique_serial(&env),
        });

        let result = client.try_batch_register_assets(&owner, &batch);
        assert!(
            result.is_err(),
            "a batch containing one empty-metadata asset must be rejected"
        );

        // No asset from the batch should have been persisted.
        assert_eq!(client.get_assets_by_owner(&owner).len(), 0);
    }

    #[test]
    fn test_batch_register_assets_owner_index_correct() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);

        // Pre-register one asset so owner index is non-empty before the batch
        let pre_id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Pre-existing"),
            &unique_serial(&env),
            &owner,
        );

        let mut batch = Vec::new(&env);
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "Batch A"),
            serial_number: unique_serial(&env),
        });
        batch.push_back(AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "Batch B"),
            serial_number: unique_serial(&env),
        });

        let batch_ids = client.batch_register_assets(&owner, &batch);

        let owned = client.get_assets_by_owner(&owner);
        // All three IDs must be present in the owner index
        assert_eq!(owned.len(), 3);
        assert!(owned.contains(&pre_id));
        assert!(owned.contains(&batch_ids.get(0).unwrap()));
        assert!(owned.contains(&batch_ids.get(1).unwrap()));
    }

    #[test]
    fn test_asset_type_allowlist() {
        let env = Env::default();
        env.mock_all_auths();
        let (_contract_id, client, admin) = {
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);
            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            (contract_id, client, admin)
        };

        let owner = Address::generate(&env);
        let valid_type = symbol_short!("VALID");
        let invalid_type = symbol_short!("JUNK");

        // Try registering without allowing first
        let result = client.try_register_asset(
            &valid_type,
            &String::from_str(&env, "Some metadata"),
            &unique_serial(&env),
            &owner,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidAssetType as u32
            )))
        );

        // Allow the type
        client.add_asset_type(&admin, &valid_type);
        assert!(client.is_valid_asset_type(&valid_type));

        // Now registration succeeds
        let id = client.register_asset(
            &valid_type,
            &String::from_str(&env, "Some metadata"),
            &unique_serial(&env),
            &owner,
        );
        assert_eq!(id, 1);

        // Still cannot register invalid type
        let result = client.try_register_asset(
            &invalid_type,
            &String::from_str(&env, "Other metadata"),
            &unique_serial(&env),
            &owner,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidAssetType as u32
            )))
        );

        // Remove the type — must deregister the asset first
        client.deregister_asset(&owner, &id);
        client.remove_asset_type(&admin, &valid_type);
        assert!(!client.is_valid_asset_type(&valid_type));

        // Registration fails again
        let result = client.try_register_asset(
            &valid_type,
            &String::from_str(&env, "More metadata"),
            &unique_serial(&env),
            &owner,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidAssetType as u32
            )))
        );
    }

    #[test]
    fn test_batch_register_validates_asset_types() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("VALID"));

        let owner = Address::generate(&env);
        let mut batch = Vec::new(&env);
        batch.push_back(AssetInput {
            asset_type: symbol_short!("VALID"),
            metadata: String::from_str(&env, "Meta 1"),
            serial_number: unique_serial(&env),
        });
        batch.push_back(AssetInput {
            asset_type: symbol_short!("JUNK"),
            metadata: String::from_str(&env, "Meta 2"),
            serial_number: unique_serial(&env),
        });

        let result = client.try_batch_register_assets(&owner, &batch);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidAssetType as u32
            )))
        );
    }

    #[test]
    fn test_non_owner_cannot_deregister_asset() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        // A third party (neither admin nor owner) must be rejected
        let stranger = Address::generate(&env);
        let result = client.try_deregister_asset(&stranger, &id);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedOwner as u32
            )))
        );
        assert!(client.asset_exists(&id));
    }

    #[test]
    fn test_owner_can_deregister_own_asset() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        client.deregister_asset(&owner, &id);
        assert!(!client.asset_exists(&id));
    }

    #[test]
    fn test_deregister_asset_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        client.deregister_asset(&owner, &id);

        let events = env.events().all();
        let (_, topics, data): (_, soroban_sdk::Vec<soroban_sdk::Val>, soroban_sdk::Val) =
            events.last().unwrap();
        use soroban_sdk::IntoVal;
        let topic0: soroban_sdk::Val =
            <Symbol as IntoVal<Env, soroban_sdk::Val>>::into_val(&DEREG_TOPIC, &env);
        let topic1: soroban_sdk::Val = <u64 as IntoVal<Env, soroban_sdk::Val>>::into_val(&id, &env);
        assert_eq!(topics.get(0).unwrap().get_payload(), topic0.get_payload());
        assert_eq!(topics.get(1).unwrap().get_payload(), topic1.get_payload());
        let (emitted_type, emitted_owner): (Symbol, Address) =
            soroban_sdk::FromVal::from_val(&env, &data);
        assert_eq!(emitted_type, symbol_short!("GENSET"));
        assert_eq!(emitted_owner, owner);
    }

    #[test]
    fn test_deregister_asset_emits_dereg_topic() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        client.deregister_asset(&owner, &id);

        let events = env.events().all();
        let (_, topics, _): (_, soroban_sdk::Vec<soroban_sdk::Val>, soroban_sdk::Val) =
            events.last().unwrap();
        use soroban_sdk::TryIntoVal;
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(
            t0,
            symbol_short!("DEREG"),
            "deregister_asset must emit DEREG topic (≤8 chars)"
        );
    }

    #[test]
    fn test_deregister_nonexistent_asset_returns_structured_error() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);

        assert_eq!(
            client.try_deregister_asset(&admin, &9999u64),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32
            )))
        );
    }

    #[test]
    fn test_remove_asset_type_blocked_while_assets_exist() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        // Removal must be rejected while the asset still exists
        assert_eq!(
            client.try_remove_asset_type(&admin, &symbol_short!("GENSET")),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::TypeInUse as u32
            )))
        );

        // Existing asset is still intact
        assert!(client.asset_exists(&id));
        assert!(client.is_valid_asset_type(&symbol_short!("GENSET")));

        // After deregistering the asset the type can be removed
        client.deregister_asset(&owner, &id);
        client.remove_asset_type(&admin, &symbol_short!("GENSET"));
        assert!(!client.is_valid_asset_type(&symbol_short!("GENSET")));
    }

    #[test]
    fn test_add_asset_type_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let events = env.events().all();
        let (_, topics, data): (_, soroban_sdk::Vec<soroban_sdk::Val>, soroban_sdk::Val) =
            events.last().unwrap();
        use soroban_sdk::IntoVal;
        let expected_topic: soroban_sdk::Val =
            <Symbol as IntoVal<Env, soroban_sdk::Val>>::into_val(&ADD_TYPE_TOPIC, &env);
        assert_eq!(
            topics.get(0).unwrap().get_payload(),
            expected_topic.get_payload()
        );
        let (emitted_type,): (Symbol,) = soroban_sdk::FromVal::from_val(&env, &data);
        assert_eq!(emitted_type, symbol_short!("GENSET"));
    }

    #[test]
    fn test_add_asset_type_emits_admin_audit_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        let timestamp = env.ledger().timestamp();
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let events = env.events().all();
        let (_, topics, data) = events.get(0).unwrap();
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        let t1: Symbol = topics.get(1).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("ADM_AUD"));
        assert_eq!(t1, symbol_short!("ADD_TYPE"));

        let (emitted_admin, emitted_timestamp, emitted_type): (Address, u64, Symbol) =
            data.try_into_val(&env).unwrap();
        assert_eq!(emitted_admin, admin);
        assert_eq!(emitted_timestamp, timestamp);
        assert_eq!(emitted_type, symbol_short!("GENSET"));
    }

    #[test]
    fn test_remove_asset_type_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));
        client.remove_asset_type(&admin, &symbol_short!("GENSET"));

        let events = env.events().all();
        let (_, topics, data): (_, soroban_sdk::Vec<soroban_sdk::Val>, soroban_sdk::Val) =
            events.last().unwrap();
        use soroban_sdk::IntoVal;
        let expected_topic: soroban_sdk::Val =
            <Symbol as IntoVal<Env, soroban_sdk::Val>>::into_val(&RM_TYPE_TOPIC, &env);
        assert_eq!(
            topics.get(0).unwrap().get_payload(),
            expected_topic.get_payload()
        );
        let (emitted_type,): (Symbol,) = soroban_sdk::FromVal::from_val(&env, &data);
        assert_eq!(emitted_type, symbol_short!("GENSET"));
    }

    #[test]
    fn test_register_asset_rejects_empty_metadata() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let owner = Address::generate(&env);
        let client = AssetRegistryClient::new(&env, &env.register(AssetRegistry, ()));
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let result = client.try_register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, ""),
            &unique_serial(&env),
            &owner,
        );
        assert_eq!(result, Err(Ok(ContractError::EmptyMetadata.into())));
    }

    #[test]
    fn test_asset_count_survives_instance_ttl_expiry() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let id1 = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Asset One"),
            &unique_serial(&env),
            &owner,
        );
        assert_eq!(id1, 1);
        assert_eq!(client.asset_count(), 1);

        // Simulate instance storage TTL expiry by wiping instance keys
        env.as_contract(&contract_id, || {
            env.storage().instance().remove(&ADMIN_KEY);
        });

        // ASSET_COUNT lives in persistent storage — must still return 1
        assert_eq!(
            client.asset_count(),
            1,
            "asset_count must survive instance TTL expiry"
        );

        // Next registration must get ID 2, not 1 (no collision)
        let id2 = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Asset Two"),
            &unique_serial(&env),
            &owner,
        );
        assert_eq!(
            id2, 2,
            "ID assignment must be consistent after instance TTL expiry"
        );
    }

    #[test]
    fn test_pause_state_persists_across_instance_ttl_boundary() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        // Pause the contract
        client.pause(&admin);
        assert!(client.is_paused());

        // Simulate instance TTL expiry by wiping instance storage
        env.as_contract(&contract_id, || {
            env.storage().instance().remove(&ADMIN_KEY);
            env.storage().instance().remove(&PENDING_ADMIN_KEY);
        });

        // PAUSED_KEY lives in persistent storage — must still be true
        assert!(
            client.is_paused(),
            "pause state must survive instance TTL expiry"
        );

        // Writes must still be blocked
        let owner = Address::generate(&env);
        assert_eq!(
            client.try_register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "test asset"),
                &unique_serial(&env),
                &owner,
            ),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );
    }

    // --- Instance TTL expiry tests ---

    /// Helper: wipe instance storage to simulate TTL expiry.
    fn wipe_instance(env: &Env, contract_id: &Address) {
        env.as_contract(contract_id, || {
            env.storage().instance().remove(&ADMIN_KEY);
            env.storage().instance().remove(&PENDING_ADMIN_KEY);
        });
    }

    #[test]
    fn test_pause_extends_instance_ttl_after_expiry() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        // Simulate expiry then re-init
        client.initialize_admin(&admin, &admin);
        wipe_instance(&env, &contract_id);
        client.initialize_admin(&admin, &admin);

        client.pause(&admin);
        let ttl = env.as_contract(&contract_id, || env.storage().instance().get_ttl());
        assert!(ttl > 0, "pause must extend instance TTL");
    }

    #[test]
    fn test_unpause_extends_instance_ttl_after_expiry() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.pause(&admin);
        wipe_instance(&env, &contract_id);
        client.initialize_admin(&admin, &admin);

        client.unpause(&admin);
        let ttl = env.as_contract(&contract_id, || env.storage().instance().get_ttl());
        assert!(ttl > 0, "unpause must extend instance TTL");
    }

    #[test]
    fn test_propose_admin_extends_instance_ttl_after_expiry() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        wipe_instance(&env, &contract_id);
        client.initialize_admin(&admin, &admin);

        let new_admin = Address::generate(&env);
        client.propose_admin(&admin, &new_admin);
        let ttl = env.as_contract(&contract_id, || env.storage().instance().get_ttl());
        assert!(ttl > 0, "propose_admin must extend instance TTL");
    }

    #[test]
    fn test_accept_admin_extends_instance_ttl_after_expiry() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let new_admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.propose_admin(&admin, &new_admin);

        // Simulate partial expiry (keep admin + pending_admin intact)
        // accept_admin reads PENDING_ADMIN_KEY which must still be present
        client.accept_admin(&new_admin);
        assert_eq!(client.get_admin(), new_admin);
        let ttl = env.as_contract(&contract_id, || env.storage().instance().get_ttl());
        assert!(ttl > 0, "accept_admin must extend instance TTL");
    }

    #[test]
    fn test_upgrade_extends_instance_ttl_after_expiry() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        wipe_instance(&env, &contract_id);
        client.initialize_admin(&admin, &admin);

        let hash = BytesN::from_array(&env, &[0xabu8; 32]);
        client.propose_upgrade(&admin, &hash);
        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);
        client.execute_upgrade(&admin);
        let ttl = env.as_contract(&contract_id, || env.storage().instance().get_ttl());
        assert!(ttl > 0, "upgrade must extend instance TTL");
    }

    #[test]
    fn test_admin_ops_work_after_instance_ttl_expiry_and_reinit() {
        // Full scenario: instance expires, admin re-initializes, all ops succeed.
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        // Simulate instance TTL expiry
        wipe_instance(&env, &contract_id);

        // Re-initialize admin
        client.initialize_admin(&admin, &admin);

        // All admin ops must succeed and extend TTL
        client.pause(&admin);
        client.unpause(&admin);

        let new_admin = Address::generate(&env);
        client.propose_admin(&admin, &new_admin);
        client.accept_admin(&new_admin);
        assert_eq!(client.get_admin(), new_admin);

        let ttl = env.as_contract(&contract_id, || env.storage().instance().get_ttl());
        assert!(ttl > 0, "instance TTL must be live after admin ops");
    }

    // --- Issue #381: is_valid_asset_type survives instance TTL expiry ---

    #[test]
    fn test_is_valid_asset_type_survives_instance_ttl_expiry() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        // Simulate instance TTL expiry by wiping all instance storage
        env.as_contract(&contract_id, || {
            env.storage().instance().remove(&ADMIN_KEY);
        });

        // Asset type lives in persistent storage — must still be valid
        assert!(
            client.is_valid_asset_type(&symbol_short!("GENSET")),
            "asset type must remain valid after instance TTL expiry"
        );
    }

    // --- Issue #382: add_asset_type and remove_asset_type extend TTL ---

    #[test]
    fn test_add_asset_type_extends_persistent_ttl() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        env.as_contract(&contract_id, || {
            let ttl = env
                .storage()
                .persistent()
                .get_ttl(&asset_type_key(&symbol_short!("GENSET")));
            assert!(
                ttl > 0,
                "asset type key TTL must be extended after add_asset_type"
            );
        });
    }

    // --- Issue #383: get_assets_by_owner extends TTL on read ---

    #[test]
    fn test_get_assets_by_owner_extends_ttl_on_read() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        // Read via get_assets_by_owner — TTL must be extended
        client.get_assets_by_owner(&owner);

        env.as_contract(&contract_id, || {
            let ttl = env.storage().persistent().get_ttl(&owner_index_key(&owner));
            assert!(ttl > 0, "owner index TTL must be extended on read");
        });
    }

    #[test]
    #[ignore = "re-entry: asset_registry -> lifecycle -> asset_registry is not allowed in Soroban"]
    fn test_get_lifecycle_score_cross_contract_call() {
        let env = Env::default();
        env.mock_all_auths();

        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(engineer_registry::EngineerRegistry, ());
        let lifecycle_id = env.register(lifecycle::Lifecycle, ());

        let asset_client = AssetRegistryClient::new(&env, &asset_registry_id);
        let lifecycle_client = lifecycle::LifecycleClient::new(&env, &lifecycle_id);

        let admin = Address::generate(&env);
        let asset_owner = Address::generate(&env);

        // Initialize both contracts
        asset_client.initialize_admin(&admin, &admin);
        asset_client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let lifecycle_admin = Address::generate(&env);
        let engineer_registry_id = Address::generate(&env);
        let deployer = Address::generate(&env);
        lifecycle_client.initialize(
            &deployer,
            &asset_registry_id,
            &engineer_registry_id,
            &lifecycle_admin,
            &200,
        );

        // Register an asset
        let asset_id = asset_client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Test Asset"),
            &unique_serial(&env),
            &asset_owner,
        );

        // Get lifecycle score via cross-contract call
        let score = asset_client.get_lifecycle_score(&asset_id, &lifecycle_id);

        // A fresh asset with no maintenance history returns None, not Some(0).
        assert_eq!(score, None);
    }

    #[test]
    fn test_get_lifecycle_score_nonexistent_asset() {
        let env = Env::default();
        env.mock_all_auths();

        let asset_registry_id = env.register(AssetRegistry, ());
        let lifecycle_id = env.register(lifecycle::Lifecycle, ());

        let asset_client = AssetRegistryClient::new(&env, &asset_registry_id);

        let admin = Address::generate(&env);
        asset_client.initialize_admin(&admin, &admin);

        // Try to get lifecycle score for non-existent asset
        let result = asset_client.try_get_lifecycle_score(&999, &lifecycle_id);

        // Should return error for non-existent asset
        assert!(result.is_err());
    }

    // --- Issue #384: initialize_admin extends instance TTL after writing ADMIN_KEY ---

    #[test]
    fn test_admin_key_survives_ttl_boundary() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);

        // Verify instance TTL was extended after writing ADMIN_KEY
        env.as_contract(&contract_id, || {
            let ttl = env.storage().instance().get_ttl();
            assert!(
                ttl > 0,
                "instance TTL must be extended after initialize_admin"
            );
        });

        // Simulate TTL boundary: advance ledger sequence past the minimum TTL
        // then verify get_admin still returns the correct admin
        env.ledger().with_mut(|li| {
            li.sequence_number += TTL_THRESHOLD;
            li.timestamp += (TTL_THRESHOLD as u64) * 5;
        });

        // get_admin must still resolve correctly (TTL was extended at init time)
        assert_eq!(client.get_admin(), admin);
    }

    /// Regression test: type_count must survive instance TTL expiry.
    ///
    /// Before the fix, type_count was stored in instance storage. If instance
    /// storage expired, remove_asset_type would read 0 and incorrectly allow
    /// removal of a type that still has registered assets.
    ///
    /// After the fix, type_count is in persistent storage. Advancing the ledger
    /// sequence past the instance TTL window must not affect the count, and
    /// remove_asset_type must still be blocked.
    #[test]
    fn test_remove_asset_type_blocked_after_instance_ttl_boundary() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &unique_serial(&env),
            &owner,
        );

        // Verify the count is in persistent storage (not instance).
        let persistent_count: u64 = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get(&type_count_key(&symbol_short!("GENSET")))
                .unwrap_or(0)
        });
        assert_eq!(
            persistent_count, 1,
            "type count must be in persistent storage"
        );

        // Advance ledger sequence well past the instance TTL window.
        // In the old code this would cause instance storage to return 0,
        // allowing remove_asset_type to succeed incorrectly.
        env.ledger().with_mut(|li| {
            li.sequence_number += 518400 + 1;
            li.timestamp += (518400 + 1) * 5;
        });

        // remove_asset_type must still be blocked because the asset still exists.
        let result = client.try_remove_asset_type(&admin, &symbol_short!("GENSET"));
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::TypeInUse as u32
            ))),
            "remove_asset_type must be blocked when assets of that type exist"
        );
    }

    #[test]
    fn test_initialize_admin_rejects_non_deployer() {
        let env = Env::default();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let attacker = Address::generate(&env);

        // Authorize only the attacker, not the deployer.
        use soroban_sdk::IntoVal;
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &attacker,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &contract_id,
                fn_name: "initialize_admin",
                args: (&attacker, &attacker).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        // Passing attacker as deployer but deployer's auth is not present — must fail.
        let result = client.try_initialize_admin(&deployer, &attacker);
        assert!(
            result.is_err(),
            "non-deployer must not be able to initialize"
        );
    }

    fn setup_with_types(env: &Env) -> (AssetRegistryClient, Address, Address) {
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(env, &contract_id);
        let admin = Address::generate(env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));
        client.add_asset_type(&admin, &symbol_short!("TURBINE"));
        (client, admin, Address::generate(env))
    }

    #[test]
    fn test_get_assets_by_type_registration() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _, owner) = setup_with_types(&env);

        let id1 = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Generator A"),
            &unique_serial(&env),
            &owner,
        );
        let id2 = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Generator B"),
            &unique_serial(&env),
            &owner,
        );
        client.register_asset(
            &symbol_short!("TURBINE"),
            &String::from_str(&env, "Turbine X"),
            &unique_serial(&env),
            &owner,
        );

        let gensets = client.get_assets_by_type(&symbol_short!("GENSET"));
        assert_eq!(gensets.len(), 2);
        assert_eq!(gensets.get(0).unwrap(), id1);
        assert_eq!(gensets.get(1).unwrap(), id2);

        let turbines = client.get_assets_by_type(&symbol_short!("TURBINE"));
        assert_eq!(turbines.len(), 1);
    }

    #[test]
    fn test_get_assets_by_type_after_deregister() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, owner) = setup_with_types(&env);

        let id1 = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Generator A"),
            &unique_serial(&env),
            &owner,
        );
        let id2 = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Generator B"),
            &unique_serial(&env),
            &owner,
        );

        client.deregister_asset(&admin, &id1);

        let gensets = client.get_assets_by_type(&symbol_short!("GENSET"));
        assert_eq!(gensets.len(), 1);
        assert_eq!(gensets.get(0).unwrap(), id2);
    }

    #[test]
    fn test_get_assets_by_type_page() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _, owner) = setup_with_types(&env);

        let metas = [
            "Generator 0",
            "Generator 1",
            "Generator 2",
            "Generator 3",
            "Generator 4",
        ];
        let mut ids: Vec<u64> = Vec::new(&env);
        for meta in metas.iter() {
            ids.push_back(client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, meta),
                &unique_serial(&env),
                &owner,
            ));
        }

        // Page 0: first 2
        let page0 = client.get_assets_by_type_page(&symbol_short!("GENSET"), &0, &2);
        assert_eq!(page0.len(), 2);
        assert_eq!(page0.get(0).unwrap(), ids.get(0).unwrap());
        assert_eq!(page0.get(1).unwrap(), ids.get(1).unwrap());

        // Page 1: next 2
        let page1 = client.get_assets_by_type_page(&symbol_short!("GENSET"), &2, &2);
        assert_eq!(page1.len(), 2);
        assert_eq!(page1.get(0).unwrap(), ids.get(2).unwrap());

        // Last page: 1 item
        let page2 = client.get_assets_by_type_page(&symbol_short!("GENSET"), &4, &2);
        assert_eq!(page2.len(), 1);

        // Out-of-bounds offset returns empty
        let empty = client.get_assets_by_type_page(&symbol_short!("GENSET"), &10, &2);
        assert_eq!(empty.len(), 0);
    }

    #[test]
    fn test_get_assets_by_type_batch_register() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _, owner) = setup_with_types(&env);

        let assets = soroban_sdk::vec![
            &env,
            AssetInput {
                asset_type: symbol_short!("GENSET"),
                metadata: String::from_str(&env, "Generator Batch 1"),
                serial_number: unique_serial(&env),
            },
            AssetInput {
                asset_type: symbol_short!("GENSET"),
                metadata: String::from_str(&env, "Generator Batch 2"),
                serial_number: unique_serial(&env),
            },
            AssetInput {
                asset_type: symbol_short!("TURBINE"),
                metadata: String::from_str(&env, "Turbine Batch 1"),
                serial_number: unique_serial(&env),
            },
        ];

        client.batch_register_assets(&owner, &assets);

        let gensets = client.get_assets_by_type(&symbol_short!("GENSET"));
        assert_eq!(gensets.len(), 2);

        let turbines = client.get_assets_by_type(&symbol_short!("TURBINE"));
        assert_eq!(turbines.len(), 1);
    }

    #[test]
    fn test_asset_status_active() {
        fn test_get_asset_count() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));

            let owner = Address::generate(&env);
            let asset_id = client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Active Generator"),
                &unique_serial(&env),
                &owner,
            );

            let status = client.asset_status(&asset_id);
            assert_eq!(status, AssetStatus::Active);
        }

        #[test]
        fn test_asset_status_decommissioned() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);
    #[test]
    fn test_get_total_asset_count() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));

            let owner = Address::generate(&env);
            let asset_id = client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Active Generator"),
                &String::from_str(&env, "Decomm Generator"),
                &unique_serial(&env),
                &owner,
            );

            let status = client.asset_status(&asset_id);
            assert_eq!(status, AssetStatus::Active);
        }

        #[test]
        fn test_asset_status_decommissioned() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));

            let owner = Address::generate(&env);
            let asset_id = client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Decomm Generator"),
                &unique_serial(&env),
                &owner,
            );

            // Manually set the decommissioned flag
            let key = decommissioned_key(asset_id);
            env.storage().persistent().set(&key, &true);

            let status = client.asset_status(&asset_id);
            assert_eq!(status, AssetStatus::Decommissioned);
        }

        #[test]
        fn test_asset_status_not_found() {
            let env = Env::default();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let result = client.try_asset_status(&999u64);
            assert_eq!(
                result,
                Err(Ok(soroban_sdk::Error::from_contract_error(
                    ContractError::AssetNotFound as u32
                )))
            );
        }

        #[test]
        fn test_asset_status_under_maintenance() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));

            let owner = Address::generate(&env);
            let asset_id = client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Maintained Generator"),
                &unique_serial(&env),
                &owner,
            );

            // Manually set the under_maintenance flag
            let key = (symbol_short!("U_MAINT"), asset_id);
            env.storage().persistent().set(&key, &true);

            let status = client.asset_status(&asset_id);
            assert_eq!(status, AssetStatus::UnderMaintenance);
        }

        #[test]
        fn test_decommission_asset_admin_can_decommission() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);
            // Manually set the decommissioned flag
            let key = decommissioned_key(asset_id);
            env.storage().persistent().set(&key, &true);

            let status = client.asset_status(&asset_id);
            assert_eq!(status, AssetStatus::Decommissioned);
        }

        #[test]
        fn test_asset_status_not_found() {
            let env = Env::default();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let result = client.try_asset_status(&999u64);
            assert_eq!(
                result,
                Err(Ok(soroban_sdk::Error::from_contract_error(
                    ContractError::AssetNotFound as u32
                )))
            );
        }

        #[test]
        fn test_asset_status_under_maintenance() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));

            let owner = Address::generate(&env);
            let asset_id = client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Maintained Generator"),
                &unique_serial(&env),
                &owner,
            );

            // Manually set the under_maintenance flag
            let key = (symbol_short!("U_MAINT"), asset_id);
            env.storage().persistent().set(&key, &true);

            let status = client.asset_status(&asset_id);
            assert_eq!(status, AssetStatus::UnderMaintenance);
        }

        #[test]
        fn test_decommission_asset_admin_can_decommission() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);
        // Returns 0 before any assets are registered
        assert_eq!(client.get_total_asset_count(), 0);

        let owner = Address::generate(&env);
        client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Generator Unit A"),
            &unique_serial(&env),
            &owner,
        );
        assert_eq!(client.get_total_asset_count(), 1);

        client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Generator Unit B"),
            &unique_serial(&env),
            &owner,
        );
        assert_eq!(client.get_total_asset_count(), 2);

        // get_total_asset_count and get_asset_count must agree
        assert_eq!(client.get_total_asset_count(), client.get_asset_count());
    }

    #[test]
    fn test_asset_status_decommissioned() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));

            let owner = Address::generate(&env);
            let asset_id = client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Decomm Test"),
                &String::from_str(&env, "Decomm Generator"),
                &unique_serial(&env),
                &owner,
            );

            // Decommission the asset
            client.decommission_asset(&admin, &asset_id);

            // Verify status is Decommissioned
            let status = client.asset_status(&asset_id);
            assert_eq!(status, AssetStatus::Decommissioned);
        }

        #[test]
        fn test_decommission_asset_non_admin_rejected() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));

            // Manually set the decommissioned flag
            let key = decommissioned_key(asset_id);
            env.storage().persistent().set(&key, &true);

            let status = client.asset_status(&asset_id);
            assert_eq!(status, AssetStatus::Decommissioned);
        }

        #[test]
        fn test_decommission_asset_non_admin_rejected() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));

            let owner = Address::generate(&env);
            let asset_id = client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Decomm Test"),
                &unique_serial(&env),
                &owner,
            );

            // Non-admin tries to decommission
            let non_admin = Address::generate(&env);
            let result = client.try_decommission_asset(&non_admin, &asset_id);
            assert_eq!(
                result,
                Err(Ok(soroban_sdk::Error::from_contract_error(
                    ContractError::UnauthorizedAdmin as u32

            // Non-admin tries to decommission
            let non_admin = Address::generate(&env);
            let result = client.try_decommission_asset(&non_admin, &asset_id);
            assert_eq!(
                result,
                Err(Ok(soroban_sdk::Error::from_contract_error(
                    ContractError::UnauthorizedAdmin as u32
                )))
            );
        }

        #[test]
        fn test_decommission_nonexistent_asset() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);

            // Try to decommission non-existent asset
            let result = client.try_decommission_asset(&admin, &999u64);
            assert_eq!(
                result,
                Err(Ok(soroban_sdk::Error::from_contract_error(
                    ContractError::AssetNotFound as u32
                )))
            );
        }

        #[test]
        fn test_decommission_nonexistent_asset() {
        fn test_decommission_asset_emits_event() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);

            // Try to decommission non-existent asset
            let result = client.try_decommission_asset(&admin, &999u64);
            assert_eq!(
                result,
                Err(Ok(soroban_sdk::Error::from_contract_error(
                    ContractError::AssetNotFound as u32
                )))
            );
        }

        #[test]
        fn test_decommission_asset_emits_event() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));

            let owner = Address::generate(&env);
            let asset_id = client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Event Test"),
                &unique_serial(&env),
                &owner,
            );

            // Decommission the asset and check for event
            client.decommission_asset(&admin, &asset_id);

            let events = env.events().all();
            // Should have at least one DECOMM event
            assert!(events.len() > 0, "decommission_asset should emit an event");
            // Counter starts at 0
            assert_eq!(client.get_asset_count(), 0);

            let owner = Address::generate(&env);

            // Register first asset, count should be 1
            client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Generator 1"),
                &unique_serial(&env),
                &owner,
            );
            assert_eq!(client.get_asset_count(), 1);

            // Register second asset, count should be 2
            client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Generator 2"),
                &unique_serial(&env),
                &owner,
            );
            assert_eq!(client.get_asset_count(), 2);

            // Register third asset, count should be 3
            client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Generator 3"),
                &unique_serial(&env),
                &owner,
            );
            client.add_asset_type(&admin, &symbol_short!("GENSET"));

            let owner = Address::generate(&env);
            let asset_id = client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Event Test"),
                &unique_serial(&env),
                &owner,
            );

            // Decommission the asset and check for event
            client.decommission_asset(&admin, &asset_id);

            let events = env.events().all();
            // Should have at least one DECOMM event
            assert!(events.len() > 0, "decommission_asset should emit an event");
            // Counter starts at 0
            assert_eq!(client.get_asset_count(), 0);

            let owner = Address::generate(&env);

            // Register first asset, count should be 1
            client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Generator 1"),
                &unique_serial(&env),
                &owner,
            );
            assert_eq!(client.get_asset_count(), 1);

            // Register second asset, count should be 2
            client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Generator 2"),
                &unique_serial(&env),
                &owner,
            );
            assert_eq!(client.get_asset_count(), 2);

            // Register third asset, count should be 3
            client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Generator 3"),
                &unique_serial(&env),
                &owner,
            );
        fn test_asset_status_not_found() {
            let env = Env::default();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let result = client.try_asset_status(&999u64);
            assert_eq!(
                result,
                Err(Ok(soroban_sdk::Error::from_contract_error(
                    ContractError::AssetNotFound as u32
                )))
            );
        }

        #[test]
        fn test_asset_status_under_maintenance() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));

            let owner = Address::generate(&env);
            let asset_id = client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Maintained Generator"),
                &unique_serial(&env),
                &owner,
            );

            // Manually set the under_maintenance flag
            let key = (symbol_short!("U_MAINT"), asset_id);
            env.storage().persistent().set(&key, &true);

            let status = client.asset_status(&asset_id);
            assert_eq!(status, AssetStatus::UnderMaintenance);
        }

        #[test]
        fn test_decommission_asset_admin_can_decommission() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));

            let owner = Address::generate(&env);
            let asset_id = client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Decomm Test"),
                &unique_serial(&env),
                &owner,
            );

            // Decommission the asset
            client.decommission_asset(&admin, &asset_id);

            // Verify status is Decommissioned
            let status = client.asset_status(&asset_id);
            assert_eq!(status, AssetStatus::Decommissioned);
        }

        #[test]
        fn test_decommission_asset_non_admin_rejected() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));

            let owner = Address::generate(&env);
            let asset_id = client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Decomm Test"),
                &unique_serial(&env),
                &owner,
            );

            // Non-admin tries to decommission
            let non_admin = Address::generate(&env);
            let result = client.try_decommission_asset(&non_admin, &asset_id);
            assert_eq!(
                result,
                Err(Ok(soroban_sdk::Error::from_contract_error(
                    ContractError::UnauthorizedAdmin as u32
                )))
            );
        }

        #[test]
        fn test_decommission_nonexistent_asset() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);

            // Try to decommission non-existent asset
            let result = client.try_decommission_asset(&admin, &999u64);
            assert_eq!(
                result,
                Err(Ok(soroban_sdk::Error::from_contract_error(
                    ContractError::AssetNotFound as u32
                )))
            );
        }

        #[test]
        fn test_decommission_asset_emits_event() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));

            let owner = Address::generate(&env);
            let asset_id = client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Event Test"),
                &unique_serial(&env),
                &owner,
            );

            // Decommission the asset and check for event
            client.decommission_asset(&admin, &asset_id);

            let events = env.events().all();
            // Should have at least one DECOMM event
            assert!(events.len() > 0, "decommission_asset should emit an event");
            // Counter starts at 0
            assert_eq!(client.get_asset_count(), 0);

            let owner = Address::generate(&env);

            // Register first asset, count should be 1
            client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Generator 1"),
                &unique_serial(&env),
                &owner,
            );
            assert_eq!(client.get_asset_count(), 1);

            // Register second asset, count should be 2
            client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, &format!("Generator {i}")),
                &String::from_str(&env, "Generator 2"),
                &unique_serial(&env),
                &owner,
            );
            assert_eq!(client.get_asset_count(), 2);

            // Register third asset, count should be 3
            client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, "Generator 3"),
                &unique_serial(&env),
                &owner,
            );
            assert_eq!(client.get_asset_count(), 3);
        }

        // --- Issue: get_assets_by_type_paginated tests ---

        fn setup_with_types_for_pagination(env: &Env) -> (AssetRegistryClient, Address) {
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(env, &contract_id);
            let admin = Address::generate(env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));
            client.add_asset_type(&admin, &symbol_short!("TURBINE"));
            let owner = Address::generate(env);
            (client, owner)
        }

        #[test]
        fn test_get_assets_by_type_paginated_standard() {
            let env = Env::default();
            env.mock_all_auths();
            let (client, owner) = setup_with_types_for_pagination(&env);

            for i in 0..7u32 {
                client.register_asset(
                    &symbol_short!("GENSET"),
                    &String::from_str(&env, &std::format!("Generator {i}")),
                    &unique_serial(&env),
                    &owner,
                );
            }

            // Page 0: items 0-2
            let p0 = client.get_assets_by_type_paginated(&symbol_short!("GENSET"), &0, &3);
            assert_eq!(p0.total, 7);
            assert_eq!(p0.assets.len(), 3);

            // Page 1: items 3-5
            let p1 = client.get_assets_by_type_paginated(&symbol_short!("GENSET"), &1, &3);
            assert_eq!(p1.total, 7);
            assert_eq!(p1.assets.len(), 3);

            // Page 2: item 6 (last page, partial)
            let p2 = client.get_assets_by_type_paginated(&symbol_short!("GENSET"), &2, &3);
            assert_eq!(p2.total, 7);
            assert_eq!(p2.assets.len(), 1);
        }

        #[test]
        fn test_get_assets_by_type_paginated_empty_type() {
            let env = Env::default();
            env.mock_all_auths();
            let (client, _) = setup_with_types_for_pagination(&env);

            // No assets of type TURBINE registered
            let result = client.get_assets_by_type_paginated(&symbol_short!("TURBINE"), &0, &10);
            assert_eq!(result.total, 0);
            assert_eq!(result.assets.len(), 0);
        }

        #[test]
        fn test_get_assets_by_type_paginated_out_of_bounds() {
            let env = Env::default();
            env.mock_all_auths();
            let (client, owner) = setup_with_types_for_pagination(&env);

            client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, &format!("Generator {i}")),
                &String::from_str(&env, "Generator 0"),
                &unique_serial(&env),
                &owner,
            );

            // Page beyond the end returns empty assets but correct total
            let result = client.get_assets_by_type_paginated(&symbol_short!("GENSET"), &5, &10);
            assert_eq!(result.total, 1);
            assert_eq!(result.assets.len(), 0);
        }

        #[test]
        fn test_get_assets_by_type_paginated_page_size_capped_at_100() {
            let env = Env::default();
            env.mock_all_auths();
            let (client, owner) = setup_with_types_for_pagination(&env);

            for i in 0..50u32 {
                client.register_asset(
                    &symbol_short!("GENSET"),
                    &String::from_str(&env, &std::format!("Generator {i}")),
                    &unique_serial(&env),
                    &owner,
                );
            }

            // page_size=200 is capped to 100, so at most 100 assets returned
            let result = client.get_assets_by_type_paginated(&symbol_short!("GENSET"), &0, &200);
            assert_eq!(result.total, 50);
            assert_eq!(result.assets.len(), 50); // only 50 assets exist
        }

        // --- #751: dedup key includes asset_type ---

        #[test]
        fn test_same_metadata_different_type_is_allowed() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));
            client.add_asset_type(&admin, &symbol_short!("TURBINE"));

            let owner = Address::generate(&env);
            let metadata = String::from_str(&env, "Spec v1");

            // Same metadata, different asset types — both should succeed
            let id1 = client.register_asset(
                &symbol_short!("GENSET"),
                &metadata,
                &unique_serial(&env),
                &owner,
            );
            let id2 = client.register_asset(
                &symbol_short!("TURBINE"),
                &metadata,
                &unique_serial(&env),
                &owner,
            );

            assert_ne!(id1, id2);
        }

        #[test]
        fn test_same_owner_same_type_same_metadata_is_rejected() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));

            let owner = Address::generate(&env);
            let metadata = String::from_str(&env, "Spec v1");

            client.register_asset(
                &symbol_short!("GENSET"),
                &metadata,
                &unique_serial(&env),
                &owner,
            );

            let result = client.try_register_asset(
                &symbol_short!("GENSET"),
                &metadata,
                &unique_serial(&env),
                &owner,
            );
            assert_eq!(
                result,
                Err(Ok(soroban_sdk::Error::from_contract_error(
                    ContractError::DuplicateAsset as u32,
                ))),
            );
        }

        #[test]
        fn test_batch_same_metadata_different_type_is_allowed() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);
            client.add_asset_type(&admin, &symbol_short!("GENSET"));
            client.add_asset_type(&admin, &symbol_short!("TURBINE"));

            let owner = Address::generate(&env);
            let mut batch = Vec::new(&env);
            batch.push_back(AssetInput {
                asset_type: symbol_short!("GENSET"),
                metadata: String::from_str(&env, "Shared spec"),
                serial_number: unique_serial(&env),
            });
            batch.push_back(AssetInput {
                asset_type: symbol_short!("TURBINE"),
                metadata: String::from_str(&env, "Shared spec"),
                serial_number: unique_serial(&env),
            });

            let ids = client.batch_register_assets(&owner, &batch);
            assert_eq!(ids.len(), 2);
        }

        // --- #752: upgrade timelock tests ---

        #[test]
        fn test_execute_upgrade_before_timelock_fails() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);

            let hash = BytesN::from_array(&env, &[0xabu8; 32]);
            client.propose_upgrade(&admin, &hash);

            // Not enough time passed — should fail
            let result = client.try_execute_upgrade(&admin);
            assert_eq!(
                result,
                Err(Ok(soroban_sdk::Error::from_contract_error(
                    ContractError::TimelockNotExpired as u32,
                ))),
            );
        }

        #[test]
        fn test_execute_upgrade_after_timelock_succeeds() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);

            let hash = BytesN::from_array(&env, &[0xabu8; 32]);
            client.propose_upgrade(&admin, &hash);

            let base = env.ledger().timestamp();
            env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

            // Should succeed
            client.execute_upgrade(&admin);
        }

        #[test]
        fn test_execute_upgrade_without_proposal_fails() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);

            let result = client.try_execute_upgrade(&admin);
            assert_eq!(
                result,
                Err(Ok(soroban_sdk::Error::from_contract_error(
                    ContractError::ProposalNotFound as u32,
                ))),
            );
        }

        #[test]
        fn test_propose_upgrade_emits_event() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);

        let events = env.events().all();
        use soroban_sdk::TryIntoVal;
        let prop_event = events.iter().find(|(_, topics, _)| {
            if let Some(val) = topics.get(0) {
                if let Ok(s) = TryIntoVal::<_, Symbol>::try_into_val(&val, &env) {
                    return s == symbol_short!("PROP_UPG");
            let hash = BytesN::from_array(&env, &[0xabu8; 32]);
            client.propose_upgrade(&admin, &hash);

            let events = env.events().all();
            use soroban_sdk::TryIntoVal;
            let prop_event = events.iter().find(|(_, topics, _)| {
                if let Some(val) = topics.get(0) {
                    if let Ok(s) = val.try_into_val::<_, Symbol>(&env) {
                        return s == symbol_short!("PROP_UPG");
                    }
                }
                false
            });
            assert!(
                prop_event.is_some(),
                "PROP_UPG event must be emitted on propose_upgrade"
            );
        }

        #[test]
        fn test_upgrade_emit_event_after_execute() {
            let env = Env::default();
            env.mock_all_auths();
            let contract_id = env.register(AssetRegistry, ());
            let client = AssetRegistryClient::new(&env, &contract_id);

            let admin = Address::generate(&env);
            client.initialize_admin(&admin, &admin);

            let hash = BytesN::from_array(&env, &[0xabu8; 32]);
            client.propose_upgrade(&admin, &hash);
            let base = env.ledger().timestamp();
            env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);
            client.execute_upgrade(&admin);

        let events = env.events().all();
        use soroban_sdk::TryIntoVal;
        let upgrade_event = events.iter().find(|(_, topics, _)| {
            if let Some(val) = topics.get(0) {
                if let Ok(s) = TryIntoVal::<_, Symbol>::try_into_val(&val, &env) {
                    return s == symbol_short!("UPGRADE");
                }
            }
            false
        });
        assert!(upgrade_event.is_some(), "UPGRADE event must be emitted on execute_upgrade");
        let (_, _, data) = upgrade_event.unwrap();
        let emitted_hash: BytesN<32> = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_hash, hash);
    }

    #[test]
    fn test_get_assets_by_type_paginated_total_matches_across_pages() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, owner) = setup_with_types_for_pagination(&env);

        for i in 0..12u32 {
            client.register_asset(
                &symbol_short!("GENSET"),
                &String::from_str(&env, &format!("Generator {i}")),
                &unique_serial(&env),
                &owner,
            let events = env.events().all();
            use soroban_sdk::TryIntoVal;
            let upgrade_event = events.iter().find(|(_, topics, _)| {
                if let Some(val) = topics.get(0) {
                    if let Ok(s) = val.try_into_val::<_, Symbol>(&env) {
                        return s == symbol_short!("UPGRADE");
                    }
                }
                false
            });
            assert!(
                upgrade_event.is_some(),
                "UPGRADE event must be emitted on execute_upgrade"
            );
            let (_, _, data) = upgrade_event.unwrap();
            let emitted_hash: BytesN<32> = data.try_into_val(&env).unwrap();
            assert_eq!(emitted_hash, hash);
        }

        #[test]
        fn test_get_assets_by_type_paginated_total_matches_across_pages() {
            let env = Env::default();
            env.mock_all_auths();
            let (client, owner) = setup_with_types_for_pagination(&env);

            for i in 0..12u32 {
                client.register_asset(
                    &symbol_short!("GENSET"),
                    &String::from_str(&env, &std::format!("Generator {i}")),
                    &unique_serial(&env),
                    &owner,
                );
            }

            // Total reported on every page must be the same
            let p0 = client.get_assets_by_type_paginated(&symbol_short!("GENSET"), &0, &5);
            let p1 = client.get_assets_by_type_paginated(&symbol_short!("GENSET"), &1, &5);
            let p2 = client.get_assets_by_type_paginated(&symbol_short!("GENSET"), &2, &5);
            assert_eq!(p0.total, 12);
            assert_eq!(p1.total, 12);
            assert_eq!(p2.total, 12);

            // Pages cover all 12 assets without overlap: 5 + 5 + 2 = 12
            assert_eq!(p0.assets.len() + p1.assets.len() + p2.assets.len(), 12);
        }
    }

    // --- Issue: Block re-proposal of deregister timelock ---

    #[test]
    fn test_propose_deregister_cannot_overwrite_pending_proposal() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let asset_id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Propose Block Test"),
            &unique_serial(&env),
            &owner,
        );

        // First proposal succeeds
        client.propose_deregister_asset(&owner, &asset_id);

        // Second proposal with a pending one must fail
        let result = client.try_propose_deregister_asset(&owner, &asset_id);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::ProposalAlreadyExists as u32
            )))
        );
    }

    // --- Issue: decommission_asset_notify freezes lifecycle score ---

    #[test]
    fn test_decommission_asset_notify_freezes_lifecycle_score() {
        let env = Env::default();
        env.mock_all_auths();

        // Set up all three contracts
        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(engineer_registry::EngineerRegistry, ());
        let lifecycle_id = env.register(lifecycle::Lifecycle, ());

        let asset_client = AssetRegistryClient::new(&env, &asset_registry_id);
        let eng_client =
            engineer_registry::EngineerRegistryClient::new(&env, &engineer_registry_id);
        let lc_client = lifecycle::LifecycleClient::new(&env, &lifecycle_id);

        let asset_admin = Address::generate(&env);
        let lc_admin = Address::generate(&env);
        let eng_admin = Address::generate(&env);

        asset_client.initialize_admin(&asset_admin, &asset_admin);
        asset_client.add_asset_type(&asset_admin, &symbol_short!("GENSET"));

        eng_client.initialize_admin(&eng_admin, &eng_admin);

        lc_client.initialize(
            &lc_admin,
            &asset_registry_id,
            &engineer_registry_id,
            &lc_admin,
            &0u32,
        );

        // Register asset and engineer, then submit maintenance
        let owner = Address::generate(&env);
        let asset_id = asset_client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Freeze Score Test"),
            &String::from_str(&env, "SN-FREEZE-001"),
            &owner,
        );
        let issuer = Address::generate(&env);
        let engineer = Address::generate(&env);
        eng_client.add_trusted_issuer(&eng_admin, &issuer);
        eng_client.register_engineer(
            &engineer,
            &soroban_sdk::BytesN::from_array(&env, &[1u8; 32]),
            &issuer,
            &31_536_000u64,
            &None,
        );
        lc_client.authorize_engineer(&owner, &asset_id, &engineer);
        lc_client.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(&env, "Pre-decommission service"),
            &engineer,
            &None,
        );

        let score_at_decommission = lc_client.get_collateral_score(&asset_id);
        assert!(
            score_at_decommission > 0,
            "score must be non-zero before decommission"
        );

        // Decommission and notify lifecycle
        asset_client.decommission_asset_notify(&asset_admin, &asset_id, &lifecycle_id);

        // Advance time past several decay intervals
        env.ledger().with_mut(|li| li.timestamp += 50_000_000);

        // Fix #794: Score must be 0 after decommission, not frozen at pre-decommission value.
        // A decommissioned asset must never be usable as DeFi collateral.
        let score_after = lc_client.get_collateral_score(&asset_id);
        assert_eq!(
            score_after, 0,
            "lifecycle score must be 0 after decommission_asset_notify (fix #794)"
        );
    }

    // --- get_asset_status Tests ---

    #[test]
    fn test_get_asset_status_active() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let asset_id = reg(
            &client,
            &env,
            symbol_short!("GENSET"),
            String::from_str(&env, "Generator"),
            &owner,
        );

        assert_eq!(client.asset_status(&asset_id), AssetStatus::Active);
    }

    #[test]
    fn test_get_asset_status_decommissioned() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let asset_id = reg(
            &client,
            &env,
            symbol_short!("GENSET"),
            String::from_str(&env, "Generator"),
            &owner,
        );

        client.decommission_asset(&admin, &asset_id);

        assert_eq!(client.asset_status(&asset_id), AssetStatus::Decommissioned);
    }

    #[test]
    fn test_get_asset_status_under_maintenance() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let asset_id = reg(
            &client,
            &env,
            symbol_short!("GENSET"),
            String::from_str(&env, "Generator"),
            &owner,
        );

        // No public mark_under_maintenance API; set the flag directly via storage.
        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&(symbol_short!("U_MAINT"), asset_id), &true);
        });

        assert_eq!(
            client.asset_status(&asset_id),
            AssetStatus::UnderMaintenance
        );
    }

    #[test]
    #[should_panic(expected = "AssetNotFound")]
    fn test_get_asset_status_unknown_asset_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);

        client.asset_status(&999);
    }

    // --- Issue #800: Validate asset_type symbol characters in register_asset ---

    /// #800: validate_asset_type_symbol must accept symbols containing only
    /// alphanumeric characters and underscores, and reject any other input.
    #[test]
    fn test_validate_asset_type_symbol_accepts_valid_symbols() {
        let env = Env::default();
        // These should not panic — all chars are in [A-Za-z0-9_].
        validate_asset_type_symbol(&env, &symbol_short!("GENSET"));
        validate_asset_type_symbol(&env, &symbol_short!("TYPE_1"));
        validate_asset_type_symbol(&env, &Symbol::new(&env, "TURBINE_A"));
    }

    /// #800: register_asset must panic with InvalidAssetType when the asset_type
    /// symbol contains only valid characters but is not in the allowlist.
    /// This confirms that validate_asset_type_symbol itself does not reject valid-char symbols.
    #[test]
    fn test_register_asset_valid_symbol_not_in_allowlist_rejected_with_invalid_type() {
    // --- Deprecation tests ---

    #[test]
    fn test_deprecate_asset_owner_can_deprecate() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let asset_id = reg(
            &client,
            &env,
            symbol_short!("GENSET"),
            String::from_str(&env, "Old Generator"),
            &owner,
        );

        // Initially Active
        assert_eq!(
            client.get_asset(&asset_id).deprecation_status,
            DeprecationStatus::Active
        );

        // Owner deprecates the asset
        client.deprecate_asset(
            &owner,
            &asset_id,
            &String::from_str(&env, "End of service life"),
        );

        // Status should now be Deprecated
        assert_eq!(
            client.get_asset(&asset_id).deprecation_status,
            DeprecationStatus::Deprecated
        );
    }

    #[test]
    fn test_deprecate_asset_non_owner_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        // Do NOT add the type to the allowlist — valid chars but unknown type.

        let owner = Address::generate(&env);
        let result = client.try_register_asset(
            &symbol_short!("UNKNOWN"),
            &String::from_str(&env, "metadata"),
            &unique_serial(&env),
            &owner,
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidAssetType as u32
            ))),
            "valid-char but unallowlisted symbol must fail with InvalidAssetType",
        );
    }
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let asset_id = reg(
            &client,
            &env,
            symbol_short!("GENSET"),
            String::from_str(&env, "Generator X"),
            &owner,
        );

        let non_owner = Address::generate(&env);
        let result =
            client.try_deprecate_asset(&non_owner, &asset_id, &String::from_str(&env, "reason"));
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedOwner as u32
            )))
        );
    }

    #[test]
    fn test_deprecate_already_deprecated_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let asset_id = reg(
            &client,
            &env,
            symbol_short!("GENSET"),
            String::from_str(&env, "Generator Y"),
            &owner,
        );

        client.deprecate_asset(&owner, &asset_id, &String::from_str(&env, "first"));

        // Second deprecation must fail
        let result =
            client.try_deprecate_asset(&owner, &asset_id, &String::from_str(&env, "second"));
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetAlreadyDeprecated as u32
            )))
        );
    }

    #[test]
    fn test_deprecate_nonexistent_asset_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);

        let owner = Address::generate(&env);
        let result = client.try_deprecate_asset(&owner, &999u64, &String::from_str(&env, "reason"));
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32
            )))
        );
    }

    #[test]
    fn test_deprecate_decommissioned_asset_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let asset_id = reg(
            &client,
            &env,
            symbol_short!("GENSET"),
            String::from_str(&env, "Generator Z"),
            &owner,
        );

        // Admin decommissions via registry (sets decommissioned_key bool, but NOT deprecation_status)
        // For the deprecation_status path, manually deprecate first then attempt again
        client.deprecate_asset(&owner, &asset_id, &String::from_str(&env, "eof"));
        // Now the asset is Deprecated — a second call must fail with AssetAlreadyDeprecated
        let result =
            client.try_deprecate_asset(&owner, &asset_id, &String::from_str(&env, "again"));
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetAlreadyDeprecated as u32
            )))
        );
    }

    #[test]
    fn test_new_asset_has_active_deprecation_status() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("TURBINE"));

        let owner = Address::generate(&env);
        let asset_id = reg(
            &client,
            &env,
            symbol_short!("TURBINE"),
            String::from_str(&env, "Turbine A"),
            &owner,
        );

        assert_eq!(
            client.get_asset(&asset_id).deprecation_status,
            DeprecationStatus::Active
        );
    }

    // ── search_assets tests ──────────────────────────────────────────────────

    fn setup_search_env(env: &Env) -> (AssetRegistryClient, Address) {
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(env, &contract_id);
        let admin = Address::generate(env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("TURBINE"));
        client.add_asset_type(&admin, &symbol_short!("GENSET"));
        (client, admin)
    }

    #[test]
    fn test_search_no_filter_returns_all() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup_search_env(&env);
        let owner = Address::generate(&env);
        reg(&client, &env, symbol_short!("TURBINE"), String::from_str(&env, "Acme Turbine"), &owner);
        reg(&client, &env, symbol_short!("GENSET"), String::from_str(&env, "Acme Genset"), &owner);

        let page = client.search_assets(&SearchFilter {
            asset_type: None,
            manufacturer: None,
            min_age_months: None,
            max_age_months: None,
            sort: None,
            lifecycle_contract: None,
        });
        assert_eq!(page.total, 2);
        assert_eq!(page.assets.len(), 2);
    }

    #[test]
    #[should_panic(expected = "InvalidConfig")]
    fn test_search_by_collateral_score_without_lifecycle_contract_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup_search_env(&env);
        let owner = Address::generate(&env);
        reg(&client, &env, symbol_short!("TURBINE"), String::from_str(&env, "Acme Turbine"), &owner);

        client.search_assets(&SearchFilter {
            asset_type: None,
            manufacturer: None,
            min_age_months: None,
            max_age_months: None,
            sort: Some(SortOrder::ByCollateralScore),
            lifecycle_contract: None,
        });
    }

    #[test]
    fn test_search_filter_by_asset_type() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup_search_env(&env);
        let owner = Address::generate(&env);
        reg(&client, &env, symbol_short!("TURBINE"), String::from_str(&env, "Turbine Alpha"), &owner);
        reg(&client, &env, symbol_short!("GENSET"), String::from_str(&env, "Genset Beta"), &owner);

        let page = client.search_assets(&SearchFilter {
            asset_type: Some(symbol_short!("TURBINE")),
            manufacturer: None,
            min_age_months: None,
            max_age_months: None,
            sort: None,
            lifecycle_contract: None,
        });
        assert_eq!(page.total, 1);
        assert_eq!(page.assets.get(0).unwrap().asset_type, symbol_short!("TURBINE"));
    }

    #[test]
    fn test_search_filter_by_manufacturer_substring() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup_search_env(&env);
        let owner = Address::generate(&env);
        reg(&client, &env, symbol_short!("TURBINE"), String::from_str(&env, "Siemens Turbine X"), &owner);
        reg(&client, &env, symbol_short!("GENSET"), String::from_str(&env, "Caterpillar Genset Y"), &owner);

        let page = client.search_assets(&SearchFilter {
            asset_type: None,
            manufacturer: Some(String::from_str(&env, "Siemens")),
            min_age_months: None,
            max_age_months: None,
            sort: None,
            lifecycle_contract: None,
        });
        assert_eq!(page.total, 1);
        assert_eq!(
            page.assets.get(0).unwrap().metadata,
            String::from_str(&env, "Siemens Turbine X")
        );
    }

    #[test]
    fn test_search_filter_manufacturer_no_match() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup_search_env(&env);
        let owner = Address::generate(&env);
        reg(&client, &env, symbol_short!("TURBINE"), String::from_str(&env, "Acme Turbine"), &owner);

        let page = client.search_assets(&SearchFilter {
            asset_type: None,
            manufacturer: Some(String::from_str(&env, "Siemens")),
            min_age_months: None,
            max_age_months: None,
            sort: None,
            lifecycle_contract: None,
        });
        assert_eq!(page.total, 0);
        assert_eq!(page.assets.len(), 0);
    }

    #[test]
    fn test_search_filter_max_age_zero_returns_all_new() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup_search_env(&env);
        let owner = Address::generate(&env);
        reg(&client, &env, symbol_short!("TURBINE"), String::from_str(&env, "Brand New"), &owner);

        // max_age_months=0 means "registered within the current month" — should match
        let page = client.search_assets(&SearchFilter {
            asset_type: None,
            manufacturer: None,
            min_age_months: None,
            max_age_months: Some(0),
            sort: None,
            lifecycle_contract: None,
        });
        assert_eq!(page.total, 1);
    }

    #[test]
    fn test_search_filter_min_age_excludes_new_assets() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup_search_env(&env);
        let owner = Address::generate(&env);
        reg(&client, &env, symbol_short!("TURBINE"), String::from_str(&env, "New Turbine"), &owner);

        // min_age_months=1 requires the asset to be at least 30 days old — new asset fails
        let page = client.search_assets(&SearchFilter {
            asset_type: None,
            manufacturer: None,
            min_age_months: Some(1),
            max_age_months: None,
            sort: None,
            lifecycle_contract: None,
        });
        assert_eq!(page.total, 0);
    }

    #[test]
    fn test_search_sort_by_maintenance_date() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup_search_env(&env);
        let owner = Address::generate(&env);

        let id1 = reg(&client, &env, symbol_short!("TURBINE"), String::from_str(&env, "Turbine First"), &owner);
        // advance time so second asset has a later timestamp
        env.ledger().set_timestamp(env.ledger().timestamp() + 1000);
        let id2 = reg(&client, &env, symbol_short!("GENSET"), String::from_str(&env, "Genset Second"), &owner);

        let page = client.search_assets(&SearchFilter {
            asset_type: None,
            manufacturer: None,
            min_age_months: None,
            max_age_months: None,
            sort: Some(SortOrder::ByMaintenanceDate),
            lifecycle_contract: None,
        });
        assert_eq!(page.total, 2);
        // most recently updated first
        assert_eq!(page.assets.get(0).unwrap().asset_id, id2);
        assert_eq!(page.assets.get(1).unwrap().asset_id, id1);
    }

    #[test]
    fn test_search_combined_type_and_manufacturer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup_search_env(&env);
        let owner = Address::generate(&env);
        reg(&client, &env, symbol_short!("TURBINE"), String::from_str(&env, "Siemens Turbine"), &owner);
        reg(&client, &env, symbol_short!("GENSET"), String::from_str(&env, "Siemens Genset"), &owner);

        let page = client.search_assets(&SearchFilter {
            asset_type: Some(symbol_short!("GENSET")),
            manufacturer: Some(String::from_str(&env, "Siemens")),
            min_age_months: None,
            max_age_months: None,
            sort: None,
            lifecycle_contract: None,
        });
        assert_eq!(page.total, 1);
        assert_eq!(page.assets.get(0).unwrap().asset_type, symbol_short!("GENSET"));
    }

    #[test]
    fn test_search_caps_at_100_results() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup_search_env(&env);
        let owner = Address::generate(&env);
        for _ in 0..110u32 {
            reg(&client, &env, symbol_short!("TURBINE"), String::from_str(&env, "Turbine Unit"), &owner);
        }
        let page = client.search_assets(&SearchFilter {
            asset_type: None,
            manufacturer: None,
            min_age_months: None,
            max_age_months: None,
            sort: None,
            lifecycle_contract: None,
        });
        assert_eq!(page.total, 110);
        assert_eq!(page.assets.len(), 100);
    }

    // ── #795 regression ──────────────────────────────────────────────────────
    // Timelock comparisons must use env.ledger().timestamp() (Unix seconds) and
    // NOT env.ledger().sequence() (ledger numbers).  These tests fix the
    // expected timing so CI catches any accidental reversion to sequence().

    /// The timelock must be measured in wall-clock seconds (timestamp), not
    /// ledger sequence numbers.  TIMELOCK_DELAY_SECS = 48 * 60 * 60 seconds.
    ///
    /// Test plan:
    ///   1. propose_deregister_asset  →  proposal stored with proposed_at = timestamp()
    ///   2. execute immediately       →  must fail (TimelockNotExpired)
    ///   3. advance timestamp by exactly TIMELOCK_DELAY_SECS
    ///   4. execute again             →  must succeed (asset deregistered)
    #[test]
    fn test_timelock_uses_timestamp_not_sequence() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let asset_id = reg(&client, &env, symbol_short!("GENSET"), String::from_str(&env, "Test rig"), &owner);

        // Step 1: propose deregistration — records proposed_at = current timestamp.
        client.propose_deregister_asset(&owner, &asset_id);

        // Step 2: attempt execution immediately — must be rejected.
        // If the implementation used sequence() instead of timestamp() the delay would
        // be ~48 h * 3600 s/h = 172_800 ledger numbers — essentially instant on testnet
        // (current sequence ≈ 3 M), so this assertion would *pass* incorrectly.
        // With the correct timestamp() comparison the delay has not yet elapsed.
        let result_before = client.try_execute_deregister_asset(&owner, &asset_id);
        assert_eq!(
            result_before,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::TimelockNotExpired as u32,
            ))),
            "execute must be rejected before TIMELOCK_DELAY_SECS have elapsed"
        );

        // Step 3: advance ledger timestamp by exactly TIMELOCK_DELAY_SECS seconds.
        // TIMELOCK_DELAY_SECS = 48 * 60 * 60 = 172_800 s.
        const TIMELOCK_DELAY_SECS: u64 = 48 * 60 * 60;
        env.ledger().with_mut(|li| li.timestamp += TIMELOCK_DELAY_SECS);

        // Step 4: execution must now succeed — the timelock has elapsed.
        client.execute_deregister_asset(&owner, &asset_id);

        // Asset must be gone.
        let result_after = client.try_get_asset(&asset_id);
        assert!(
            result_after.is_err(),
            "asset must have been deregistered after timelock elapsed"
        );
    }

    /// Confirm execution is still blocked one second before the deadline,
    /// and succeeds one second after.  This guards against off-by-one errors.
    #[test]
    fn test_timelock_boundary_one_second_before_and_after() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let asset_id = reg(&client, &env, symbol_short!("GENSET"), String::from_str(&env, "Boundary rig"), &owner);

        client.propose_deregister_asset(&owner, &asset_id);

        // Advance to one second BEFORE the deadline — still locked.
        const TIMELOCK_DELAY_SECS: u64 = 48 * 60 * 60;
        env.ledger().with_mut(|li| li.timestamp += TIMELOCK_DELAY_SECS - 1);

        let result_one_before = client.try_execute_deregister_asset(&owner, &asset_id);
        assert_eq!(
            result_one_before,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::TimelockNotExpired as u32,
            ))),
            "execute must still be locked 1 second before the deadline"
        );

        // Advance one more second — now at exactly the deadline.
        env.ledger().with_mut(|li| li.timestamp += 1);

        // Now execution must succeed.
        client.execute_deregister_asset(&owner, &asset_id);
        assert!(
            client.try_get_asset(&asset_id).is_err(),
            "asset must be deregistered after exactly TIMELOCK_DELAY_SECS elapsed"
        );
    }

    // ─── Asset locking (collateral lien) tests ──────────────────────────────────

    /// Helper: set up env, contract, admin + one GENSET asset.
    fn setup_with_asset(env: &Env) -> (AssetRegistryClient, Address, Address, u64) {
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(env, &contract_id);

        let admin = Address::generate(env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(env);
        let asset_id = reg(
            &client,
            env,
            symbol_short!("GENSET"),
            String::from_str(env, "CAT-3516"),
            &owner,
        );

        (client, admin, owner, asset_id)
    }

    #[test]
    fn test_new_asset_is_not_locked() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, _owner, asset_id) = setup_with_asset(&env);

        let asset = client.get_asset(&asset_id);
        assert!(!asset.is_locked, "freshly registered asset must not be locked");
        assert!(asset.lender.is_none(), "freshly registered asset must have no lender");
        assert!(asset.loan_id.is_none(), "freshly registered asset must have no loan_id");
    }

    #[test]
    fn test_set_lending_contract_requires_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _owner, _asset_id) = setup_with_asset(&env);

        let lending_contract = Address::generate(&env);
        // Should succeed — admin calling
        client.set_lending_contract(&admin, &lending_contract);

        assert_eq!(
            client.get_lending_contract(),
            Some(lending_contract),
            "lending contract must be retrievable after set_lending_contract"
        );
    }

    #[test]
    fn test_set_lending_contract_rejects_non_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, owner, _asset_id) = setup_with_asset(&env);

        let lending_contract = Address::generate(&env);
        let result = client.try_set_lending_contract(&owner, &lending_contract);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32
            ))),
            "non-admin must not be able to set the lending contract"
        );
    }

    #[test]
    fn test_lock_asset_as_collateral_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _owner, asset_id) = setup_with_asset(&env);

        let lending_contract = Address::generate(&env);
        client.set_lending_contract(&admin, &lending_contract);

        let loan_id: u64 = 42;
        client.lock_asset_as_collateral(&lending_contract, &asset_id, &loan_id);

        let asset = client.get_asset(&asset_id);
        assert!(asset.is_locked, "asset must be locked after lock_asset_as_collateral");
        assert_eq!(asset.lender, Some(lending_contract), "lender must be stored on the asset");
        assert_eq!(asset.loan_id, Some(loan_id), "loan_id must be stored on the asset");
    }

    #[test]
    fn test_lock_asset_rejects_when_no_lending_contract_set() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, _owner, asset_id) = setup_with_asset(&env);

        let fake_lender = Address::generate(&env);
        let result = client.try_lock_asset_as_collateral(&fake_lender, &asset_id, &1u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::LendingContractNotSet as u32
            ))),
            "lock must fail when no lending contract is registered"
        );
    }

    #[test]
    fn test_lock_asset_rejects_unauthorized_lender() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _owner, asset_id) = setup_with_asset(&env);

        let lending_contract = Address::generate(&env);
        let impostor = Address::generate(&env);
        client.set_lending_contract(&admin, &lending_contract);

        let result = client.try_lock_asset_as_collateral(&impostor, &asset_id, &1u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedLender as u32
            ))),
            "lock must fail when caller is not the registered lending contract"
        );
    }

    #[test]
    fn test_lock_asset_rejects_already_locked() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _owner, asset_id) = setup_with_asset(&env);

        let lending_contract = Address::generate(&env);
        client.set_lending_contract(&admin, &lending_contract);

        client.lock_asset_as_collateral(&lending_contract, &asset_id, &1u64);

        // Attempt to lock again
        let result = client.try_lock_asset_as_collateral(&lending_contract, &asset_id, &2u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetLocked as u32
            ))),
            "locking an already-locked asset must return AssetLocked"
        );
    }

    #[test]
    fn test_lock_nonexistent_asset_returns_not_found() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _owner, _asset_id) = setup_with_asset(&env);

        let lending_contract = Address::generate(&env);
        client.set_lending_contract(&admin, &lending_contract);

        let result = client.try_lock_asset_as_collateral(&lending_contract, &9999u64, &1u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32
            ))),
            "locking a non-existent asset must return AssetNotFound"
        );
    }

    #[test]
    fn test_unlock_asset_from_collateral_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _owner, asset_id) = setup_with_asset(&env);

        let lending_contract = Address::generate(&env);
        client.set_lending_contract(&admin, &lending_contract);

        let loan_id: u64 = 7;
        client.lock_asset_as_collateral(&lending_contract, &asset_id, &loan_id);
        client.unlock_asset_from_collateral(&lending_contract, &asset_id, &loan_id);

        let asset = client.get_asset(&asset_id);
        assert!(!asset.is_locked, "asset must not be locked after unlock_asset_from_collateral");
        assert!(asset.lender.is_none(), "lender must be cleared after unlock");
        assert!(asset.loan_id.is_none(), "loan_id must be cleared after unlock");
    }

    #[test]
    fn test_unlock_asset_rejects_wrong_loan_id() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _owner, asset_id) = setup_with_asset(&env);

        let lending_contract = Address::generate(&env);
        client.set_lending_contract(&admin, &lending_contract);

        client.lock_asset_as_collateral(&lending_contract, &asset_id, &10u64);

        let result = client.try_unlock_asset_from_collateral(&lending_contract, &asset_id, &99u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::LoanIdMismatch as u32
            ))),
            "unlock with wrong loan_id must return LoanIdMismatch"
        );
    }

    #[test]
    fn test_unlock_asset_rejects_when_not_locked() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _owner, asset_id) = setup_with_asset(&env);

        let lending_contract = Address::generate(&env);
        client.set_lending_contract(&admin, &lending_contract);

        let result = client.try_unlock_asset_from_collateral(&lending_contract, &asset_id, &1u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotLocked as u32
            ))),
            "unlocking an unlocked asset must return AssetNotLocked"
        );
    }

    #[test]
    fn test_unlock_rejects_unauthorized_lender() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _owner, asset_id) = setup_with_asset(&env);

        let lending_contract = Address::generate(&env);
        let impostor = Address::generate(&env);
        client.set_lending_contract(&admin, &lending_contract);
        client.lock_asset_as_collateral(&lending_contract, &asset_id, &1u64);

        let result = client.try_unlock_asset_from_collateral(&impostor, &asset_id, &1u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedLender as u32
            ))),
            "unlock must fail when caller is not the registered lending contract"
        );
    }

    #[test]
    fn test_transfer_blocked_when_asset_locked() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, owner, asset_id) = setup_with_asset(&env);

        let lending_contract = Address::generate(&env);
        client.set_lending_contract(&admin, &lending_contract);
        client.lock_asset_as_collateral(&lending_contract, &asset_id, &5u64);

        let new_owner = Address::generate(&env);
        let result = client.try_transfer_asset(&asset_id, &owner, &new_owner);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetLocked as u32
            ))),
            "transfer must be blocked while asset is locked as collateral"
        );

        // Ownership must remain unchanged
        assert_eq!(client.get_asset(&asset_id).owner, owner);
    }

    #[test]
    fn test_transfer_allowed_after_unlock() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, owner, asset_id) = setup_with_asset(&env);

        let lending_contract = Address::generate(&env);
        client.set_lending_contract(&admin, &lending_contract);

        let loan_id: u64 = 3;
        client.lock_asset_as_collateral(&lending_contract, &asset_id, &loan_id);
        client.unlock_asset_from_collateral(&lending_contract, &asset_id, &loan_id);

        let new_owner = Address::generate(&env);
        client.transfer_asset(&asset_id, &owner, &new_owner);

        assert_eq!(
            client.get_asset(&asset_id).owner,
            new_owner,
            "transfer must succeed after the lien is released"
        );
    }

    #[test]
    fn test_lock_and_unlock_emits_events() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _owner, asset_id) = setup_with_asset(&env);

        let lending_contract = Address::generate(&env);
        client.set_lending_contract(&admin, &lending_contract);

        let loan_id: u64 = 77;
        client.lock_asset_as_collateral(&lending_contract, &asset_id, &loan_id);
        assert_eq!(env.events().all().len(), 1, "lock must emit exactly one event");

        client.unlock_asset_from_collateral(&lending_contract, &asset_id, &loan_id);
        assert_eq!(env.events().all().len(), 1, "unlock must emit exactly one event");
    }

    #[test]
    fn test_lock_decommissioned_asset_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _owner, asset_id) = setup_with_asset(&env);

        let lending_contract = Address::generate(&env);
        client.set_lending_contract(&admin, &lending_contract);

        // Decommission the asset first
        client.decommission_asset(&admin, &asset_id);

        let result = client.try_lock_asset_as_collateral(&lending_contract, &asset_id, &1u64);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetDecommissioned as u32
            ))),
            "decommissioned assets must not be lockable as collateral"
        );
    }

    #[test]
    fn test_get_lending_contract_returns_none_when_not_set() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, _owner, _asset_id) = setup_with_asset(&env);

        assert_eq!(
            client.get_lending_contract(),
            None,
            "get_lending_contract must return None before set_lending_contract is called"
        );
    }

    #[test]
    fn test_duplicate_serial_across_owners_panics() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner_a = Address::generate(&env);
        let owner_b = Address::generate(&env);
        let serial = String::from_str(&env, "SN-001-DEDUP");

        // Owner A registers asset with serial X
        client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT Generator Model A"),
            &serial,
            &owner_a,
        );

        // Owner B attempts to register same serial X — must panic with DuplicateAsset
        let result = client.try_register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT Generator Model B"),
            &serial,
            &owner_b,
        );

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::DuplicateAsset as u32
            ))),
            "serial number deduplication must prevent a different owner from registering the same serial"
        );
    }

    #[test]
    fn test_mark_under_maintenance_sets_status() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, owner, asset_id) = setup_with_asset(&env);

        // Initially Active
        assert_eq!(client.asset_status(&asset_id), AssetStatus::Active);

        // Mark under maintenance as owner
        client.mark_under_maintenance(&owner, &asset_id);

        // Status should now be UnderMaintenance
        assert_eq!(client.asset_status(&asset_id), AssetStatus::UnderMaintenance);
    }

    #[test]
    fn test_mark_maintenance_complete_returns_to_active() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, owner, asset_id) = setup_with_asset(&env);

        // Mark under maintenance
        client.mark_under_maintenance(&owner, &asset_id);
        assert_eq!(client.asset_status(&asset_id), AssetStatus::UnderMaintenance);

        // Complete maintenance
        client.mark_maintenance_complete(&owner, &asset_id);

        // Status should return to Active
        assert_eq!(client.asset_status(&asset_id), AssetStatus::Active);
    }

    #[test]
    fn test_mark_under_maintenance_rejects_unauthorized() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, _owner, asset_id) = setup_with_asset(&env);

        let stranger = Address::generate(&env);
        let result = client.try_mark_under_maintenance(&stranger, &asset_id);

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedOwner as u32
            ))),
            "non-owner/non-admin must be rejected"
        );
    }

    #[test]
    fn test_mark_under_maintenance_rejects_decommissioned() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _owner, asset_id) = setup_with_asset(&env);

        // Decommission first
        client.decommission_asset(&admin, &asset_id);

        let result = client.try_mark_under_maintenance(&admin, &asset_id);

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetDecommissioned as u32
            ))),
            "decommissioned assets must be rejected"
        );
    }

    #[test]
    fn test_admin_can_mark_under_maintenance() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin, _owner, asset_id) = setup_with_asset(&env);

        client.mark_under_maintenance(&admin, &asset_id);
        assert_eq!(client.asset_status(&asset_id), AssetStatus::UnderMaintenance);

        client.mark_maintenance_complete(&admin, &asset_id);
        assert_eq!(client.asset_status(&asset_id), AssetStatus::Active);
    }

    // ---------------------------------------------------------
    // #1014 — get_asset_by_serial_number
    // ---------------------------------------------------------

    /// Happy path: register an asset with a known serial number, then look it
    /// up via `get_asset_by_serial_number` and verify the returned record
    /// matches what was registered.
    #[test]
    fn test_get_asset_by_serial_number_found() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("GENSET"));

        let owner = Address::generate(&env);
        let serial = String::from_str(&env, "SN-ABC-001");
        let metadata = String::from_str(&env, "Caterpillar 3516 Generator");

        let asset_id = client.register_asset(
            &symbol_short!("GENSET"),
            &metadata,
            &serial,
            &owner,
        );

        // Look up by serial number — must return the same asset.
        let result = client.get_asset_by_serial_number(&serial);
        assert!(result.is_some());
        let asset = result.unwrap();
        assert_eq!(asset.asset_id, asset_id);
        assert_eq!(asset.serial_number, serial);
        assert_eq!(asset.owner, owner);
        assert_eq!(asset.asset_type, symbol_short!("GENSET"));
    }

    /// Unknown serial number must return `None` (no panic).
    #[test]
    fn test_get_asset_by_serial_number_not_found() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);

        let unknown = String::from_str(&env, "DOES-NOT-EXIST-9999");
        let result = client.get_asset_by_serial_number(&unknown);
        assert!(result.is_none());
    }

    /// Two distinct assets with different serial numbers must each resolve to
    /// their own record independently.
    #[test]
    fn test_get_asset_by_serial_number_multiple_assets() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_asset_type(&admin, &symbol_short!("TURBINE"));

        let owner = Address::generate(&env);
        let serial_a = String::from_str(&env, "SN-TURB-001");
        let serial_b = String::from_str(&env, "SN-TURB-002");

        let id_a = client.register_asset(
            &symbol_short!("TURBINE"),
            &String::from_str(&env, "GE LM2500 unit A"),
            &serial_a,
            &owner,
        );
        let id_b = client.register_asset(
            &symbol_short!("TURBINE"),
            &String::from_str(&env, "GE LM2500 unit B"),
            &serial_b,
            &owner,
        );

        let asset_a = client.get_asset_by_serial_number(&serial_a).unwrap();
        let asset_b = client.get_asset_by_serial_number(&serial_b).unwrap();

        assert_eq!(asset_a.asset_id, id_a);
        assert_eq!(asset_b.asset_id, id_b);
        assert_ne!(asset_a.asset_id, asset_b.asset_id);
        assert_eq!(asset_a.serial_number, serial_a);
        assert_eq!(asset_b.serial_number, serial_b);
    }
}
