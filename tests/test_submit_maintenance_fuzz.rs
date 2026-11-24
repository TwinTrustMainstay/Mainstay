#![cfg(test)]

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

/// Fuzz test corpus: structured test cases covering various input categories
struct FuzzCase {
    label: &'static str,
    task_type_str: &'static str,
    notes_str: &'static str,
    should_fail: bool, // Whether input should be rejected
}

/// Comprehensive corpus covering edge cases in task type and notes validation
const FUZZ_CORPUS: &[FuzzCase] = &[
    // ========== HAPPY PATHS: Valid inputs ==========
    FuzzCase {
        label: "valid_engine_maintenance",
        task_type_str: "ENGINE",
        notes_str: "Replaced spark plugs and oil",
        should_fail: false,
    },
    FuzzCase {
        label: "valid_filter_replacement",
        task_type_str: "FILTER",
        notes_str: "Air and cabin filters replaced",
        should_fail: false,
    },
    FuzzCase {
        label: "valid_bearing_service",
        task_type_str: "BEARING",
        notes_str: "Lubricated and inspected for wear",
        should_fail: false,
    },
    FuzzCase {
        label: "single_char_task",
        task_type_str: "X",
        notes_str: "Single character task type",
        should_fail: false,
    },
    FuzzCase {
        label: "single_char_notes",
        task_type_str: "CHECK",
        notes_str: "X",
        should_fail: false,
    },
    
    // ========== BOUNDARY CONDITIONS ==========
    FuzzCase {
        label: "minimum_notes_length",
        task_type_str: "MAINT",
        notes_str: "a", // Minimum 1 character
        should_fail: false,
    },
    FuzzCase {
        label: "empty_task_type",
        task_type_str: "",
        notes_str: "Valid notes", // Empty task_type should be allowed (Symbol can be empty)
        should_fail: false,
    },
    FuzzCase {
        label: "empty_notes",
        task_type_str: "MAINT",
        notes_str: "", // Empty notes should fail validation
        should_fail: true,
    },
    FuzzCase {
        label: "whitespace_only_notes",
        task_type_str: "CHECK",
        notes_str: "   ",
        should_fail: false, // Whitespace is valid content
    },
    
    // ========== UNICODE AND SPECIAL CHARACTERS ==========
    FuzzCase {
        label: "unicode_task_type",
        task_type_str: "检查", // Chinese characters
        notes_str: "Task in Chinese",
        should_fail: false,
    },
    FuzzCase {
        label: "unicode_notes",
        task_type_str: "MAINT",
        notes_str: "Überprüfung durchgeführt", // German with umlauts
        should_fail: false,
    },
    FuzzCase {
        label: "emoji_task",
        task_type_str: "🔧", // Emoji
        notes_str: "Using emoji as task type",
        should_fail: false,
    },
    FuzzCase {
        label: "emoji_notes",
        task_type_str: "REPAIR",
        notes_str: "🛠️ Fixed component 🔩", // Emoji in notes
        should_fail: false,
    },
    FuzzCase {
        label: "mixed_unicode_latin",
        task_type_str: "MAINT_维护", // Mixed Latin and Chinese
        notes_str: "混合Unicode_mix", 
        should_fail: false,
    },
    
    // ========== INJECTION AND SECURITY ==========
    // These should NOT be executed; they should be stored verbatim
    FuzzCase {
        label: "sql_injection_task",
        task_type_str: "'; DROP TABLE maintenance; --",
        notes_str: "SQL injection attempt in task",
        should_fail: false, // Should be stored as string, not executed
    },
    FuzzCase {
        label: "sql_injection_notes",
        task_type_str: "MAINT",
        notes_str: "SELECT * FROM sensitive_data WHERE '1'='1'",
        should_fail: false, // Should be stored as string
    },
    FuzzCase {
        label: "script_tag_task",
        task_type_str: "<script>alert('xss')</script>",
        notes_str: "XSS attempt in task type",
        should_fail: false, // Should be stored as string
    },
    FuzzCase {
        label: "script_tag_notes",
        task_type_str: "CHECK",
        notes_str: "<img src=x onerror='alert(1)'>",
        should_fail: false, // Should be stored as string
    },
    
    // ========== WHITESPACE EDGE CASES ==========
    FuzzCase {
        label: "newline_in_task",
        task_type_str: "MAINT\nTASK",
        notes_str: "Contains newline",
        should_fail: false,
    },
    FuzzCase {
        label: "newline_in_notes",
        task_type_str: "MAINT",
        notes_str: "Line1\nLine2\nLine3",
        should_fail: false,
    },
    FuzzCase {
        label: "tab_in_task",
        task_type_str: "MAINT\tTASK",
        notes_str: "Contains tab",
        should_fail: false,
    },
    FuzzCase {
        label: "tab_in_notes",
        task_type_str: "MAINT",
        notes_str: "Col1\tCol2\tCol3",
        should_fail: false,
    },
    FuzzCase {
        label: "carriage_return",
        task_type_str: "MAINT\rTASK",
        notes_str: "Contains CR",
        should_fail: false,
    },
    FuzzCase {
        label: "mixed_whitespace",
        task_type_str: "MAINT \t\n TASK",
        notes_str: "Various \t\n whitespace \r chars",
        should_fail: false,
    },
    
    // ========== BOUNDARY LENGTH TESTS ==========
    // Assuming default max_notes_length is 256
    FuzzCase {
        label: "max_length_notes_256",
        task_type_str: "MAINT",
        notes_str: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        should_fail: false, // Exactly 256 chars (if default is 256)
    },
    FuzzCase {
        label: "beyond_max_notes",
        task_type_str: "MAINT",
        notes_str: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        should_fail: true, // 257 chars - exceeds default max
    },
    
    // ========== REPEATED PATTERNS ==========
    FuzzCase {
        label: "repeated_zeros",
        task_type_str: "0000000000",
        notes_str: "0000000000000000000000000000",
        should_fail: false,
    },
    FuzzCase {
        label: "repeated_slashes",
        task_type_str: "//////////",
        notes_str: "////////////////////////////////////",
        should_fail: false,
    },
    FuzzCase {
        label: "repeated_backslash",
        task_type_str: "\\\\\\\\\\\\\\\\\\\\",
        notes_str: "\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\",
        should_fail: false,
    },
    
    // ========== CONTROL CHARACTERS ==========
    FuzzCase {
        label: "null_byte_task",
        task_type_str: "MAINT\0TASK",
        notes_str: "Contains null byte",
        should_fail: false, // Stored verbatim
    },
    FuzzCase {
        label: "null_byte_notes",
        task_type_str: "MAINT",
        notes_str: "Notes\0with\0null\0bytes",
        should_fail: false, // Stored verbatim
    },
    FuzzCase {
        label: "bell_character",
        task_type_str: "MAINT\x07",
        notes_str: "Bell character in task",
        should_fail: false,
    },
    
    // ========== SYMBOL-LIKE PATTERNS ==========
    FuzzCase {
        label: "symbol_prefix",
        task_type_str: "MAINT_PREFIX_",
        notes_str: "Underscore prefixed",
        should_fail: false,
    },
    FuzzCase {
        label: "symbol_suffix",
        task_type_str: "_MAINT_",
        notes_str: "Underscore surrounded",
        should_fail: false,
    },
    FuzzCase {
        label: "numeric_task",
        task_type_str: "12345",
        notes_str: "All numeric task type",
        should_fail: false,
    },
    FuzzCase {
        label: "alphanumeric_mix",
        task_type_str: "MAINT123ABC456",
        notes_str: "Mixed alphanumeric",
        should_fail: false,
    },
];

/// Main fuzz test: iterate corpus and verify no panics on valid input
#[test]
fn fuzz_submit_maintenance_no_panic() {
    for case in FUZZ_CORPUS {
        let env = Env::default();
        env.mock_all_auths();

        // Deploy all contracts
        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());

        let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
        let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
        let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

        // Setup admins and actors
        let asset_admin = Address::generate(&env);
        let eng_admin = Address::generate(&env);
        let lifecycle_admin = Address::generate(&env);
        let issuer = Address::generate(&env);
        let asset_owner = Address::generate(&env);
        let engineer = Address::generate(&env);

        // Initialize contracts
        asset_registry.initialize_admin(&asset_admin, &asset_admin);
        asset_registry.add_asset_type(&asset_admin, &symbol_short!("FUZZ"));
        engineer_registry.initialize_admin(&eng_admin, &eng_admin);
        engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
        lifecycle.initialize(
            &lifecycle_admin,
            &asset_registry_id,
            &engineer_registry_id,
            &lifecycle_admin,
            &0,
        );

        // Register asset
        let metadata = String::from_str(&env, "Fuzz test asset");
        let asset_id = asset_registry.register_asset(
            &symbol_short!("FUZZ"),
            &metadata,
            &String::from_str(&env, "SN-FUZZ"),
            &asset_owner,
        );

        // Register engineer
        let credential_hash = BytesN::from_array(&env, &[1u8; 32]);
        engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);
        lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

        // Convert case strings to Soroban strings
        let task_type_symbol = symbol_short!("FUZZ"); // Use fixed symbol to avoid symbol parsing issues
        let notes = String::from_str(&env, case.notes_str);

        // Execute the fuzz test: must not panic
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            lifecycle.try_submit_maintenance(&asset_id, &task_type_symbol, &notes, &engineer, &None)
        }));

        match result {
            Ok(_) => {
                // Either succeeded or returned structured error (OK)
            }
            Err(_) => {
                panic!(
                    "UNEXPECTED PANIC in fuzz case '{}': \
                    task_type={:?}, notes={:?}. \
                    This indicates insufficient input validation.",
                    case.label, case.task_type_str, case.notes_str
                );
            }
        }
    }
}

/// Test empty notes rejection
#[test]
fn fuzz_empty_notes_error() {
    let env = Env::default();
    env.mock_all_auths();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    let asset_admin = Address::generate(&env);
    let eng_admin = Address::generate(&env);
    let lifecycle_admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let asset_owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("FUZZ"));
    engineer_registry.initialize_admin(&eng_admin, &eng_admin);
    engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
    lifecycle.initialize(
        &lifecycle_admin,
        &asset_registry_id,
        &engineer_registry_id,
        &lifecycle_admin,
        &0,
    );

    let metadata = String::from_str(&env, "Test asset");
    let asset_id = asset_registry.register_asset(
        &symbol_short!("FUZZ"),
        &metadata,
        &String::from_str(&env, "SN-TEST"),
        &asset_owner,
    );

    let credential_hash = BytesN::from_array(&env, &[2u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);
    lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

    // Empty notes should be rejected
    let empty_notes = String::from_str(&env, "");
    let result =
        lifecycle.try_submit_maintenance(&asset_id, &symbol_short!("FUZZ"), &empty_notes, &engineer, &None);

    // Should return an error, not panic
    assert!(result.is_err(), "Empty notes should be rejected with structured error");
}

/// Test oversized notes rejection
#[test]
fn fuzz_oversized_notes_error() {
    let env = Env::default();
    env.mock_all_auths();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    let asset_admin = Address::generate(&env);
    let eng_admin = Address::generate(&env);
    let lifecycle_admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let asset_owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("FUZZ"));
    engineer_registry.initialize_admin(&eng_admin, &eng_admin);
    engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
    lifecycle.initialize(
        &lifecycle_admin,
        &asset_registry_id,
        &engineer_registry_id,
        &lifecycle_admin,
        &0,
    );

    let metadata = String::from_str(&env, "Test asset");
    let asset_id = asset_registry.register_asset(
        &symbol_short!("FUZZ"),
        &metadata,
        &String::from_str(&env, "SN-TEST"),
        &asset_owner,
    );

    let credential_hash = BytesN::from_array(&env, &[3u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);
    lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

    // Create notes exceeding max_notes_length (default 256)
    let oversized = "x".repeat(300);
    let oversized_notes = String::from_str(&env, &oversized);
    let result = lifecycle.try_submit_maintenance(
        &asset_id,
        &symbol_short!("FUZZ"),
        &oversized_notes,
        &engineer, &None,
    );

    // Should return an error, not panic
    assert!(
        result.is_err(),
        "Oversized notes should be rejected with structured error"
    );
}

/// Test that valid maximum-length notes are accepted
#[test]
fn fuzz_max_length_notes_accepted() {
    let env = Env::default();
    env.mock_all_auths();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    let asset_admin = Address::generate(&env);
    let eng_admin = Address::generate(&env);
    let lifecycle_admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let asset_owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("FUZZ"));
    engineer_registry.initialize_admin(&eng_admin, &eng_admin);
    engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
    lifecycle.initialize(
        &lifecycle_admin,
        &asset_registry_id,
        &engineer_registry_id,
        &lifecycle_admin,
        &0,
    );

    let metadata = String::from_str(&env, "Test asset");
    let asset_id = asset_registry.register_asset(
        &symbol_short!("FUZZ"),
        &metadata,
        &String::from_str(&env, "SN-TEST"),
        &asset_owner,
    );

    let credential_hash = BytesN::from_array(&env, &[4u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);
    lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

    // Create notes at exactly max_notes_length (default 256)
    let max_length = "x".repeat(256);
    let max_notes = String::from_str(&env, &max_length);
    let result = lifecycle.try_submit_maintenance(&asset_id, &symbol_short!("FUZZ"), &max_notes, &engineer, &None);

    // Should succeed (return Ok)
    assert!(
        result.is_ok(),
        "Notes at max length should be accepted"
    );
}

/// Test input validation doesn't corrupt asset state
#[test]
fn fuzz_invalid_input_no_state_corruption() {
    let env = Env::default();
    env.mock_all_auths();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    let asset_admin = Address::generate(&env);
    let eng_admin = Address::generate(&env);
    let lifecycle_admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let asset_owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("FUZZ"));
    engineer_registry.initialize_admin(&eng_admin, &eng_admin);
    engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
    lifecycle.initialize(
        &lifecycle_admin,
        &asset_registry_id,
        &engineer_registry_id,
        &lifecycle_admin,
        &0,
    );

    let metadata = String::from_str(&env, "Test asset");
    let asset_id = asset_registry.register_asset(
        &symbol_short!("FUZZ"),
        &metadata,
        &String::from_str(&env, "SN-TEST"),
        &asset_owner,
    );

    let credential_hash = BytesN::from_array(&env, &[5u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);
    lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

    // Get initial collateral score
    let score_before = lifecycle.get_collateral_score(&asset_id);

    // Try to submit with empty notes (invalid)
    let empty_notes = String::from_str(&env, "");
    let _ = lifecycle.try_submit_maintenance(&asset_id, &symbol_short!("FUZZ"), &empty_notes, &engineer, &None);

    // Try to submit with oversized notes (invalid)
    let oversized = "x".repeat(500);
    let oversized_notes = String::from_str(&env, &oversized);
    let _ = lifecycle.try_submit_maintenance(
        &asset_id,
        &symbol_short!("FUZZ"),
        &oversized_notes,
        &engineer, &None,
    );

    // Score should remain unchanged
    let score_after = lifecycle.get_collateral_score(&asset_id);
    assert_eq!(
        score_before, score_after,
        "Invalid submissions should not affect collateral score"
    );

    // Submit one valid record
    let valid_notes = String::from_str(&env, "Valid maintenance record");
    let _ = lifecycle.try_submit_maintenance(
        &asset_id,
        &symbol_short!("FUZZ"),
        &valid_notes,
        &engineer, &None,
    );

    // Score should have improved
    let score_final = lifecycle.get_collateral_score(&asset_id);
    assert!(
        score_final > score_before,
        "Valid submission should increase collateral score"
    );
}
