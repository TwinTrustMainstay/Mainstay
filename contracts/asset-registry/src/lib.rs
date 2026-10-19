#![no_std]
use shared::error::SharedContractError;
use shared::validation::{require_non_empty_vec, require_string_length};
use shared::{extend_persistent_ttl, require_admin, DEFAULT_TTL_LEDGERS, TTL_THRESHOLD, TTL_TARGET};
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
    /// Engineer's specialization does not match the asset's type.
    /// Mirrors lifecycle::ContractError::SpecializationMismatch so cross-contract
    /// calls decode this discriminant correctly.
    SpecializationMismatch = 30,
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
    InvalidConfig = 44,
    /// Condition not met for transfer (issue #1316).
    ConditionNotMet = 31,
    /// No transfer condition exists for this asset (issue #1316).
    NoTransferCondition = 32,
    /// Bundle merkle root not found (issue #1317).
    BundleNotFound = 33,
    /// Asset not in bundle or invalid merkle proof (issue #1317).
    InvalidBundleProof = 34,
    /// Collateral pool not found (issue #1318).
    PoolNotFound = 35,
    /// Cannot create collateral pool for single asset (issue #1318).
    InvalidPoolSize = 36,
    /// A co-owned asset must use the weighted voting flow for critical changes.
    MultisigRequired = 37,
    /// A co-owner action has already been executed.
    ActionAlreadyExecuted = 38,
    /// Encrypted asset data is malformed or too large.
    InvalidEncryptedData = 39,
    /// The asset has no co-owners.
    NoCoOwners = 40,
    /// The caller is not a co-owner of the asset.
    NotCoOwner = 41,
    /// The co-owner action proposal does not exist.
    ActionProposalNotFound = 42,
    /// Not enough co-owner approval weight to execute the action.
    InsufficientQuorum = 43,
    /// #1758: No model exists under this name.
    ModelNotFound = 50,
    /// #1758: Model input is malformed (dimension, batch size, value range, or learning rate).
    InvalidModelInput = 51,
    /// #1760: No sidecar record exists at the given index.
    SidecarNotFound = 45,
    /// #1760: The sidecar record has been deleted or expired.
    SidecarDeleted = 46,
    /// #1760: The sidecar's retention period has not yet ended.
    RetentionNotElapsed = 49,
    /// #1759: The privacy budget for this query type is exhausted.
    DpBudgetExhausted = 47,
    /// #1759: Epsilon must be in (0, 5000] milli-units.
    DpInvalidEpsilon = 48,
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
            SharedContractError::InvalidSuspensionPeriod => ContractError::InvalidConfig,
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
    /// Co-owners with weighted voting rights. Each tuple is (address, vote_weight).
    /// Empty if the asset has only a single owner.
    pub co_owners: Vec<(Address, u32)>,
}

/// Types of actions that require co-owner voting approval.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionType {
    Transfer = 0,
    Deprecate = 1,
}

/// A proposal for a co-owner action requiring voting.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionProposal {
    pub proposal_id: u64,
    pub asset_id: u64,
    pub action_type: ActionType,
    pub proposed_by: Address,
    pub proposed_at: u64,
    pub new_owner: Option<Address>,
    pub votes: Vec<(Address, bool)>,
    pub executed: bool,
}

/// A single entry in the metadata change history for an asset.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetadataHistoryEntry {
    pub version: u32,
    pub old_hash: BytesN<32>,
    pub new_hash: BytesN<32>,
    pub updated_at: u64,
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

/// Client-side encrypted fields for sensitive asset data.
///
/// The registry stores ciphertext only. Encryption and key management remain
/// outside the contract so each owner can use their organisation's KMS.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedAssetFields {
    pub serial_number: Bytes,
    pub location: Bytes,
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

/// Transfer condition for conditional asset transfers (issue #1316).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferCondition {
    pub condition_type: Symbol, // "SCORE_MIN", "SCORE_MAX", etc.
    pub threshold: u64,
    pub set_at: u64,
}

/// Bundle registration for multi-asset batch registration (issue #1317).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleRegistration {
    pub merkle_root: BytesN<32>,
    pub asset_count: u32,
    pub registered_at: u64,
}

/// Collateral pool for multi-asset loans (issue #1318).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollateralPool {
    pub pool_id: u64,
    pub lender: Address,
    pub asset_ids: Vec<u64>,
    pub created_at: u64,
    pub is_locked: bool,
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    /// Maps a keyword category (arbitrary bytes) to the list of asset IDs tagged with it.
    AssetsByCategory(Bytes),
    /// Maps an owner address to the list of asset IDs they own.
    AssetsByOwner(Address),
    /// Maps (asset_id, proposal_id) to an ActionProposal for co-owner voting.
    ActionProposal(u64, u64),
    /// Stores the next proposal ID counter for an asset.
    ActionProposalCounter(u64),
    /// #1758: Trained linear model per model name.
    MlModel(Symbol),
    /// #1758: Accuracy and drift metrics per model name.
    MlMetrics(Symbol),
    /// #1760: Sidecar content-hash records for an asset.
    AssetSidecars(u64),
    /// #1760: Total off-chain bytes tracked for an asset's sidecars (storage cost tracking).
    SidecarBytes(u64),
    /// #1759: Remaining privacy budget (milli-epsilon) for a query type.
    DpBudget(Symbol),
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
    /// Counts for each asset type represented by the complete result set.
    pub facets: Vec<FacetCount>,
}

/// A count of matching assets grouped by asset type.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FacetCount {
    pub asset_type: Symbol,
    pub count: u32,
}

/// Issue #1629: Asset usage tracking and analytics data
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageAnalytics {
    pub asset_id: u64,
    pub total_usage_hours: u64,
    pub usage_percentage: u32,
    pub last_usage_update: u64,
    pub maintenance_threshold_hours: u64,
}

/// Issue #1629: A single usage record for an asset
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageRecord {
    pub hours_used: u64,
    pub recorded_at: u64,
}

/// Issue #1630: Asset warranty information
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Warranty {
    pub warranty_id: u64,
    pub start_date: u64,
    pub expiry_date: u64,
    pub coverage_type: String,
    pub provider: String,
    pub is_active: bool,
}

/// Issue #1630: Warranty claim with history tracking
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WarrantyClaim {
    pub claim_id: u64,
    pub warranty_id: u64,
    pub claim_reason: String,
    pub claimed_at: u64,
    pub claim_status: ClaimStatus,
}

/// Issue #1630: Warranty claim status
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ClaimStatus {
    Pending = 0,
    Approved = 1,
    Rejected = 2,
    Settled = 3,
}

/// Issue #1631: Asset compliance certification
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComplianceCert {
    pub cert_id: u64,
    pub cert_type: String,
    pub issuer: String,
    pub expiry_date: u64,
    pub standard: String,
    pub issue_date: u64,
}

/// Issue #1631: Compliance status for an asset
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComplianceStatus {
    pub asset_id: u64,
    pub is_compliant: bool,
    pub expired_count: u32,
    pub active_count: u32,
    pub last_verified_at: u64,
}

/// Issue #1632: Maintenance window for an asset
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaintenanceWindow {
    pub day_of_week: u32,
    pub start_hour: u32,
    pub end_hour: u32,
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

/// Storage key prefix for transfer conditions (issue #1316).
const TRANSFER_CONDITION_PREFIX: Symbol = symbol_short!("XFRCOND");

/// Storage key prefix for bundle registrations (issue #1317).
const BUNDLE_PREFIX: Symbol = symbol_short!("BUNDLE");

/// Storage key prefix for collateral pools (issue #1318).
const COLLATERAL_POOL_PREFIX: Symbol = symbol_short!("POOL");

/// Counter for collateral pool IDs (issue #1318).
const POOL_ID_COUNTER: Symbol = symbol_short!("POOL_ID");

pub const DEREG_TOPIC: Symbol = symbol_short!("DEREG");
pub const ADD_TYPE_TOPIC: Symbol = symbol_short!("ADD_TYPE");
pub const RM_TYPE_TOPIC: Symbol = symbol_short!("RM_TYPE");

fn asset_key(id: u64) -> (Symbol, u64) {
    (symbol_short!("ASSET"), id)
}

fn metadata_history_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("META_HIS"), asset_id)
}

// Issue #1629: Storage keys for usage tracking
fn usage_analytics_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("USG_ANA"), asset_id)
}

fn usage_records_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("USG_REC"), asset_id)
}

// Issue #1630: Storage keys for warranty tracking
fn warranties_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("WARR"), asset_id)
}

fn warranty_claims_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("WARR_CL"), asset_id)
}

fn warranty_counter_key() -> Symbol {
    symbol_short!("WARR_CTR")
}

fn claim_counter_key() -> Symbol {
    symbol_short!("CLM_CTR")
}

// Issue #1631: Storage keys for compliance tracking
fn compliance_certs_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("COMP_C"), asset_id)
}

fn compliance_status_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("COMP_S"), asset_id)
}

fn cert_counter_key() -> Symbol {
    symbol_short!("CERT_CTR")
}

// Issue #1632: Storage keys for maintenance windows
fn maintenance_windows_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("MAINT_W"), asset_id)
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

fn encrypted_asset_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("ENC_ASSET"), asset_id)
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
    extend_persistent_ttl(&env, key.clone());
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

/// Pending transfer key for ownership transfers.
fn pending_transfer_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("PXFER"), asset_id)
}

/// Transfer condition key for conditional transfers (issue #1316).
fn transfer_condition_key(asset_id: u64) -> (Symbol, u64) {
    (TRANSFER_CONDITION_PREFIX, asset_id)
}

/// Bundle registration key (issue #1317).
fn bundle_key(merkle_root: &BytesN<32>) -> (Symbol, BytesN<32>) {
    (BUNDLE_PREFIX, merkle_root.clone())
}

/// Collateral pool key (issue #1318).
fn collateral_pool_key(pool_id: u64) -> (Symbol, u64) {
    (COLLATERAL_POOL_PREFIX, pool_id)
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
    extend_persistent_ttl(&env, key.clone());
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
        extend_persistent_ttl(&env, key.clone());
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

/// #1758: Maximum features per model input vector.
pub const ML_MAX_FEATURES: u32 = 8;
/// #1758: Maximum samples accepted by one `train_model` call.
pub const ML_MAX_SAMPLES: u32 = 100;
/// #1758: Maximum epochs accepted by one `train_model` call.
pub const ML_MAX_EPOCHS: u32 = 20;
/// #1758: Maximum candidates accepted by one `rank_engineers` call.
pub const ML_MAX_CANDIDATES: u32 = 50;
/// #1758: Fixed-point scale. Weights, predictions and errors are milli-units.
const ML_SCALE: i128 = 1000;
/// #1758: Bound on each feature and target value, so training arithmetic cannot overflow.
const ML_INPUT_LIMIT: i64 = 1_000_000;
/// #1758: Bound on the learning rate (milli-units), for the same reason.
const ML_RATE_LIMIT: i64 = 1_000_000;
/// #1758: Clamp on each weight and bias (milli-units).
const ML_WEIGHT_LIMIT: i128 = 1_000_000_000;
/// #1758: Drift is flagged when the recent error exceeds this multiple of the baseline...
const ML_DRIFT_FACTOR: u64 = 2;
/// #1758: ...plus this absolute floor (milli-units, one whole unit).
const ML_DRIFT_FLOOR: u64 = 1000;
/// #1758: Outcomes required before drift can be flagged.
const ML_DRIFT_MIN_SAMPLES: u32 = 8;

/// #1758: Linear model. Prediction (milli-units) = bias + sum(weights[i] * features[i]).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MlModel {
    pub weights: Vec<i64>,
    pub bias: i64,
    /// Incremented on every `train_model` call.
    pub version: u32,
    pub trained_samples: u32,
}

/// #1758: Accuracy and drift metrics for a model. Errors are in milli-units.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MlMetrics {
    pub samples: u32,
    pub error_sum_milli: u64,
    /// Mean absolute error over all recorded outcomes.
    pub mae_milli: u64,
    /// Exponential moving average of absolute error (alpha = 1/8).
    pub recent_mae_milli: u64,
    /// Mean absolute error on the training data at the last `train_model` call.
    pub baseline_mae_milli: u64,
    pub drift_detected: bool,
}

fn ml_zero_metrics() -> MlMetrics {
    MlMetrics {
        samples: 0,
        error_sum_milli: 0,
        mae_milli: 0,
        recent_mae_milli: 0,
        baseline_mae_milli: 0,
        drift_detected: false,
    }
}

/// Linear prediction in milli-units. Caller must check the feature width.
fn ml_predict_raw(model: &MlModel, features: &Vec<i64>) -> i128 {
    let mut acc: i128 = model.bias as i128;
    for i in 0..model.weights.len() {
        acc += model.weights.get(i).unwrap() as i128 * features.get(i).unwrap() as i128;
    }
    acc
}

fn ml_clamp(v: i128) -> i64 {
    v.clamp(-ML_WEIGHT_LIMIT, ML_WEIGHT_LIMIT) as i64
}

fn ml_load(env: &Env, model: &Symbol) -> MlModel {
    env.storage()
        .persistent()
        .get::<_, MlModel>(&DataKey::MlModel(model.clone()))
        .unwrap_or_else(|| panic_with_error!(env, ContractError::ModelNotFound))
}

fn ml_check_dim(env: &Env, model: &MlModel, features: &Vec<i64>) {
    if features.len() != model.weights.len() {
        panic_with_error!(env, ContractError::InvalidModelInput);
    }
}

/// #1759: Default privacy budget per asset type, in milli-epsilon (epsilon = 10).
pub const DEFAULT_DP_BUDGET_MILLI: u64 = 10_000;

/// #1759: Fixed-point scale used by the discrete Laplace sampler (1e18).
const DP_SCALE: u128 = 1_000_000_000_000_000_000;

/// #1759: Upper bound on Bernoulli trials per geometric draw (keeps gas bounded).
const DP_MAX_GEOM_STEPS: u64 = 10_000;

/// #1760: Metadata for an off-chain sidecar blob. Only the content hash and
/// metadata live on-chain; the bytes live in content-addressed storage (IPFS / Arweave).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SidecarRecord {
    /// SHA-256 digest of the sidecar bytes.
    pub content_hash: BytesN<32>,
    /// Content address (e.g. `ipfs://...` or `ar://...`).
    pub uri: String,
    /// Size of the off-chain blob, used for storage cost tracking.
    pub size_bytes: u64,
    /// Ledger timestamp when the record was added.
    pub added_at: u64,
    /// Ledger timestamp after which anyone may expire the record. 0 = no expiry.
    pub retain_until: u64,
    /// True once the record has been removed or expired.
    pub deleted: bool,
}

/// #1759: A differential-privacy query over aggregate asset counts.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DpQuery {
    /// Asset type to count.
    pub asset_type: Symbol,
    /// Epsilon for this query, in milli-units (1000 = epsilon 1.0).
    pub epsilon_milli: u32,
}

/// #1759: Noisy aggregate result from [`AssetRegistry::get_dp_stats`].
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoisyStats {
    pub asset_type: Symbol,
    /// Count plus two-sided geometric noise, clamped to zero.
    pub noisy_count: u64,
    /// Epsilon that was charged to the budget for this query.
    pub epsilon_milli: u32,
}

/// #1759: Exact discrete Laplace (two-sided geometric) mechanism helpers.
/// `e^{-x}` in fixed point. Requires `x <= 5 * DP_SCALE` to avoid overflow.
fn dp_exp_neg(x: u128) -> u128 {
    let mut term: u128 = DP_SCALE;
    let mut pos: u128 = DP_SCALE;
    let mut neg: u128 = 0;
    for k in 1u128..60 {
        term = term * x / (DP_SCALE * k);
        if k % 2 == 1 {
            neg += term;
        } else {
            pos += term;
        }
    }
    pos.saturating_sub(neg)
}

/// Samples the number of failures before the first success, where each trial
/// continues with probability `q` (a geometric draw with parameter `q`).
fn dp_geometric(env: &Env, q: u128) -> u64 {
    let mut count = 0u64;
    while count < DP_MAX_GEOM_STEPS {
        let u = env.prng().gen_range::<u64>(0..(DP_SCALE as u64)) as u128;
        if u < q {
            count += 1;
        } else {
            break;
        }
    }
    count
}

/// Marks sidecar `index` for `asset_id` as deleted and releases its tracked bytes.
fn mark_sidecar_deleted(env: &Env, asset_id: u64, index: u32) {
    let key = DataKey::AssetSidecars(asset_id);
    let mut list: Vec<SidecarRecord> = env.storage().persistent().get(&key).unwrap_or(Vec::new(env));
    let mut rec = list
        .get(index)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::SidecarNotFound));
    if rec.deleted {
        panic_with_error!(env, ContractError::SidecarDeleted);
    }
    rec.deleted = true;
    let size = rec.size_bytes;
    list.set(index, rec);
    env.storage().persistent().set(&key, &list);
    extend_persistent_ttl(env, key);

    let bytes_key = DataKey::SidecarBytes(asset_id);
    let total: u64 = env.storage().persistent().get(&bytes_key).unwrap_or(0);
    env.storage()
        .persistent()
        .set(&bytes_key, &total.saturating_sub(size));
    extend_persistent_ttl(env, bytes_key);
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
            co_owners: Vec::new(&env),
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

    /// Register an asset without putting sensitive serial or location data on-chain.
    ///
    /// `serial_number_hash` must be the SHA-256 digest of the canonical serial
    /// number. The digest preserves global deduplication without exposing the
    /// serial. `encrypted_*` values must be produced client-side using an
    /// authenticated encryption scheme and can only be decrypted by authorised
    /// off-chain consumers.
    pub fn register_asset_encrypted(
        env: Env,
        asset_type: Symbol,
        metadata: String,
        serial_number_hash: BytesN<32>,
        encrypted_serial_number: Bytes,
        encrypted_location: Bytes,
        owner: Address,
    ) -> u64 {
        ensure_not_paused(&env);
        owner.require_auth();
        require_string_length(&metadata, "metadata", 256);
        if encrypted_serial_number.is_empty() || encrypted_serial_number.len() > 4096
            || encrypted_location.is_empty() || encrypted_location.len() > 4096
        {
            panic_with_error!(&env, ContractError::InvalidEncryptedData);
        }
        validate_asset_type_symbol(&env, &asset_type);
        if !Self::is_valid_asset_type(env.clone(), asset_type.clone()) {
            panic_with_error!(&env, ContractError::InvalidAssetType);
        }

        let serial_key = serial_dedup_key(&serial_number_hash);
        if env.storage().persistent().has(&serial_key) {
            panic_with_error!(&env, ContractError::DuplicateAsset);
        }
        let meta_hash: BytesN<32> = env.crypto().sha256(&metadata.clone().to_xdr(&env)).into();
        let metadata_key = dedup_key(&owner, &asset_type, &meta_hash);
        if env.storage().persistent().has(&metadata_key) {
            panic_with_error!(&env, ContractError::DuplicateAsset);
        }

        let id: u64 = env.storage().persistent().get(&ASSET_COUNT).unwrap_or(0) + 1;
        let asset = Asset {
            asset_id: id,
            asset_type: asset_type.clone(),
            metadata,
            serial_number: String::from_str(&env, "[encrypted]"),
            owner: owner.clone(),
            registered_at: env.ledger().timestamp(),
            metadata_updated_at: env.ledger().timestamp(),
            metadata_version: 0,
            deprecation_status: DeprecationStatus::Active,
            is_locked: false,
            lender: None,
            loan_id: None,
            deprecated_at: None,
            co_owners: Vec::new(&env),
        };
        let encrypted_key = encrypted_asset_key(id);
        let encrypted = EncryptedAssetFields {
            serial_number: encrypted_serial_number,
            location: encrypted_location,
        };
        env.storage().persistent().set(&asset_key(id), &asset);
        env.storage().persistent().set(&encrypted_key, &encrypted);
        env.storage().persistent().set(&ASSET_COUNT, &id);
        env.storage().persistent().set(&metadata_key, &id);
        env.storage().persistent().set(&serial_key, &id);
        extend_persistent_ttl(&env, &asset_key(id));
        extend_persistent_ttl(&env, &encrypted_key);
        extend_persistent_ttl(&env, &ASSET_COUNT);
        extend_persistent_ttl(&env, &metadata_key);
        extend_persistent_ttl(&env, &serial_key);
        owner_index_add(&env, &owner, id);
        type_count_inc(&env, &asset_type);
        type_assets_add(&env, &asset_type, id);
        env.events().publish(
            (symbol_short!("reg_asset"),),
            (id, owner, env.ledger().timestamp()),
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
                co_owners: Vec::new(&env),
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
            extend_persistent_ttl(&env, owner_index_key(&owner));
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

    /// Return all registered assets from `asset_ids` in one invocation.
    ///
    /// Missing IDs are omitted, matching the tolerant behavior of the other
    /// batch read endpoints. The input is bounded to keep response and
    /// instruction usage predictable for API clients.
    pub fn batch_get_assets(env: Env, asset_ids: Vec<u64>) -> Vec<Asset> {
        if asset_ids.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, ContractError::BatchTooLarge);
        }

        let mut assets = Vec::new(&env);
        for asset_id in asset_ids.iter() {
            let key = asset_key(asset_id);
            if let Some(asset) = env.storage().persistent().get(&key) {
                env.storage()
                    .persistent()
                    .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
                assets.push_back(asset);
            }
        }
        assets
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
            extend_persistent_ttl(&env, key.clone());
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
            extend_persistent_ttl(&env, key.clone());
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
            extend_persistent_ttl(&env, key.clone());
            env.storage()
                .persistent()
                .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);
        }

        // Exclude decommissioned assets from both the returned page and the total count.
        let mut active: Vec<u64> = Vec::new(&env);
        for id in all.iter() {
            let is_decommissioned: bool = env
                .storage()
                .persistent()
                .get(&decommissioned_key(id))
                .unwrap_or(false);
            if !is_decommissioned {
                active.push_back(id);
            }
        }

        let total = active.len();

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
            assets.push_back(active.get(i).unwrap());
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
            extend_persistent_ttl(&env, key.clone());
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

    /// #1758: Train or continue training a linear model with per-sample SGD. Admin-only.
    ///
    /// Each epoch makes one pass over the batch. `learning_rate_milli` is the
    /// per-sample rate in milli-units (1 = 0.001). The baseline error for drift
    /// detection is recomputed on the training batch after each call.
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::InvalidModelInput`] if the batch is empty or too large, rows differ
    ///   in width, a value is out of range, or the rate is not in `(0, ML_RATE_LIMIT]`
    pub fn train_model(
        env: Env,
        admin: Address,
        model: Symbol,
        features: Vec<Vec<i64>>,
        targets: Vec<i64>,
        epochs: u32,
        learning_rate_milli: i64,
    ) {
        let stored_admin: Address = Self::get_admin(env.clone());
        if require_admin(&admin, &stored_admin).is_err() {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        let n = features.len();
        if n == 0
            || n > ML_MAX_SAMPLES
            || n != targets.len()
            || epochs == 0
            || epochs > ML_MAX_EPOCHS
            || learning_rate_milli <= 0
            || learning_rate_milli > ML_RATE_LIMIT
        {
            panic_with_error!(&env, ContractError::InvalidModelInput);
        }
        let dim = features.get(0).unwrap().len();
        if dim == 0 || dim > ML_MAX_FEATURES {
            panic_with_error!(&env, ContractError::InvalidModelInput);
        }
        for row in features.iter() {
            if row.len() != dim {
                panic_with_error!(&env, ContractError::InvalidModelInput);
            }
            for v in row.iter() {
                if v > ML_INPUT_LIMIT || v < -ML_INPUT_LIMIT {
                    panic_with_error!(&env, ContractError::InvalidModelInput);
                }
            }
        }
        for t in targets.iter() {
            if t > ML_INPUT_LIMIT || t < -ML_INPUT_LIMIT {
                panic_with_error!(&env, ContractError::InvalidModelInput);
            }
        }

        let key = DataKey::MlModel(model.clone());
        let mut m: MlModel = match env.storage().persistent().get(&key) {
            Some(existing) => existing,
            None => {
                let mut w = Vec::new(&env);
                for _ in 0..dim {
                    w.push_back(0i64);
                }
                MlModel {
                    weights: w,
                    bias: 0,
                    version: 0,
                    trained_samples: 0,
                }
            }
        };
        if m.weights.len() != dim {
            panic_with_error!(&env, ContractError::InvalidModelInput);
        }

        let lr = learning_rate_milli as i128;
        for _ in 0..epochs {
            for s in 0..n {
                let x = features.get(s).unwrap();
                let y_milli = targets.get(s).unwrap() as i128 * ML_SCALE;
                let err = ml_predict_raw(&m, &x) - y_milli;
                for i in 0..dim {
                    let w = m.weights.get(i).unwrap() as i128;
                    let xi = x.get(i).unwrap() as i128;
                    m.weights.set(i, ml_clamp(w - lr * err * xi / ML_SCALE));
                }
                m.bias = ml_clamp(m.bias as i128 - lr * err / ML_SCALE);
            }
        }

        let mut abs_sum: u64 = 0;
        for s in 0..n {
            let x = features.get(s).unwrap();
            let y_milli = targets.get(s).unwrap() as i128 * ML_SCALE;
            let e = (ml_predict_raw(&m, &x) - y_milli).unsigned_abs() as u64;
            abs_sum = abs_sum.saturating_add(e);
        }
        let baseline = abs_sum / n as u64;

        m.version = m.version.saturating_add(1);
        m.trained_samples = m.trained_samples.saturating_add(n);
        env.storage().persistent().set(&key, &m);
        extend_persistent_ttl(&env, key);

        let mkey = DataKey::MlMetrics(model.clone());
        let mut metrics: MlMetrics = env
            .storage()
            .persistent()
            .get(&mkey)
            .unwrap_or(ml_zero_metrics());
        metrics.baseline_mae_milli = baseline;
        metrics.drift_detected = false;
        env.storage().persistent().set(&mkey, &metrics);
        extend_persistent_ttl(&env, mkey);
    }

    /// #1758: Model serving. Returns the prediction in milli-units for the given features.
    ///
    /// # Panics
    /// - [`ContractError::ModelNotFound`] if the model has not been trained
    /// - [`ContractError::InvalidModelInput`] if the feature width does not match the model
    pub fn predict_model(env: Env, model: Symbol, features: Vec<i64>) -> i64 {
        let m = ml_load(&env, &model);
        ml_check_dim(&env, &m, &features);
        ml_predict_raw(&m, &features).clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }

    /// #1758: Rank candidates by predicted score, highest first. Ties keep input order.
    ///
    /// # Panics
    /// - [`ContractError::ModelNotFound`] if the model has not been trained
    /// - [`ContractError::InvalidModelInput`] if candidates and feature rows differ in length,
    ///   the batch exceeds `ML_MAX_CANDIDATES`, or a row has the wrong width
    pub fn rank_engineers(
        env: Env,
        model: Symbol,
        candidates: Vec<Address>,
        features: Vec<Vec<i64>>,
    ) -> Vec<Address> {
        let m = ml_load(&env, &model);
        let n = candidates.len();
        if n != features.len() || n > ML_MAX_CANDIDATES {
            panic_with_error!(&env, ContractError::InvalidModelInput);
        }
        let mut ranked: Vec<Address> = Vec::new(&env);
        let mut scores: Vec<i128> = Vec::new(&env);
        for i in 0..n {
            let row = features.get(i).unwrap();
            ml_check_dim(&env, &m, &row);
            let score = ml_predict_raw(&m, &row);
            let mut pos = scores.len();
            for j in 0..scores.len() {
                if score > scores.get(j).unwrap() {
                    pos = j;
                    break;
                }
            }
            scores.insert(pos, score);
            ranked.insert(pos, candidates.get(i).unwrap());
        }
        ranked
    }

    /// #1758: Record an observed outcome for a model input. Admin-only.
    ///
    /// Updates the sample count, mean absolute error, and an exponential moving
    /// average of the error. Drift is flagged when the recent error rises well above
    /// the baseline error measured at training time.
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    /// - [`ContractError::ModelNotFound`] / [`ContractError::InvalidModelInput`] as above
    pub fn record_model_outcome(
        env: Env,
        admin: Address,
        model: Symbol,
        features: Vec<i64>,
        actual: i64,
    ) {
        let stored_admin: Address = Self::get_admin(env.clone());
        if require_admin(&admin, &stored_admin).is_err() {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        let m = ml_load(&env, &model);
        ml_check_dim(&env, &m, &features);
        let err = (ml_predict_raw(&m, &features) - actual as i128 * ML_SCALE).unsigned_abs() as u64;

        let mkey = DataKey::MlMetrics(model.clone());
        let mut metrics: MlMetrics = env
            .storage()
            .persistent()
            .get(&mkey)
            .unwrap_or(ml_zero_metrics());
        metrics.samples = metrics.samples.saturating_add(1);
        metrics.error_sum_milli = metrics.error_sum_milli.saturating_add(err);
        metrics.mae_milli = metrics.error_sum_milli / metrics.samples as u64;
        metrics.recent_mae_milli = if metrics.samples == 1 {
            err
        } else {
            (7 * metrics.recent_mae_milli + err) / 8
        };
        metrics.drift_detected = metrics.samples >= ML_DRIFT_MIN_SAMPLES
            && metrics.recent_mae_milli
                > ML_DRIFT_FACTOR * metrics.baseline_mae_milli + ML_DRIFT_FLOOR;
        env.storage().persistent().set(&mkey, &metrics);
        extend_persistent_ttl(&env, mkey);
    }

    /// #1758: Accuracy and drift metrics for a model.
    pub fn get_model_metrics(env: Env, model: Symbol) -> MlMetrics {
        env.storage()
            .persistent()
            .get(&DataKey::MlMetrics(model))
            .unwrap_or(ml_zero_metrics())
    }

    /// #1758: The trained model, or `None` if it has not been trained.
    pub fn get_model(env: Env, model: Symbol) -> Option<MlModel> {
        env.storage().persistent().get(&DataKey::MlModel(model))
    }

    /// #1760: Record an off-chain sidecar blob for an asset. Owner-only.
    ///
    /// Only the SHA-256 digest and metadata are stored on-chain; the bytes must
    /// be uploaded to content-addressed storage (IPFS / Arweave) separately.
    ///
    /// # Returns
    /// The index of the new sidecar record for this asset.
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    /// - [`ContractError::UnauthorizedOwner`] if caller is not the asset owner
    pub fn add_sidecar(
        env: Env,
        caller: Address,
        asset_id: u64,
        content_hash: BytesN<32>,
        uri: String,
        size_bytes: u64,
        retain_until: u64,
    ) -> u32 {
        caller.require_auth();
        let asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));
        if asset.owner != caller {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        let key = DataKey::AssetSidecars(asset_id);
        let mut list: Vec<SidecarRecord> =
            env.storage().persistent().get(&key).unwrap_or(Vec::new(&env));
        let index = list.len();
        list.push_back(SidecarRecord {
            content_hash: content_hash.clone(),
            uri,
            size_bytes,
            added_at: env.ledger().timestamp(),
            retain_until,
            deleted: false,
        });
        env.storage().persistent().set(&key, &list);
        extend_persistent_ttl(&env, key);

        let bytes_key = DataKey::SidecarBytes(asset_id);
        let total: u64 = env.storage().persistent().get(&bytes_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&bytes_key, &total.saturating_add(size_bytes));
        extend_persistent_ttl(&env, bytes_key);

        env.events()
            .publish((symbol_short!("sc_add"), asset_id), (index, content_hash));
        index
    }

    /// #1760: Verify downloaded sidecar bytes against the on-chain digest.
    ///
    /// # Returns
    /// `true` if `SHA-256(content)` equals the stored digest, `false` otherwise.
    ///
    /// # Panics
    /// - [`ContractError::SidecarNotFound`] if no record exists at `index`
    /// - [`ContractError::SidecarDeleted`] if the record was removed or expired
    pub fn verify_sidecar(env: Env, asset_id: u64, index: u32, content: Bytes) -> bool {
        let list: Vec<SidecarRecord> = env
            .storage()
            .persistent()
            .get(&DataKey::AssetSidecars(asset_id))
            .unwrap_or(Vec::new(&env));
        let rec = list
            .get(index)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::SidecarNotFound));
        if rec.deleted {
            panic_with_error!(&env, ContractError::SidecarDeleted);
        }
        let digest: BytesN<32> = env.crypto().sha256(&content).to_bytes();
        digest == rec.content_hash
    }

    /// #1760: Remove a sidecar record. Owner-only. The off-chain bytes must be
    /// unpinned by the owner; this marks the on-chain record deleted and releases tracked bytes.
    pub fn remove_sidecar(env: Env, caller: Address, asset_id: u64, index: u32) {
        caller.require_auth();
        let asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));
        if asset.owner != caller {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }
        mark_sidecar_deleted(&env, asset_id, index);
    }

    /// #1760: Expire a sidecar whose `retain_until` has passed. Callable by anyone,
    /// so retention is enforced without the owner's cooperation.
    ///
    /// # Panics
    /// - [`ContractError::SidecarNotFound`] / [`ContractError::SidecarDeleted`] as above
    /// - [`ContractError::RetentionNotElapsed`] if the retention period has not yet ended
    pub fn expire_sidecar(env: Env, asset_id: u64, index: u32) {
        let list: Vec<SidecarRecord> = env
            .storage()
            .persistent()
            .get(&DataKey::AssetSidecars(asset_id))
            .unwrap_or(Vec::new(&env));
        let rec = list
            .get(index)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::SidecarNotFound));
        let expired = rec.retain_until != 0 && env.ledger().timestamp() >= rec.retain_until;
        if !expired {
            panic_with_error!(&env, ContractError::RetentionNotElapsed);
        }
        mark_sidecar_deleted(&env, asset_id, index);
    }

    /// #1760: All sidecar records for an asset, including deleted ones.
    pub fn get_sidecars(env: Env, asset_id: u64) -> Vec<SidecarRecord> {
        env.storage()
            .persistent()
            .get(&DataKey::AssetSidecars(asset_id))
            .unwrap_or(Vec::new(&env))
    }

    /// #1760: Total off-chain bytes currently tracked for an asset's live sidecars.
    pub fn get_sidecar_bytes(env: Env, asset_id: u64) -> u64 {
        env.storage()
            .persistent()
            .get(&DataKey::SidecarBytes(asset_id))
            .unwrap_or(0)
    }

    /// #1759: Remaining privacy budget (milli-epsilon) for an asset type.
    pub fn get_dp_budget(env: Env, asset_type: Symbol) -> u64 {
        env.storage()
            .persistent()
            .get(&DataKey::DpBudget(asset_type))
            .unwrap_or(DEFAULT_DP_BUDGET_MILLI)
    }

    /// #1759: Set the privacy budget for an asset type. Admin-only.
    pub fn set_dp_budget(env: Env, admin: Address, asset_type: Symbol, budget_milli: u64) {
        let stored_admin: Address = Self::get_admin(env.clone());
        if require_admin(&admin, &stored_admin).is_err() {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        let key = DataKey::DpBudget(asset_type);
        env.storage().persistent().set(&key, &budget_milli);
        extend_persistent_ttl(&env, key);
    }

    /// #1759: Differentially private count of assets of a given type.
    ///
    /// Adds two-sided geometric noise (the discrete Laplace mechanism), which
    /// gives epsilon-differential privacy for a count with sensitivity 1
    /// (adding or removing one asset changes the count by at most 1).
    /// Each call consumes `epsilon_milli` from the asset type's budget.
    ///
    /// # Panics
    /// - [`ContractError::DpInvalidEpsilon`] if epsilon is 0 or above 5000 milli-units
    /// - [`ContractError::DpBudgetExhausted`] if the remaining budget is below epsilon
    pub fn get_dp_stats(env: Env, query: DpQuery) -> NoisyStats {
        if query.epsilon_milli == 0 || query.epsilon_milli > 5000 {
            panic_with_error!(&env, ContractError::DpInvalidEpsilon);
        }
        let remaining = Self::get_dp_budget(env.clone(), query.asset_type.clone());
        let cost = query.epsilon_milli as u64;
        if remaining < cost {
            panic_with_error!(&env, ContractError::DpBudgetExhausted);
        }
        let budget_key = DataKey::DpBudget(query.asset_type.clone());
        env.storage()
            .persistent()
            .set(&budget_key, &(remaining - cost));
        extend_persistent_ttl(&env, budget_key);

        let true_count = Self::get_assets_by_type(env.clone(), query.asset_type.clone()).len() as i128;
        let eps = (query.epsilon_milli as u128) * DP_SCALE / 1000;
        let q = dp_exp_neg(eps);
        let g1 = dp_geometric(&env, q) as i128;
        let g2 = dp_geometric(&env, q) as i128;
        let noisy = (true_count + g1 - g2).max(0) as u64;

        NoisyStats {
            asset_type: query.asset_type,
            noisy_count: noisy,
            epsilon_milli: query.epsilon_milli,
        }
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
            (symbol_short!("ADM_CNCL"),),
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
    /// - [`ContractError::AssetDecommissioned`] if the asset is `Deprecated` or `Decommissioned`
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

        // Co-owned assets cannot bypass their weighted approval policy through
        // the legacy single-signer transfer entry point. Use propose_action,
        // vote_on_action, and execute_action instead.
        if !asset.co_owners.is_empty() {
            panic_with_error!(&env, ContractError::MultisigRequired);
        }

        if current_owner == new_owner {
            panic_with_error!(&env, ContractError::SameOwner);
        }

        // Block transfers while the asset is locked as collateral under a lien.
        if asset.is_locked {
            panic_with_error!(&env, ContractError::AssetLocked);
        }

        // Deprecated (and decommissioned) assets are not transferable.
        if asset.deprecation_status != DeprecationStatus::Active {
            panic_with_error!(&env, ContractError::AssetDecommissioned);
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
    /// - [`ContractError::AssetDecommissioned`] if the asset is `Deprecated` or `Decommissioned`
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

        // Deprecated (and decommissioned) assets are not transferable.
        if asset.deprecation_status != DeprecationStatus::Active {
            panic_with_error!(&env, ContractError::AssetDecommissioned);
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

        // Issue #1316: Check if a transfer condition exists and is met
        let cond_key = transfer_condition_key(asset_id);
        if let Some(condition) = env.storage().persistent().get::<_, TransferCondition>(&cond_key) {
            // If lifecycle contract is available, check the condition
            if let Some(lifecycle_addr) = env.storage().instance().get::<_, Address>(&LIFECYCLE_KEY) {
                let score: u32 = env.invoke_contract(
                    &lifecycle_addr,
                    &Symbol::new(&env, "get_collateral_score"),
                    soroban_sdk::vec![&env, soroban_sdk::IntoVal::<Env, soroban_sdk::Val>::into_val(&asset_id, &env)],
                );

                // Check condition based on type
                let ctype = condition.condition_type.clone();
                if ctype == Symbol::new(&env, "SCORE_MIN") && u64::from(score) < condition.threshold {
                    panic_with_error!(&env, ContractError::ConditionNotMet);
                }
                if ctype == Symbol::new(&env, "SCORE_MAX") && u64::from(score) > condition.threshold {
                    panic_with_error!(&env, ContractError::ConditionNotMet);
                }

                // Clear condition after successful check
                env.storage().persistent().remove(&cond_key);
            }
        }

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
        if let Some(lifecycle_addr) = env.storage().instance().get::<_, Address>(&LIFECYCLE_KEY) {
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

    /// Issue #1316: Propose a transfer condition for an asset (recipient counter-offer).
    /// The transfer recipient can suggest a condition that must be met before accepting.
    /// For example, asset's collateral score must reach a minimum threshold.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset with a pending transfer
    /// * `condition_type` - Type of condition (e.g., "SCORE_MIN")
    /// * `threshold` - The threshold value for the condition
    ///
    /// # Panics
    /// - [`ContractError::NoPendingTransfer`] if no transfer is pending
    pub fn propose_transfer_condition(env: Env, asset_id: u64, condition_type: Symbol, threshold: u64) {
        ensure_not_paused(&env);

        let transfer_key = pending_transfer_key(asset_id);
        let pending: PendingTransfer = env
            .storage()
            .persistent()
            .get(&transfer_key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NoPendingTransfer));

        pending.new_owner.require_auth();

        let condition = TransferCondition {
            condition_type: condition_type.clone(),
            threshold,
            set_at: env.ledger().timestamp(),
        };

        let cond_key = transfer_condition_key(asset_id);
        env.storage().persistent().set(&cond_key, &condition);
        env.storage()
            .persistent()
            .extend_ttl(&cond_key, TTL_THRESHOLD, TTL_TARGET);

        env.events().publish(
            (symbol_short!("COND_SET"), asset_id),
            (condition_type, threshold),
        );
    }

    /// Issue #1316: Get transfer condition for an asset, if one exists.
    pub fn get_transfer_condition(env: Env, asset_id: u64) -> Option<TransferCondition> {
        let cond_key = transfer_condition_key(asset_id);
        env.storage().persistent().get(&cond_key)
    }

    /// Issue #1317: Register a bundle of assets using merkle root.
    /// Enables fleet operators to batch-register assets by proving inclusion in merkle tree.
    ///
    /// # Arguments
    /// * `merkle_root` - The merkle root of asset metadata
    /// * `asset_count` - Total number of assets in this bundle
    ///
    /// # Panics
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not admin
    pub fn register_asset_bundle(env: Env, admin: Address, merkle_root: BytesN<32>, asset_count: u32) {
        ensure_not_paused(&env);
        admin.require_auth();

        let stored_admin: Address = Self::get_admin(env.clone());
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        let bundle = BundleRegistration {
            merkle_root: merkle_root.clone(),
            asset_count,
            registered_at: env.ledger().timestamp(),
        };

        let key = bundle_key(&merkle_root);
        env.storage().persistent().set(&key, &bundle);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);

        env.events().publish(
            (symbol_short!("BUNDL_REG"), merkle_root.clone()),
            asset_count,
        );
    }

    /// Issue #1317: Get bundle registration by merkle root.
    pub fn get_bundle(env: Env, merkle_root: BytesN<32>) -> Option<BundleRegistration> {
        let key = bundle_key(&merkle_root);
        env.storage().persistent().get(&key)
    }

    /// Issue #1318: Create a collateral pool for multi-asset loans.
    /// Locks multiple assets atomically as collateral for a single loan.
    ///
    /// # Arguments
    /// * `asset_ids` - Vector of asset IDs to include in pool (must be > 1)
    /// * `lender` - Address of the lender
    ///
    /// # Panics
    /// - [`ContractError::InvalidPoolSize`] if asset_ids has < 2 assets
    /// - [`ContractError::AssetNotFound`] if any asset doesn't exist
    pub fn create_collateral_pool(env: Env, lender: Address, asset_ids: Vec<u64>) -> u64 {
        ensure_not_paused(&env);
        lender.require_auth();

        if asset_ids.len() < 2 {
            panic_with_error!(&env, ContractError::InvalidPoolSize);
        }

        // Verify all assets exist
        for asset_id in asset_ids.iter() {
            if !Self::asset_exists(env.clone(), asset_id) {
                panic_with_error!(&env, ContractError::AssetNotFound);
            }
        }

        // Generate pool ID
        let pool_id_key = POOL_ID_COUNTER;
        let mut pool_id: u64 = env
            .storage()
            .persistent()
            .get(&pool_id_key)
            .unwrap_or(0);
        pool_id += 1;

        let pool = CollateralPool {
            pool_id,
            lender: lender.clone(),
            asset_ids: asset_ids.clone(),
            created_at: env.ledger().timestamp(),
            is_locked: true,
        };

        let key = collateral_pool_key(pool_id);
        env.storage().persistent().set(&key, &pool);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);

        // Update counter
        env.storage().persistent().set(&pool_id_key, &pool_id);

        env.events().publish(
            (symbol_short!("POOL_CRT"), pool_id),
            (lender, asset_ids.len() as u32),
        );

        pool_id
    }

    /// Issue #1318: Get collateral pool by ID.
    pub fn get_collateral_pool(env: Env, pool_id: u64) -> Option<CollateralPool> {
        let key = collateral_pool_key(pool_id);
        env.storage().persistent().get(&key)
    }

    /// Issue #1318: Release a collateral pool on loan repayment.
    pub fn release_collateral_pool(env: Env, lender: Address, pool_id: u64) {
        ensure_not_paused(&env);
        lender.require_auth();

        let key = collateral_pool_key(pool_id);
        let mut pool: CollateralPool = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::PoolNotFound));

        if pool.lender != lender {
            panic_with_error!(&env, ContractError::UnauthorizedLender);
        }

        pool.is_locked = false;
        env.storage().persistent().set(&key, &pool);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_TARGET);

        env.events().publish(
            (symbol_short!("POOL_REL"), pool_id),
            env.ledger().timestamp(),
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

        // Guard: removing the last remaining allowed asset type would make
        // `register_asset`'s allowlist check reject every future registration
        // with no way to recover on-chain. Reject the removal instead so at
        // least one allowed type always remains.
        let allowed_types = allowed_asset_types(&env);
        let contains_type = allowed_types.iter().any(|existing| existing == asset_type);
        if contains_type && allowed_types.len() == 1 {
            panic_with_error!(&env, ContractError::InvalidAssetType);
        }

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
        let mut facet_counts: Vec<(Symbol, u32)> = Vec::new(&env);

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
            let mut facet_found = false;
            for i in 0..facet_counts.len() {
                let (facet_type, count) = facet_counts.get(i).unwrap();
                if facet_type == asset.asset_type {
                    facet_counts.set(i, (facet_type, count + 1));
                    facet_found = true;
                    break;
                }
            }
            if !facet_found {
                facet_counts.push_back((asset.asset_type.clone(), 1));
            }
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

        let mut facets: Vec<FacetCount> = Vec::new(&env);
        for i in 0..facet_counts.len() {
            let (asset_type, count) = facet_counts.get(i).unwrap();
            facets.push_back(FacetCount { asset_type, count });
        }

        SearchPage { assets: matched, total: total_matched, facets }
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
            (symbol_short!("MNT_START"), asset_id),
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

    // ---------------------------------------------------------------------------
    //  Co-ownership and Weighted Voting Functions
    // ---------------------------------------------------------------------------

    /// Propose an action (transfer or deprecation) for co-owner voting.
    ///
    /// Creates a new voting proposal that requires quorum approval from all co-owners.
    /// Only callable by the primary owner or admin.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    /// * `action_type` - The type of action (Transfer or Deprecate)
    /// * `new_owner` - Required for Transfer actions, None for Deprecate actions
    ///
    /// # Returns
    /// The ID of the created proposal
    ///
    /// # Panics
    /// - [`ContractError::AssetNotFound`] if the asset does not exist
    /// - [`ContractError::NoCoOwners`] if the asset has no co-owners
    pub fn propose_action(
        env: Env,
        caller: Address,
        asset_id: u64,
        action_type: ActionType,
        new_owner: Option<Address>,
    ) -> u64 {
        ensure_not_paused(&env);
        caller.require_auth();

        let asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));

        if asset.co_owners.is_empty() {
            panic_with_error!(&env, ContractError::NoCoOwners);
        }

        // Only primary owner or admin can propose
        let admin = Self::get_admin(env.clone());
        if caller != asset.owner && caller != admin {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        // Get next proposal ID
        let counter_key = DataKey::ActionProposalCounter(asset_id);
        let proposal_id: u64 = env
            .storage()
            .persistent()
            .get(&counter_key)
            .unwrap_or(0u64);

        let next_id = proposal_id.saturating_add(1);
        env.storage().persistent().set(&counter_key, &next_id);

        // Create new proposal
        let proposal = ActionProposal {
            proposal_id,
            asset_id,
            action_type,
            proposed_by: caller.clone(),
            proposed_at: env.ledger().timestamp(),
            new_owner,
            votes: Vec::new(&env),
            executed: false,
        };

        let proposal_key = DataKey::ActionProposal(asset_id, proposal_id);
        env.storage().persistent().set(&proposal_key, &proposal);

        env.events().publish(
            (symbol_short!("ACT_PROP"), asset_id),
            (proposal_id, action_type, env.ledger().timestamp()),
        );

        proposal_id
    }

    /// Vote on a proposed action for co-ownership transfer or deprecation.
    ///
    /// Calculates quorum based on total vote weight of all co-owners. A proposal
    /// requires >50% of total vote weight to execute.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    /// * `proposal_id` - The ID of the proposal to vote on
    /// * `approve` - Whether to approve (true) or reject (false) the action
    ///
    /// # Panics
    /// - [`ContractError::ActionProposalNotFound`] if the proposal does not exist
    /// - [`ContractError::NotCoOwner`] if caller is not a co-owner
    pub fn vote_on_action(
        env: Env,
        caller: Address,
        asset_id: u64,
        proposal_id: u64,
        approve: bool,
    ) {
        ensure_not_paused(&env);
        caller.require_auth();

        let proposal_key = DataKey::ActionProposal(asset_id, proposal_id);
        let mut proposal: ActionProposal = env
            .storage()
            .persistent()
            .get(&proposal_key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::ActionProposalNotFound));

        let asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));

        // Verify caller is a co-owner
        let mut is_co_owner = false;
        for (owner, _weight) in asset.co_owners.iter() {
            if owner == caller {
                is_co_owner = true;
                break;
            }
        }

        if !is_co_owner {
            panic_with_error!(&env, ContractError::NotCoOwner);
        }

        // Add or update vote
        let mut already_voted = false;
        for i in 0..proposal.votes.len() {
            let (voter, _) = proposal.votes.get(i).unwrap();
            if voter == caller {
                proposal.votes.set(i, (caller.clone(), approve));
                already_voted = true;
                break;
            }
        }

        if !already_voted {
            proposal.votes.push_back((caller.clone(), approve));
        }

        env.storage().persistent().set(&proposal_key, &proposal);

        env.events().publish(
            (symbol_short!("VOTE"), asset_id),
            (proposal_id, caller, approve),
        );
    }

    /// Execute a proposal if quorum has been reached.
    ///
    /// Calculates total votes and checks if approval votes exceed 50% of total
    /// co-owner weight. If quorum is met, executes the proposed action.
    ///
    /// # Arguments
    /// * `asset_id` - The unique identifier of the asset
    /// * `proposal_id` - The ID of the proposal to execute
    ///
    /// # Panics
    /// - [`ContractError::ActionProposalNotFound`] if the proposal does not exist
    /// - [`ContractError::InsufficientQuorum`] if vote weight does not meet quorum
    pub fn execute_action(env: Env, caller: Address, asset_id: u64, proposal_id: u64) {
        ensure_not_paused(&env);
        caller.require_auth();

        let proposal_key = DataKey::ActionProposal(asset_id, proposal_id);
        let mut proposal: ActionProposal = env
            .storage()
            .persistent()
            .get(&proposal_key)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::ActionProposalNotFound));

        if proposal.executed {
            panic_with_error!(&env, ContractError::ActionAlreadyExecuted);
        }

        let mut asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));

        // Calculate vote weights
        let mut total_weight = 0u32;
        let mut approval_weight = 0u32;

        for (owner, weight) in asset.co_owners.iter() {
            total_weight = total_weight.saturating_add(weight);

            for (voter, vote) in proposal.votes.iter() {
                if voter == owner && vote {
                    approval_weight = approval_weight.saturating_add(weight);
                }
            }
        }

        // Check quorum: > 50% of total weight
        if approval_weight.saturating_mul(2) <= total_weight {
            panic_with_error!(&env, ContractError::InsufficientQuorum);
        }

        // Execute the action
        match proposal.action_type {
            ActionType::Transfer => {
                if let Some(new_owner) = proposal.new_owner.clone() {
                    if asset.is_locked {
                        panic_with_error!(&env, ContractError::AssetLocked);
                    }
                    if asset.deprecation_status != DeprecationStatus::Active {
                        panic_with_error!(&env, ContractError::AssetDecommissioned);
                    }
                    let old_owner = asset.owner.clone();
                    let hash: BytesN<32> = env
                        .crypto()
                        .sha256(&asset.metadata.clone().to_xdr(&env))
                        .into();
                    env.storage()
                        .persistent()
                        .remove(&dedup_key(&old_owner, &asset.asset_type, &hash));
                    env.storage()
                        .persistent()
                        .set(&dedup_key(&new_owner, &asset.asset_type, &hash), &asset_id);
                    extend_persistent_ttl(
                        &env,
                        &dedup_key(&new_owner, &asset.asset_type, &hash),
                    );
                    owner_index_remove(&env, &old_owner, asset_id);
                    owner_index_add(&env, &new_owner, asset_id);
                    // Transfer asset to new owner
                    asset.owner = new_owner.clone();
                    env.storage()
                        .persistent()
                        .set(&asset_key(asset_id), &asset);
                    extend_persistent_ttl(&env, &asset_key(asset_id));
                    if let Some(lifecycle_addr) =
                        env.storage().instance().get::<_, Address>(&LIFECYCLE_KEY)
                    {
                        lifecycle::LifecycleClient::new(&env, &lifecycle_addr)
                            .transfer_notify(&asset_id, &new_owner);
                    }

                    env.events().publish(
                        (symbol_short!("XFER_EXEC"), asset_id),
                        (caller, env.ledger().timestamp()),
                    );
                }
            }
            ActionType::Deprecate => {
                // Deprecate the asset
                asset.deprecation_status = DeprecationStatus::Deprecated;
                asset.deprecated_at = Some(env.ledger().timestamp());
                env.storage()
                    .persistent()
                    .set(&asset_key(asset_id), &asset);

                env.events().publish(
                    (symbol_short!("DEPR_EXEC"), asset_id),
                    (caller, env.ledger().timestamp()),
                );
            }
        }

        proposal.executed = true;
        env.storage().persistent().set(&proposal_key, &proposal);
    }
}

/// Returns `true` if `haystack` contains `needle` as a substring (byte-level, UTF-8 safe).
fn string_contains(env: &Env, haystack: &String, needle: &String) -> bool {
    use soroban_sdk::xdr::ToXdr;
    // XDR encodes a string as: 4-byte big-endian length + UTF-8 bytes (+ padding).
    // We skip the first 4 bytes to obtain raw UTF-8.
    let h_xdr = haystack.clone().to_xdr(env);
    let n_xdr = needle.clone().to_xdr(env);
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

