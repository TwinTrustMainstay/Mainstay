// tests/test_decommission_blocks_maintenance.rs
//
// Issue #837 — Test: asset decommission blocks further maintenance
//
// Verifies that submitting maintenance for a decommissioned asset is rejected
// with ContractError::AssetDecommissioned (error code 22).
//
// Test steps:
//   1. Register an asset and a certified engineer; authorise the engineer.
//   2. Confirm a pre-decommission maintenance submission succeeds.
//   3. Decommission the asset via the asset-registry admin.
//   4. Attempt submit_maintenance — assert AssetDecommissioned error.
//   5. Assert the maintenance history did not grow.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::Address as _,
    Address, BytesN, Env, String,
};

/// lifecycle: ContractError::AssetDecommissioned = 22
const LIFECYCLE_ASSET_DECOMMISSIONED: u32 = 22;

// ─── Setup helper ────────────────────────────────────────────────────────────

/// Wires all three core contracts, registers one asset and one certified
/// engineer (authorised for that asset), and returns convenient handles.
struct Setup<'a> {
    env: &'a Env,
    asset_registry: AssetRegistryClient<'a>,
    lifecycle: LifecycleClient<'a>,
    /// Admin of the asset-registry (required to call `decommission_asset`).
    asset_admin: Address,
    /// Owner of the pre-registered asset.
    owner: Address,
    /// The pre-registered asset's on-chain ID.
    asset_id: u64,
    /// A pre-registered, active engineer authorised for the asset.
    engineer: Address,
}

impl<'a> Setup<'a> {
    fn new(env: &'a Env) -> Self {
        env.mock_all_auths();

        // Deploy all three contracts.
        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());

        let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
        let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
        let lifecycle = LifecycleClient::new(env, &lifecycle_id);

        // Distinct admin addresses per contract.
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

        // Bootstrap lifecycle and point it at the two registries.
        lifecycle.initialize(
            &lc_admin,
            &asset_registry_id,
            &engineer_registry_id,
            &lc_admin,
            &0, // no history cap
        );

        // Register one asset.
        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(env, "Caterpillar 3516 – decommission test"),
            &String::from_str(env, "SN-DECOMM-837-001"),
            &owner,
        );

        // Register one certified engineer and authorise them for the asset.
        let credential_hash = BytesN::from_array(env, &[0xddu8; 32]);
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
            asset_registry,
            lifecycle,
            asset_admin,
            owner,
            asset_id,
            engineer,
        }
    }
}

// ─── Issue #837 tests ─────────────────────────────────────────────────────────

/// A decommissioned asset must reject `submit_maintenance` with
/// `ContractError::AssetDecommissioned` (error code 22).
///
/// Steps:
///   1. Confirm maintenance succeeds before decommissioning.
///   2. Decommission the asset.
///   3. Assert the next submission is rejected with the structured error.
///   4. Assert the maintenance history was not extended by the rejected call.
#[test]
fn test_decommission_blocks_submit_maintenance() {
    let env = Env::default();
    let s = Setup::new(&env);

    // ── Step 1: pre-decommission submission must succeed ─────────────────────
    s.lifecycle.submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Pre-decommission service — should succeed"),
        &s.engineer,
    );
    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        1,
        "expected exactly one maintenance record before decommission",
    );

    // ── Step 2: decommission the asset via the asset-registry admin ──────────
    s.asset_registry
        .decommission_asset(&s.asset_admin, &s.asset_id);

    // ── Step 3: submission on decommissioned asset must be rejected ──────────
    let result = s.lifecycle.try_submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Post-decommission attempt — must be rejected"),
        &s.engineer,
    );

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_ASSET_DECOMMISSIONED,
        ))),
        "submit_maintenance must return AssetDecommissioned (error {LIFECYCLE_ASSET_DECOMMISSIONED}) \
         after the asset is decommissioned",
    );

    // ── Step 4: history must be unchanged ────────────────────────────────────
    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        1,
        "maintenance history must not grow after a rejected post-decommission submission",
    );
}

/// Decommissioning must block all subsequent submissions, not just the first
/// rejected one.  Three back-to-back calls after decommission must each return
/// the same structured error.
#[test]
fn test_decommission_blocks_repeated_submissions() {
    let env = Env::default();
    let s = Setup::new(&env);

    // Decommission immediately (no prior maintenance needed for this variant).
    s.asset_registry
        .decommission_asset(&s.asset_admin, &s.asset_id);

    for attempt in 0..3u32 {
        let result = s.lifecycle.try_submit_maintenance(
            &s.asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(
                &env,
                "Repeated post-decommission attempt",
            ),
            &s.engineer,
        );

        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                LIFECYCLE_ASSET_DECOMMISSIONED,
            ))),
            "attempt {attempt}: submit_maintenance must keep returning AssetDecommissioned",
        );
    }

    // History must remain empty — no record from any of the rejected calls.
    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        0,
        "history must be empty; all three post-decommission submissions must have been rejected",
    );
}
