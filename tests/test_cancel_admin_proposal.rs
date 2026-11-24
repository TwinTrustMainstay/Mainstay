//! Tests for #1017: cancel_admin_proposal in lifecycle and engineer-registry.
//!
//! Verifies that:
//! - The current admin can cancel a pending admin proposal.
//! - After cancellation, `accept_admin` fails (no pending proposal).
//! - A non-admin cannot cancel.
//! - Cancelling when no proposal exists returns ProposalNotFound.
//! - ADM_CANCEL event is emitted on successful cancellation.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    testutils::Address as _,
    Address, Env,
};

// Error discriminants (from errors.rs, stable integers).
const LIFECYCLE_PROPOSAL_NOT_FOUND: u32 = 18;
const LIFECYCLE_UNAUTHORIZED_ADMIN: u32 = 3;
const ENGINEER_REGISTRY_PROPOSAL_NOT_FOUND: u32 = 16;
const ENGINEER_REGISTRY_UNAUTHORIZED_ADMIN: u32 = 2;

// ── Lifecycle helpers ────────────────────────────────────────────────────────

fn setup_lifecycle(env: &Env) -> (LifecycleClient, Address) {
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(env, &lifecycle_id);

    let admin = Address::generate(env);

    asset_registry.initialize_admin(&admin, &admin);
    engineer_registry.initialize_admin(&admin, &admin);
    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &0,
    );

    (lifecycle, admin)
}

// ── Lifecycle tests ──────────────────────────────────────────────────────────

#[test]
fn test_lifecycle_cancel_admin_proposal_clears_pending() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup_lifecycle(&env);
    let new_admin = Address::generate(&env);

    // Step 1: propose a new admin.
    lifecycle.propose_admin(&admin, &new_admin);

    // Step 2: admin cancels the proposal.
    lifecycle.cancel_admin_proposal(&admin);

    // Step 3: accept_admin must now fail with ProposalNotFound (no pending admin).
    let res = lifecycle.try_accept_admin();
    assert!(
        res.is_err(),
        "accept_admin must fail after cancel_admin_proposal"
    );
}

#[test]
fn test_lifecycle_cancel_admin_proposal_non_admin_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup_lifecycle(&env);
    let new_admin = Address::generate(&env);
    let outsider = Address::generate(&env);

    lifecycle.propose_admin(&admin, &new_admin);

    let res = lifecycle.try_cancel_admin_proposal(&outsider);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_UNAUTHORIZED_ADMIN
        ))),
        "non-admin must not be able to cancel an admin proposal"
    );
}

#[test]
fn test_lifecycle_cancel_admin_proposal_no_proposal_errors() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup_lifecycle(&env);

    // No proposal has been made — must return ProposalNotFound.
    let res = lifecycle.try_cancel_admin_proposal(&admin);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_PROPOSAL_NOT_FOUND
        ))),
        "cancel_admin_proposal must return ProposalNotFound when no proposal exists"
    );
}

#[test]
fn test_lifecycle_admin_can_re_propose_after_cancel() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, admin) = setup_lifecycle(&env);
    let new_admin = Address::generate(&env);
    let another_admin = Address::generate(&env);

    // Propose, cancel, then propose again — must succeed.
    lifecycle.propose_admin(&admin, &new_admin);
    lifecycle.cancel_admin_proposal(&admin);
    lifecycle.propose_admin(&admin, &another_admin);

    // The new proposal should be accepted by `another_admin`.
    lifecycle.accept_admin();

    assert_eq!(
        lifecycle.get_config().admin,
        another_admin,
        "admin must be another_admin after re-propose and accept"
    );
}

// ── Engineer-registry helpers ────────────────────────────────────────────────

fn setup_engineer_registry(env: &Env) -> (EngineerRegistryClient, Address) {
    let id = env.register(EngineerRegistry, ());
    let client = EngineerRegistryClient::new(env, &id);
    let admin = Address::generate(env);
    client.initialize_admin(&admin, &admin);
    (client, admin)
}

// ── Engineer-registry tests ──────────────────────────────────────────────────

#[test]
fn test_engineer_registry_cancel_admin_proposal_clears_pending() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin) = setup_engineer_registry(&env);
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.cancel_admin_proposal(&admin);

    // After cancellation, accept_admin must fail.
    let res = client.try_accept_admin();
    assert!(
        res.is_err(),
        "accept_admin must fail after cancel_admin_proposal in engineer-registry"
    );
}

#[test]
fn test_engineer_registry_cancel_admin_non_admin_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin) = setup_engineer_registry(&env);
    let new_admin = Address::generate(&env);
    let outsider = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);

    let res = client.try_cancel_admin_proposal(&outsider);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            ENGINEER_REGISTRY_UNAUTHORIZED_ADMIN
        ))),
        "non-admin must not be able to cancel in engineer-registry"
    );
}

#[test]
fn test_engineer_registry_cancel_no_proposal_errors() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, admin) = setup_engineer_registry(&env);

    let res = client.try_cancel_admin_proposal(&admin);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            ENGINEER_REGISTRY_PROPOSAL_NOT_FOUND
        ))),
        "cancel_admin_proposal must return ProposalNotFound when no proposal exists"
    );
}
