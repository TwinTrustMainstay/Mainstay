/// Compile-time assertions that all ContractError discriminants are unique across
/// every contract. Duplicate discriminants cause ambiguous error codes that break
/// client-side error matching.
use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, String};

// ── Discriminant-uniqueness helpers ──────────────────────────────────────

fn assert_unique_discriminants(discriminants: &[u32], contract: &str) {
    let mut sorted: Vec<u32> = discriminants.to_vec();
    sorted.sort();
    for i in 1..sorted.len() {
        assert_ne!(
            sorted[i],
            sorted[i - 1],
            "Duplicate discriminant {} found in {} ContractError",
            sorted[i],
            contract,
        );
    }
}

// ── Asset Registry ───────────────────────────────────────────────────────

#[test]
fn test_asset_registry_discriminants_are_unique() {
    use asset_registry::ContractError::{
        AdminAlreadyInitialized, AssetAlreadyDeprecated, AssetDecommissioned, AssetNotFound,
        BatchTooLarge, DuplicateAsset, EmptyMetadata, InvalidAssetType, NotInitialized, Paused,
        PendingAdminAlreadyExists, ProposalAlreadyExists, ProposalNotFound, SameOwner,
        TimelockNotExpired, TypeInUse, UnauthorizedAdmin, UnauthorizedOwner,
    };

    let discriminants = [
        AssetNotFound as u32,
        DuplicateAsset as u32,
        UnauthorizedAdmin as u32,
        UnauthorizedOwner as u32,
        NotInitialized as u32,
        AdminAlreadyInitialized as u32,
        Paused as u32,
        InvalidAssetType as u32,
        PendingAdminAlreadyExists as u32,
        TypeInUse as u32,
        EmptyMetadata as u32,
        SameOwner as u32,
        TimelockNotExpired as u32,
        ProposalNotFound as u32,
        AssetDecommissioned as u32,
        ProposalAlreadyExists as u32,
        AssetAlreadyDeprecated as u32,
        BatchTooLarge as u32,
    ];

    assert_unique_discriminants(&discriminants, "AssetRegistry");
}

// ── Engineer Registry ────────────────────────────────────────────────────

#[test]
fn test_engineer_registry_discriminants_are_unique() {
    use engineer_registry::ContractError::{
        AdminAlreadyInitialized, BatchRevokeTooLarge, CredentialAlreadyRevoked, CredentialExpired,
        CredentialRevoked, CredentialSuspended, EngineerAlreadyRegistered, EngineerAlreadySuspended,
        EngineerNotFound, InvalidCredentialHash, InvalidSuspensionPeriod, InvalidValidityPeriod,
        IssuerNotFound, IssuerRemoved, NotInitialized, Paused, PendingAdminAlreadyExists,
        ProposalNotFound, TimelockNotExpired, UnauthorizedAdmin, UntrustedIssuer,
    };

    let discriminants = [
        CredentialAlreadyRevoked as u32,
        UnauthorizedAdmin as u32,
        EngineerNotFound as u32,
        NotInitialized as u32,
        AdminAlreadyInitialized as u32,
        UntrustedIssuer as u32,
        InvalidCredentialHash as u32,
        Paused as u32,
        CredentialRevoked as u32,
        EngineerAlreadyRegistered as u32,
        IssuerNotFound as u32,
        PendingAdminAlreadyExists as u32,
        InvalidValidityPeriod as u32,
        IssuerRemoved as u32,
        TimelockNotExpired as u32,
        ProposalNotFound as u32,
        CredentialSuspended as u32,
        EngineerAlreadySuspended as u32,
        InvalidSuspensionPeriod as u32,
        BatchRevokeTooLarge as u32,
        CredentialExpired as u32,
    ];

    assert_unique_discriminants(&discriminants, "EngineerRegistry");
}

// ── Lifecycle ────────────────────────────────────────────────────────────

#[test]
fn test_lifecycle_discriminants_are_unique() {
    use lifecycle::ContractError::{
        AlreadyInitialized, AssetDecommissioned, AssetNotFound, BatchTooLarge, DuplicateAdmin,
        EngineerNotAuthorized, HistoryCapReached, IndexOutOfBounds, InsufficientSigners,
        InvalidConfig, InvalidTaskType, NoMaintenanceHistory, NotInitialized, NotesTooLong, Paused,
        PendingAdminAlreadyExists, ProposalNotFound, SameRegistryAddress, ScoreFrozen,
        ScoreOverflow, TimelockNotExpired, TooManyAdmins, UnauthorizedAdmin, UnauthorizedEngineer,
        UnauthorizedOwner, ZeroAddress,
    };

    let discriminants = [
        NoMaintenanceHistory as u32,
        UnauthorizedEngineer as u32,
        UnauthorizedAdmin as u32,
        HistoryCapReached as u32,
        AssetNotFound as u32,
        NotInitialized as u32,
        AlreadyInitialized as u32,
        InvalidConfig as u32,
        Paused as u32,
        InvalidTaskType as u32,
        PendingAdminAlreadyExists as u32,
        ZeroAddress as u32,
        SameRegistryAddress as u32,
        IndexOutOfBounds as u32,
        UnauthorizedOwner as u32,
        EngineerNotAuthorized as u32,
        TimelockNotExpired as u32,
        ProposalNotFound as u32,
        ScoreOverflow as u32,
        NotesTooLong as u32,
        ScoreFrozen as u32,
        AssetDecommissioned as u32,
        BatchTooLarge as u32,
        InsufficientSigners as u32,
        DuplicateAdmin as u32,
        TooManyAdmins as u32,
    ];

    assert_unique_discriminants(&discriminants, "Lifecycle");
}

// ── Lending ──────────────────────────────────────────────────────────────

#[test]
fn test_lending_discriminants_are_unique() {
    use lending::ContractError::{
        AlreadyInitialized, ContractPaused, DuplicateVouch, InsufficientFunds, InvalidAdminAddress,
        InvalidTokenAddress, LoanAlreadyActive, NoActiveLoan, NotInitialized, StakeBelowMinimum,
        StakeSummationOverflow, TooManyVouchers, UnauthorizedAdmin, UnauthorizedBorrower,
        VouchWithdrawNotAllowed, ZeroStake,
    };

    let discriminants = [
        LoanAlreadyActive as u32,
        NoActiveLoan as u32,
        DuplicateVouch as u32,
        ZeroStake as u32,
        NotInitialized as u32,
        AlreadyInitialized as u32,
        UnauthorizedAdmin as u32,
        InsufficientFunds as u32,
        StakeBelowMinimum as u32,
        StakeSummationOverflow as u32,
        InvalidAdminAddress as u32,
        InvalidTokenAddress as u32,
        ContractPaused as u32,
        TooManyVouchers as u32,
        VouchWithdrawNotAllowed as u32,
        UnauthorizedBorrower as u32,
    ];

    assert_unique_discriminants(&discriminants, "Lending");
}

// ── Integration test: valuation_history_push is callable ─────────────────

/// Exercises the full lifecycle path to verify that `valuation_history_push`
/// (moved to module scope in scoring.rs) compiles and is callable.
#[test]
fn test_valuation_history_push_is_callable() {
    let env = Env::default();
    env.mock_all_auths();

    let asset_registry_id = env.register(asset_registry::AssetRegistry, ());
    let engineer_registry_id = env.register(engineer_registry::EngineerRegistry, ());
    let lifecycle_id = env.register(lifecycle::Lifecycle, ());

    let asset_client = asset_registry::Client::new(&env, &asset_registry_id);
    let eng_client = engineer_registry::Client::new(&env, &engineer_registry_id);
    let lc_client = lifecycle::Client::new(&env, &lifecycle_id);

    let deployer = Address::generate(&env);
    let admin = Address::generate(&env);
    let owner = Address::generate(&env);
    let issuer = Address::generate(&env);
    let engineer = Address::generate(&env);

    // Initialize registries
    asset_client.initialize_admin(&deployer, &admin);
    eng_client.initialize_admin(&deployer, &admin);
    lc_client.initialize(&deployer, &asset_registry_id, &engineer_registry_id, &admin, &200);

    // Add asset type and trusted issuer
    let genset = soroban_sdk::symbol_short!("GENSET");
    asset_client.add_asset_type(&admin, &genset);
    eng_client.add_trusted_issuer(&admin, &issuer);

    // Register asset
    let serial = String::from_str(&env, "SN-001");
    let meta = String::from_str(&env, "Test generator");
    let asset_id = asset_client.register_asset(&genset, &meta, &serial, &owner);

    // Register engineer
    let cred_hash = BytesN::from_array(&env, &[1u8; 32]);
    eng_client.register_engineer(&engineer, &cred_hash, &issuer, &31_536_000, &None);

    // Authorize engineer
    lc_client.authorize_engineer(&owner, &asset_id, &engineer);

    // Submit maintenance — exercises score_history_push -> valuation_history_push
    lc_client.submit_maintenance(
        &asset_id,
        &soroban_sdk::symbol_short!("ENGINE"),
        &String::from_str(&env, "Routine inspection"),
        &engineer,
    );

    // Verify score was recorded
    let score = lc_client.get_collateral_score(&asset_id);
    assert!(score > 0, "score should be > 0 after maintenance");
}

// ── Integration test: engineer auth cleared on asset transfer ────────────

#[test]
fn test_old_engineer_auth_invalid_after_transfer() {
    let env = Env::default();
    env.mock_all_auths();

    let asset_registry_id = env.register(asset_registry::AssetRegistry, ());
    let engineer_registry_id = env.register(engineer_registry::EngineerRegistry, ());
    let lifecycle_id = env.register(lifecycle::Lifecycle, ());

    let asset_client = asset_registry::Client::new(&env, &asset_registry_id);
    let eng_client = engineer_registry::Client::new(&env, &engineer_registry_id);
    let lc_client = lifecycle::Client::new(&env, &lifecycle_id);

    let deployer = Address::generate(&env);
    let admin = Address::generate(&env);
    let owner1 = Address::generate(&env);
    let owner2 = Address::generate(&env);
    let issuer = Address::generate(&env);
    let engineer = Address::generate(&env);

    // Initialize
    asset_client.initialize_admin(&deployer, &admin);
    eng_client.initialize_admin(&deployer, &admin);
    lc_client.initialize(&deployer, &asset_registry_id, &engineer_registry_id, &admin, &200);

    let genset = soroban_sdk::symbol_short!("GENSET");
    asset_client.add_asset_type(&admin, &genset);
    eng_client.add_trusted_issuer(&admin, &issuer);

    let serial = String::from_str(&env, "SN-TRANSFER");
    let meta = String::from_str(&env, "Transfer test asset");
    let asset_id = asset_client.register_asset(&genset, &meta, &serial, &owner1);

    let cred_hash = BytesN::from_array(&env, &[2u8; 32]);
    eng_client.register_engineer(&engineer, &cred_hash, &issuer, &31_536_000, &None);

    // Owner1 authorizes engineer
    lc_client.authorize_engineer(&owner1, &asset_id, &engineer);

    // Transfer asset from owner1 to owner2
    asset_client.transfer_asset(&asset_id, &owner1, &owner2);

    // Record transfer in lifecycle — should clear EngineerAuth entries
    lc_client.record_transfer(&asset_id, &owner1, &owner2);

    // Engineer should NOT be able to submit maintenance after transfer
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lc_client.submit_maintenance(
            &asset_id,
            &soroban_sdk::symbol_short!("ENGINE"),
            &String::from_str(&env, "Post-transfer maintenance"),
            &engineer,
        );
    }));
    assert!(
        result.is_err(),
        "Engineer should NOT be authorized after ownership transfer"
    );

    // Owner2 authorizes the engineer — now it should work
    lc_client.authorize_engineer(&owner2, &asset_id, &engineer);
    let result2 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lc_client.submit_maintenance(
            &asset_id,
            &soroban_sdk::symbol_short!("ENGINE"),
            &String::from_str(&env, "Re-authorized maintenance"),
            &engineer,
        );
    }));
    assert!(
        result2.is_ok(),
        "Engineer should be able to submit maintenance after re-authorization"
    );
}
