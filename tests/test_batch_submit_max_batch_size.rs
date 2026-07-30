// tests/test_batch_submit_max_batch_size.rs
//
// Issue #1038 — Add Test: batch_submit_maintenance respects MAX_BATCH_SIZE cap
//
// Verifies that `batch_submit_maintenance` rejects a batch larger than
// `MAX_BATCH_SIZE = 50` with a `ContractError::BatchTooLarge` error, and
// that no partial state is written to storage when the cap is exceeded.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{BatchRecord, Lifecycle, LifecycleClient, MAX_BATCH_SIZE};
use soroban_sdk::{
    symbol_short,
    testutils::Address as _,
    Address, BytesN, Env, String, Symbol, TryIntoVal, Vec,
};

/// lifecycle: ContractError::BatchTooLarge = 23
const LIFECYCLE_BATCH_TOO_LARGE: u32 = 23;

// ── Setup helpers ─────────────────────────────────────────────────────────────

/// Deploy and initialise all three contracts, register one asset and one
/// engineer, and return convenient handles for use in each test.
struct Setup<'a> {
    env: &'a Env,
    lifecycle: LifecycleClient<'a>,
    lc_admin: Address,
    asset_id: u64,
    asset_owner: Address,
    engineer: Address,
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
        let asset_owner = Address::generate(env);
        let engineer = Address::generate(env);

        // Initialise asset-registry
        asset_registry.initialize_admin(&asset_admin, &asset_admin);
        asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));

        // Initialise engineer-registry
        engineer_registry.initialize_admin(&eng_admin, &eng_admin);
        engineer_registry.add_trusted_issuer(&eng_admin, &issuer);

        // Initialise lifecycle (max_history = 0 means unlimited)
        lifecycle.initialize(
            &lc_admin,
            &asset_registry_id,
            &engineer_registry_id,
            &lc_admin,
            &0,
        );

        // Register one asset
        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(env, "Industrial generator for batch test"),
            &String::from_str(env, "SN-BATCH-001"),
            &asset_owner,
        );

        // Register one engineer with a valid credential
        let credential_hash = BytesN::from_array(env, &[5u8; 32]);
        engineer_registry.register_engineer(
            &engineer,
            &credential_hash,
            &issuer,
            &31_536_000, // 1 year validity
            &None,
        );

        // Authorise the engineer to submit maintenance for the asset
        lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

        Setup {
            env,
            lifecycle,
            lc_admin,
            asset_id,
            asset_owner,
            engineer,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Issue #1038 — A batch of 51 `BatchRecord` entries (MAX_BATCH_SIZE + 1) must
/// be rejected with `ContractError::BatchTooLarge` before any state is written.
#[test]
fn test_batch_submit_maintenance_rejects_batch_exceeding_max_batch_size() {
    let env = Env::default();
    let s = Setup::new(&env);

    // Build a batch with one more entry than the hard cap allows.
    let mut records = soroban_sdk::Vec::new(&env);
    for _ in 0..(MAX_BATCH_SIZE + 1) {
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "Routine oil change"),
        });
    }

    assert_eq!(records.len(), MAX_BATCH_SIZE + 1, "batch must have 51 entries");

    let result = s
        .lifecycle
        .try_batch_submit_maintenance(&s.asset_id, &records, &s.engineer);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_BATCH_TOO_LARGE,
        ))),
        "batch_submit_maintenance must reject a batch of 51 with BatchTooLarge"
    );

    // Confirm no partial state was written: history must remain empty and
    // collateral score must remain at zero.
    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        0,
        "no maintenance records must be persisted after a rejected oversized batch"
    );
    assert_eq!(
        s.lifecycle.get_collateral_score(&s.asset_id),
        0,
        "collateral score must remain 0 after a rejected oversized batch"
    );
}

/// Boundary test: a batch with exactly `MAX_BATCH_SIZE` (50) records must be
/// accepted — the cap is inclusive on the allowed side.
#[test]
fn test_batch_submit_maintenance_accepts_exactly_max_batch_size() {
    let env = Env::default();
    let s = Setup::new(&env);

    // Build a batch with exactly the maximum allowed number of records.
    let mut records = soroban_sdk::Vec::new(&env);
    for _ in 0..MAX_BATCH_SIZE {
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "Routine oil change"),
        });
    }

    assert_eq!(records.len(), MAX_BATCH_SIZE, "batch must have exactly 50 entries");

    // Should succeed without error.
    s.lifecycle
        .batch_submit_maintenance(&s.asset_id, &records, &s.engineer);

    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        MAX_BATCH_SIZE,
        "all 50 records must be persisted for a batch at the exact cap"
    );
}

/// Issue #1037 — `prune_asset_history` must emit a `PRUNED` event with the
/// number of pruned records and the oldest pruned record's timestamp.
#[test]
fn test_prune_asset_history_emits_pruned_event_with_correct_record_count_and_oldest_timestamp() {
    let env = Env::default();
    let s = Setup::new(&env);

    // Submit 10 maintenance records with distinct timestamps.
    for i in 0..10 {
        env.ledger().set_timestamp(1_000 + i);
        s.lifecycle.submit_maintenance(
            &s.asset_id,
            &symbol_short!("INSPECT"),
            &String::from_str(&env, "maintenance event"),
            &s.engineer,
        );
    }

    s.lifecycle.update_max_history(&s.lc_admin, &5);
    s.lifecycle.prune_asset_history(&s.lc_admin, &s.asset_id);

    let events = env.events().all();
    let pruned_event = events.iter().find(|(_, topics, _)| {
        topics.len() == 1
            && topics
                .get(0)
                .and_then(|v| TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                .map(|s| s == symbol_short!("PRUNED"))
                .unwrap_or(false)
    });

    assert!(pruned_event.is_some(), "expected PRUNED event");
    let (_, _, data) = pruned_event.unwrap();
    let (emitted_asset_id, pruned_count, oldest_pruned_timestamp): (u64, u32, u64) =
        data.try_into_val(&env).unwrap();

    assert_eq!(emitted_asset_id, s.asset_id);
    assert_eq!(pruned_count, 5);
    assert_eq!(oldest_pruned_timestamp, 1_000);
    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        5,
        "maintenance history should be pruned to 5"
    );
}
