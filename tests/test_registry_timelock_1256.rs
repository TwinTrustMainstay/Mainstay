//! Tests for #1256: registry address changes must go through a 48-hour timelock.
//!
//! A malicious admin could point the engineer registry to a contract they
//! control, bypassing certification checks.  The fix requires:
//!   1. `propose_update_asset_registry` / `propose_update_engineer_registry`
//!      to start the timelock.
//!   2. `execute_update_asset_registry` / `execute_update_engineer_registry`
//!      (already existing) to apply the change after the delay.
//!   3. The legacy direct `update_asset_registry` / `update_engineer_registry`
//!      entry points are now gated behind the same timelock — calling them
//!      without a prior proposal returns `TimelockNotExpired` or
//!      `ProposalNotFound`.
//!
//! Verifies:
//! - `update_asset_registry` without a prior proposal is rejected.
//! - `update_engineer_registry` without a prior proposal is rejected.
//! - `execute_update_asset_registry` without a prior proposal is rejected.
//! - `execute_update_engineer_registry` without a prior proposal is rejected.
//! - Full propose → wait → execute flow succeeds for both registries.
//! - Execution before the timelock expires is rejected.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env,
};

/// On-ledger delay used by the lifecycle timelock (48 h in seconds).
const TIMELOCK_DELAY_SECS: u64 = 48 * 60 * 60;

// Stable error discriminants from lifecycle/src/errors.rs.
const LIFECYCLE_TIMELOCK_NOT_EXPIRED: u32 = 17;
const LIFECYCLE_PROPOSAL_NOT_FOUND: u32 = 18;

// ── helpers ──────────────────────────────────────────────────────────────────

struct Setup {
    lifecycle: LifecycleClient,
    admin: Address,
    /// A second asset registry deployment available for use as a migration target.
    new_asset_registry: Address,
    /// A second engineer registry deployment available for use as a migration target.
    new_engineer_registry: Address,
}

fn setup(env: &Env) -> Setup {
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    // Register additional registries to use as migration targets.
    let new_asset_registry_id = env.register(AssetRegistry, ());
    let new_engineer_registry_id = env.register(EngineerRegistry, ());

    let lifecycle = LifecycleClient::new(env, &lifecycle_id);
    let admin = Address::generate(env);

    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    asset_registry.initialize_admin(&admin, &admin);

    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
    engineer_registry.initialize_admin(&admin, &admin);

    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &0,
    );

    Setup {
        lifecycle,
        admin,
        new_asset_registry: new_asset_registry_id,
        new_engineer_registry: new_engineer_registry_id,
    }
}

// ── asset registry timelock tests ────────────────────────────────────────────

/// Calling `execute_update_asset_registry` with no prior proposal must fail.
#[test]
fn test_execute_update_asset_registry_without_proposal_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let s = setup(&env);

    let result = s.lifecycle.try_execute_update_asset_registry(
        &s.admin,
        &s.new_asset_registry,
    );
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_PROPOSAL_NOT_FOUND,
        ))),
        "execute_update_asset_registry without prior proposal must return ProposalNotFound (#1256)"
    );
}

/// Calling `update_asset_registry` (the legacy direct endpoint) with no prior
/// proposal must now fail — the endpoint is gated behind the timelock.
#[test]
fn test_update_asset_registry_without_proposal_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let s = setup(&env);

    let result = s.lifecycle.try_update_asset_registry(
        &s.admin,
        &s.new_asset_registry,
    );
    // ProposalNotFound because no proposal has been created.
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_PROPOSAL_NOT_FOUND,
        ))),
        "update_asset_registry must be gated behind the timelock (#1256)"
    );
}

/// Proposing an asset registry change and immediately trying to execute must
/// fail with `TimelockNotExpired`.
#[test]
fn test_execute_update_asset_registry_too_early_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let s = setup(&env);

    // Step 1: Propose.
    s.lifecycle.propose_update_asset_registry(&s.admin, &s.new_asset_registry);

    // Step 2: Try to execute immediately — must be rejected.
    let result = s.lifecycle.try_execute_update_asset_registry(
        &s.admin,
        &s.new_asset_registry,
    );
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_TIMELOCK_NOT_EXPIRED,
        ))),
        "execute_update_asset_registry must fail before timelock expires (#1256)"
    );
}

/// Full happy-path: propose → wait 48 h → execute.
/// The asset registry address must be updated after execution.
#[test]
fn test_propose_and_execute_update_asset_registry_full_flow() {
    let env = Env::default();
    env.mock_all_auths();
    let s = setup(&env);

    // Step 1: Propose.
    s.lifecycle.propose_update_asset_registry(&s.admin, &s.new_asset_registry);

    // Step 2: Advance past the timelock.
    let base = env.ledger().timestamp();
    env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

    // Step 3: Execute — must succeed.
    s.lifecycle.execute_update_asset_registry(&s.admin, &s.new_asset_registry);

    // Step 4: Verify the registry address was updated.
    let stored = s.lifecycle.get_asset_registry();
    assert_eq!(
        stored, s.new_asset_registry,
        "asset registry address must be updated after execute (#1256)"
    );
}

/// Executing exactly one second before the timelock expires must still fail.
#[test]
fn test_execute_update_asset_registry_one_second_before_expiry() {
    let env = Env::default();
    env.mock_all_auths();
    let s = setup(&env);

    s.lifecycle.propose_update_asset_registry(&s.admin, &s.new_asset_registry);

    let base = env.ledger().timestamp();
    env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS - 1);

    let result = s.lifecycle.try_execute_update_asset_registry(
        &s.admin,
        &s.new_asset_registry,
    );
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_TIMELOCK_NOT_EXPIRED,
        ))),
        "execute must still fail one second before timelock expires (#1256)"
    );
}

// ── engineer registry timelock tests ─────────────────────────────────────────

/// Calling `execute_update_engineer_registry` with no prior proposal must fail.
#[test]
fn test_execute_update_engineer_registry_without_proposal_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let s = setup(&env);

    let result = s.lifecycle.try_execute_update_engineer_registry(
        &s.admin,
        &s.new_engineer_registry,
    );
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_PROPOSAL_NOT_FOUND,
        ))),
        "execute_update_engineer_registry without prior proposal must return ProposalNotFound (#1256)"
    );
}

/// Calling `update_engineer_registry` (the legacy direct endpoint) with no
/// prior proposal must now fail.
#[test]
fn test_update_engineer_registry_without_proposal_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let s = setup(&env);

    let result = s.lifecycle.try_update_engineer_registry(
        &s.admin,
        &s.new_engineer_registry,
    );
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_PROPOSAL_NOT_FOUND,
        ))),
        "update_engineer_registry must be gated behind the timelock (#1256)"
    );
}

/// Proposing an engineer registry change and immediately executing must fail.
#[test]
fn test_execute_update_engineer_registry_too_early_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let s = setup(&env);

    s.lifecycle
        .propose_update_engineer_registry(&s.admin, &s.new_engineer_registry);

    let result = s.lifecycle.try_execute_update_engineer_registry(
        &s.admin,
        &s.new_engineer_registry,
    );
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_TIMELOCK_NOT_EXPIRED,
        ))),
        "execute_update_engineer_registry must fail before timelock expires (#1256)"
    );
}

/// Full happy-path: propose → wait 48 h → execute engineer registry change.
#[test]
fn test_propose_and_execute_update_engineer_registry_full_flow() {
    let env = Env::default();
    env.mock_all_auths();
    let s = setup(&env);

    s.lifecycle
        .propose_update_engineer_registry(&s.admin, &s.new_engineer_registry);

    let base = env.ledger().timestamp();
    env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

    s.lifecycle
        .execute_update_engineer_registry(&s.admin, &s.new_engineer_registry);

    let stored = s.lifecycle.get_engineer_registry();
    assert_eq!(
        stored, s.new_engineer_registry,
        "engineer registry address must be updated after execute (#1256)"
    );
}

/// Executing one second before the timelock expires must still be rejected.
#[test]
fn test_execute_update_engineer_registry_one_second_before_expiry() {
    let env = Env::default();
    env.mock_all_auths();
    let s = setup(&env);

    s.lifecycle
        .propose_update_engineer_registry(&s.admin, &s.new_engineer_registry);

    let base = env.ledger().timestamp();
    env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS - 1);

    let result = s.lifecycle.try_execute_update_engineer_registry(
        &s.admin,
        &s.new_engineer_registry,
    );
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_TIMELOCK_NOT_EXPIRED,
        ))),
        "execute must still fail one second before timelock expires (#1256)"
    );
}
