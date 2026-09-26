//! Storage key helpers for the Lifecycle contract.
//!
//! All persistent-storage keys are defined here so that key construction is
//! centralised and consistent across the codebase.  Every key-returning
//! function is `pub(crate)` so it can be called from `lib.rs` and `admin.rs`
//! without being part of the public contract ABI.

use soroban_sdk::{symbol_short, Address, Env, Symbol};

// ---------------------------------------------------------------------------
// Per-asset keys
// ---------------------------------------------------------------------------

/// Maintenance history for an asset: `Vec<MaintenanceRecord>`.
pub(crate) fn history_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("HIST"), asset_id)
}

/// Environmental impact measurements keyed by asset and history index.
pub(crate) fn environmental_impact_key(asset_id: u64, record_index: u32) -> (Symbol, u64, u32) {
    (symbol_short!("ENV_IMP"), asset_id, record_index)
}

/// Addresses interested in real-time lifecycle updates for an asset.
pub(crate) fn update_subscribers_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("UPD_SUB"), asset_id)
}

/// Append-only corrections for an asset's maintenance record.
pub(crate) fn maintenance_corrections_key(asset_id: u64, record_index: u32) -> (Symbol, u64, u32) {
    (symbol_short!("MNT_CORR"), asset_id, record_index)
}

/// Current collateral score for an asset: `u32`.
pub(crate) fn score_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("SCORE"), asset_id)
}

/// Score history (time-series) for an asset: `Vec<ScoreEntry>`.
pub(crate) fn score_history_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("SCHIST"), asset_id)
}

/// Timestamp of the last score update for an asset: `u64`.
pub(crate) fn last_update_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("LUPD"), asset_id)
}

/// Frozen flag for an asset: `bool`.
pub(crate) fn frozen_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("FROZEN"), asset_id)
}

/// Frozen score for an asset (score captured at freeze time): `u32`.
///
/// TTL strategy: this key's TTL is extended both when it is written (freeze
/// time) and on every subsequent read (`get_collateral_score_opt`,
/// `batch_is_collateral_eligible`). A frozen asset can otherwise go
/// unqueried for long stretches; extending only on write would let the TTL
/// lapse and silently drop the stored value, causing reads to fall back to
/// `0` instead of the preserved frozen score.
pub(crate) fn frozen_score_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("FRZ_SCR"), asset_id)
}

/// Health-snapshot list for an asset: `Vec<HealthSnapshot>`.
pub(crate) fn health_snapshot_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("HLTH_SNP"), asset_id)
}

/// Ownership-transfer history for an asset: `Vec<TransferRecord>`.
pub(crate) fn transfer_hist_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("XFER_HIST"), asset_id)
}

pub(crate) fn maintenance_audit_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("MNT_AUD"), asset_id)
}

pub(crate) fn maintenance_attestations_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("MNT_ATT"), asset_id)
}

pub(crate) fn attestor_auth_key(asset_id: u64, attestor: &Address) -> (Symbol, u64, Address) {
    (symbol_short!("ATT_AUTH"), asset_id, attestor.clone())
}

// ---------------------------------------------------------------------------
// Per-engineer keys
// ---------------------------------------------------------------------------

/// List of asset IDs an engineer has worked on: `Vec<u64>`.
pub(crate) fn engineer_history_key(engineer: &Address) -> (Symbol, Address) {
    (symbol_short!("ENG_HIST"), engineer.clone())
}

/// Per-asset authorization flag for an engineer: `bool`.
pub(crate) fn engineer_auth_key(asset_id: u64, engineer: &Address) -> (Symbol, u64, Address) {
    (symbol_short!("ENG_AUTH"), asset_id, engineer.clone())
}

/// Rolling-hour submission rate window for an engineer: `(window_start: u64, count: u32)`.
pub(crate) fn submission_window_key(engineer: &Address) -> (Symbol, Address) {
    (symbol_short!("SUB_WIN"), engineer.clone())
}

/// Optional per-user override for the rolling-hour submission limit.
pub(crate) fn user_submission_limit_key(user: &Address) -> (Symbol, Address) {
    (symbol_short!("USR_RATE"), user.clone())
}

/// Timelock proposal for revoking an engineer's auth on an asset: `TimelockProposal`.
pub(crate) fn revoke_eng_timelock_key(
    asset_id: u64,
    engineer: &Address,
) -> (Symbol, u64, Address) {
    (symbol_short!("RVK_TL"), asset_id, engineer.clone())
}

// ---------------------------------------------------------------------------
// Timelock keys
// ---------------------------------------------------------------------------

/// Generic timelock proposal key, keyed by operation symbol: `TimelockProposal`.
pub(crate) fn timelock_key(op: Symbol) -> (Symbol, Symbol) {
    (symbol_short!("TL_PROP"), op)
}

// ---------------------------------------------------------------------------
// Asset-type keys
// ---------------------------------------------------------------------------

/// Maintenance standard bytes for an asset type: `Bytes`.
pub(crate) fn standard_key(asset_type: &Symbol) -> (Symbol, Symbol) {
    (symbol_short!("MSTD"), asset_type.clone())
}

/// Dynamic frequency scoring weights JSON for an asset type: `Bytes`.
///
/// `env` is required to construct the `Symbol` used as a discriminant; it is
/// not stored itself.
pub(crate) fn scoring_weights_key(_env: &Env, asset_type: &Symbol) -> (Symbol, Symbol) {
    (symbol_short!("SCR_WGT"), asset_type.clone())
}

/// Retirement state for an asset: `RetirementState`.
pub(crate) fn retirement_state_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("RTR_STE"), asset_id)
}

/// Retirement certificate for an asset: `RetirementCertificate`.
pub(crate) fn retirement_certificate_key(asset_id: u64) -> (Symbol, u64) {
    (symbol_short!("RTR_CRT"), asset_id)
}

/// Coordinated task information: `CoordinatedTask`.
pub(crate) fn coordinated_task_key(task_id: u64) -> (Symbol, u64) {
    (symbol_short!("COOD_TSK"), task_id)
}

/// Subtask status for coordinated task: `Vec<CoordinatedSubtask>`.
pub(crate) fn coordinated_subtasks_key(task_id: u64) -> (Symbol, u64) {
    (symbol_short!("COOD_SUB"), task_id)
}

/// Seasonal adjustment factors for asset type: `SeasonalAdjustment`.
pub(crate) fn seasonal_adjustment_key(asset_type: &Symbol) -> (Symbol, Symbol) {
    (symbol_short!("SEAS_ADJ"), asset_type.clone())
}
