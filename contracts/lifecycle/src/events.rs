//! Event symbol constants for the Lifecycle contract.
//!
//! Centralising all `EVENT_*` constants here makes it straightforward to audit
//! which on-chain events the contract emits and ensures consistent naming
//! across the codebase.

use soroban_sdk::{symbol_short, Symbol};

/// Emitted once when the contract is first initialised.
pub(crate) const EVENT_INIT: Symbol = symbol_short!("INIT");

/// Emitted on every successful `submit_maintenance` call.
pub(crate) const EVENT_MAINT: Symbol = symbol_short!("MAINT");

/// Emitted when a score-decay step is applied to an asset.
pub(crate) const EVENT_DECAY: Symbol = symbol_short!("DECAY");

/// Emitted when an asset is registered in the asset registry cross-contract call.
pub(crate) const EVENT_REG_AST: Symbol = symbol_short!("REG_AST");

/// Emitted when an engineer is registered in the engineer registry cross-contract call.
pub(crate) const EVENT_REG_ENG: Symbol = symbol_short!("REG_ENG");

/// Emitted when an asset's collateral score is reset by an admin.
pub(crate) const EVENT_RST_SCR: Symbol = symbol_short!("RST_SCR");

/// Emitted when an asset ownership transfer sentinel is written.
pub(crate) const EVENT_XFER: Symbol = symbol_short!("XFER");

/// Emitted when a new admin is proposed (step 1 of the 2-step admin transfer).
pub(crate) const EVENT_PROP_ADMIN: Symbol = symbol_short!("PROP_ADM");

/// Emitted when a pending admin accepts and becomes the active admin.
pub(crate) const EVENT_ADMIN_SET: Symbol = symbol_short!("ADMIN_SET");

/// Emitted when a maintenance history or score history is pruned by an admin.
pub(crate) const EVENT_PRUNED: Symbol = symbol_short!("PRUNED");

/// Emitted when an admin proposes a new task-type weight via `propose_weight_change`.
pub(crate) const EVENT_WEIGHT_PROP: Symbol = symbol_short!("WT_PROP");

/// Emitted when an admin executes a pending weight-change proposal via `execute_weight_change`.
pub(crate) const EVENT_WEIGHT_EXEC: Symbol = symbol_short!("WT_EXEC");

/// Emitted when an admin anchors maintenance history to a health snapshot via `anchor_history_to_snapshot`.
pub(crate) const EVENT_RECONSTR: Symbol = symbol_short!("RECONSTR");

/// Emitted when a recurring task is approaching its due date.
pub(crate) const EVENT_TASK_DUE_SOON: Symbol = symbol_short!("TASK_SOON");

/// Emitted when an admin records a score anomaly via `record_score_anomaly`.
pub(crate) const EVENT_ANOMALY: Symbol = symbol_short!("ANOMALY");

/// Emitted when an anomaly's investigation status is changed via `update_anomaly_status`.
pub(crate) const EVENT_ANOMALY_STS: Symbol = symbol_short!("ANOM_STS");

/// Emitted when a peer group is registered or updated via `register_peer_group`.
pub(crate) const EVENT_PEER_GROUP: Symbol = symbol_short!("PEER_GRP");

/// Emitted when an engineer's Ed25519 signature over a maintenance record is stored via `sign_maintenance_record`.
pub(crate) const EVENT_MNT_SIG: Symbol = symbol_short!("MNT_SIG");

/// Emitted when a new external score provider is added via `register_score_provider`.
pub(crate) const EVENT_PROV_REG: Symbol = symbol_short!("PROV_REG");

/// Emitted when an external score provider is removed via `remove_score_provider`.
pub(crate) const EVENT_PROV_RM: Symbol = symbol_short!("PROV_RM");

/// Emitted when an external score from a registered provider is recorded via `submit_external_score`.
pub(crate) const EVENT_EXT_SCR: Symbol = symbol_short!("EXT_SCR");

/// Emitted when a degradation curve is registered or updated for an asset category via `register_degradation_curve`.
pub(crate) const EVENT_DEG_CRV: Symbol = symbol_short!("DEG_CRV");
