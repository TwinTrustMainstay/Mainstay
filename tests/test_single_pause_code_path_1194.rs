// tests/test_single_pause_code_path_1194.rs
//
// Issue #1194 — Fix: pause and unpause were defined twice in admin.rs with
// identical bodies.  The duplicates have been removed.  This test confirms
// that exactly one code path controls the paused state and that the contract
// behaves consistently whether pause/unpause is called via the immediate entry
// points or via the propose → execute timelock path.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

/// lifecycle: ContractError::Paused = 9
const LIFECYCLE_PAUSED: u32 = 9;

/// Minimum timelock delay in seconds required by the lifecycle contract.
const TIMELOCK_DELAY_SECS: u64 = 48 * 60 * 60;

// ── Setup helper ─────────────────────────────────────────────────────────────

fn deploy_lifecycle(env: &Env) -> (LifecycleClient<'_>, Address) {
    env.mock_all_auths();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_admin = Address::generate(env);
    let eng_admin = Address::generate(env);
    let lc_admin = Address::generate(env);
    let issuer = Address::generate(env);

    AssetRegistryClient::new(env, &asset_registry_id)
        .initialize_admin(&asset_admin, &asset_admin);
    AssetRegistryClient::new(env, &asset_registry_id)
        .add_asset_type(&asset_admin, &symbol_short!("GENSET"));

    EngineerRegistryClient::new(env, &engineer_registry_id)
        .initialize_admin(&eng_admin, &eng_admin);
    EngineerRegistryClient::new(env, &engineer_registry_id)
        .add_trusted_issuer(&eng_admin, &issuer);

    let client = LifecycleClient::new(env, &lifecycle_id);
    client.initialize(
        &lc_admin,
        &asset_registry_id,
        &engineer_registry_id,
        &lc_admin,
        &0,
    );

    // Register an asset and engineer so write operations are available for
    // verifying the paused-rejection behaviour below.
    let owner = Address::generate(env);
    let asset_id = AssetRegistryClient::new(env, &asset_registry_id).register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(env, "Test Generator"),
        &String::from_str(env, "SN-1194-001"),
        &owner,
    );
    let engineer = Address::generate(env);
    let credential_hash = BytesN::from_array(env, &[42u8; 32]);
    EngineerRegistryClient::new(env, &engineer_registry_id).register_engineer(
        &engineer,
        &credential_hash,
        &issuer,
        &31_536_000,
        &None,
    );
    client.authorize_engineer(&owner, &asset_id, &engineer);

    (client, lc_admin)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Issue #1194 — Single code path: direct pause / unpause
// ═══════════════════════════════════════════════════════════════════════════════

/// After `pause`, `is_paused` must return `true`.
/// After `unpause`, `is_paused` must return `false`.
/// There is only one storage key that controls the flag; both entry points must
/// read and write the same key (i.e. the canonical single implementation).
#[test]
fn test_pause_sets_paused_flag_via_single_code_path() {
    let env = Env::default();
    let (client, admin) = deploy_lifecycle(&env);

    assert!(!client.is_paused(), "contract must start unpaused");

    client.pause(&admin);
    assert!(client.is_paused(), "is_paused must return true after pause()");

    client.unpause(&admin);
    assert!(!client.is_paused(), "is_paused must return false after unpause()");
}

/// Calling `pause` twice in a row must leave the contract paused (idempotent
/// set — no second code path that could accidentally clear the flag).
#[test]
fn test_pause_twice_leaves_contract_paused() {
    let env = Env::default();
    let (client, admin) = deploy_lifecycle(&env);

    client.pause(&admin);
    client.pause(&admin);

    assert!(
        client.is_paused(),
        "contract must still be paused after pausing twice"
    );
}

/// Calling `unpause` while already unpaused must be a no-op (no panic, flag
/// stays false). Proves there is no second code path with different behaviour.
#[test]
fn test_unpause_while_unpaused_is_noop() {
    let env = Env::default();
    let (client, admin) = deploy_lifecycle(&env);

    // Should not panic
    client.unpause(&admin);
    assert!(!client.is_paused(), "contract must remain unpaused");
}

/// `pause` followed immediately by `unpause` must result in the contract being
/// unpaused — verifies that one write does not silently override the other via
/// a second definition that reads a different storage key.
#[test]
fn test_pause_then_unpause_state_is_unpaused() {
    let env = Env::default();
    let (client, admin) = deploy_lifecycle(&env);

    client.pause(&admin);
    assert!(client.is_paused());

    client.unpause(&admin);
    assert!(!client.is_paused());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Issue #1194 — Single code path: timelock propose → execute path
// ═══════════════════════════════════════════════════════════════════════════════

/// `propose_pause` + `execute_pause` must set the same flag as the direct
/// `pause` entry point. There is only one PAUSED_KEY in storage.
#[test]
fn test_execute_pause_sets_same_flag_as_direct_pause() {
    let env = Env::default();
    let (client, admin) = deploy_lifecycle(&env);

    // Propose and wait for timelock
    client.propose_pause(&admin);
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + TIMELOCK_DELAY_SECS + 1);

    client.execute_pause(&admin);

    assert!(
        client.is_paused(),
        "execute_pause must set the same paused flag as direct pause()"
    );
}

/// `propose_unpause` + `execute_unpause` must clear the same flag as the direct
/// `unpause` entry point.
#[test]
fn test_execute_unpause_clears_same_flag_as_direct_unpause() {
    let env = Env::default();
    let (client, admin) = deploy_lifecycle(&env);

    // First pause via the direct path
    client.pause(&admin);
    assert!(client.is_paused());

    // Then unpause via the timelock path
    client.propose_unpause(&admin);
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + TIMELOCK_DELAY_SECS + 1);
    client.execute_unpause(&admin);

    assert!(
        !client.is_paused(),
        "execute_unpause must clear the same paused flag as direct unpause()"
    );
}

/// Cross-path check: pause via timelock, unpause via direct call.
/// Both must operate on the same storage key.
#[test]
fn test_execute_pause_then_direct_unpause_cross_path() {
    let env = Env::default();
    let (client, admin) = deploy_lifecycle(&env);

    // Pause via timelock
    client.propose_pause(&admin);
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + TIMELOCK_DELAY_SECS + 1);
    client.execute_pause(&admin);
    assert!(client.is_paused());

    // Unpause via the direct path
    client.unpause(&admin);
    assert!(
        !client.is_paused(),
        "direct unpause must clear the flag set by execute_pause"
    );
}

/// Cross-path check: pause via direct call, unpause via timelock.
#[test]
fn test_direct_pause_then_execute_unpause_cross_path() {
    let env = Env::default();
    let (client, admin) = deploy_lifecycle(&env);

    // Pause via the direct path
    client.pause(&admin);
    assert!(client.is_paused());

    // Unpause via the timelock path
    client.propose_unpause(&admin);
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + TIMELOCK_DELAY_SECS + 1);
    client.execute_unpause(&admin);

    assert!(
        !client.is_paused(),
        "execute_unpause must clear the flag set by direct pause()"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Issue #1194 — Write operations are blocked when paused via either path
// ═══════════════════════════════════════════════════════════════════════════════

/// A write call made after `execute_pause` must be rejected with the Paused
/// error — same as after a direct `pause`. Confirms the single canonical
/// check (`ensure_not_paused`) guards all write paths consistently.
#[test]
fn test_execute_pause_blocks_writes() {
    let env = Env::default();
    let (client, admin) = deploy_lifecycle(&env);

    client.propose_pause(&admin);
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + TIMELOCK_DELAY_SECS + 1);
    client.execute_pause(&admin);

    let owner = Address::generate(&env);
    let engineer = Address::generate(&env);
    let result = client.try_authorize_engineer(&owner, &1u64, &engineer);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(LIFECYCLE_PAUSED))),
        "write operations must be blocked after execute_pause"
    );
}
