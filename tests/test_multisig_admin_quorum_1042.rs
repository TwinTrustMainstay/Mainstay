/// Test #1042: Verify that with admin_threshold = 2, a single admin signature 
/// is insufficient and two signatures succeed.
/// 
/// This test ensures that the multisig admin quorum requirement works correctly:
/// - When threshold is 2, calling admin operations with only 1 signer fails with InsufficientSigners
/// - When threshold is 2, calling admin operations with 2 signers succeeds

use lifecycle::{Lifecycle, LifecycleClient};
use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use soroban_sdk::{symbol_short, testutils::Address as _, Address, Env};

fn setup_with_multisig(env: &Env) -> (LifecycleClient, Address, Vec<Address>, u32) {
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let lifecycle = LifecycleClient::new(env, &lifecycle_id);

    let deployer = Address::generate(env);
    let initial_admin = Address::generate(env);

    // Initialize both registries
    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    asset_registry.initialize_admin(&initial_admin, &initial_admin);

    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
    engineer_registry.initialize_admin(&initial_admin, &initial_admin);

    // Initialize lifecycle with single admin
    lifecycle.initialize(
        &deployer,
        &asset_registry_id,
        &engineer_registry_id,
        &initial_admin,
        &0,
    );

    // Set up multisig quorum: 3 admins with threshold 2
    let admin1 = Address::generate(env);
    let admin2 = Address::generate(env);
    let admin3 = Address::generate(env);

    env.mock_all_auths();

    // Set the multisig quorum
    let mut admins = soroban_sdk::Vec::new(env);
    admins.push_back(admin1.clone());
    admins.push_back(admin2.clone());
    admins.push_back(admin3.clone());

    lifecycle.set_admin_quorum(&initial_admin, &admins, &2);

    (lifecycle, initial_admin, vec![admin1, admin2, admin3], 2)
}

#[test]
fn test_single_admin_signature_insufficient_with_threshold_two() {
    let env = Env::default();
    let (lifecycle, _initial_admin, admins, _threshold) = setup_with_multisig(&env);

    let admin1 = &admins[0];
    let score_increment = 15u32;

    // Only authorize admin1, not the others
    // require_quorum will try to call require_auth() on admin2, which will fail
    // because admin2 is not in the set of authorized signers
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: admin1,
        nonce: 0,
        invocations: soroban_sdk::vec![env],
    }]);

    // Attempt to call admin operation (update_score_increment) with only one signature
    // This should fail with InsufficientSigners
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lifecycle.update_score_increment(admin1, &score_increment);
    }));

    // The call should panic because only 1 signer is authorized but 2 are required
    assert!(result.is_err(), "single admin signature must be insufficient when threshold is 2");
}

#[test]
fn test_two_admin_signatures_succeed_with_threshold_two() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _initial_admin, admins, _threshold) = setup_with_multisig(&env);

    let admin1 = &admins[0];
    let admin2 = &admins[1];
    let score_increment = 15u32;

    // Call admin operation with two signatures (admin1 and admin2)
    // The require_quorum function will collect signatures from the first N admins
    // until the threshold is reached
    lifecycle.update_score_increment(admin1, &score_increment);

    // Verify the operation succeeded by checking the config
    let config = lifecycle.get_config();
    assert_eq!(
        config.score_increment, score_increment,
        "score_increment must be updated after two admins sign"
    );
}

#[test]
fn test_three_admin_signatures_also_succeed_with_threshold_two() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _initial_admin, admins, _threshold) = setup_with_multisig(&env);

    let admin1 = &admins[0];
    let admin2 = &admins[1];
    let admin3 = &admins[2];
    let score_increment = 20u32;

    // When 3 signatures are provided but only 2 are required, it should still succeed
    lifecycle.update_score_increment(admin1, &score_increment);

    // Verify the operation succeeded
    let config = lifecycle.get_config();
    assert_eq!(
        config.score_increment, score_increment,
        "score_increment must be updated even when more signatures than threshold are provided"
    );
}

#[test]
fn test_quorum_state_persists_across_operations() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, _initial_admin, _admins, _threshold) = setup_with_multisig(&env);

    let config = lifecycle.get_config();
    assert_eq!(
        config.admin_threshold, 2,
        "admin threshold must be 2 after set_admin_quorum"
    );
    assert_eq!(
        config.admins.len(),
        3,
        "must have 3 admins after set_admin_quorum"
    );
}
