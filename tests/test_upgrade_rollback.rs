//! Rollback tests for contract upgrades (issue #1725).
//!
//! Soroban has no on-chain downgrade primitive. Rolling back means proposing
//! the previously deployed WASM hash through the same timelocked path that a
//! forward upgrade uses. These tests verify that rollback behaves safely:
//!
//! 1. **Timelock restart**: Re-proposing a hash overwrites the pending
//!    proposal and restarts the 48-hour timelock, so a superseded upgrade
//!    cannot execute early and its age does not carry over to the rollback.
//! 2. **Rollback executes after its own timelock**: A rollback proposal is
//!    executable once its own delay has elapsed.
//! 3. **Data survives a rollback**: Storage written before the bad upgrade
//!    attempt is intact after the rollback executes.
//! 4. **Admin-only rollback**: Only the admin may propose a rollback.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

const TIMELOCK_SECS: u64 = 48 * 60 * 60;
const ONE_HOUR_SECS: u64 = 60 * 60;

fn advance_time(env: &Env, secs: u64) {
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + secs);
}

/// Registers an asset-registry, initializes its admin and returns the client
/// together with the admin address.
fn setup(env: &Env) -> (AssetRegistryClient<'_>, Address) {
    let contract_id = env.register(AssetRegistry, ());
    let client = AssetRegistryClient::new(env, &contract_id);
    let admin = Address::generate(env);
    client.initialize_admin(&admin, &admin);
    (client, admin)
}

/// Re-proposing the previous (known-good) hash must overwrite the bad pending
/// proposal and restart the timelock. Without the restart, the bad proposal
/// would be executable one hour from now.
#[test]
fn test_rollback_proposal_restarts_timelock() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup(&env);

    let bad_hash = BytesN::from_array(&env, &[0xAAu8; 32]);
    let previous_hash = BytesN::from_array(&env, &[0x11u8; 32]);

    client.propose_upgrade(&admin, &bad_hash);

    // Almost at the end of the bad proposal's timelock, roll back.
    advance_time(&env, TIMELOCK_SECS - ONE_HOUR_SECS);
    client.propose_upgrade(&admin, &previous_hash);

    // The bad proposal's original window would have elapsed by now, but the
    // rollback's own timelock has only just started.
    advance_time(&env, ONE_HOUR_SECS + 1);
    let early = client.try_execute_upgrade(&admin);
    assert!(
        early.is_err(),
        "rollback proposal must wait its own timelock, not inherit the superseded one"
    );
}

/// After the rollback's own timelock elapses, `execute_upgrade` must succeed.
#[test]
fn test_rollback_executes_after_its_own_timelock() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup(&env);

    let bad_hash = BytesN::from_array(&env, &[0xAAu8; 32]);
    let previous_hash = BytesN::from_array(&env, &[0x11u8; 32]);

    client.propose_upgrade(&admin, &bad_hash);
    advance_time(&env, TIMELOCK_SECS - ONE_HOUR_SECS);
    client.propose_upgrade(&admin, &previous_hash);

    advance_time(&env, TIMELOCK_SECS + 1);
    let result = client.try_execute_upgrade(&admin);
    assert!(result.is_ok(), "rollback must execute after its own timelock");
    assert_eq!(client.get_admin(), admin, "admin must persist across rollback");
}

/// Assets registered before a bad upgrade attempt must remain readable and
/// unchanged after the rollback has been proposed and executed.
#[test]
fn test_data_intact_after_rollback() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup(&env);
    let owner = Address::generate(&env);

    client.add_asset_type(&admin, &symbol_short!("GENSET"));
    let asset_id = client.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Rollback survival asset"),
        &String::from_str(&env, "SN-RB-001"),
        &owner,
    );
    let count_before = client.get_asset_count();

    let bad_hash = BytesN::from_array(&env, &[0xAAu8; 32]);
    let previous_hash = BytesN::from_array(&env, &[0x11u8; 32]);
    client.propose_upgrade(&admin, &bad_hash);
    client.propose_upgrade(&admin, &previous_hash);

    advance_time(&env, TIMELOCK_SECS + 1);
    client.execute_upgrade(&admin);

    assert!(client.asset_exists(&asset_id), "asset must survive rollback");
    assert_eq!(
        client.get_asset(&asset_id).owner,
        owner,
        "asset owner must be unchanged by rollback"
    );
    assert_eq!(
        client.get_asset_count(),
        count_before,
        "asset counter must not be reset by rollback"
    );
}

/// A non-admin must not be able to schedule a rollback.
#[test]
fn test_non_admin_cannot_propose_rollback() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin) = setup(&env);

    let attacker = Address::generate(&env);
    let previous_hash = BytesN::from_array(&env, &[0x11u8; 32]);

    let result = client.try_propose_upgrade(&attacker, &previous_hash);
    assert!(result.is_err(), "non-admin rollback proposal must be rejected");
}
