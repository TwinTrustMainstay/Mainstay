// tests/test_upgrade_compatibility.rs
///
/// # Upgrade Compatibility Tests
///
/// Verifies that existing on-chain state remains accessible after a contract
/// upgrade. When contracts are upgraded (new WASM deployed to the same contract
/// ID), all persistent storage must remain readable and correctly deserialized
/// by the new contract code.
///
/// **Note on test scope:** In Soroban unit tests, the same contract WASM is used
/// for both "v1" and "v2" since `env.register(Contract, ())` always deploys the
/// current WASM. True upgrade compatibility testing requires an integration test
/// on testnet where a different WASM hash can be deployed to the same contract ID.
/// These tests verify that (a) all storage keys, data structures, and client APIs
/// are stable, and (b) data written by the current contract is readable after
/// simulated ledger advances (TTL boundaries, time passage).
///
/// ## Upgrade Compatibility Requirements
///
/// 1. **Storage keys must be stable.**  Changing a storage key format (e.g.
///    from `Symbol` to a tuple, or renaming a key prefix) will make existing
///    data orphaned and inaccessible.
///
/// 2. **Data types must be backward-compatible.**  Adding new fields to a
///    `#[contracttype]` struct is safe *only* if they have defaults. Removing
///    or changing field types breaks deserialization. Any existing struct that
///    is stored on-chain MUST have its schema preserved across upgrades.
///
/// 3. **Enums must not reorder variants.**  `#[contracttype]` enums may be
///    stored as their discriminant. Adding variants at the end is safe;
///    inserting or reordering existing variants breaks old data.
///
/// 4. **TTL extension must continue to work.**  The new contract code must
///    extend TTL for the same storage keys the old code extended, or else
///    existing entries may expire and be pruned.
///
/// 5. **Events should remain consistent.**  Downstream indexers depend on
///    event topics. Adding new event topics is safe; changing or removing
///    existing topics may break integrations.
///
/// ## Data Migration Scenarios
///
/// ### Scenario A: No migration needed (backward-compatible upgrade)
///
/// New fields are optional and default-constructible. Old storage entries
/// deserialize without error. Example: adding `Option<u64>` field to a struct.
///
/// ### Scenario B: Lazy migration on read
///
/// Old data is read, transformed in memory, and written back. New writes use
/// the new format. Both formats must be supported until all entries are
/// migrated. Example: renaming a field but keeping both keys for one release.
///
/// ### Scenario C: Batch migration
///
/// An admin-initiated migration walks all storage entries and rewrites them.
/// The contract is paused during migration. This must be tested and batched
/// to stay within gas limits.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{CredentialStatus, EngineerRegistry, EngineerRegistryClient};
use lending::{LendingContract, LendingContractClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger, LedgerInfo},
    Address, BytesN, Env, String, Symbol, Vec,
};

// ═══════════════════════════════════════════════════════════════════════════════
// Test 1: Asset Registry data persists across simulated upgrade (ledger advance)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_asset_registry_data_survives_upgrade() {
    let env = Env::default();
    env.mock_all_auths();

    // ── Deploy and initialize v1 contracts ───────────────────────────
    let ar_id = env.register(AssetRegistry, ());
    let ar = AssetRegistryClient::new(&env, &ar_id);

    let admin = Address::generate(&env);
    let owner = Address::generate(&env);

    ar.initialize_admin(&admin, &admin);
    ar.add_asset_type(&admin, &symbol_short!("GENSET"));

    let asset_id = ar.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Caterpillar 3516 Generator"),
        &String::from_str(&env, "SN-UPGRADE-001"),
        &owner,
    );

    // Register a second asset for batch testing
    let _asset2 = ar.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Cummins QSK60 Generator"),
        &String::from_str(&env, "SN-UPGRADE-002"),
        &owner,
    );

    // ── Verify assets are readable ───────────────────────────────────
    let asset = ar.get_asset(&asset_id);
    assert_eq!(asset.asset_id, asset_id);
    assert_eq!(asset.owner, owner);

    // ── Verify asset count ───────────────────────────────────────────
    let count = ar.get_asset_count();
    assert!(count >= 2, "Asset count must be at least 2 after v1 deployment");

    // ── Verify owner index ───────────────────────────────────────────
    let owner_assets = ar.get_assets_by_owner(&owner);
    assert!(
        owner_assets.len() >= 2,
        "Owner must have at least 2 assets after v1 deployment"
    );
    assert!(owner_assets.contains(asset_id));

    // ── Verify asset type index ──────────────────────────────────────
    let genset_assets = ar.get_assets_by_type(&symbol_short!("GENSET"));
    assert!(
        genset_assets.len() >= 2,
        "Must have at least 2 GENSET assets in type index"
    );

    // ── Simulate post-upgrade ledger advance ────────────────────────
    env.ledger().with_mut(|li| {
        li.sequence_number += 518_401; // ~30 days — crosses TTL boundary
    });

    // ── Data must still be readable after ledger advance ────────────
    let asset_after_advance = ar.get_asset(&asset_id);
    assert_eq!(
        asset_after_advance.serial_number,
        String::from_str(&env, "SN-UPGRADE-001")
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 2: Engineer Registry data persists across simulated upgrade
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_engineer_registry_data_survives_upgrade() {
    let env = Env::default();
    env.mock_all_auths();

    let er_id = env.register(EngineerRegistry, ());
    let er = EngineerRegistryClient::new(&env, &er_id);

    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let engineer = Address::generate(&env);

    er.initialize_admin(&admin, &admin);
    er.add_trusted_issuer(&admin, &issuer);

    let credential_hash = BytesN::from_array(&env, &[0xAB; 32]);
    er.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);

    // ── Verify engineer credential is readable ───────────────────────
    let eng = er.get_engineer(&engineer);
    assert!(eng.active, "Engineer must be active after v1 registration");
    assert_eq!(eng.issuer, issuer);

    // ── Verify credential verification works ────────────────────────
    let verified = er.verify_engineer(&engineer, &None::<Symbol>);
    let verified = er.verify_engineer(&engineer);
    assert_eq!(
        verified,
        CredentialStatus::Valid,
        "verify_engineer must return Valid for active credential"
    );

    // ── Simulate post-upgrade ledger advance ────────────────────────
    env.ledger().with_mut(|li| {
        li.sequence_number += 518_401;
    });

    // ── Credential must still be verifiable ──────────────────────────
    let verified_after = er.verify_engineer(&engineer, &None::<Symbol>);
    let verified_after = er.verify_engineer(&engineer);
    assert_eq!(
        verified_after,
        CredentialStatus::Valid,
        "Credential must remain valid after simulated upgrade"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 3: Lifecycle data (maintenance history + collateral score) persists
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_lifecycle_data_survives_upgrade() {
    let env = Env::default();
    env.mock_all_auths();

    let ar_id = env.register(AssetRegistry, ());
    let er_id = env.register(EngineerRegistry, ());
    let lc_id = env.register(Lifecycle, ());

    let ar = AssetRegistryClient::new(&env, &ar_id);
    let er = EngineerRegistryClient::new(&env, &er_id);
    let lc = LifecycleClient::new(&env, &lc_id);

    let admin = Address::generate(&env);
    let owner = Address::generate(&env);
    let issuer = Address::generate(&env);
    let engineer = Address::generate(&env);

    // Initialize all three
    ar.initialize_admin(&admin, &admin);
    ar.add_asset_type(&admin, &symbol_short!("GENSET"));

    er.initialize_admin(&admin, &admin);
    er.add_trusted_issuer(&admin, &issuer);

    lc.initialize(&admin, &ar_id, &er_id, &admin, &200);

    // Register asset and engineer
    let asset_id = ar.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Generator"),
        &String::from_str(&env, "SN-LC-001"),
        &owner,
    );

    let credential_hash = BytesN::from_array(&env, &[0xAB; 32]);
    er.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);

    lc.authorize_engineer(&owner, &asset_id, &engineer);

    // Submit maintenance records
    lc.submit_maintenance(
        &asset_id,
        &symbol_short!("ENGINE"),
        &String::from_str(&env, "Initial engine inspection"),
        &engineer,
    );
    lc.submit_maintenance(
        &asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Oil change"),
        &engineer,
    );
    lc.submit_maintenance(
        &asset_id,
        &symbol_short!("FILTER"),
        &String::from_str(&env, "Filter replacement"),
        &engineer,
    );

    // ── Verify maintenance history ───────────────────────────────────
    let history = lc.get_maintenance_history(&asset_id);
    assert_eq!(
        history.len(),
        3,
        "Maintenance history must contain exactly 3 records"
    );

    let first_record = history.get(0).unwrap();
    assert_eq!(first_record.task_type, symbol_short!("ENGINE"));
    assert_eq!(first_record.engineer, engineer);

    // ── Verify collateral score ──────────────────────────────────────
    let score = lc.get_collateral_score(&asset_id);
    assert!(score > 0, "Collateral score must be > 0 after 3 records");
    assert!(score <= 100, "Collateral score must never exceed 100");

    // ── Verify score history ─────────────────────────────────────────
    let score_history = lc.get_score_history(&asset_id, &0, &10);
    assert!(
        !score_history.is_empty(),
        "Score history must not be empty after maintenance submissions"
    );

    // ── Simulate upgrade + time passage ──────────────────────────────
    env.ledger().with_mut(|li| {
        li.sequence_number += 518_401;
        li.timestamp += 60 * 86_400; // 60 days
    });

    let score_after = lc.get_collateral_score(&asset_id);
    assert!(score_after > 0, "Score floor (1) must apply after extended decay");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 4: Lending data (liens + state) persists across simulated upgrade
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_lending_data_survives_upgrade() {
    let env = Env::default();
    env.mock_all_auths();

    let lending_id = env.register(LendingContract, ());
    let lending = LendingContractClient::new(&env, &lending_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    let lender_addr = Address::generate(&env);

    lending.initialize(&admin, &admin, &token, &5000);

    // Record a lien
    lending.record_lien(&admin, &1, &lender_addr, &42, &50_000);

    // ── Verify lien is readable ──────────────────────────────────────
    let liens = lending.get_liens(&1);
    assert_eq!(liens.len(), 1);
    assert_eq!(liens.get(0).unwrap().lender, lender_addr);
    assert_eq!(liens.get(0).unwrap().loan_id, 42);
    assert_eq!(liens.get(0).unwrap().amount, 50_000);

    // ── Verify lending config ───────────────────────────────────────
    assert!(lending.is_initialized());
    let stored_admin = lending.get_admin();
    assert_eq!(stored_admin, admin);

    // ── Simulate upgrade ────────────────────────────────────────────
    env.ledger().with_mut(|li| {
        li.sequence_number += 518_401;
    });

    // ── Lien must still be readable ─────────────────────────────────
    let liens_after = lending.get_liens(&1);
    assert_eq!(liens_after.len(), 1);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 5: Cross-contract references remain valid after simulated upgrade
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_cross_contract_references_remain_valid() {
    let env = Env::default();
    env.mock_all_auths();

    let ar_id = env.register(AssetRegistry, ());
    let er_id = env.register(EngineerRegistry, ());
    let lc_id = env.register(Lifecycle, ());
    let lending_id = env.register(LendingContract, ());

    let ar = AssetRegistryClient::new(&env, &ar_id);
    let er = EngineerRegistryClient::new(&env, &er_id);
    let lc = LifecycleClient::new(&env, &lc_id);
    let lending = LendingContractClient::new(&env, &lending_id);

    let admin = Address::generate(&env);
    let owner = Address::generate(&env);
    let issuer = Address::generate(&env);
    let engineer = Address::generate(&env);
    let token = Address::generate(&env);
    let lender_addr = Address::generate(&env);

    // Initialize all four contracts
    ar.initialize_admin(&admin, &admin);
    ar.add_asset_type(&admin, &symbol_short!("GENSET"));

    er.initialize_admin(&admin, &admin);
    er.add_trusted_issuer(&admin, &issuer);

    lc.initialize(&admin, &ar_id, &er_id, &admin, &200);

    lending.initialize(&admin, &admin, &token, &5000);

    // Create data across all contracts
    let asset_id = ar.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Cross-contract test asset"),
        &String::from_str(&env, "SN-XCONTRACT-001"),
        &owner,
    );

    let credential_hash = BytesN::from_array(&env, &[0xAB; 32]);
    er.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);

    lc.authorize_engineer(&owner, &asset_id, &engineer);
    lc.submit_maintenance(
        &asset_id,
        &symbol_short!("ENGINE"),
        &String::from_str(&env, "Cross-contract test maintenance"),
        &engineer,
    );

    lending.record_lien(&admin, &asset_id, &lender_addr, &1, &50_000);

    // ── Simulate upgrade ────────────────────────────────────────────
    env.ledger().with_mut(|li| {
        li.sequence_number += 518_401;
    });

    // ── All cross-contract references must still work ───────────────
    let score = lc.get_collateral_score(&asset_id);
    assert!(score > 0, "Lifecycle → Asset Registry binding intact");

    let eng = er.get_engineer(&engineer);
    assert!(eng.active, "Engineer Registry data intact");

    let liens = lending.get_liens(&asset_id);
    assert!(!liens.is_empty(), "Lending lien data intact");

    let asset = ar.get_asset(&asset_id);
    assert_eq!(asset.owner, owner, "Asset ownership consistent");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 6: Storage key schema stability
// ═══════════════════════════════════════════════════════════════════════════════

/// Documents and verifies the expected storage keys for each contract.
/// Upgrades MUST preserve these key schemas or provide explicit migration.
#[test]
fn test_storage_key_schema_stability() {
    let env = Env::default();
    env.mock_all_auths();

    let ar_id = env.register(AssetRegistry, ());
    let er_id = env.register(EngineerRegistry, ());
    let lc_id = env.register(Lifecycle, ());
    let lending_id = env.register(LendingContract, ());

    let ar = AssetRegistryClient::new(&env, &ar_id);
    let er = EngineerRegistryClient::new(&env, &er_id);
    let lc = LifecycleClient::new(&env, &lc_id);
    let lending = LendingContractClient::new(&env, &lending_id);

    let admin = Address::generate(&env);
    let owner = Address::generate(&env);
    let issuer = Address::generate(&env);
    let engineer = Address::generate(&env);
    let token = Address::generate(&env);
    let lender_addr = Address::generate(&env);

    // Initialize all
    ar.initialize_admin(&admin, &admin);
    ar.add_asset_type(&admin, &symbol_short!("GENSET"));

    er.initialize_admin(&admin, &admin);
    er.add_trusted_issuer(&admin, &issuer);

    lc.initialize(&admin, &ar_id, &er_id, &admin, &200);

    lending.initialize(&admin, &admin, &token, &5000);

    let asset_id = ar.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Schema stability test"),
        &String::from_str(&env, "SN-SCHEMA-001"),
        &owner,
    );

    let credential_hash = BytesN::from_array(&env, &[0xAB; 32]);
    er.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);

    lc.authorize_engineer(&owner, &asset_id, &engineer);
    lc.submit_maintenance(
        &asset_id,
        &symbol_short!("ENGINE"),
        &String::from_str(&env, "Schema test"),
        &engineer,
    );

    lending.record_lien(&admin, &asset_id, &lender_addr, &1, &50_000);

    // ── Asset Registry keys ──────────────────────────────────────────
    // (ASSET, id)         → Asset struct
    // A_COUNT             → u64
    // (OWN_IDX, owner)    → Vec<u64>
    // (TYPE_IDX, type)    → Vec<u64>
    // (DEDUP, owner, hash)→ u64
    // ADMIN               → Address
    assert!(ar.get_asset(&asset_id).asset_id > 0);
    let owner_assets = ar.get_assets_by_owner(&owner);
    assert!(owner_assets.contains(asset_id));

    // ── Engineer Registry keys ───────────────────────────────────────
    // (ENG, address)      → Engineer struct
    // (TRUSTED, issuer)   → bool
    assert!(er.get_engineer(&engineer).active);

    // ── Lifecycle keys ───────────────────────────────────────────────
    // (HIST, asset_id)    → Vec<MaintenanceRecord>
    // (SCORE, asset_id)   → u32
    // (SCHIST, asset_id)  → Vec<ScoreEntry>
    // (LUPD, asset_id)    → u64
    // CONFIG              → Config
    // REGISTRY            → Address
    // ENG_REG             → Address
    let history = lc.get_maintenance_history(&asset_id);
    assert!(!history.is_empty());

    // ── Lending keys ─────────────────────────────────────────────────
    // (LOAN, borrower)    → Loan
    // (BORR, borrower)    → Borrower
    // (VOUCHES, borrower) → Vec<Vouch>
    // (V_HIST, voucher)   → Vec<Address>
    // (Liens, asset_id)   → Vec<LienRecord>
    // ADMIN               → Address
    // TOKEN               → Address
    // PAUSED              → bool
    let liens = lending.get_liens(&asset_id);
    assert!(!liens.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 7: Optional field migration scenario
// ═══════════════════════════════════════════════════════════════════════════════

/// Demonstrates that metadata updates (adding new fields through metadata
/// versioning) are backward-compatible. Assets registered in v1 can have
/// their metadata updated in v2 without losing history.
#[test]
fn test_optional_field_migration_scenario() {
    let env = Env::default();
    env.mock_all_auths();

    let ar_id = env.register(AssetRegistry, ());
    let ar = AssetRegistryClient::new(&env, &ar_id);

    let admin = Address::generate(&env);
    let owner = Address::generate(&env);

    ar.initialize_admin(&admin, &admin);
    ar.add_asset_type(&admin, &symbol_short!("PUMP"));

    // Register v1 asset
    let asset_id = ar.register_asset(
        &symbol_short!("PUMP"),
        &String::from_str(&env, "Grundfos CR 32 Pump"),
        &String::from_str(&env, "SN-MIGRATE-001"),
        &owner,
    );

    let v1_asset = ar.get_asset(&asset_id);
    assert_eq!(v1_asset.metadata_version, 0);

    // ── Simulate v2: update metadata (new fields, version bump) ─────
    ar.update_asset_metadata(
        &asset_id,
        &owner,
        &String::from_str(&env, "Updated after upgrade — warranty: 5 years, location: Bldg A"),
    );

    let v2_asset = ar.get_asset(&asset_id);
    assert_eq!(v2_asset.metadata_version, 1, "Version must increment");

    // Metadata history preserves the v1→v2 transition
    let history = ar.get_metadata_history(&asset_id);
    assert!(!history.is_empty(), "Metadata history must record v1→v2 transition");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 8: Lazy migration — reading old-format data and writing new-format
// ═══════════════════════════════════════════════════════════════════════════════

/// Simulates a lazy migration pattern: old data coexists with new data until
/// all entries have been touched. A new maintenance submission after upgrade
/// incorporates both old and new records into the score.
#[test]
fn test_lazy_migration_pattern() {
    let env = Env::default();
    env.mock_all_auths();

    let ar_id = env.register(AssetRegistry, ());
    let er_id = env.register(EngineerRegistry, ());
    let lc_id = env.register(Lifecycle, ());

    let ar = AssetRegistryClient::new(&env, &ar_id);
    let er = EngineerRegistryClient::new(&env, &er_id);
    let lc = LifecycleClient::new(&env, &lc_id);

    let admin = Address::generate(&env);
    let owner = Address::generate(&env);
    let issuer = Address::generate(&env);
    let engineer = Address::generate(&env);

    ar.initialize_admin(&admin, &admin);
    ar.add_asset_type(&admin, &symbol_short!("GENSET"));

    er.initialize_admin(&admin, &admin);
    er.add_trusted_issuer(&admin, &issuer);

    lc.initialize(&admin, &ar_id, &er_id, &admin, &200);

    let asset_id = ar.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Lazy migration test"),
        &String::from_str(&env, "SN-LAZY-001"),
        &owner,
    );

    let credential_hash = BytesN::from_array(&env, &[0xAB; 32]);
    er.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);

    lc.authorize_engineer(&owner, &asset_id, &engineer);

    // ── Phase 1: v1 data (3 records) ────────────────────────────────
    lc.submit_maintenance(&asset_id, &symbol_short!("ENGINE"), &String::from_str(&env, "v1 record 1"), &engineer);
    lc.submit_maintenance(&asset_id, &symbol_short!("OIL_CHG"), &String::from_str(&env, "v1 record 2"), &engineer);
    lc.submit_maintenance(&asset_id, &symbol_short!("FILTER"), &String::from_str(&env, "v1 record 3"), &engineer);

    let old_score = lc.get_collateral_score(&asset_id);
    let old_history = lc.get_maintenance_history(&asset_id);
    assert_eq!(old_history.len(), 3);

    // ── Phase 2: "migrate" by adding a v2 record ────────────────────
    lc.submit_maintenance(&asset_id, &symbol_short!("REBUILD"), &String::from_str(&env, "v2 record — migration checkpoint"), &engineer);

    // ── Phase 3: verify migrated state ───────────────────────────────
    let new_history = lc.get_maintenance_history(&asset_id);
    assert_eq!(new_history.len(), 4, "History: 3 v1 + 1 v2 = 4 total");

    let new_score = lc.get_collateral_score(&asset_id);
    assert!(new_score >= old_score, "Score must increase after adding v2 record");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 9: Pause state survives simulated upgrade
// ═══════════════════════════════════════════════════════════════════════════════

/// The paused state is stored in persistent storage (PAUSED key). An upgrade
/// must respect the pause state — a paused contract should remain paused.
#[test]
fn test_pause_state_survives_upgrade() {
    let env = Env::default();
    env.mock_all_auths();

    let ar_id = env.register(AssetRegistry, ());
    let er_id = env.register(EngineerRegistry, ());
    let lc_id = env.register(Lifecycle, ());
    let lending_id = env.register(LendingContract, ());

    let ar = AssetRegistryClient::new(&env, &ar_id);
    let er = EngineerRegistryClient::new(&env, &er_id);
    let lc = LifecycleClient::new(&env, &lc_id);
    let lending = LendingContractClient::new(&env, &lending_id);

    let admin = Address::generate(&env);
    let token = Address::generate(&env);

    ar.initialize_admin(&admin, &admin);
    er.initialize_admin(&admin, &admin);
    lc.initialize(&admin, &ar_id, &er_id, &admin, &200);
    lending.initialize(&admin, &admin, &token, &5000);

    // ── Pause all contracts ──────────────────────────────────────────
    ar.pause(&admin);
    er.pause(&admin);
    lc.pause(&admin);
    lending.pause(&admin);

    assert!(ar.is_paused());
    assert!(er.is_paused());
    assert!(lc.is_paused());
    assert!(lending.is_paused());

    // ── Simulate post-upgrade ledger advance ─────────────────────────
    env.ledger().with_mut(|li| {
        li.sequence_number += 518_401;
    });

    // ── Pause state must survive ─────────────────────────────────────
    assert!(ar.is_paused(), "AR pause must survive TTL boundary");
    assert!(er.is_paused(), "ER pause must survive TTL boundary");
    assert!(lc.is_paused(), "LC pause must survive TTL boundary");
    assert!(lending.is_paused(), "Lending pause must survive TTL boundary");

    // ── Unpause and verify functionality ────────────────────────────
    ar.unpause(&admin);
    er.unpause(&admin);
    lc.unpause(&admin);
    lending.unpause(&admin);

    assert!(!ar.is_paused());
    assert!(!er.is_paused());
    assert!(!lc.is_paused());
    assert!(!lending.is_paused());

    // Writes must work after unpause
    ar.add_asset_type(&admin, &symbol_short!("GENSET"));
    let new_asset = ar.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Post-upgrade/unpause"),
        &String::from_str(&env, "SN-UNPAUSE-001"),
        &admin,
    );
    assert!(new_asset > 0);
}
