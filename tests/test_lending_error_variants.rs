// tests/test_lending_error_variants.rs
//
// Issue #993 — lending ContractError has dedicated variants for
// TimelockNotExpired and ProposalNotFound
//
// Before this fix, the From<SharedContractError> impl mapped both
// TimelockNotExpired and ProposalNotFound (and PendingAdminAlreadyExists) to
// ContractError::NotInitialized, producing misleading errors for timelock
// operations.
//
// Strategy
// --------
// 1. Verify the new enum variants exist and have the correct discriminant values.
// 2. Verify the From impl maps each SharedContractError variant to the correct
//    ContractError variant (compile-time + runtime correctness).

use lending::{ContractError, SharedError};

/// Verify the new variants are distinct and have the expected discriminants.
#[test]
fn test_timelock_not_expired_variant_exists_with_correct_discriminant() {
    // Discriminants are part of the contract ABI; changing them is a breaking
    // change.  #993 assigns 19 to TimelockNotExpired and 20 to ProposalNotFound.
    assert_eq!(ContractError::TimelockNotExpired as u32, 19);
    assert_eq!(ContractError::ProposalNotFound as u32, 20);
}

#[test]
fn test_timelock_not_expired_not_equal_to_not_initialized() {
    assert_ne!(
        ContractError::TimelockNotExpired,
        ContractError::NotInitialized,
        "TimelockNotExpired must be a distinct variant from NotInitialized"
    );
}

#[test]
fn test_proposal_not_found_not_equal_to_not_initialized() {
    assert_ne!(
        ContractError::ProposalNotFound,
        ContractError::NotInitialized,
        "ProposalNotFound must be a distinct variant from NotInitialized"
    );
}

#[test]
fn test_from_shared_timelock_not_expired_maps_to_dedicated_variant() {
    let converted = ContractError::from(SharedError::TimelockNotExpired);
    assert_eq!(
        converted,
        ContractError::TimelockNotExpired,
        "SharedContractError::TimelockNotExpired must map to ContractError::TimelockNotExpired, \
         not to ContractError::NotInitialized"
    );
}

#[test]
fn test_from_shared_proposal_not_found_maps_to_dedicated_variant() {
    let converted = ContractError::from(SharedError::ProposalNotFound);
    assert_eq!(
        converted,
        ContractError::ProposalNotFound,
        "SharedContractError::ProposalNotFound must map to ContractError::ProposalNotFound, \
         not to ContractError::NotInitialized"
    );
}

#[test]
fn test_from_shared_not_initialized_still_maps_correctly() {
    let converted = ContractError::from(SharedError::NotInitialized);
    assert_eq!(converted, ContractError::NotInitialized);
}

#[test]
fn test_from_shared_already_initialized_still_maps_correctly() {
    let converted = ContractError::from(SharedError::AlreadyInitialized);
    assert_eq!(converted, ContractError::AlreadyInitialized);
}

#[test]
fn test_from_shared_unauthorized_admin_still_maps_correctly() {
    let converted = ContractError::from(SharedError::UnauthorizedAdmin);
    assert_eq!(converted, ContractError::UnauthorizedAdmin);
}

#[test]
fn test_from_shared_paused_still_maps_correctly() {
    let converted = ContractError::from(SharedError::Paused);
    assert_eq!(converted, ContractError::ContractPaused);
}

/// Regression: all seven SharedContractError variants must be handled;
/// this test will fail to compile if the From impl is non-exhaustive.
#[test]
fn test_from_impl_is_exhaustive() {
    let cases = [
        (SharedError::NotInitialized, ContractError::NotInitialized),
        (SharedError::AlreadyInitialized, ContractError::AlreadyInitialized),
        (SharedError::UnauthorizedAdmin, ContractError::UnauthorizedAdmin),
        (SharedError::Paused, ContractError::ContractPaused),
        (SharedError::TimelockNotExpired, ContractError::TimelockNotExpired),
        (SharedError::ProposalNotFound, ContractError::ProposalNotFound),
        // PendingAdminAlreadyExists maps to AlreadyInitialized (not NotInitialized).
        (SharedError::PendingAdminAlreadyExists, ContractError::AlreadyInitialized),
    ];

    for (shared, expected) in cases {
        let got = ContractError::from(shared);
        assert_eq!(
            got, expected,
            "From<SharedContractError::{:?}> should produce ContractError::{:?}, got {:?}",
            shared, expected, got
        );
    }
}
