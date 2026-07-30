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
