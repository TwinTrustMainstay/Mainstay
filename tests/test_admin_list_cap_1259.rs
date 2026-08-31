//! Tests for #1259: `set_admin_quorum` must cap the admins list to MAX_ADMINS (10).
//!
//! Verifies that:
//! - Passing a list with more than 10 addresses is rejected with `TooManyAdmins`.
//! - Passing exactly 10 addresses is accepted.
//! - Passing fewer than 10 addresses continues to work.
//! - The existing duplicate-address check still applies after the cap check.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{ContractError, Lifecycle, LifecycleClient};
use soroban_sdk::{testutils::Address as _, Address, Env, Vec};

/// Error discriminant for `TooManyAdmins` — must match errors.rs.
const LIFECYCLE_TOO_MANY_ADMINS: u32 = 39;

// ── helpers ──────────────────────────────────────────────────────────────────

fn setup_lifecycle(env: &Env) -> (LifecycleClient, Address) {
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let lifecycle = LifecycleClient::new(env, &lifecycle_id);
    let admin = Address::generate(env);

    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    asset_registry.initialize_admin(&admin, &admin);

    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
    engineer_registry.initialize_admin(&admin, &admin);

    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &0,
    );

    (lifecycle, admin)
}

/// Build a Vec of `n` unique addresses.
fn unique_addresses(env: &Env, n: u32) -> Vec<Address> {
    let mut addrs: Vec<Address> = Vec::new(env);
    for _ in 0..n {
        addrs.push_back(Address::generate(env));
    }
    addrs
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Passing 11 unique admins must be rejected with `TooManyAdmins` (code 39).
#[test]
fn test_set_admin_quorum_rejects_list_over_max() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let admins = unique_addresses(&env, 11);

    let result = lifecycle.try_set_admin_quorum(&admin, &admins, &5);
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_TOO_MANY_ADMINS,
        ))),
        "set_admin_quorum must reject a list with 11 addresses (#1259)"
    );
}

/// Passing exactly 10 unique admins must succeed.
#[test]
fn test_set_admin_quorum_accepts_exactly_max() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let admins = unique_addresses(&env, 10);

    // threshold = 5 (majority), all addresses unique → must succeed
    lifecycle.set_admin_quorum(&admin, &admins, &5);

    let config = lifecycle.get_config();
    assert_eq!(
        config.admins.len(),
        10,
        "set_admin_quorum must store exactly 10 admins when the list is at the cap"
    );
    assert_eq!(config.admin_threshold, 5);
}

/// Passing 5 admins (well below the cap) must continue to work.
#[test]
fn test_set_admin_quorum_accepts_list_below_max() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let admins = unique_addresses(&env, 5);

    lifecycle.set_admin_quorum(&admin, &admins, &3);

    let config = lifecycle.get_config();
    assert_eq!(config.admins.len(), 5);
    assert_eq!(config.admin_threshold, 3);
}

/// Passing 100 unique admins must be rejected with `TooManyAdmins`.
///
/// This is the DoS scenario from the issue: an unbounded list could push
/// `require_quorum`'s O(n) scan toward Soroban instruction limits.
#[test]
fn test_set_admin_quorum_rejects_large_list() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let admins = unique_addresses(&env, 100);

    let result = lifecycle.try_set_admin_quorum(&admin, &admins, &1);
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_TOO_MANY_ADMINS,
        ))),
        "set_admin_quorum must reject a list of 100 addresses (#1259)"
    );
}

/// The cap is checked before the duplicate check, so a list of 11+ entries
/// returns `TooManyAdmins` even when duplicates are present.
#[test]
fn test_set_admin_quorum_cap_checked_before_duplicate() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    // Build 11-element list where first element is duplicated.
    let dup = Address::generate(&env);
    let mut admins: Vec<Address> = Vec::new(&env);
    admins.push_back(dup.clone());
    for _ in 0..9 {
        admins.push_back(Address::generate(&env));
    }
    admins.push_back(dup.clone()); // duplicate — but list is 11 entries (over cap)

    let result = lifecycle.try_set_admin_quorum(&admin, &admins, &2);
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_TOO_MANY_ADMINS,
        ))),
        "cap check must fire before duplicate check for over-length lists (#1259)"
    );
}
