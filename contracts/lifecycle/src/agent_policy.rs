//! Issue #1767/#1768: autonomous agent policy and audit trail for
//! operational tasks (maintenance scheduling, collateral rebalancing,
//! covenant monitoring).
//!
//! Soroban contracts have no native scheduler — they only execute when
//! invoked. "Autonomous" execution therefore requires an off-chain runner
//! (a bot/cron service) that reads the policy configured here to decide
//! what to do and when, then submits the matching contract call. This
//! module is the on-chain half of that: the declarative policy an off-chain
//! agent reads, the entry point it calls to record that it acted, and the
//! resulting audit trail.
//!
//! Each `#[contractimpl]` wrapper in `lib.rs` delegates directly to a
//! function here, matching the existing split used by `admin.rs`.

use crate::errors::ContractError;
use crate::require_admin;
use crate::types::{AgentActionRecord, AgentPolicy, AgentType, DataKey};
use shared::extend_persistent_ttl;
use soroban_sdk::{panic_with_error, Address, Env, Symbol, Vec};

fn policy_key(agent_type: AgentType) -> DataKey {
    DataKey::AgentPolicy(agent_type as u32)
}

fn last_action_key(agent_type: AgentType) -> DataKey {
    DataKey::AgentLastAction(agent_type as u32)
}

fn audit_key(agent_type: AgentType) -> DataKey {
    DataKey::AgentAudit(agent_type as u32)
}

/// Admin-only. Configures whether an agent type is enabled, its minimum
/// interval between actions, and its agent-specific threshold.
pub(crate) fn set_agent_policy(
    env: Env,
    admin: Address,
    agent_type: AgentType,
    enabled: bool,
    min_interval_secs: u64,
    threshold: u32,
) {
    require_admin(&env, &admin);

    let policy = AgentPolicy {
        agent_type,
        enabled,
        min_interval_secs,
        threshold,
    };
    let key = policy_key(agent_type);
    env.storage().persistent().set(&key, &policy);
    extend_persistent_ttl(&env, &key);
}

/// Current policy for `agent_type`. Defaults to disabled with zeroed
/// thresholds if never explicitly configured.
pub(crate) fn get_agent_policy(env: Env, agent_type: AgentType) -> AgentPolicy {
    env.storage()
        .persistent()
        .get(&policy_key(agent_type))
        .unwrap_or(AgentPolicy {
            agent_type,
            enabled: false,
            min_interval_secs: 0,
            threshold: 0,
        })
}

/// Called by an off-chain agent runner (or an admin acting on its behalf)
/// once it has decided — per the policy above — that it should act, and has
/// taken that action (e.g. scheduled maintenance, rebalanced collateral,
/// flagged a covenant breach). Enforces that the agent type is enabled and
/// that `min_interval_secs` has elapsed since its last recorded action, then
/// appends to the audit trail.
///
/// # Panics
/// - [`ContractError::AgentDisabled`] if the targeted agent type is disabled
/// - [`ContractError::AgentRateLimited`] if called before `min_interval_secs`
///   has elapsed since the agent type's last recorded action
pub(crate) fn record_agent_action(
    env: Env,
    executor: Address,
    agent_type: AgentType,
    target_id: u64,
    action: Symbol,
) {
    executor.require_auth();

    let policy = get_agent_policy(env.clone(), agent_type);
    if !policy.enabled {
        panic_with_error!(&env, ContractError::AgentDisabled);
    }

    let now = env.ledger().timestamp();
    let last_key = last_action_key(agent_type);
    if let Some(last) = env.storage().persistent().get::<_, u64>(&last_key) {
        if now < last.saturating_add(policy.min_interval_secs) {
            panic_with_error!(&env, ContractError::AgentRateLimited);
        }
    }
    env.storage().persistent().set(&last_key, &now);
    extend_persistent_ttl(&env, &last_key);

    let record = AgentActionRecord {
        agent_type,
        timestamp: now,
        target_id,
        action,
        executor,
    };
    let hist_key = audit_key(agent_type);
    let mut history: Vec<AgentActionRecord> = env
        .storage()
        .persistent()
        .get(&hist_key)
        .unwrap_or(Vec::new(&env));
    history.push_back(record);
    env.storage().persistent().set(&hist_key, &history);
    extend_persistent_ttl(&env, &hist_key);
}

/// Full recorded action history for `agent_type`.
pub(crate) fn get_agent_action_history(env: Env, agent_type: AgentType) -> Vec<AgentActionRecord> {
    env.storage()
        .persistent()
        .get(&audit_key(agent_type))
        .unwrap_or(Vec::new(&env))
}
