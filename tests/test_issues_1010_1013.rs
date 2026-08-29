// tests/test_issues_1010_1013.rs
//
// Tests for GitHub issues #1010, #1011, #1012, and #1013.
//
// #1010 — get_health_snapshots_paginated(asset_id, offset, limit)
// #1011 — revoke_engineer_authorization(owner, asset_id, engineer)
// #1012 — get_authorized_engineers(asset_id)
// #1013 — decommission_asset(owner, asset_id)

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String, Vec,
};

// ─── Error discriminants ──────────────────────────────────────────────────────

/// ContractError::AssetDecommissioned = 22
const LIFECYCLE_ASSET_DECOMMISSIONED: u32 = 22;
/// ContractError::ScoreFrozen = 21
const LIFECYCLE_SCORE_FROZEN: u32 = 21;
/// ContractError::UnauthorizedOwner = 15
const LIFECYCLE_UNAUTHORIZED_OWNER: u32 = 15;

// ─── Shared setup ─────────────────────────────────────────────────────────────

/// Full test harness: three contracts wired together, one asset, one engineer.
struct Setup<'a> {
    env: &'a Env,
    asset_registry: AssetRegistryClient<'a>,
    engineer_registry: EngineerRegistryClient<'a>,
    lifecycle: LifecycleClient<'a>,
    asset_admin: Address,
    lc_admin: Address,
    owner: Address,
    asset_id: u64,
    engineer: Address,
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

        let asset_admin = Address::generate(env);
        let eng_admin = Address::generate(env);
        let lc_admin = Address::generate(env);
        let issuer = Address::generate(env);
        let owner = Address::generate(env);
        let engineer = Address::generate(env);

        // Bootstrap contracts.
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
            &String::from_str(env, "Test asset — issues 1010-1013"),
            &String::from_str(env, "SN-1010-1013-001"),
            &owner,
        );

        let credential_hash = BytesN::from_array(env, &[0xaau8; 32]);
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
            engineer_registry,
            lifecycle,
            asset_admin,
            lc_admin,
            owner,
            asset_id,
            engineer,
            issuer,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Issue #1011 — revoke_engineer_authorization
// ═══════════════════════════════════════════════════════════════════════════════

/// After revocation the engineer's auth flag must be cleared.
#[test]
fn test_1011_revoke_removes_authorization() {
    let env = Env::default();
    let s = Setup::new(&env);

    // Before revoke: engineer is authorized (submit_maintenance should succeed).
    s.lifecycle.submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Pre-revoke service"),
        &s.engineer,
    );
    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        1,
        "maintenance should succeed before revocation"
    );

    // Revoke the engineer's authorization.
    s.lifecycle
        .revoke_engineer_authorization(&s.owner, &s.asset_id, &s.engineer);

    // After revoke: submit_maintenance must be rejected.
    let result = s.lifecycle.try_submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Post-revoke attempt — must fail"),
        &s.engineer,
    );
    assert!(
        result.is_err(),
        "submit_maintenance must fail after engineer authorization is revoked"
    );

    // History must not have grown.
    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        1,
        "history must not grow after rejected submission"
    );
}

/// A non-owner must not be able to revoke an engineer's authorization.
#[test]
fn test_1011_revoke_rejects_non_owner() {
    let env = Env::default();
    let s = Setup::new(&env);
    let attacker = Address::generate(&env);

    let result = s.lifecycle.try_revoke_engineer_authorization(
        &attacker,
        &s.asset_id,
        &s.engineer,
    );

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_UNAUTHORIZED_OWNER,
        ))),
        "non-owner revoke must return UnauthorizedOwner"
    );
}

/// Revoking and then re-authorizing must restore submission rights.
#[test]
fn test_1011_revoke_then_reauthorize() {
    let env = Env::default();
    let s = Setup::new(&env);

    // Revoke.
    s.lifecycle
        .revoke_engineer_authorization(&s.owner, &s.asset_id, &s.engineer);

    // Re-authorize.
    s.lifecycle
        .authorize_engineer(&s.owner, &s.asset_id, &s.engineer);

    // Now maintenance must succeed again.
    s.lifecycle.submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "After re-authorization — should succeed"),
        &s.engineer,
    );
    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        1,
        "maintenance must succeed after re-authorization"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Issue #1012 — get_authorized_engineers
// ═══════════════════════════════════════════════════════════════════════════════

/// Initially exactly one engineer is authorized (from Setup).
#[test]
fn test_1012_get_authorized_engineers_single() {
    let env = Env::default();
    let s = Setup::new(&env);

    let list = s.lifecycle.get_authorized_engineers(&s.asset_id);
    assert_eq!(list.len(), 1, "exactly one engineer should be authorized");
    assert_eq!(
        list.get(0).unwrap(),
        s.engineer,
        "the authorized engineer must match"
    );
}

/// Adding a second engineer must appear in the list.
#[test]
fn test_1012_get_authorized_engineers_multiple() {
    let env = Env::default();
    let s = Setup::new(&env);

    let engineer2 = Address::generate(&env);
    let credential_hash2 = BytesN::from_array(&env, &[0xbbu8; 32]);
    s.engineer_registry.register_engineer(
        &engineer2,
        &credential_hash2,
        &s.issuer,
        &31_536_000,
        &None,
    );
    s.lifecycle
        .authorize_engineer(&s.owner, &s.asset_id, &engineer2);

    let list = s.lifecycle.get_authorized_engineers(&s.asset_id);
    assert_eq!(list.len(), 2, "two engineers should be authorized");
}

/// Authorizing the same engineer twice must not create duplicates.
#[test]
fn test_1012_no_duplicate_on_reauthorize() {
    let env = Env::default();
    let s = Setup::new(&env);

    // Authorize the same engineer again.
    s.lifecycle
        .authorize_engineer(&s.owner, &s.asset_id, &s.engineer);

    let list = s.lifecycle.get_authorized_engineers(&s.asset_id);
    assert_eq!(
        list.len(),
        1,
        "re-authorizing the same engineer must not create duplicates"
    );
}

/// Bulk revocation clears all requested authorizations in one call.
#[test]
fn test_bulk_revoke_engineer_authorizations() {
    let env = Env::default();
    let s = Setup::new(&env);
    let engineer2 = Address::generate(&env);
    let credential_hash2 = BytesN::from_array(&env, &[0xbbu8; 32]);
    s.engineer_registry.register_engineer(
        &engineer2,
        &credential_hash2,
        &s.issuer,
        &31_536_000,
        &None,
    );
    s.lifecycle.authorize_engineer(&s.owner, &s.asset_id, &engineer2);

    let mut engineers = Vec::new(&env);
    engineers.push_back(s.engineer.clone());
    engineers.push_back(engineer2.clone());
    s.lifecycle.batch_revoke_engineer_authorizations(&s.owner, &s.asset_id, &engineers);

    assert!(s.lifecycle.get_authorized_engineers(&s.asset_id).is_empty());
}

/// A non-owner must not be able to bulk revoke authorizations.
#[test]
fn test_bulk_revoke_rejects_non_owner() {
    let env = Env::default();
    let s = Setup::new(&env);
    let attacker = Address::generate(&env);
    let mut engineers = Vec::new(&env);
    engineers.push_back(s.engineer.clone());

    let result = s.lifecycle.try_batch_revoke_engineer_authorizations(
        &attacker,
        &s.asset_id,
        &engineers,
    );
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_UNAUTHORIZED_OWNER,
        )))
    );
}

/// After revocation the engineer must be removed from the list.
#[test]
fn test_1012_revoke_removes_from_list() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.lifecycle
        .revoke_engineer_authorization(&s.owner, &s.asset_id, &s.engineer);

    let list = s.lifecycle.get_authorized_engineers(&s.asset_id);
    assert_eq!(
        list.len(),
        0,
        "authorized engineers list must be empty after revocation"
    );
}

/// Query for an asset that never had any authorizations returns empty vec.
#[test]
fn test_1012_empty_list_for_fresh_asset() {
    let env = Env::default();
    let s = Setup::new(&env);

    // Register a second asset with no engineers authorized.
    let asset_id2 = s.asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Fresh asset — no engineers"),
        &String::from_str(&env, "SN-1012-FRESH"),
        &s.owner,
    );

    let list = s.lifecycle.get_authorized_engineers(&asset_id2);
    assert_eq!(
        list.len(),
        0,
        "fresh asset should have no authorized engineers"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Issue #1010 — get_health_snapshots_paginated
// ═══════════════════════════════════════════════════════════════════════════════

/// Paginate over zero snapshots — returns empty.
#[test]
fn test_1010_paginated_no_snapshots() {
    let env = Env::default();
    let s = Setup::new(&env);

    let page = s
        .lifecycle
        .get_health_snapshots_paginated(&s.asset_id, &0, &10);
    assert_eq!(page.len(), 0, "should return empty when no snapshots exist");
}

/// limit=0 always returns empty.
#[test]
fn test_1010_paginated_zero_limit() {
    let env = Env::default();
    let s = Setup::new(&env);

    // Take a snapshot so there is data.
    s.lifecycle.take_health_snapshot(&s.asset_id);

    let page = s
        .lifecycle
        .get_health_snapshots_paginated(&s.asset_id, &0, &0);
    assert_eq!(page.len(), 0, "limit=0 must return empty page");
}

/// offset beyond end returns empty.
#[test]
fn test_1010_paginated_offset_beyond_end() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.lifecycle.take_health_snapshot(&s.asset_id);
    s.lifecycle.take_health_snapshot(&s.asset_id);

    let page = s
        .lifecycle
        .get_health_snapshots_paginated(&s.asset_id, &100, &10);
    assert_eq!(page.len(), 0, "offset beyond end must return empty page");
}

/// First page returns expected count.
#[test]
fn test_1010_paginated_first_page() {
    let env = Env::default();
    let s = Setup::new(&env);

    // Take 5 snapshots.
    for _ in 0..5u32 {
        // Advance ledger so each snapshot has a distinct timestamp.
        env.ledger().with_mut(|li| li.timestamp += 60);
        s.lifecycle.take_health_snapshot(&s.asset_id);
    }

    let page = s
        .lifecycle
        .get_health_snapshots_paginated(&s.asset_id, &0, &3);
    assert_eq!(page.len(), 3, "first page must contain 3 entries");
}

/// Second page continues where first left off.
#[test]
fn test_1010_paginated_second_page() {
    let env = Env::default();
    let s = Setup::new(&env);

    for _ in 0..5u32 {
        env.ledger().with_mut(|li| li.timestamp += 60);
        s.lifecycle.take_health_snapshot(&s.asset_id);
    }

    let page1 = s
        .lifecycle
        .get_health_snapshots_paginated(&s.asset_id, &0, &3);
    let page2 = s
        .lifecycle
        .get_health_snapshots_paginated(&s.asset_id, &3, &3);

    assert_eq!(page1.len(), 3);
    assert_eq!(page2.len(), 2, "second page should contain remaining 2 entries");

    // Pages must not overlap — last element of page1 != first of page2.
    let last_p1 = page1.get(2).unwrap();
    let first_p2 = page2.get(0).unwrap();
    assert_ne!(
        last_p1.timestamp,
        first_p2.timestamp,
        "pages must not overlap"
    );
}

/// Limit larger than remaining entries returns only remaining.
#[test]
fn test_1010_paginated_limit_larger_than_remaining() {
    let env = Env::default();
    let s = Setup::new(&env);

    for _ in 0..3u32 {
        env.ledger().with_mut(|li| li.timestamp += 60);
        s.lifecycle.take_health_snapshot(&s.asset_id);
    }

    let page = s
        .lifecycle
        .get_health_snapshots_paginated(&s.asset_id, &1, &100);
    assert_eq!(
        page.len(),
        2,
        "when limit > remaining, return only the remaining entries"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Issue #1013 — decommission_asset (owner-callable)
// ═══════════════════════════════════════════════════════════════════════════════

/// Owner can decommission their own asset; collateral score becomes 0.
#[test]
fn test_1013_owner_can_decommission() {
    let env = Env::default();
    let s = Setup::new(&env);

    // Build up a positive score first.
    s.lifecycle.submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Pre-decommission service"),
        &s.engineer,
    );
    let score_before = s.lifecycle.get_collateral_score(&s.asset_id);
    assert!(score_before > 0, "score should be positive before decommission");

    // Decommission via owner.
    s.lifecycle.decommission_asset(&s.owner, &s.asset_id);

    // Score must now be 0.
    let score_after = s.lifecycle.get_collateral_score(&s.asset_id);
    assert_eq!(
        score_after, 0,
        "collateral score must be 0 after decommission"
    );
}

/// After owner decommission, submit_maintenance must be blocked.
#[test]
fn test_1013_decommission_blocks_maintenance() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.lifecycle.decommission_asset(&s.owner, &s.asset_id);

    let result = s.lifecycle.try_submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Post-decommission attempt — must fail"),
        &s.engineer,
    );

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_ASSET_DECOMMISSIONED,
        ))),
        "submit_maintenance must return AssetDecommissioned after owner decommission"
    );
}

/// After owner decommission, decay_score must return 0 (no-op).
#[test]
fn test_1013_decommission_blocks_decay() {
    let env = Env::default();
    let s = Setup::new(&env);

    // Earn a positive score.
    s.lifecycle.submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "Pre-decommission service"),
        &s.engineer,
    );

    s.lifecycle.decommission_asset(&s.owner, &s.asset_id);

    // Advance time well past a decay interval.
    env.ledger().with_mut(|li| {
        li.timestamp += 10_000_000; // >> 30 days
    });

    let score = s.lifecycle.decay_score(&s.asset_id);
    assert_eq!(score, 0, "decay_score must return 0 for decommissioned asset");
}

/// A non-owner must not be able to decommission an asset.
#[test]
fn test_1013_decommission_rejects_non_owner() {
    let env = Env::default();
    let s = Setup::new(&env);
    let attacker = Address::generate(&env);

    let result = s
        .lifecycle
        .try_decommission_asset(&attacker, &s.asset_id);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_UNAUTHORIZED_OWNER,
        ))),
        "non-owner decommission must return UnauthorizedOwner"
    );
}

/// Double-decommission must return ScoreFrozen.
#[test]
fn test_1013_double_decommission_rejected() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.lifecycle.decommission_asset(&s.owner, &s.asset_id);

    let result = s
        .lifecycle
        .try_decommission_asset(&s.owner, &s.asset_id);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_SCORE_FROZEN,
        ))),
        "second decommission call must return ScoreFrozen"
    );
}
