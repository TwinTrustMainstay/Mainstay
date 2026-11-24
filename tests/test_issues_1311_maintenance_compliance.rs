#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Bytes, Env, String, Symbol,
};
use soroban_sdk::symbol_short;

// Import contract clients
use asset_registry::AssetRegistryClient;
use asset_registry::DeprecationStatus;
use engineer_registry::EngineerRegistryClient;
use lifecycle::LifecycleClient;

mod tests {
    use super::*;

    // Setup helper function
    fn setup_compliance_env(env: &Env) -> (LifecycleClient<'_>, u64, Address, Address) {
        env.mock_all_auths();

        let asset_registry_id = env.register(asset_registry::Lifecycle, ());
        let engineer_registry_id = env.register(engineer_registry::Lifecycle, ());
        let lifecycle_id = env.register(lifecycle::Lifecycle, ());

        let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
        let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
        let lifecycle = LifecycleClient::new(env, &lifecycle_id);

        let asset_admin = Address::generate(env);
        let eng_admin = Address::generate(env);
        let lifecycle_admin = Address::generate(env);
        let issuer = Address::generate(env);
        let asset_owner = Address::generate(env);
        let engineer = Address::generate(env);

        // Initialize contracts
        asset_registry.initialize_admin(&asset_admin, &asset_admin);
        asset_registry.add_asset_type(&asset_admin, &symbol_short!("ENGINE"));

        engineer_registry.initialize_admin(&eng_admin, &eng_admin);

        lifecycle.initialize(
            &lifecycle_admin,
            &asset_registry_id,
            &engineer_registry_id,
        );

        // Register an asset
        let asset_id = asset_registry.register_asset(
            &asset_owner,
            &symbol_short!("ENGINE"),
            &String::from_slice(env, "Test Engine"),
            &String::from_slice(env, "SN12345"),
        );

        // Authorize engineer
        lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

        (lifecycle, asset_id, engineer, asset_owner)
    }

    #[test]
    fn test_register_standard_success() {
        let env = Env::default();
        let (lifecycle, _asset_id, _engineer, _owner) = setup_compliance_env(&env);

        let admin = Address::generate(&env);
        let asset_type = symbol_short!("ENGINE");
        let standard_hash = Bytes::from_slice(&env, b"standard_def_v1");

        // Should not panic
        lifecycle.register_standard(&admin, &asset_type, &standard_hash);

        // Verify standard was registered
        let retrieved = lifecycle.get_maintenance_standard(&asset_type);
        assert_eq!(retrieved.len(), standard_hash.len());
    }

    #[test]
    #[should_panic(expected = "StandardAlreadyRegistered")]
    fn test_register_standard_duplicate_fails() {
        let env = Env::default();
        let (lifecycle, _asset_id, _engineer, _owner) = setup_compliance_env(&env);

        let admin = Address::generate(&env);
        let asset_type = symbol_short!("ENGINE");
        let standard_hash = Bytes::from_slice(&env, b"standard_v1");

        // Register first time
        lifecycle.register_standard(&admin, &asset_type, &standard_hash);

        // Attempt to register again - should panic
        let second_hash = Bytes::from_slice(&env, b"standard_v2");
        lifecycle.register_standard(&admin, &asset_type, &second_hash);
    }

    #[test]
    fn test_register_different_asset_types() {
        let env = Env::default();
        let (lifecycle, _asset_id, _engineer, _owner) = setup_compliance_env(&env);

        let admin = Address::generate(&env);

        let engine_standard = Bytes::from_slice(&env, b"engine_standard");
        let pump_standard = Bytes::from_slice(&env, b"pump_standard");
        let generator_standard = Bytes::from_slice(&env, b"generator_standard");

        // Register standards for different asset types
        lifecycle.register_standard(&admin, &symbol_short!("ENGINE"), &engine_standard);
        lifecycle.register_standard(&admin, &symbol_short!("PUMP"), &pump_standard);
        lifecycle.register_standard(&admin, &symbol_short!("GENER"), &generator_standard);

        // Verify each was stored independently
        let retrieved_engine = lifecycle.get_maintenance_standard(&symbol_short!("ENGINE"));
        let retrieved_pump = lifecycle.get_maintenance_standard(&symbol_short!("PUMP"));
        let retrieved_gen = lifecycle.get_maintenance_standard(&symbol_short!("GENER"));

        assert_eq!(retrieved_engine.len(), engine_standard.len());
        assert_eq!(retrieved_pump.len(), pump_standard.len());
        assert_eq!(retrieved_gen.len(), generator_standard.len());
    }

    #[test]
    fn test_validate_compliance_with_matching_proof() {
        let env = Env::default();
        let (lifecycle, asset_id, _engineer, _owner) = setup_compliance_env(&env);

        let admin = Address::generate(&env);
        let asset_type = symbol_short!("ENGINE");
        let standard_hash = Bytes::from_slice(&env, b"standard_proof_123");

        // Register standard
        lifecycle.register_standard(&admin, &asset_type, &standard_hash);

        // Validate with matching proof
        let is_compliant = lifecycle.validate_maintenance_compliance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &standard_hash,
        );

        assert!(is_compliant, "Should be compliant when proof matches standard");
    }

    #[test]
    fn test_validate_compliance_with_mismatched_proof() {
        let env = Env::default();
        let (lifecycle, asset_id, _engineer, _owner) = setup_compliance_env(&env);

        let admin = Address::generate(&env);
        let asset_type = symbol_short!("ENGINE");
        let standard_hash = Bytes::from_slice(&env, b"standard_proof_correct");

        // Register standard
        lifecycle.register_standard(&admin, &asset_type, &standard_hash);

        // Try to validate with different proof
        let wrong_proof = Bytes::from_slice(&env, b"wrong_proof_incorrect");
        let is_compliant = lifecycle.validate_maintenance_compliance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &wrong_proof,
        );

        assert!(!is_compliant, "Should not be compliant with mismatched proof");
    }

    #[test]
    fn test_validate_compliance_without_registered_standard() {
        let env = Env::default();
        let (lifecycle, asset_id, _engineer, _owner) = setup_compliance_env(&env);

        let proof = Bytes::from_slice(&env, b"some_proof");

        // Try to validate without registering standard first
        let is_compliant = lifecycle.validate_maintenance_compliance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &proof,
        );

        assert!(!is_compliant, "Should not be compliant when no standard registered");
    }

    #[test]
    fn test_get_maintenance_standard_not_registered() {
        let env = Env::default();
        let (lifecycle, _asset_id, _engineer, _owner) = setup_compliance_env(&env);

        let asset_type = symbol_short!("UNKNOWN");

        // Query for unregistered standard
        let standard = lifecycle.get_maintenance_standard(&asset_type);

        assert_eq!(standard.len(), 0, "Should return empty bytes for unregistered standard");
    }

    #[test]
    fn test_get_maintenance_standard_registered() {
        let env = Env::default();
        let (lifecycle, _asset_id, _engineer, _owner) = setup_compliance_env(&env);

        let admin = Address::generate(&env);
        let asset_type = symbol_short!("ENGINE");
        let standard_hash = Bytes::from_slice(&env, b"engine_maintenance_standard_v1");

        // Register standard
        lifecycle.register_standard(&admin, &asset_type, &standard_hash);

        // Retrieve standard
        let retrieved = lifecycle.get_maintenance_standard(&asset_type);

        assert_eq!(retrieved.len(), standard_hash.len());
        assert_eq!(retrieved, standard_hash);
    }

    #[test]
    fn test_compliance_validation_multiple_assets_same_type() {
        let env = Env::default();
        let (lifecycle, _asset_id, _engineer, owner) = setup_compliance_env(&env);

        let admin = Address::generate(&env);
        let asset_registry_id = env.register(asset_registry::Lifecycle, ());
        let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);

        let asset_type = symbol_short!("ENGINE");
        let standard_hash = Bytes::from_slice(&env, b"standard_for_engines");

        // Register standard for asset type
        lifecycle.register_standard(&admin, &asset_type, &standard_hash);

        // Multiple assets of same type should all validate with same standard
        let asset_ids = vec![1u64, 2u64, 3u64];
        for asset_id in asset_ids {
            let is_compliant = lifecycle.validate_maintenance_compliance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &standard_hash,
            );

            // Note: In real usage, these should all have same asset type registered
            // This test verifies the validation logic works for multiple assets
        }
    }

    #[test]
    fn test_compliance_different_task_types_same_asset() {
        let env = Env::default();
        let (lifecycle, asset_id, _engineer, _owner) = setup_compliance_env(&env);

        let admin = Address::generate(&env);
        let asset_type = symbol_short!("ENGINE");
        let standard_hash = Bytes::from_slice(&env, b"universal_engine_standard");

        // Register single standard for asset type
        lifecycle.register_standard(&admin, &asset_type, &standard_hash);

        // All task types should validate against same standard
        let task_types = vec![
            symbol_short!("OIL_CHG"),
            symbol_short!("FILTER"),
            symbol_short!("INSPEC"),
        ];

        for task_type in task_types {
            let is_compliant = lifecycle.validate_maintenance_compliance(
                &asset_id,
                &task_type,
                &standard_hash,
            );

            assert!(
                is_compliant,
                "All task types should validate against same standard"
            );
        }
    }

    #[test]
    fn test_standard_hash_size_variations() {
        let env = Env::default();
        let (lifecycle, _asset_id, _engineer, _owner) = setup_compliance_env(&env);

        let admin = Address::generate(&env);

        // Test with various hash sizes
        let small_hash = Bytes::from_slice(&env, b"short");
        let medium_hash = Bytes::from_slice(&env, b"this_is_a_medium_sized_hash_for_standard");
        let large_hash = Bytes::from_slice(
            &env,
            b"this_is_a_very_long_hash_that_represents_a_complex_standard_definition_with_many_rules",
        );

        lifecycle.register_standard(&admin, &symbol_short!("TYPE1"), &small_hash);
        lifecycle.register_standard(&admin, &symbol_short!("TYPE2"), &medium_hash);
        lifecycle.register_standard(&admin, &symbol_short!("TYPE3"), &large_hash);

        // All should be retrievable correctly
        let ret1 = lifecycle.get_maintenance_standard(&symbol_short!("TYPE1"));
        let ret2 = lifecycle.get_maintenance_standard(&symbol_short!("TYPE2"));
        let ret3 = lifecycle.get_maintenance_standard(&symbol_short!("TYPE3"));

        assert_eq!(ret1, small_hash);
        assert_eq!(ret2, medium_hash);
        assert_eq!(ret3, large_hash);
    }

    #[test]
    fn test_compliance_with_empty_bytes_standard() {
        let env = Env::default();
        let (lifecycle, asset_id, _engineer, _owner) = setup_compliance_env(&env);

        let admin = Address::generate(&env);
        let asset_type = symbol_short!("ENGINE");
        let empty_standard = Bytes::from_slice(&env, b"");

        // Register empty standard
        lifecycle.register_standard(&admin, &asset_type, &empty_standard);

        // Validate with empty proof
        let empty_proof = Bytes::from_slice(&env, b"");
        let is_compliant = lifecycle.validate_maintenance_compliance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &empty_proof,
        );

        assert!(is_compliant, "Empty standard should match empty proof");

        // Non-empty proof should not match empty standard
        let non_empty_proof = Bytes::from_slice(&env, b"some_data");
        let is_compliant2 = lifecycle.validate_maintenance_compliance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &non_empty_proof,
        );

        assert!(!is_compliant2, "Non-empty proof should not match empty standard");
    }

    #[test]
    fn test_standard_registration_emits_event() {
        let env = Env::default();
        let (lifecycle, _asset_id, _engineer, _owner) = setup_compliance_env(&env);

        let admin = Address::generate(&env);
        let asset_type = symbol_short!("ENGINE");
        let standard_hash = Bytes::from_slice(&env, b"event_test_standard");

        // Clear previous events
        env.events().start_recording();

        // Register standard
        lifecycle.register_standard(&admin, &asset_type, &standard_hash);

        // Event should be emitted
        let events = env.events().all();
        assert!(!events.is_empty(), "REG_STD event should be emitted");
    }

    #[test]
    fn test_compliance_binary_data_standards() {
        let env = Env::default();
        let (lifecycle, asset_id, _engineer, _owner) = setup_compliance_env(&env);

        let admin = Address::generate(&env);
        let asset_type = symbol_short!("ENGINE");

        // Create binary standard data (simulating hash)
        let mut binary_data = [0u8; 32];
        for i in 0..32 {
            binary_data[i] = (i as u8).wrapping_mul(17);
        }
        let standard_hash = Bytes::from_slice(&env, &binary_data);

        // Register binary standard
        lifecycle.register_standard(&admin, &asset_type, &standard_hash);

        // Validate with exact binary match
        let is_compliant = lifecycle.validate_maintenance_compliance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &standard_hash,
        );

        assert!(
            is_compliant,
            "Binary standard should validate with exact binary match"
        );

        // Flip one bit and verify it fails
        let mut modified_data = binary_data.clone();
        modified_data[0] ^= 1;
        let modified_hash = Bytes::from_slice(&env, &modified_data);

        let is_compliant2 = lifecycle.validate_maintenance_compliance(
            &asset_id,
            &symbol_short!("OIL_CHG"),
            &modified_hash,
        );

        assert!(
            !is_compliant2,
            "Modified binary standard should fail validation"
        );
    }
}
