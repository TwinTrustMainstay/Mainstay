// tests/test_maintenance_history_paginated.rs
//
// Issue #996 — get_maintenance_history_paginated
//
// Verifies that the new paginated endpoint:
//   • Returns the correct slice for a given offset/limit
//   • Caps limit at MAX_PAGINATED_LIMIT (50)
//   • Returns an empty vec when offset ≥ history length
//   • Returns an empty vec when limit = 0
//   • Returns a partial page when the slice reaches the end of history

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient, MAX_PAGINATED_LIMIT};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

/// Number of maintenance records to insert in the multi-record tests.
const HISTORY_SIZE: u32 = 60;

/// Set up a minimal environment with one asset, one engineer, and
/// `count` submitted maintenance records.
fn setup(env: &Env, count: u32) -> (LifecycleClient, u64) {
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(env, &lifecycle_id);

    let admin = Address::generate(env);
    let owner = Address::generate(env);
    let engineer = Address::generate(env);
    let issuer = Address::generate(env);

    asset_registry.initialize_admin(&admin, &admin);
    asset_registry.add_asset_type(&admin, &symbol_short!("GENSET"));
    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);
    // max_history = 0 means unlimited
    lifecycle.initialize(&admin, &asset_registry_id, &engineer_registry_id, &admin, &0);

    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(env, "Pagination test asset"),
        &String::from_str(env, "SN-PAGE-001"),
        &owner,
    );

    let credential_hash = BytesN::from_array(env, &[5u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    for _ in 0..count {
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &String::from_str(env, "routine oil change"),
            &engineer,
            &None,
        );
        env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    }

    (lifecycle, asset_id)
}

#[test]
fn test_paginated_first_page() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id) = setup(&env, HISTORY_SIZE);

    let page = lifecycle.get_maintenance_history_paginated(&asset_id, &0, &10);
    assert_eq!(
        page.len(),
        10,
        "first page of 10 should return exactly 10 records"
    );
}

#[test]
fn test_paginated_second_page() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id) = setup(&env, HISTORY_SIZE);

    let page1 = lifecycle.get_maintenance_history_paginated(&asset_id, &0, &10);
    let page2 = lifecycle.get_maintenance_history_paginated(&asset_id, &10, &10);

    assert_eq!(page1.len(), 10);
    assert_eq!(page2.len(), 10);

    // The two pages must not overlap: first record of page2 differs from last of page1.
    let last_p1 = page1.get(9).unwrap();
    let first_p2 = page2.get(0).unwrap();
    assert_ne!(
        last_p1.timestamp, first_p2.timestamp,
        "pages should not overlap"
    );
}

#[test]
fn test_paginated_limit_capped_at_50() {
    let env = Env::default();
    env.mock_all_auths();

    // Insert HISTORY_SIZE (60) records so we have more than MAX_PAGINATED_LIMIT.
    let (lifecycle, asset_id) = setup(&env, HISTORY_SIZE);

    // Requesting more than MAX_PAGINATED_LIMIT should be silently capped.
    let page = lifecycle.get_maintenance_history_paginated(&asset_id, &0, &200);
    assert_eq!(
        page.len(),
        MAX_PAGINATED_LIMIT,
        "limit must be capped at MAX_PAGINATED_LIMIT ({})",
        MAX_PAGINATED_LIMIT
    );
}

#[test]
fn test_paginated_offset_beyond_end_returns_empty() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id) = setup(&env, 5);

    let page = lifecycle.get_maintenance_history_paginated(&asset_id, &100, &10);
    assert!(
        page.is_empty(),
        "offset beyond history length should return empty vec"
    );
}

#[test]
fn test_paginated_zero_limit_returns_empty() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id) = setup(&env, 5);

    let page = lifecycle.get_maintenance_history_paginated(&asset_id, &0, &0);
    assert!(page.is_empty(), "limit = 0 should return empty vec");
}

#[test]
fn test_paginated_partial_last_page() {
    let env = Env::default();
    env.mock_all_auths();

    // 7 records total; fetch from offset 5 with limit 10 → should get 2 records.
    let (lifecycle, asset_id) = setup(&env, 7);

    let page = lifecycle.get_maintenance_history_paginated(&asset_id, &5, &10);
    assert_eq!(
        page.len(),
        2,
        "partial last page should contain remaining records (7 - 5 = 2)"
    );
}

#[test]
fn test_paginated_matches_full_history_in_order() {
    let env = Env::default();
    env.mock_all_auths();

    let (lifecycle, asset_id) = setup(&env, 10);

    let full = lifecycle.get_maintenance_history(&asset_id);
    let page = lifecycle.get_maintenance_history_paginated(&asset_id, &0, &10);

    assert_eq!(
        full.len(),
        page.len(),
        "paginated(offset=0, limit=10) should equal full history for 10 records"
    );
    for i in 0..full.len() {
        assert_eq!(
            full.get(i).unwrap().timestamp,
            page.get(i).unwrap().timestamp,
            "record {} should match between full history and paginated page",
            i
        );
    }
}
