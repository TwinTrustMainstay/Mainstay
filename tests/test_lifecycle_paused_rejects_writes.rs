// tests/test_lifecycle_paused_rejects_writes.rs
//
// Issue #1039 — Add Test: lifecycle contract rejects all writes when paused
//
// Verifies that submit_maintenance and batch_submit_maintenance (on the
// lifecycle contract), register_asset (on the asset-registry contract), and
// register_engineer (on the engineer-registry contract) each return their
// contract's ContractError::Paused when that specific contract is paused,
// and that unpausing restores normal write access.
//
// Tasks:
//   1. Pause the relevant contract.
//   2. Attempt each write operation.
//   3. Assert a Paused error for all.
//   4. Unpause and verify writes succeed.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{BatchRecord, Lifecycle, LifecycleClient};
use soroban_sdk::{symbol_short, testutils::Address as _, Address, BytesN, Env, String};

/// lifecycle: ContractError::Paused = 9
const LIFECYCLE_PAUSED: u32 = 9;
/// asset-registry: ContractError::Paused = 7
const ASSET_REGISTRY_PAUSED: u32 = 7;
/// engineer-registry: ContractError::Paused = 8
const ENGINEER_REGISTRY_PAUSED: u32 = 8;

struct Setup<'a> {
    env: &'a Env,
    asset_registry: AssetRegistryClient<'a>,
    engineer_registry: EngineerRegistryClient<'a>,
    lifecycle: LifecycleClient<'a>,
    asset_admin: Address,
    eng_admin: Address,
    lc_admin: Address,
    issuer: Address,
    owner: Address,
    asset_id: u64,
    engineer: Address,
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
            &String::from_str(env, "Generator for pause test"),
            &String::from_str(env, "SN-1039-001"),
            &owner,
        );

        let credential_hash = BytesN::from_array(env, &[0x39u8; 32]);
        engineer_registry.register_engineer(
            &engineer,
            &credential_hash,
            &issuer,
            &31_536_000,
            &None,
        );

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
        }
    }
}

// ── submit_maintenance ──────────────────────────────────────────────────────

#[test]
fn test_paused_lifecycle_rejects_submit_maintenance_then_unpause_succeeds() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.lifecycle.pause(&s.lc_admin);

    let result = s.lifecycle.try_submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Routine oil change"),
        &s.engineer,
    );
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(LIFECYCLE_PAUSED))),
        "submit_maintenance must return Paused while lifecycle is paused"
    );

    s.lifecycle.unpause(&s.lc_admin);

    s.lifecycle.submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Routine oil change"),
        &s.engineer,
    );
    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        1,
        "submit_maintenance must succeed once lifecycle is unpaused"
    );
}

// ── batch_submit_maintenance ────────────────────────────────────────────────

#[test]
fn test_paused_lifecycle_rejects_batch_submit_maintenance_then_unpause_succeeds() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.lifecycle.pause(&s.lc_admin);

    let records = soroban_sdk::vec![
        &env,
        BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "Paused batch attempt"),
        }
    ];
    let result = s
        .lifecycle
        .try_batch_submit_maintenance(&s.asset_id, &records, &s.engineer);
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(LIFECYCLE_PAUSED))),
        "batch_submit_maintenance must return Paused while lifecycle is paused"
    );

    s.lifecycle.unpause(&s.lc_admin);

    s.lifecycle
        .batch_submit_maintenance(&s.asset_id, &records, &s.engineer);
    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        1,
        "batch_submit_maintenance must succeed once lifecycle is unpaused"
    );
}

// ── register_asset ───────────────────────────────────────────────────────────

#[test]
fn test_paused_asset_registry_rejects_register_asset_then_unpause_succeeds() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.asset_registry.pause(&s.asset_admin);

    let result = s.asset_registry.try_register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Paused registration attempt"),
        &String::from_str(&env, "SN-1039-002"),
        &s.owner,
    );
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(ASSET_REGISTRY_PAUSED))),
        "register_asset must return Paused while asset-registry is paused"
    );

    s.asset_registry.unpause(&s.asset_admin);

    let new_id = s.asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Post-unpause registration"),
        &String::from_str(&env, "SN-1039-003"),
        &s.owner,
    );
    assert!(new_id > 0, "register_asset must succeed once asset-registry is unpaused");
}

// ── register_engineer ────────────────────────────────────────────────────────

#[test]
fn test_paused_engineer_registry_rejects_register_engineer_then_unpause_succeeds() {
    let env = Env::default();
    let s = Setup::new(&env);
    let new_engineer = Address::generate(&env);
    let new_hash = BytesN::from_array(&env, &[0x40u8; 32]);

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
        "register_engineer must return Paused while engineer-registry is paused"
    );

    s.engineer_registry.unpause(&s.eng_admin);

    s.engineer_registry.register_engineer(
        &new_engineer,
        &new_hash,
        &s.issuer,
        &31_536_000,
        &None,
    );
    let record = s.engineer_registry.get_engineer(&new_engineer);
    assert!(record.active, "register_engineer must succeed once engineer-registry is unpaused");
}
