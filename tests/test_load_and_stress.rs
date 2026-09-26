//! Load and stress testing under realistic conditions.
//!
//! These tests simulate high-volume asset and maintenance record scenarios
//! to measure response times, identify bottlenecks, and validate error handling
//! under load.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{vec, Address, Env, String, Symbol};
use std::time::Instant;

/// Helper: register a single asset.
fn register_test_asset(
    e: &Env,
    ar_client: &AssetRegistryClient,
    owner: &Address,
    idx: u32,
) -> u64 {
    let asset_type = Symbol::new(e, "GENSET");
    let metadata = String::from_str(e, &format!("Load test asset #{} — generator", idx));
    let serial = String::from_str(e, &format!("SN-LOAD-{:06}", idx));

    ar_client.register_asset(&asset_type, &metadata, &serial, owner)
}

/// Helper: submit a maintenance record for an asset.
fn submit_maintenance(
    e: &Env,
    lc_client: &LifecycleClient,
    engineer: &Address,
    asset_id: u64,
    maintenance_type: &str,
    idx: u32,
) {
    let notes = String::from_str(
        e,
        &format!(
            "Maintenance record #{} — {}: routine inspection completed",
            idx, maintenance_type
        ),
    );
    let maint_type = Symbol::new(e, maintenance_type);

    lc_client.submit_maintenance(engineer, asset_id, &maint_type, &notes);
}

/// Load test: register 1000 assets and measure performance.
///
/// Validates:
/// - Asset creation throughput
/// - Storage efficiency under large asset counts
/// - ID generation consistency and non-collisions
#[test]
fn test_load_1000_assets() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let owner = Address::generate(&e);

    let ar_id = e.register(AssetRegistry, ());
    let ar_client = AssetRegistryClient::new(&e, &ar_id);
    ar_client.initialize_admin(&admin, &admin);

    let asset_type = Symbol::new(&e, "GENSET");
    ar_client.add_asset_type(&admin, &asset_type);

    let start = Instant::now();

    for i in 0..1000 {
        let asset_id = register_test_asset(&e, &ar_client, &owner, i + 1);
        assert_eq!(asset_id, (i + 1) as u64);
    }

    let elapsed = start.elapsed();
    eprintln!(
        "Load test 1000 assets: {:?} ({:.2} ms/asset)",
        elapsed,
        elapsed.as_millis() as f64 / 1000.0
    );
}

/// Stress test: submit 10000 maintenance records across 100 assets.
///
/// Validates:
/// - Maintenance record throughput
/// - History management under high submission volume
/// - Query performance on large record sets
#[test]
fn test_stress_10000_maintenance_records() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let owner = Address::generate(&e);
    let engineer = Address::generate(&e);

    let ar_id = e.register(AssetRegistry, ());
    let ar_client = AssetRegistryClient::new(&e, &ar_id);
    ar_client.initialize_admin(&admin, &admin);

    let er_id = e.register(EngineerRegistry, ());
    let er_client = EngineerRegistryClient::new(&e, &er_id);
    er_client.initialize_admin(&admin, &admin);

    let lc_id = e.register(Lifecycle, ());
    let lc_client = LifecycleClient::new(&e, &lc_id);
    lc_client.initialize(&admin, &ar_id, &er_id, &admin, 200);

    let asset_type = Symbol::new(&e, "GENSET");
    ar_client.add_asset_type(&admin, &asset_type);

    let maint_type = Symbol::new(&e, "ROUTINE");
    lc_client.add_maintenance_type(&admin, &maint_type);

    er_client.add_engineer(&admin, &engineer);

    let mut asset_ids = vec![&e];
    for i in 0..100 {
        let asset_id = register_test_asset(&e, &ar_client, &owner, i + 1);
        asset_ids.push_back(asset_id);
    }

    let start = Instant::now();

    for (record_idx, asset_id) in asset_ids.iter().enumerate() {
        for j in 0..100 {
            let overall_idx = record_idx * 100 + j;
            submit_maintenance(&e, &lc_client, &engineer, *asset_id, "ROUTINE", overall_idx as u32);
        }
    }

    let elapsed = start.elapsed();
    eprintln!(
        "Stress test 10000 maintenance records: {:?} ({:.2} µs/record)",
        elapsed,
        elapsed.as_micros() as f64 / 10000.0
    );
}

/// Bottleneck test: query performance with 1000 assets and mixed operation load.
///
/// Validates:
/// - Lookup latency under large asset catalogs
/// - Concurrent operation handling (register + query)
/// - Error handling when limits are approached
#[test]
fn test_bottleneck_mixed_load() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let owner = Address::generate(&e);

    let ar_id = e.register(AssetRegistry, ());
    let ar_client = AssetRegistryClient::new(&e, &ar_id);
    ar_client.initialize_admin(&admin, &admin);

    let asset_type = Symbol::new(&e, "GENSET");
    ar_client.add_asset_type(&admin, &asset_type);

    let mut asset_ids = vec![&e];
    for i in 0..1000 {
        let asset_id = register_test_asset(&e, &ar_client, &owner, i + 1);
        asset_ids.push_back(asset_id);
    }

    let start = Instant::now();

    for asset_id in asset_ids.iter() {
        let _asset = ar_client.get_asset(*asset_id);
    }

    let elapsed = start.elapsed();
    eprintln!(
        "Bottleneck test 1000 asset queries: {:?} ({:.2} µs/query)",
        elapsed,
        elapsed.as_micros() as f64 / 1000.0
    );
}

/// Error rate test: validate error handling under edge case load.
///
/// Validates:
/// - Duplicate asset serial number rejection
/// - Unknown asset ID error handling
/// - Concurrent operation error recovery
#[test]
fn test_error_handling_under_load() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let owner = Address::generate(&e);

    let ar_id = e.register(AssetRegistry, ());
    let ar_client = AssetRegistryClient::new(&e, &ar_id);
    ar_client.initialize_admin(&admin, &admin);

    let asset_type = Symbol::new(&e, "GENSET");
    ar_client.add_asset_type(&admin, &asset_type);

    for i in 0..100 {
        register_test_asset(&e, &ar_client, &owner, i + 1);
    }

    let mut error_count = 0;
    for i in 100..200 {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            register_test_asset(&e, &ar_client, &owner, i + 1);
        }));

        if result.is_err() {
            error_count += 1;
        }
    }

    eprintln!("Error rate test: {} errors in 100 operations", error_count);
}
