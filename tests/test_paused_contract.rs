// tests/test_paused_contract.rs
//
// Issue #843 — Test: paused contract rejects all write operations
//
// Verifies that every state-mutating function in the three core contracts
// (asset-registry, engineer-registry, lifecycle) returns ContractError::Paused
// when the contract is paused, and that unpausing fully restores functionality.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::Address as _,
    Address, BytesN, Env, String,
};

// ── Shared error-code constants ──────────────────────────────────────────────

/// asset-registry: ContractError::Paused = 7
const ASSET_REGISTRY_PAUSED: u32 = 7;
/// engineer-registry: ContractError::Paused = 8
const ENGINEER_REGISTRY_PAUSED: u32 = 8;
/// lifecycle: ContractError::Paused = 9
const LIFECYCLE_PAUSED: u32 = 9;

// ── Setup helpers ────────────────────────────────────────────────────────────

/// Fully wired setup: deploy all three contracts, initialise admin/issuer/types,
/// register one asset and one engineer, and return convenient handles for all.
struct Setup<'a> {
    env: &'a Env,
    asset_registry: AssetRegistryClient<'a>,
    engineer_registry: EngineerRegistryClient<'a>,
    lifecycle: LifecycleClient<'a>,
    /// Admin of asset-registry and lifecycle
    asset_admin: Address,
    /// Admin of engineer-registry
    eng_admin: Address,
    /// Lifecycle admin
    lc_admin: Address,
    /// A trusted credential issuer
    issuer: Address,
    /// An already-registered asset owner
    owner: Address,
    /// A pre-registered asset
    asset_id: u64,
    /// A pre-registered engineer (active credential)
    engineer: Address,
    /// Credential hash used when registering the engineer
    credential_hash: BytesN<32>,
}

impl<'a> Setup<'a> {
    fn new(env: &'a Env) -> Self {
        env.mock_all_auths();

        // Deploy contracts
        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());

        let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
        let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
        let lifecycle = LifecycleClient::new(env, &lifecycle_id);

        // Addresses
        let asset_admin = Address::generate(env);
        let eng_admin = Address::generate(env);
        let lc_admin = Address::generate(env);
        let issuer = Address::generate(env);
        let owner = Address::generate(env);
        let engineer = Address::generate(env);

        // Initialise asset-registry
        asset_registry.initialize_admin(&asset_admin, &asset_admin);
        asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));

        // Initialise engineer-registry
        engineer_registry.initialize_admin(&eng_admin, &eng_admin);
        engineer_registry.add_trusted_issuer(&eng_admin, &issuer);

        // Initialise lifecycle
        lifecycle.initialize(
            &lc_admin,
            &asset_registry_id,
            &engineer_registry_id,
            &lc_admin,
            &0,
        );

        // Pre-register one asset (used in lifecycle write tests)
        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(env, "Generator Model X"),
            &String::from_str(env, "SN-PAUSED-001"),
            &owner,
        );

        // Pre-register one engineer (used in lifecycle write tests)
        let credential_hash = BytesN::from_array(env, &[42u8; 32]);
        engineer_registry.register_engineer(
            &engineer,
            &credential_hash,
            &issuer,
            &31_536_000, // 1 year validity
            &None,
        );

        // Authorise the engineer to submit maintenance for the pre-registered asset
        lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

        Setup {
            env,
            asset_registry,
            engineer_registry,
            lifecycle,
            asset_admin,
            eng_admin,
            lc_admin,
            issuer,
            owner,
            asset_id,
            engineer,
            credential_hash,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Issue #843 — Asset Registry: paused contract rejects all write operations
// ═══════════════════════════════════════════════════════════════════════════════

/// Pausing the asset-registry must block `register_asset`.
#[test]
fn test_asset_registry_paused_rejects_register_asset() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.asset_registry.pause(&s.asset_admin);

    let result = s.asset_registry.try_register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Paused registration attempt"),
        &String::from_str(&env, "SN-PAUSED-REG-001"),
        &s.owner,
    );

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(ASSET_REGISTRY_PAUSED))),
        "register_asset must return Paused error when contract is paused"
    );
}

/// Pausing the asset-registry must block `batch_register_assets`.
#[test]
fn test_asset_registry_paused_rejects_batch_register_assets() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.asset_registry.pause(&s.asset_admin);

    let inputs = soroban_sdk::vec![
        &env,
        asset_registry::AssetInput {
            asset_type: symbol_short!("GENSET"),
            metadata: String::from_str(&env, "Batch asset 1"),
            serial_number: String::from_str(&env, "SN-BATCH-P-001"),
        }
    ];
    let result = s.asset_registry.try_batch_register_assets(&s.owner, &inputs);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(ASSET_REGISTRY_PAUSED))),
        "batch_register_assets must return Paused error when contract is paused"
    );
}

/// Pausing the asset-registry must block `update_asset_metadata`.
#[test]
fn test_asset_registry_paused_rejects_update_metadata() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.asset_registry.pause(&s.asset_admin);

    let result = s.asset_registry.try_update_asset_metadata(
        &s.asset_id,
        &s.owner,
        &String::from_str(&env, "New metadata while paused"),
    );

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(ASSET_REGISTRY_PAUSED))),
        "update_asset_metadata must return Paused error when contract is paused"
    );
}

/// Pausing the asset-registry must block `transfer_asset`.
#[test]
fn test_asset_registry_paused_rejects_transfer_asset() {
    let env = Env::default();
    let s = Setup::new(&env);
    let new_owner = Address::generate(&env);

    s.asset_registry.pause(&s.asset_admin);

    let result = s.asset_registry.try_transfer_asset(
        &s.asset_id,
        &s.owner,
        &new_owner,
    );

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(ASSET_REGISTRY_PAUSED))),
        "transfer_asset must return Paused error when contract is paused"
    );
}

/// Pausing the asset-registry must block `decommission_asset`.
#[test]
fn test_asset_registry_paused_rejects_decommission_asset() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.asset_registry.pause(&s.asset_admin);

    let result = s.asset_registry.try_decommission_asset(&s.asset_admin, &s.asset_id);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(ASSET_REGISTRY_PAUSED))),
        "decommission_asset must return Paused error when contract is paused"
    );
}

/// After unpausing the asset-registry, `register_asset` must succeed.
#[test]
fn test_asset_registry_unpause_restores_register_asset() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.asset_registry.pause(&s.asset_admin);
    s.asset_registry.unpause(&s.asset_admin);

    // Must not error after unpause
    let new_id = s.asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Post-unpause registration"),
        &String::from_str(&env, "SN-UNPAUSE-001"),
        &s.owner,
    );

    assert!(new_id > 0, "register_asset must succeed after unpause");

    let asset = s.asset_registry.get_asset(&new_id);
    assert_eq!(asset.asset_type, symbol_short!("GENSET"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Issue #843 — Engineer Registry: paused contract rejects all write operations
// ═══════════════════════════════════════════════════════════════════════════════

/// Pausing the engineer-registry must block `register_engineer`.
#[test]
fn test_engineer_registry_paused_rejects_register_engineer() {
    let env = Env::default();
    let s = Setup::new(&env);
    let new_engineer = Address::generate(&env);
    let new_hash = BytesN::from_array(&env, &[99u8; 32]);

    s.engineer_registry.pause(&s.eng_admin);

    let result = s.engineer_registry.try_register_engineer(
        &new_engineer,
        &new_hash,
        &s.issuer,
        &31_536_000,
        &None,
    );

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(ENGINEER_REGISTRY_PAUSED))),
        "register_engineer must return Paused error when contract is paused"
    );
}

/// Pausing the engineer-registry must block `revoke_credential`.
#[test]
fn test_engineer_registry_paused_rejects_revoke_credential() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.engineer_registry.pause(&s.eng_admin);

    let result = s.engineer_registry.try_revoke_credential(&s.engineer);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(ENGINEER_REGISTRY_PAUSED))),
        "revoke_credential must return Paused error when contract is paused"
    );
}

/// Pausing the engineer-registry must block `renew_credential`.
#[test]
fn test_engineer_registry_paused_rejects_renew_credential() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.engineer_registry.pause(&s.eng_admin);

    let result = s.engineer_registry.try_renew_credential(
        &s.engineer,
        &31_536_000,
    );

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(ENGINEER_REGISTRY_PAUSED))),
        "renew_credential must return Paused error when contract is paused"
    );
}

/// Pausing the engineer-registry must block `add_trusted_issuer`.
#[test]
fn test_engineer_registry_paused_rejects_add_trusted_issuer() {
    let env = Env::default();
    let s = Setup::new(&env);
    let new_issuer = Address::generate(&env);

    s.engineer_registry.pause(&s.eng_admin);

    let result = s.engineer_registry.try_add_trusted_issuer(&s.eng_admin, &new_issuer);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(ENGINEER_REGISTRY_PAUSED))),
        "add_trusted_issuer must return Paused error when contract is paused"
    );
}

/// After unpausing the engineer-registry, `register_engineer` must succeed.
#[test]
fn test_engineer_registry_unpause_restores_register_engineer() {
    let env = Env::default();
    let s = Setup::new(&env);
    let new_engineer = Address::generate(&env);
    let new_hash = BytesN::from_array(&env, &[77u8; 32]);

    s.engineer_registry.pause(&s.eng_admin);
    s.engineer_registry.unpause(&s.eng_admin);

    // Must not error after unpause
    s.engineer_registry.register_engineer(
        &new_engineer,
        &new_hash,
        &s.issuer,
        &31_536_000,
        &None,
    );

    let record = s.engineer_registry.get_engineer(&new_engineer);
    assert!(record.active, "Newly registered engineer must be active after unpause");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Issue #843 — Lifecycle: paused contract rejects all write operations
// ═══════════════════════════════════════════════════════════════════════════════

/// Pausing the lifecycle contract must block `submit_maintenance`.
#[test]
fn test_lifecycle_paused_rejects_submit_maintenance() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.lifecycle.pause(&s.lc_admin);

    let result = s.lifecycle.try_submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Routine oil change"),
        &s.engineer,
        &None,
    );

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(LIFECYCLE_PAUSED))),
        "submit_maintenance must return Paused error when lifecycle is paused"
    );
}

/// Pausing the lifecycle contract must block `authorize_engineer`.
#[test]
fn test_lifecycle_paused_rejects_authorize_engineer() {
    let env = Env::default();
    let s = Setup::new(&env);
    let another_engineer = Address::generate(&env);

    s.lifecycle.pause(&s.lc_admin);

    let result = s.lifecycle.try_authorize_engineer(
        &s.owner,
        &s.asset_id,
        &another_engineer,
    );

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(LIFECYCLE_PAUSED))),
        "authorize_engineer must return Paused error when lifecycle is paused"
    );
}

/// Pausing the lifecycle contract must block `batch_submit_maintenance`.
#[test]
fn test_lifecycle_paused_rejects_batch_submit_maintenance() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.lifecycle.pause(&s.lc_admin);

    let records = soroban_sdk::vec![
        &env,
        lifecycle::BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "Paused batch record"),
        }
    ];
    let result = s.lifecycle.try_batch_submit_maintenance(
        &s.asset_id,
        &records,
        &s.engineer,
        &None,
    );

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(LIFECYCLE_PAUSED))),
        "batch_submit_maintenance must return Paused error when lifecycle is paused"
    );
}

/// Pausing the lifecycle contract must block `decay_score`.
#[test]
fn test_lifecycle_paused_rejects_decay_score() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.lifecycle.pause(&s.lc_admin);

    let result = s.lifecycle.try_decay_score(&s.asset_id);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(LIFECYCLE_PAUSED))),
        "decay_score must return Paused error when lifecycle is paused"
    );
}

/// After unpausing the lifecycle contract, `submit_maintenance` must succeed
/// and the collateral score must increase.
#[test]
fn test_lifecycle_unpause_restores_submit_maintenance() {
    let env = Env::default();
    let s = Setup::new(&env);

    let score_before = s.lifecycle.get_collateral_score(&s.asset_id);

    s.lifecycle.pause(&s.lc_admin);
    s.lifecycle.unpause(&s.lc_admin);

    // Must succeed after unpause
    s.lifecycle.submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Post-unpause oil change"),
        &s.engineer,
        &None,
    );

    let history = s.lifecycle.get_maintenance_history(&s.asset_id);
    assert_eq!(history.len(), 1, "Exactly one maintenance record must exist after unpause");

    let score_after = s.lifecycle.get_collateral_score(&s.asset_id);
    assert!(
        score_after > score_before,
        "Collateral score must increase after maintenance submission post-unpause: {} -> {}",
        score_before,
        score_after
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Issue #843 — Cross-contract: pause each contract independently
// ═══════════════════════════════════════════════════════════════════════════════

/// Pausing asset-registry must NOT affect engineer-registry write operations.
#[test]
fn test_asset_registry_pause_does_not_affect_engineer_registry() {
    let env = Env::default();
    let s = Setup::new(&env);
    let new_engineer = Address::generate(&env);
    let new_hash = BytesN::from_array(&env, &[55u8; 32]);

    // Pause only asset-registry
    s.asset_registry.pause(&s.asset_admin);

    // Engineer-registry writes must still succeed
    s.engineer_registry.register_engineer(
        &new_engineer,
        &new_hash,
        &s.issuer,
        &31_536_000,
        &None,
    );

    let record = s.engineer_registry.get_engineer(&new_engineer);
    assert!(
        record.active,
        "engineer-registry must remain functional when only asset-registry is paused"
    );
}

/// Pausing engineer-registry must NOT affect asset-registry write operations.
#[test]
fn test_engineer_registry_pause_does_not_affect_asset_registry() {
    let env = Env::default();
    let s = Setup::new(&env);

    // Pause only engineer-registry
    s.engineer_registry.pause(&s.eng_admin);

    // Asset-registry writes must still succeed
    let new_id = s.asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Independent registration"),
        &String::from_str(&env, "SN-INDEP-001"),
        &s.owner,
    );

    assert!(
        new_id > 0,
        "asset-registry must remain functional when only engineer-registry is paused"
    );
}
