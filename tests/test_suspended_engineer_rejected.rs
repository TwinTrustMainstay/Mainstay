// tests/test_suspended_engineer_rejected.rs
//
// Issue #1033 — Add Test: submit_maintenance by suspended engineer should be rejected
//
// Verifies that a suspended engineer (suspension_end_time in the future) cannot
// submit maintenance records during the suspension window, and that the same
// engineer is accepted again once the suspension has expired.
//
// Test steps:
//   1. Register asset, engineer, and authorize the engineer for the asset.
//   2. Confirm a pre-suspension submission succeeds (sanity check).
//   3. Suspend the engineer with a future end timestamp.
//   4. Attempt submit_maintenance while suspended — assert UnauthorizedEngineer.
//   5. Advance ledger past the suspension end time.
//   6. Attempt submit_maintenance after suspension — assert success.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

/// lifecycle: ContractError::UnauthorizedEngineer = 2
const LIFECYCLE_UNAUTHORIZED_ENGINEER: u32 = 2;

// ─── Setup helper ────────────────────────────────────────────────────────────

struct Setup<'a> {
    env: &'a Env,
    engineer_registry: EngineerRegistryClient<'a>,
    lifecycle: LifecycleClient<'a>,
    /// The asset registered for this test.
    asset_id: u64,
    /// The engineer to be suspended.
    engineer: Address,
    /// The issuer who credentialed the engineer (required for suspend_engineer).
    issuer: Address,
}

impl<'a> Setup<'a> {
    fn new(env: &'a Env) -> Self {
        env.mock_all_auths();

        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());

        let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
        let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
        let lifecycle = LifecycleClient::new(env, &lifecycle_id);

        let asset_admin = Address::generate(env);
        let eng_admin = Address::generate(env);
        let lc_admin = Address::generate(env);
        let issuer = Address::generate(env);
        let owner = Address::generate(env);
        let engineer = Address::generate(env);

        // Bootstrap asset-registry.
        asset_registry.initialize_admin(&asset_admin, &asset_admin);
        asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));

        // Bootstrap engineer-registry.
        engineer_registry.initialize_admin(&eng_admin, &eng_admin);
        engineer_registry.add_trusted_issuer(&eng_admin, &issuer);

        // Bootstrap lifecycle.
        lifecycle.initialize(
            &lc_admin,
            &asset_registry_id,
            &engineer_registry_id,
            &lc_admin,
            &0,
        );

        // Register an asset.
        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(env, "Suspension test asset"),
            &String::from_str(env, "SN-SUSP-1033-001"),
            &owner,
        );

        // Register and authorize the engineer.
        let credential_hash = BytesN::from_array(env, &[0xeeu8; 32]);
        engineer_registry.register_engineer(
            &engineer,
            &credential_hash,
            &issuer,
            &31_536_000, // 1-year validity
            &None,
        );
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        Setup {
            env,
            engineer_registry,
            lifecycle,
            asset_id,
            engineer,
            issuer,
        }
    }
}

// ─── Issue #1033 tests ────────────────────────────────────────────────────────

/// A suspended engineer must not be able to submit maintenance records.
///
/// Steps:
///   1. Confirm a pre-suspension submission succeeds.
///   2. Suspend the engineer for one hour.
///   3. Assert that submit_maintenance is rejected with
///      ContractError::UnauthorizedEngineer (code 2) while suspended.
///   4. Assert the maintenance history was not extended by the rejected call.
#[test]
fn test_suspended_engineer_cannot_submit_maintenance() {
    let env = Env::default();
    let s = Setup::new(&env);

    // ── Step 1: pre-suspension submission must succeed ────────────────────────
    s.lifecycle.submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Pre-suspension service — should succeed"),
        &s.engineer,
    );
    let history_len_before = s.lifecycle.get_maintenance_history(&s.asset_id).len();
    assert_eq!(
        history_len_before, 1,
        "expected exactly one record before suspension",
    );

    // ── Step 2: suspend the engineer for 1 hour ───────────────────────────────
    let now = env.ledger().timestamp();
    let suspension_end = now + 3_600; // 1 hour from now
    s.engineer_registry.suspend_engineer(
        &s.engineer,
        &suspension_end,
        &String::from_str(&env, "Ongoing investigation"),
    );

    // ── Step 3: attempt submission while suspended ────────────────────────────
    let result = s.lifecycle.try_submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "During-suspension attempt — must be rejected"),
        &s.engineer,
    );

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_UNAUTHORIZED_ENGINEER,
        ))),
        "submit_maintenance must return UnauthorizedEngineer (error {LIFECYCLE_UNAUTHORIZED_ENGINEER}) \
         while the engineer is suspended",
    );

    // ── Step 4: history must not have grown ───────────────────────────────────
    let history_len_after = s.lifecycle.get_maintenance_history(&s.asset_id).len();
    assert_eq!(
        history_len_after, history_len_before,
        "maintenance history must not grow when the submission is rejected",
    );
}

/// A previously-suspended engineer can submit maintenance records again once
/// the suspension window has elapsed.
///
/// Steps:
///   1. Suspend the engineer for 1 hour.
///   2. Confirm submission is rejected during suspension.
///   3. Advance the ledger to exactly the suspension end timestamp.
///   4. Assert that submit_maintenance now succeeds.
#[test]
fn test_engineer_can_submit_maintenance_after_suspension_expires() {
    let env = Env::default();
    let s = Setup::new(&env);

    // ── Step 1: suspend the engineer for 1 hour ───────────────────────────────
    let now = env.ledger().timestamp();
    let suspension_end = now + 3_600;
    s.engineer_registry.suspend_engineer(
        &s.engineer,
        &suspension_end,
        &String::from_str(&env, "Temporary suspension"),
    );

    // ── Step 2: confirm submission is rejected mid-suspension ─────────────────
    let mid_suspension = now + 1_800; // 30 minutes in
    env.ledger().set_timestamp(mid_suspension);

    let rejected = s.lifecycle.try_submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Mid-suspension attempt — must be rejected"),
        &s.engineer,
    );

    assert_eq!(
        rejected,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_UNAUTHORIZED_ENGINEER,
        ))),
        "submit_maintenance must be rejected mid-suspension",
    );

    // ── Step 3: advance to the suspension end timestamp ───────────────────────
    // The suspension window is defined as timestamp < suspension_end_time, so
    // at exactly suspension_end the engineer's status must revert to Valid.
    env.ledger().set_timestamp(suspension_end);

    // ── Step 4: submission after suspension must succeed ─────────────────────
    s.lifecycle.submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Post-suspension service — should succeed"),
        &s.engineer,
    );

    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        1,
        "expected exactly one successful maintenance record after suspension expired",
    );
}
