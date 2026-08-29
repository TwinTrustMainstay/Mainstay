//! Backup verification tests.
//!
//! These tests validate that the backup export/restore workflow produces
//! correct and verifiable output.  They run inside the Soroban test
//! environment and do not require external network access.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{vec, Address, Env, String, Symbol};

/// Helper: register a single asset and return its ID, type, and owner.
fn register_test_asset(e: &Env, client: &AssetRegistryClient, owner: &Address, idx: u32) -> u64 {
    let asset_type = Symbol::new(e, "GENSET");
    let metadata = String::from_str(e, &format!("Backup test asset #{} — diesel generator", idx));
    let serial = String::from_str(e, &format!("SN-BACKUP-TEST-{:04}", idx));

    // Ensure the asset type is in the allowlist.
    // (In test we may need the admin to add it first.)
    client.register_asset(&asset_type, &metadata, &serial, owner)
}

/// Test that we can enumerate all assets by iterating through asset IDs
/// up to the asset count and retrieve each one — this simulates the
/// backup export flow.
#[test]
fn test_backup_export_all_assets() {
    let e = Env::default();
    e.mock_all_auths();

    // Deploy asset registry, engineer registry, and lifecycle
    let admin = Address::generate(&e);
    let owner = Address::generate(&e);
    let engineer = Address::generate(&e);

    let ar_id = e.register(AssetRegistry, ());
    let ar_client = AssetRegistryClient::new(&e, &ar_id);
    ar_client.initialize_admin(&admin, &admin);

    // Register asset types and assets
    let asset_type = Symbol::new(&e, "GENSET");
    ar_client.add_asset_type(&admin, &asset_type);

    // Register multiple assets
    let mut asset_ids: Vec<u64> = Vec::new(&e);
    for i in 0..5 {
        let id = register_test_asset(&e, &ar_client, &owner, i + 1);
        asset_ids.push_back(id);
    }

    // Verify asset count matches
    let count = ar_client.get_asset_count();
    assert_eq!(count, 5);
    assert_eq!(asset_ids.len(), 5);

    // Simulate backup: iterate all asset IDs and retrieve each one
    let mut retrieved = 0u32;
    for id in 1..=count {
        let asset = ar_client.get_asset(&id);
        assert_eq!(asset.asset_id, id);
        assert_eq!(asset.asset_type, asset_type);
        assert!(asset.metadata.contains(&String::from_str(&e, "Backup test")));
        assert_eq!(asset.owner, owner);
        retrieved += 1;
    }
    assert_eq!(retrieved, 5);
}

/// Test that asset metadata survives round-trip: register → retrieve → verify.
#[test]
fn test_backup_roundtrip_metadata_integrity() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let owner = Address::generate(&e);

    let ar_id = e.register(AssetRegistry, ());
    let ar_client = AssetRegistryClient::new(&e, &ar_id);
    ar_client.initialize_admin(&admin, &admin);

    let asset_type = Symbol::new(&e, "TURBINE");
    ar_client.add_asset_type(&admin, &asset_type);

    let metadata = String::from_str(&e, "Siemens SGT-800 gas turbine, 50 MW, s/n: T2024-001");
    let serial = String::from_str(&e, "T2024-001");

    let id = ar_client.register_asset(&asset_type, &metadata, &serial, &owner);

    // Retrieve and verify all fields match
    let asset = ar_client.get_asset(&id);
    assert_eq!(asset.asset_id, id);
    assert_eq!(asset.asset_type, asset_type);
    assert_eq!(asset.metadata, metadata);
    assert_eq!(asset.serial_number, serial);
    assert_eq!(asset.owner, owner);
    assert_eq!(asset.metadata_version, 0);

    // Update metadata and verify version increments
    let new_meta = String::from_str(&e, "Siemens SGT-800 after 2024 overhaul");
    ar_client.update_asset_metadata(&id, &owner, &new_meta);

    let updated = ar_client.get_asset(&id);
    assert_eq!(updated.metadata, new_meta);
    assert_eq!(updated.metadata_version, 1);

    // Metadata history should be preserved
    let history = ar_client.get_metadata_history(&id);
    assert_eq!(history.len(), 1); // one version bump
    assert_eq!(history.first().unwrap().version, 1);
}

/// Test that the asset count does not decrease after a deregistration,
/// ensuring backup iterators never skip IDs.
#[test]
fn test_backup_asset_count_monotonic() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let owner = Address::generate(&e);

    let ar_id = e.register(AssetRegistry, ());
    let ar_client = AssetRegistryClient::new(&e, &ar_id);
    ar_client.initialize_admin(&admin, &admin);

    let asset_type = Symbol::new(&e, "PUMP");
    ar_client.add_asset_type(&admin, &asset_type);

    let meta = String::from_str(&e, "Centrifugal pump");
    let serial_a = String::from_str(&e, "PUMP-A-001");
    let serial_b = String::from_str(&e, "PUMP-B-002");

    let id_a = ar_client.register_asset(&asset_type, &meta, &serial_a, &owner);
    let id_b = ar_client.register_asset(&asset_type, &meta, &serial_b, &owner);

    assert_eq!(ar_client.get_asset_count(), 2);

    // Deregister asset A (with timelock)
    ar_client.propose_deregister_asset(&owner, &id_a);
    e.jump(48 * 60 * 60 + 1); // 48 hours + 1 second
    ar_client.execute_deregister_asset(&owner, &id_a);

    // Asset count should NOT decrease (monotonic counter)
    assert_eq!(ar_client.get_asset_count(), 2);

    // Asset A should no longer be retrievable
    assert!(!ar_client.asset_exists(&id_a));

    // Asset B should still be retrievable
    assert!(ar_client.asset_exists(&id_b));
    let asset_b = ar_client.get_asset(&id_b);
    assert_eq!(asset_b.asset_id, id_b);
}

/// Test that backup manifest fields are populated correctly when programmatically
/// constructing a backup manifest from on-chain data.
#[test]
fn test_backup_manifest_generation() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let owner = Address::generate(&e);

    let ar_id = e.register(AssetRegistry, ());
    let ar_client = AssetRegistryClient::new(&e, &ar_id);
    ar_client.initialize_admin(&admin, &admin);

    let asset_type = Symbol::new(&e, "COMPRESSOR");
    ar_client.add_asset_type(&admin, &asset_type);

    // Register 3 assets
    for i in 0..3 {
        let meta = String::from_str(&e, &format!("Air compressor unit {}", i + 1));
        let serial = String::from_str(&e, &format!("CMP-{:04}", i + 1));
        ar_client.register_asset(&asset_type, &meta, &serial, &owner);
    }

    // Collect backup facts
    let total = ar_client.get_asset_count();
    assert_eq!(total, 3);

    let mut serials: Vec<String> = Vec::new(&e);
    let mut metadatas: Vec<String> = Vec::new(&e);
    for id in 1..=total {
        let a = ar_client.get_asset(&id);
        serials.push_back(a.serial_number.clone());
        metadatas.push_back(a.metadata.clone());
    }

    // Verify all expected serial numbers exist
    let expected: Vec<String> = vec![
        &e,
        String::from_str(&e, "CMP-0001"),
        String::from_str(&e, "CMP-0002"),
        String::from_str(&e, "CMP-0003"),
    ];
    for exp in expected.iter() {
        assert!(serials.contains(&exp));
    }
    assert_eq!(metadatas.len(), 3);
}
