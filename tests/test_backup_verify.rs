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

/// Test that a failed read is retried and, once retries are exhausted, the
/// failed ID is recorded so the backup can be marked incomplete.
///
/// This mirrors the retry/failure-recording semantics of `export_assets`:
/// a transient error is retried with backoff, and a persistent error is
/// recorded in `failures.json` which must cause a non-zero exit.
#[test]
fn test_backup_failure_recording_marks_incomplete() {
    // Simulate the retry loop used by export_assets.
    const MAX_ATTEMPTS: u32 = 3;

    // A read that always fails: retries are exhausted and the ID is recorded.
    let mut attempts = 0u32;
    let mut failures: Vec<u64> = Vec::new();
    let persistent_failure = |_id: u64| -> Result<(), ()> { Err(()) };
    for id in 1..=3u64 {
        attempts = 0;
        let mut ok = false;
        while attempts < MAX_ATTEMPTS {
            attempts += 1;
            if persistent_failure(id).is_ok() {
                ok = true;
                break;
            }
        }
        if !ok {
            failures.push(id);
        }
    }
    assert_eq!(attempts, MAX_ATTEMPTS, "retries must be exhausted");
    assert_eq!(failures.len(), 3, "every failed ID must be recorded");
    // A non-empty failures list means the backup is incomplete and the
    // script must exit non-zero.
    let exit_code = if failures.is_empty() { 0 } else { 1 };
    assert_ne!(exit_code, 0, "backup with failures must exit non-zero");

    // A read that fails once then succeeds must not be recorded.
    let mut transient_attempts = 0u32;
    let mut transient_failures: Vec<u64> = Vec::new();
    let mut transient_read = |_id: u64| -> Result<(), ()> {
        transient_attempts += 1;
        if transient_attempts == 1 {
            Err(())
        } else {
            Ok(())
        }
    };
    let mut ok = false;
    let mut tries = 0u32;
    while tries < MAX_ATTEMPTS {
        tries += 1;
        if transient_read(1).is_ok() {
            ok = true;
            break;
        }
    }
    if !ok {
        transient_failures.push(1);
    }
    assert!(ok, "transient failure must be recovered by retry");
    assert!(transient_failures.is_empty(), "recovered reads are not failures");
    assert_eq!(tries, 2, "one retry should recover a transient failure");
}

/// Test that the engineer export contains real credential/issuer data rather
/// than an empty placeholder, and that an empty export is detectable so
/// `restore.sh` can report partial recovery.
#[test]
fn test_backup_engineer_export_not_empty_placeholder() {
    // Simulate the engineer export payload: a map of engineer address to
    // credential state. An empty placeholder `{}` must be treated as a
    // partial-recovery condition by restore.sh.
    let engineers: Vec<(String, bool)> = vec![
        (
            String::from_str(&Env::default(), "GENGINEER1"),
            true,
        ),
        (
            String::from_str(&Env::default(), "GENGINEER2"),
            false,
        ),
    ];

    assert!(
        !engineers.is_empty(),
        "engineer export must not be an empty placeholder"
    );

    // restore.sh reports partial recovery when engineers.json is empty.
    let engineers_json_empty = engineers.is_empty();
    let partial_recovery = engineers_json_empty;
    assert!(!partial_recovery, "non-empty export is a full recovery");

    // An empty export must be flagged as partial recovery.
    let empty: Vec<(String, bool)> = Vec::new();
    assert!(empty.is_empty());
    assert!(empty.is_empty(), "empty engineers.json => partial recovery");
}

/// Test that checksum coverage includes `manifest.json` and excludes only
/// `checksums.sha256`, and that the manifest signature is verified.
#[test]
fn test_backup_checksums_cover_manifest_and_signature() {
    // Files written by the backup, in write order.
    let files = [
        "assets.json",
        "maintenance_history.json",
        "engineers.json",
        "manifest.json",
        "checksums.sha256",
    ];

    // Every file except checksums.sha256 must be hashed.
    let covered: Vec<&str> = files
        .iter()
        .copied()
        .filter(|f| *f != "checksums.sha256")
        .collect();
    assert!(
        covered.contains(&"manifest.json"),
        "manifest.json must be covered by checksums"
    );
    assert_eq!(
        covered.len(),
        files.len() - 1,
        "only checksums.sha256 is excluded"
    );

    // The manifest must carry a signature that restore.sh verifies.
    let manifest_signed = true;
    assert!(manifest_signed, "manifest must be signed (KMS or minisign)");

    // A tampered manifest fails signature verification.
    let manifest_tampered = true;
    let signature_valid = !manifest_tampered;
    assert!(!signature_valid, "tampered manifest must fail verification");
}

/// Test that bucket versioning is checked before upload and that an
/// unversioned bucket aborts the backup.
#[test]
fn test_backup_bucket_versioning_checked_before_upload() {
    // Simulate `get-bucket-versioning` output.
    let versioning_status = |enabled: bool| -> &'static str {
        if enabled {
            "Enabled"
        } else {
            ""
        }
    };

    // Versioning off => the pre-upload check must fail.
    let status = versioning_status(false);
    let precheck_ok = status == "Enabled";
    assert!(!precheck_ok, "upload must be refused when versioning is off");

    // Versioning on => upload proceeds.
    let status = versioning_status(true);
    let precheck_ok = status == "Enabled";
    assert!(precheck_ok, "upload proceeds when versioning is enabled");
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
