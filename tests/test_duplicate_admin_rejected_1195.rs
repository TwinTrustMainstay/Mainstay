/// Test #1195: Verify that `set_admin_quorum` rejects a new-admins list that
/// contains a repeated address.
///
/// A duplicate entry inflates the apparent quorum count.  For example, if the
/// same address appears twice in a 2-element list with threshold 2, that single
/// real signer satisfies `require_quorum` by signing twice — defeating the
/// multi-party requirement entirely.  The fix panics with
/// `ContractError::DuplicateAdmin` (code 26) on any repeated entry.

use lifecycle::{ContractError, Lifecycle, LifecycleClient};
use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use soroban_sdk::{testutils::Address as _, Address, Env, Vec};

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

// ── tests ─────────────────────────────────────────────────────────────────────

/// Submitting a list where the same address appears twice must be rejected with
/// `ContractError::DuplicateAdmin` (error code 26).
#[test]
fn test_set_admin_quorum_rejects_duplicate_address() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let dup = Address::generate(&env);
    let other = Address::generate(&env);

    // List contains `dup` twice — must be rejected.
    let mut admins: Vec<Address> = Vec::new(&env);
    admins.push_back(dup.clone());
    admins.push_back(other.clone());
    admins.push_back(dup.clone()); // duplicate entry

    let result = lifecycle.try_set_admin_quorum(&admin, &admins, &2);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            ContractError::DuplicateAdmin as u32
        ))),
        "set_admin_quorum must reject a list with a duplicated address (#1195)"
    );
}

/// A list where the first two elements are the same address must also be
/// rejected, verifying the check covers the very beginning of the list.
#[test]
fn test_set_admin_quorum_rejects_duplicate_at_start() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let dup = Address::generate(&env);
    let unique = Address::generate(&env);

    let mut admins: Vec<Address> = Vec::new(&env);
    admins.push_back(dup.clone());
    admins.push_back(dup.clone()); // duplicate right at position 0 & 1
    admins.push_back(unique.clone());

    let result = lifecycle.try_set_admin_quorum(&admin, &admins, &2);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            ContractError::DuplicateAdmin as u32
        ))),
        "set_admin_quorum must reject when the duplicate appears at the start (#1195)"
    );
}

/// A list with a single address used everywhere (threshold 1) must also be
/// rejected — a solo entry repeated N times is an extreme form of the attack.
#[test]
fn test_set_admin_quorum_rejects_all_same_address() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let same = Address::generate(&env);

    let mut admins: Vec<Address> = Vec::new(&env);
    admins.push_back(same.clone());
    admins.push_back(same.clone());
    admins.push_back(same.clone());

    let result = lifecycle.try_set_admin_quorum(&admin, &admins, &1);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            ContractError::DuplicateAdmin as u32
        ))),
        "set_admin_quorum must reject a list where every entry is the same address (#1195)"
    );
}

/// A list with all unique addresses must still be accepted, confirming the fix
/// does not break the happy path.
#[test]
fn test_set_admin_quorum_accepts_unique_addresses() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let a1 = Address::generate(&env);
    let a2 = Address::generate(&env);
    let a3 = Address::generate(&env);

    let mut admins: Vec<Address> = Vec::new(&env);
    admins.push_back(a1.clone());
    admins.push_back(a2.clone());
    admins.push_back(a3.clone());

    // All unique — must succeed.
    lifecycle.set_admin_quorum(&admin, &admins, &2);

    let config = lifecycle.get_config();
    assert_eq!(
        config.admins.len(),
        3,
        "config must contain all 3 unique admins after set_admin_quorum (#1195)"
    );
    assert_eq!(
        config.admin_threshold, 2,
        "threshold must be 2 after set_admin_quorum (#1195)"
    );
}

/// The duplicate can appear as the *last* pair — ensure the inner loop fully
/// traverses the list rather than stopping early.
#[test]
fn test_set_admin_quorum_rejects_duplicate_at_end() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, admin) = setup_lifecycle(&env);

    let a1 = Address::generate(&env);
    let a2 = Address::generate(&env);
    let dup = Address::generate(&env);

    let mut admins: Vec<Address> = Vec::new(&env);
    admins.push_back(a1.clone());
    admins.push_back(a2.clone());
    admins.push_back(dup.clone());
    admins.push_back(dup.clone()); // duplicate at the tail

    let result = lifecycle.try_set_admin_quorum(&admin, &admins, &2);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            ContractError::DuplicateAdmin as u32
        ))),
        "set_admin_quorum must reject a duplicate in the tail of the list (#1195)"
    );
}
