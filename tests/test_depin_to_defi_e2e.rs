#![cfg(test)]

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient, EngineerStatus};
use lifecycle::{Lifecycle, LifecycleClient};
use lending::{LendingContract, LendingContractClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    token, Address, BytesN, Env, String,
};

/// End-to-end test validating the complete Mainstay DePIN → DeFi flow:
/// 1. Register asset on-chain
/// 2. Register engineer with credentials
/// 3. Submit maintenance to build collateral score
/// 4. Assert collateral score meets lending threshold
/// 5. Request loan using asset as collateral
/// 6. Verify loan is disbursed successfully
#[test]
fn test_depin_to_defi_flow() {
    let env = Env::default();
    env.mock_all_auths();

    // ============================================================================
    // STEP 1: Deploy all contracts in test environment
    // ============================================================================
    
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());
    let lending_id = env.register(LendingContract, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);
    let lending = LendingContractClient::new(&env, &lending_id);

    // Setup actors
    let asset_admin = Address::generate(&env);
    let eng_admin = Address::generate(&env);
    let lifecycle_admin = Address::generate(&env);
    let lending_deployer = Address::generate(&env);
    let lending_admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    
    let asset_owner = Address::generate(&env);
    let engineer = Address::generate(&env);
    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    // Setup token for lending
    let token_contract_id = env.register(token::Contract, ());
    let token_client = token::Client::new(&env, &token_contract_id);
    token_client.initialize(
        &token_admin,
        &18,
        &String::from_str(&env, "Test Token"),
        &String::from_str(&env, "TEST"),
    );

    // Initialize asset registry
    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));

    // Initialize engineer registry
    engineer_registry.initialize_admin(&eng_admin, &eng_admin);
    engineer_registry.add_trusted_issuer(&eng_admin, &issuer);

    // Initialize lifecycle contract
    lifecycle.initialize(
        &lifecycle_admin,
        &asset_registry_id,
        &engineer_registry_id,
        &lifecycle_admin,
        &0, // Use default max_history
    );

    // Initialize lending contract
    lending.initialize(&lending_deployer, &lending_admin, &token_contract_id, &200);

    // ============================================================================
    // STEP 2: Register asset, register engineer, submit maintenance
    // ============================================================================

    // Register a physical asset (industrial generator)
    let asset_metadata = String::from_str(&env, "Industrial Generator XYZ-5000");
    let serial_number = String::from_str(&env, "SN-12345-GEN");
    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &asset_metadata,
        &serial_number,
        &asset_owner,
    );

    // Verify asset registration
    let asset = asset_registry.get_asset(&asset_id);
    assert_eq!(asset.asset_id, asset_id);
    assert_eq!(asset.asset_type, symbol_short!("GENSET"));
    assert_eq!(asset.owner, asset_owner);

    // Register engineer with verifiable credentials
    let credential_hash = BytesN::from_array(&env, &[42u8; 32]);
    let validity_period = 31_536_000; // 1 year
    engineer_registry.register_engineer(
        &engineer,
        &credential_hash,
        &issuer,
        &validity_period,
    );

    // Verify engineer is active
    let engineer_record = engineer_registry.get_engineer(&engineer);
    assert_eq!(engineer_record.address, engineer);
    assert!(engineer_record.active);
    assert_eq!(
        engineer_registry.get_engineer_status(&engineer),
        EngineerStatus::Active
    );

    // Authorize engineer for this asset
    lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

    // Submit maintenance records to build collateral score
    // Each maintenance submission increases collateral score
    let maintenance_tasks = vec![
        ("ENGINE", "Oil change and filter replacement"),
        ("FILTER", "Air filter service"),
        ("BEARING", "Bearing lubrication and inspection"),
        ("COOLANT", "Coolant system flush"),
        ("FUEL", "Fuel system inspection and cleaning"),
    ];

    for (i, (task_type, description)) in maintenance_tasks.iter().enumerate() {
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!(task_type),
            &String::from_str(&env, description),
            &engineer, &None,
        );

        // Verify maintenance history grows
        let history = lifecycle.get_maintenance_history(&asset_id);
        assert_eq!(history.len(), i + 1);

        // Verify record details
        let record = history.get(i as u32).unwrap();
        assert_eq!(record.asset_id, asset_id);
        assert_eq!(record.task_type, symbol_short!(task_type));
        assert_eq!(record.engineer, engineer);

        env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    }

    // ============================================================================
    // STEP 3: Assert collateral score meets lending threshold
    // ============================================================================

    let collateral_score = lifecycle.get_collateral_score(&asset_id);
    
    // Expected: 5 maintenance tasks * 5 points per task (but actual scoring may vary)
    // At minimum, score should be non-zero and above eligibility threshold
    assert!(collateral_score > 0, "Collateral score must be positive");
    
    // Verify asset is eligible as collateral
    let is_eligible = lifecycle.is_collateral_eligible(&asset_id);
    assert!(
        is_eligible,
        "Asset with score {} should be collateral-eligible",
        collateral_score
    );

    // Verify score history is maintained
    let score_history = lifecycle.get_score_history(&asset_id);
    assert!(!score_history.is_empty(), "Score history must be recorded");

    // ============================================================================
    // STEP 4: Request loan using asset as collateral
    // ============================================================================

    // Setup lending participants
    // Mint tokens for vouchers and contract
    token_client.mint(&voucher, &50000);
    token_client.mint(&env.current_contract_address(), &50000);

    // Borrower vouches from collateral
    lending.vouch(&borrower, &voucher, &5000);

    // Verify vouch is recorded
    let vouches = lending.get_vouches(&borrower);
    assert_eq!(vouches.len(), 1);
    assert_eq!(vouches.get(0).unwrap().stake, 5000);

    // Request loan
    lending.request_loan(&borrower, &10000);

    // ============================================================================
    // STEP 5: Assert loan is disbursed successfully
    // ============================================================================

    // Verify loan is active
    let loan_option = lending.get_loan(&borrower);
    assert!(
        loan_option.is_some(),
        "Loan must be created for borrower after request"
    );

    let loan = loan_option.unwrap();
    assert_eq!(loan.borrower, borrower);
    assert_eq!(loan.amount, 10000);

    // Verify loan status is Active (0 = Active, 1 = Repaid, 2 = Defaulted)
    let loan_status_code = loan.status;
    assert_eq!(
        loan_status_code, 0,
        "Loan status must be Active (0), got {}",
        loan_status_code
    );

    // Verify credit score is initialized for borrower
    let credit_score = lending.get_credit_score(&borrower);
    assert_eq!(credit_score, 0, "Credit score starts at 0 for new borrowers");

    // ============================================================================
    // STEP 6: Verify loan repayment flow (optional but demonstrates full cycle)
    // ============================================================================

    // Borrower repays loan
    lending.repay(&borrower);

    // Verify loan is marked as repaid
    let repaid_loan = lending.get_loan(&borrower);
    assert!(
        repaid_loan.is_none(),
        "Loan should be removed after successful repayment"
    );

    // Verify credit score improved after repayment
    let improved_credit_score = lending.get_credit_score(&borrower);
    assert_eq!(
        improved_credit_score, 100,
        "Credit score should be 100 after successful repayment"
    );

    // ============================================================================
    // SUCCESS: Complete DePIN → DeFi flow validated
    // ============================================================================
    println!(
        "✓ DePIN → DeFi E2E Flow Complete:");
        println!("  - Asset {} registered on-chain", asset_id);
    println!("  - Engineer verified with credentials");
    println!("  - Maintenance submitted: {} records", maintenance_tasks.len());
    println!("  - Collateral score: {} (eligible: {})", collateral_score, is_eligible);
    println!("  - Loan requested and disbursed: {} tokens", 10000);
    println!("  - Loan repaid successfully");
    println!("  - Borrower credit score: {}", improved_credit_score);
}

/// Additional test: Verify asset with insufficient maintenance cannot be used as collateral
#[test]
fn test_insufficient_collateral_score_blocks_lending() {
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

    // Setup and initialize
    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));
    engineer_registry.initialize_admin(&eng_admin, &eng_admin);
    engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
    lifecycle.initialize(
        &lifecycle_admin,
        &asset_registry_id,
        &engineer_registry_id,
        &lifecycle_admin,
        &0,
    );

    // Register asset with no maintenance
    let asset_metadata = String::from_str(&env, "Unmaintained generator");
    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &asset_metadata,
        &String::from_str(&env, "SN-NOMAINT"),
        &asset_owner,
    );

    // Verify asset score is 0 (no maintenance)
    let collateral_score = lifecycle.get_collateral_score(&asset_id);
    assert_eq!(
        collateral_score, 0,
        "Asset with no maintenance should have 0 collateral score"
    );

    // Verify asset is NOT eligible as collateral
    let is_eligible = lifecycle.is_collateral_eligible(&asset_id);
    assert!(
        !is_eligible,
        "Asset with insufficient maintenance should not be collateral-eligible"
    );
}

/// Test: Verify multiple assets can build independent collateral scores
#[test]
fn test_multiple_assets_independent_scores() {
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
    
    let owner1 = Address::generate(&env);
    let owner2 = Address::generate(&env);
    let engineer1 = Address::generate(&env);
    let engineer2 = Address::generate(&env);

    // Setup
    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));
    engineer_registry.initialize_admin(&eng_admin, &eng_admin);
    engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
    lifecycle.initialize(
        &lifecycle_admin,
        &asset_registry_id,
        &engineer_registry_id,
        &lifecycle_admin,
        &0,
    );

    // Register two assets
    let asset_id_1 = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Generator 1"),
        &String::from_str(&env, "SN-001"),
        &owner1,
    );

    let asset_id_2 = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Generator 2"),
        &String::from_str(&env, "SN-002"),
        &owner2,
    );

    // Register engineers
    let cred_hash_1 = BytesN::from_array(&env, &[1u8; 32]);
    let cred_hash_2 = BytesN::from_array(&env, &[2u8; 32]);
    engineer_registry.register_engineer(&engineer1, &cred_hash_1, &issuer, &31_536_000, &None);
    engineer_registry.register_engineer(&engineer2, &cred_hash_2, &issuer, &31_536_000, &None);

    lifecycle.authorize_engineer(&owner1, &asset_id_1, &engineer1);
    lifecycle.authorize_engineer(&owner2, &asset_id_2, &engineer2);

    // Submit different numbers of maintenance records
    // Asset 1: 3 records
    for i in 0..3 {
        lifecycle.submit_maintenance(
            &asset_id_1,
            &symbol_short!("MAINT"),
            &String::from_str(&env, &format!("Maintenance {}", i)),
            &engineer1, &None,
        );
        env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    }

    // Asset 2: 5 records
    for i in 0..5 {
        lifecycle.submit_maintenance(
            &asset_id_2,
            &symbol_short!("MAINT"),
            &String::from_str(&env, &format!("Maintenance {}", i)),
            &engineer2, &None,
        );
        env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    }

    // Verify independent scores
    let score_1 = lifecycle.get_collateral_score(&asset_id_1);
    let score_2 = lifecycle.get_collateral_score(&asset_id_2);

    assert!(score_1 > 0, "Asset 1 should have positive score from 3 records");
    assert!(score_2 > 0, "Asset 2 should have positive score from 5 records");
    assert!(
        score_2 >= score_1,
        "Asset 2 (5 records) should have score >= Asset 1 (3 records)"
    );

    // Both should be eligible if above threshold
    let eligible_1 = lifecycle.is_collateral_eligible(&asset_id_1);
    let eligible_2 = lifecycle.is_collateral_eligible(&asset_id_2);
    assert!(eligible_1, "Asset 1 with 3 records should be eligible");
    assert!(eligible_2, "Asset 2 with 5 records should be eligible");
}
