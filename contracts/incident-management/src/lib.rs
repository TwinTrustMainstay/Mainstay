#![no_std]

//! Incident tracking and postmortem register (issue #1734).
//!
//! Incidents move through a fixed lifecycle (`Open` → `Mitigated` → `Resolved` → `Closed`).
//! Sev1 and Sev2 incidents cannot be closed until a root-cause postmortem is recorded.
//! Each incident carries action items, which the owner marks complete to track lessons learned.

#[cfg(test)]
mod test;

use shared::{extend_persistent_ttl, require_admin};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    Env, String, Vec,
};

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
    /// No incident exists with the given id.
    IncidentNotFound = 4,
    /// The requested status change is not allowed from the current status.
    InvalidTransition = 5,
    /// Sev1/Sev2 incidents need a recorded root cause before they can be closed.
    PostmortemRequired = 6,
    /// No action item exists with the given id on this incident.
    ActionItemNotFound = 7,
    /// A required text field (title, root cause, description) was empty.
    EmptyField = 8,
    /// Caller is not the owner of the action item.
    NotActionItemOwner = 9,
}

/// Incident severity. Lower numbers are more severe.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Severity {
    /// Critical: customer-facing outage or loss of funds.
    Sev1 = 1,
    /// Major: significant degradation with no safe workaround.
    Sev2 = 2,
    /// Minor: degradation with a workaround available.
    Sev3 = 3,
    /// Cosmetic or informational.
    Sev4 = 4,
}

/// Incident lifecycle status.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Status {
    Open = 0,
    Mitigated = 1,
    Resolved = 2,
    Closed = 3,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Incident {
    pub id: u64,
    pub title: String,
    pub severity: Severity,
    pub status: Status,
    pub reporter: Address,
    pub opened_at: u64,
    /// Ledger timestamp when the incident reached `Resolved`; `0` while unresolved.
    pub resolved_at: u64,
    /// Root cause from the postmortem; empty until recorded.
    pub root_cause: String,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionItem {
    pub id: u32,
    pub description: String,
    pub owner: Address,
    pub completed: bool,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    NextId,
    Incident(u64),
    ActionItems(u64),
}

#[contract]
pub struct IncidentManagement;

fn load_incident(env: &Env, id: u64) -> Incident {
    env.storage()
        .persistent()
        .get(&DataKey::Incident(id))
        .unwrap_or_else(|| panic_with_error!(env, ContractError::IncidentNotFound))
}

fn save_incident(env: &Env, incident: &Incident) {
    let key = DataKey::Incident(incident.id);
    env.storage().persistent().set(&key, incident);
    extend_persistent_ttl(env, key);
}

fn load_action_items(env: &Env, id: u64) -> Vec<ActionItem> {
    env.storage()
        .persistent()
        .get(&DataKey::ActionItems(id))
        .unwrap_or_else(|| Vec::new(env))
}

fn save_action_items(env: &Env, id: u64, items: &Vec<ActionItem>) {
    let key = DataKey::ActionItems(id);
    env.storage().persistent().set(&key, items);
    extend_persistent_ttl(env, key);
}

fn require_stored_admin(env: &Env, admin: &Address) {
    let stored: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized));
    if require_admin(admin, &stored).is_err() {
        panic_with_error!(env, ContractError::UnauthorizedAdmin);
    }
}

#[contractimpl]
impl IncidentManagement {
    /// Sets the admin who manages incident status, postmortems and action items.
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, ContractError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::NextId, &1u64);
    }

    /// Opens a new incident. Any signed reporter may open one. Returns the incident id.
    pub fn open_incident(env: Env, reporter: Address, title: String, severity: Severity) -> u64 {
        reporter.require_auth();
        if title.is_empty() {
            panic_with_error!(&env, ContractError::EmptyField);
        }

        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(1);
        let incident = Incident {
            id,
            title,
            severity,
            status: Status::Open,
            reporter,
            opened_at: env.ledger().timestamp(),
            resolved_at: 0,
            root_cause: String::from_str(&env, ""),
        };
        save_incident(&env, &incident);
        env.storage().instance().set(&DataKey::NextId, &(id + 1));

        env.events()
            .publish((symbol_short!("inc_open"),), (id, severity));
        id
    }

    /// Moves an incident to a new status. Admin only.
    ///
    /// Allowed transitions: `Open` → `Mitigated`/`Resolved`, `Mitigated` → `Resolved`,
    /// `Resolved` → `Closed`. Closing a Sev1/Sev2 incident requires a recorded postmortem.
    pub fn update_status(env: Env, admin: Address, id: u64, status: Status) {
        require_stored_admin(&env, &admin);
        let mut incident = load_incident(&env, id);

        let allowed = matches!(
            (incident.status, status),
            (Status::Open, Status::Mitigated)
                | (Status::Open, Status::Resolved)
                | (Status::Mitigated, Status::Resolved)
                | (Status::Resolved, Status::Closed)
        );
        if !allowed {
            panic_with_error!(&env, ContractError::InvalidTransition);
        }

        if status == Status::Closed && is_major(incident.severity) && incident.root_cause.is_empty()
        {
            panic_with_error!(&env, ContractError::PostmortemRequired);
        }

        if status == Status::Resolved {
            incident.resolved_at = env.ledger().timestamp();
        }
        incident.status = status;
        save_incident(&env, &incident);

        env.events()
            .publish((symbol_short!("inc_stat"),), (id, status));
    }

    /// Records the root cause for an incident. Admin only. The incident must be `Resolved`
    /// or `Closed`, so the postmortem reflects a finished investigation.
    pub fn record_postmortem(env: Env, admin: Address, id: u64, root_cause: String) {
        require_stored_admin(&env, &admin);
        if root_cause.is_empty() {
            panic_with_error!(&env, ContractError::EmptyField);
        }
        let mut incident = load_incident(&env, id);
        if !matches!(incident.status, Status::Resolved | Status::Closed) {
            panic_with_error!(&env, ContractError::InvalidTransition);
        }
        incident.root_cause = root_cause;
        save_incident(&env, &incident);

        env.events().publish((symbol_short!("inc_pm"),), id);
    }

    /// Adds a lessons-learned action item to an incident. Admin only. Returns the item id.
    pub fn add_action_item(
        env: Env,
        admin: Address,
        id: u64,
        description: String,
        owner: Address,
    ) -> u32 {
        require_stored_admin(&env, &admin);
        if description.is_empty() {
            panic_with_error!(&env, ContractError::EmptyField);
        }
        load_incident(&env, id);

        let mut items = load_action_items(&env, id);
        let item_id = items.len();
        items.push_back(ActionItem {
            id: item_id,
            description,
            owner,
            completed: false,
        });
        save_action_items(&env, id, &items);
        item_id
    }

    /// Marks an action item complete. Only the item's owner may do this.
    pub fn complete_action_item(env: Env, owner: Address, id: u64, item_id: u32) {
        owner.require_auth();
        let mut items = load_action_items(&env, id);

        let mut found = false;
        for i in 0..items.len() {
            let mut item = items.get(i).unwrap();
            if item.id == item_id {
                if item.owner != owner {
                    panic_with_error!(&env, ContractError::NotActionItemOwner);
                }
                item.completed = true;
                items.set(i, item);
                found = true;
                break;
            }
        }
        if !found {
            panic_with_error!(&env, ContractError::ActionItemNotFound);
        }
        save_action_items(&env, id, &items);

        env.events()
            .publish((symbol_short!("inc_act"),), (id, item_id));
    }

    pub fn get_incident(env: Env, id: u64) -> Incident {
        load_incident(&env, id)
    }

    pub fn get_action_items(env: Env, id: u64) -> Vec<ActionItem> {
        load_action_items(&env, id)
    }

    /// Returns the number of incidents opened so far.
    pub fn get_incident_count(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::NextId).unwrap_or(1) - 1
    }
}

fn is_major(severity: Severity) -> bool {
    matches!(severity, Severity::Sev1 | Severity::Sev2)
}
