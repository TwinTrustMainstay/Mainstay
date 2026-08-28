// tests/test_engineer_history_cap.rs
//
// Issue #997 — Fix: engineer_history is not pruned and grows unboundedly
//
// Verifies that:
//   1. An engineer's per-address asset history is pruned to `max_engineer_history`
//      entries on write (sliding-window, oldest dropped first).
//   2. The history never exceeds the cap, even when an engineer works on many
//      distinct assets.
//   3. `update_max_engineer_history` correctly updates the cap, and subsequent
//      writes respect the new value.
//   4. Setting the cap to 0 is rejected with `ContractError::InvalidConfig`.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::Address as _,
    Address, BytesN, Env, String,
};
use core::sync::atomic::{AtomicU64, Ordering};

// ContractError::InvalidConfig = 8 (from contracts/lifecycle/src/errors.rs)
const LIFECYCLE_INVALID_CONFIG: u32 = 8;

/// Global counter for unique serial numbers across all assets in this test file.
static SN_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_serial(env: &Env) -> String {
    let n = SN_COUNTER.fetch_add(1, Ordering::Relaxed);
    // Build a short "SN-<n>" string from scratch (no_std-style, but env is std-friendly here).
    let s = std::format!("SN-EHC-{}", n);
    String::from_str(env, &s)
}

// ---------------------------------------------------------------------------
// Setup helpers
// ---------------------------------------------------------------------------

struct Setup<'a> {
    env: &'a Env,
    lifecycle: LifecycleClient<'a>,
    asset_registry: AssetRegistryClient<'a>,
    engineer_registry: EngineerRegistryClient<'a>,
    admin: Address,
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

        let admin = Address::generate(env);
        let issuer = Address::generate(env);

        asset_registry.initialize_admin(&admin, &admin);
        asset_registry.add_asset_type(&admin, &symbol_short!("GENSET"));

        engineer_registry.initialize_admin(&admin, &admin);
        engineer_registry.add_trusted_issuer(&admin, &issuer);

        lifecycle.initialize(
            &admin,
            &asset_registry_id,
            &engineer_registry_id,
            &admin,
            &0, // max_history = 0 → use default (200)
        );

        Self {
            env,
            lifecycle,
            asset_registry,
            engineer_registry,
            admin,
            issuer,
        }
    }

    /// Register a new asset owned by `owner` and return its ID.
    fn register_asset(&self, owner: &Address) -> u64 {
        self.asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(self.env, "test asset"),
            &next_serial(self.env),
            owner,
        )
    }

    /// Register a new engineer and return their address.
    fn register_engineer(&self) -> Address {
        let engineer = Address::generate(self.env);
        let credential_hash = BytesN::from_array(self.env, &[3u8; 32]);
        self.engineer_registry.register_engineer(
            &engineer,
            &credential_hash,
            &self.issuer,
            &31_536_000,
            &None,
        );
        engineer
    }

    /// Authorize `engineer` for `asset_id` (owner must be the asset owner).
    fn authorize(&self, owner: &Address, asset_id: &u64, engineer: &Address) {
        self.lifecycle.authorize_engineer(owner, asset_id, engineer);
    }

    /// Submit one maintenance record and advance the ledger timestamp.
    fn submit(&self, asset_id: &u64, engineer: &Address) {
        self.lifecycle.submit_maintenance(
            asset_id,
            &symbol_short!("INSPECT"),
            &String::from_str(self.env, "cap enforcement test"),
            engineer,
            &None,
        );
        self.env
            .ledger()
            .set_timestamp(self.env.ledger().timestamp() + 1);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The engineer history must not exceed `max_engineer_history` entries.
/// When the cap is reached, the oldest asset ID is dropped before the newest
/// is appended (sliding-window pruning).
#[test]
fn test_engineer_history_cap_enforced_on_submit() {
    let env = Env::default();
    let s = Setup::new(&env);

    let engineer = s.register_engineer();

    // Lower the cap to 3 so we can test pruning without registering hundreds of assets.
    s.lifecycle
        .update_max_engineer_history(&s.admin, &3u32);

    // Register 5 distinct assets and submit maintenance for each.
    let owner = Address::generate(&env);
    let mut asset_ids: Vec<u64> = Vec::new();
    for _ in 0..5 {
        let id = s.register_asset(&owner);
        s.authorize(&owner, &id, &engineer);
        s.submit(&id, &engineer);
        asset_ids.push(id);
    }

    let history = s.lifecycle.get_engineer_maintenance_history(&engineer);

    // History must be capped at 3 entries.
    assert_eq!(
        history.len(),
        3,
        "engineer history must be capped at max_engineer_history (3), got {}",
        history.len()
    );
    assert_eq!(s.lifecycle.get_engineer_history_count(&engineer), 3);

    // The retained entries must be the 3 most-recently worked-on assets.
    // asset_ids[0] and asset_ids[1] should have been pruned (oldest first).
    assert_eq!(history.get(0).unwrap(), asset_ids[2], "slot 0 must be the 3rd asset (oldest retained)");
    assert_eq!(history.get(1).unwrap(), asset_ids[3], "slot 1 must be the 4th asset");
    assert_eq!(history.get(2).unwrap(), asset_ids[4], "slot 2 must be the 5th asset (most recent)");

    // Pruned entries must not appear in the history.
    for id in history.iter() {
        assert_ne!(id, asset_ids[0], "first asset (oldest) must have been pruned");
        assert_ne!(id, asset_ids[1], "second asset must have been pruned");
    }
}

/// Submitting maintenance for an asset that is already in the engineer's history
/// must not add a duplicate — the cap must still not be exceeded and no
/// slot should be wasted.
#[test]
fn test_engineer_history_no_duplicate_entries() {
    let env = Env::default();
    let s = Setup::new(&env);

    let engineer = s.register_engineer();
    let owner = Address::generate(&env);
    let asset_id = s.register_asset(&owner);
    s.authorize(&owner, &asset_id, &engineer);

    // Submit the same asset multiple times.
    for _ in 0..5 {
        s.submit(&asset_id, &engineer);
    }

    let history = s.lifecycle.get_engineer_maintenance_history(&engineer);
    assert_eq!(
        history.len(),
        1,
        "repeated maintenance on the same asset must not create duplicate history entries"
    );
}

/// `update_max_engineer_history` must update the cap, and subsequent writes
/// must respect the new (lower) cap.
#[test]
fn test_update_max_engineer_history_takes_effect() {
    let env = Env::default();
    let s = Setup::new(&env);

    let engineer = s.register_engineer();
    let owner = Address::generate(&env);

    // Start with a cap of 5 and submit 5 maintenance events on distinct assets.
    s.lifecycle
        .update_max_engineer_history(&s.admin, &5u32);

    for _ in 0..5 {
        let id = s.register_asset(&owner);
        s.authorize(&owner, &id, &engineer);
        s.submit(&id, &engineer);
    }

    assert_eq!(
        s.lifecycle
            .get_engineer_maintenance_history(&engineer)
            .len(),
        5,
        "with cap = 5, engineer history should have 5 entries"
    );

    // Now lower the cap to 2 and add two more maintenance events.
    s.lifecycle
        .update_max_engineer_history(&s.admin, &2u32);

    for _ in 0..2 {
        let id = s.register_asset(&owner);
        s.authorize(&owner, &id, &engineer);
        s.submit(&id, &engineer);
    }

    // The history should now have 2 entries — the new cap applies on each write.
    assert_eq!(
        s.lifecycle
            .get_engineer_maintenance_history(&engineer)
            .len(),
        2,
        "after lowering cap to 2 and adding 2 new entries, history must be pruned to 2"
    );
}

/// `update_max_engineer_history` must reject 0 with `ContractError::InvalidConfig`.
#[test]
fn test_update_max_engineer_history_rejects_zero() {
    let env = Env::default();
    let s = Setup::new(&env);

    let result = s
        .lifecycle
        .try_update_max_engineer_history(&s.admin, &0u32);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_INVALID_CONFIG
        ))),
        "update_max_engineer_history(0) must return ContractError::InvalidConfig"
    );
}

/// The default `max_engineer_history` reported by `get_config()` must be 200.
#[test]
fn test_engineer_history_count_empty() {
    let s = Setup::new();
    let engineer = Address::generate(&s.env);

    assert_eq!(s.lifecycle.get_engineer_history_count(&engineer), 0);
}

#[test]
fn test_default_max_engineer_history_is_200() {
    let env = Env::default();
    let s = Setup::new(&env);

    let config = s.lifecycle.get_config();
    assert_eq!(
        config.max_engineer_history, 200,
        "default max_engineer_history must be 200"
    );
}
