// tests/test_suspended_engineer_grace_period.rs
//
// Issue: Suspension and credential expiry grace period are separate
// mechanisms. There was no test verifying that a suspended engineer whose
// credential is still technically within its grace period is blocked from
// submitting maintenance — i.e. that `is_suspended` is checked before (and
// independently of) the grace-period allowance in
// `EngineerRegistry::get_credential_status`.
//
// Strategy
// --------
// 1. Register an engineer with a short validity period.
// 2. Advance the ledger just past `expires_at`, into the (default 7-day)
//    grace period — confirm submission still succeeds here (sanity check
//    that grace period alone does not block submission).
// 3. Suspend the engineer while still within the grace window.
// 4. Attempt submit_maintenance — assert it is rejected with
//    ContractError::UnauthorizedEngineer, proving suspension overrides the
//    grace-period allowance.
// 5. Advance past the suspension end (still within the grace window) and
//    verify submission succeeds again.

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

struct Setup<'a> {
    env: &'a Env,
    engineer_registry: EngineerRegistryClient<'a>,
    lifecycle: LifecycleClient<'a>,
    asset_id: u64,
    engineer: Address,
    expires_at: u64,
}

impl<'a> Setup<'a> {
    fn new(env: &'a Env, validity_period: u64) -> Self {
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

        asset_registry.initialize_admin(&asset_admin, &asset_admin);
        asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));

        engineer_registry.initialize_admin(&eng_admin, &eng_admin);
        engineer_registry.add_trusted_issuer(&eng_admin, &issuer);

        lifecycle.initialize(
            &lc_admin,
            &asset_registry_id,
            &engineer_registry_id,
            &lc_admin,
            &0,
        );

        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(env, "Grace period + suspension test asset"),
            &String::from_str(env, "SN-GRACE-SUSP-001"),
            &owner,
        );

        let now = env.ledger().timestamp();
        let credential_hash = BytesN::from_array(env, &[0xabu8; 32]);
        engineer_registry.register_engineer(
            &engineer,
            &credential_hash,
            &issuer,
            &validity_period,
            &None,
        );
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        Setup {
            env,
            engineer_registry,
            lifecycle,
            asset_id,
            engineer,
            expires_at: now + validity_period,
        }
    }
}

/// A suspended engineer must be rejected even while their credential is
/// still within its grace period — suspension takes precedence over the
/// grace-period allowance.
#[test]
fn test_suspended_engineer_in_grace_period_is_rejected() {
    let env = Env::default();
    // Short validity so the credential expires quickly and enters its grace window.
    let validity_period: u64 = 100;
    let s = Setup::new(&env, validity_period);

    // ── Step 1: advance just past expiry, into the grace period ───────────
    let grace_entry = s.expires_at + 10;
    env.ledger().set_timestamp(grace_entry);

    // Sanity: submission succeeds purely on grace-period allowance.
    s.lifecycle.submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Grace-period submission — should succeed"),
        &s.engineer,
    );
    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        1,
        "grace-period submission alone must succeed before any suspension"
    );

    // ── Step 2: suspend the engineer while still in the grace period ──────
    let suspension_end = grace_entry + 50;
    s.engineer_registry.suspend_engineer(
        &s.engineer,
        &suspension_end,
        &String::from_str(&env, "Compliance investigation"),
    );

    // ── Step 3: submission must now be rejected, despite being in grace ───
    let result = s.lifecycle.try_submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "During-suspension, in-grace attempt — must be rejected"),
        &s.engineer,
    );
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_UNAUTHORIZED_ENGINEER,
        ))),
        "a suspended engineer must be rejected with UnauthorizedEngineer even while \
         their credential is still within its grace period",
    );
    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        1,
        "rejected submission must not grow the maintenance history"
    );

    // ── Step 4: after suspension ends (still within grace), submission succeeds ──
    env.ledger().set_timestamp(suspension_end);

    s.lifecycle.submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Post-suspension, still-in-grace submission — should succeed"),
        &s.engineer,
    );
    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        2,
        "engineer must be able to submit again once the suspension window ends"
    );
}
