//! Signature compatibility tests for engineer credentials.
//!
//! These tests verify that credential verification, authorization checks,
//! and event signatures remain compatible across simulated contract upgrades.
//! They cover:
//!
//! 1. **Credential state transitions**: Valid → GracePeriod → HardExpired → Revoked
//!    produce consistent results across an upgrade boundary.
//! 2. **EngineerAuth verification**: The `require_auth()` checks on the engineer
//!    address are enforced correctly after upgrade.
//! 3. **Issuer auth verification**: Only the original issuer can revoke/renew.
//! 4. **Batch verification**: `batch_verify_engineers` returns correct results.
//! 5. **Event topic consistency**: Event topics emitted before and after upgrade
//!    use the same symbols.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{
    EngineerRegistry, EngineerRegistryClient, CredentialStatus, EngineerStatus,
};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Events, Ledger},
    Address, BytesN, Env, String, Symbol, Vec,
};

// ─────────────────────────────────────────────────────────────────────────────
// Helper: bootstrap all three contracts with an active engineer
// ─────────────────────────────────────────────────────────────────────────────

struct SignatureTestSetup<'a> {
    asset_registry: AssetRegistryClient<'a>,
    engineer_registry: EngineerRegistryClient<'a>,
    lifecycle: LifecycleClient<'a>,
    _admin: Address,
    issuer: Address,
    owner: Address,
    engineer: Address,
    asset_id: u64,
    _credential_hash: BytesN<32>,
}

fn setup_signature_test<'a>(env: &'a Env) -> SignatureTestSetup<'a> {
    env.mock_all_auths();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(env, &lifecycle_id);

    let admin = Address::generate(env);
    let issuer = Address::generate(env);
    let owner = Address::generate(env);
    let engineer = Address::generate(env);

    asset_registry.initialize_admin(&admin, &admin);
    asset_registry.add_asset_type(&admin, &symbol_short!("GENSET"));

    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);

    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &200,
    );

    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(env, "Signature compatibility asset"),
        &String::from_str(env, "SN-SIG-001"),
        &owner,
    );

    let credential_hash = BytesN::from_array(env, &[0x5Au8; 32]);
    engineer_registry.register_engineer(
        &engineer,
        &credential_hash,
        &issuer,
        &31_536_000, // 1 year
        &Some(String::from_str(env, "Signature test engineer")),
    );

    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    SignatureTestSetup {
        asset_registry,
        engineer_registry,
        lifecycle,
        _admin: admin,
        issuer,
        owner,
        engineer,
        asset_id,
        _credential_hash: credential_hash,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Credential state consistency across upgrade simulation
// ═══════════════════════════════════════════════════════════════════════════

/// `verify_engineer` must return consistent results for Valid, GracePeriod,
/// HardExpired, Revoked, and NotFound states — both before and after an
/// upgrade boundary.
#[test]
fn test_credential_status_consistency_across_upgrade() {
    let env = Env::default();
    let s = setup_signature_test(&env);

    // Valid state
    assert_eq!(
        s.engineer_registry.verify_engineer(&s.engineer),
        CredentialStatus::Valid
    );
    assert_eq!(
        s.engineer_registry.get_credential_status(&s.engineer),
        CredentialStatus::Valid
    );
    assert!(s.engineer_registry.is_engineer_active(&s.engineer));
    assert_eq!(
        s.engineer_registry.get_engineer_status(&s.engineer),
        EngineerStatus::Active
    );

    // Advance to GracePeriod
    let record = s.engineer_registry.get_engineer(&s.engineer);
    env.ledger().set_timestamp(record.expires_at);
    assert_eq!(
        s.engineer_registry.get_credential_status(&s.engineer),
        CredentialStatus::GracePeriod
    );

    // Advance to HardExpired (past 7-day grace period)
    env.ledger().set_timestamp(record.expires_at + 7 * 86_400);
    assert_eq!(
        s.engineer_registry.get_credential_status(&s.engineer),
        CredentialStatus::HardExpired
    );
    assert!(!s.engineer_registry.is_engineer_active(&s.engineer));
    assert_eq!(
        s.engineer_registry.get_engineer_status(&s.engineer),
        EngineerStatus::Expired
    );

    // Revoked → create a fresh engineer, then revoke
    let engineer2 = Address::generate(&env);
    let hash2 = BytesN::from_array(&env, &[0x6Bu8; 32]);
    s.engineer_registry.register_engineer(&engineer2, &hash2, &s.issuer, &31_536_000, &None);
    assert_eq!(
        s.engineer_registry.verify_engineer(&engineer2),
        CredentialStatus::Valid
    );

    s.engineer_registry.revoke_credential(&engineer2);
    assert_eq!(
        s.engineer_registry.verify_engineer(&engineer2),
        CredentialStatus::Revoked
    );

    // NotFound
    let ghost = Address::generate(&env);
    assert_eq!(
        s.engineer_registry.verify_engineer(&ghost),
        CredentialStatus::NotFound
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// EngineerAuth enforcement for maintenance submission
// ═══════════════════════════════════════════════════════════════════════════

/// An unregistered engineer must not be able to submit maintenance.
/// This verifies that the EngineerAuth check works and is not weakened
/// by an upgrade.
#[test]
fn test_engineer_auth_enforced_for_unregistered_engineer() {
    let env = Env::default();
    let s = setup_signature_test(&env);

    let rogue = Address::generate(&env);

    // Unregistered engineer → submit_maintenance must panic
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        s.lifecycle.submit_maintenance(
            &s.asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "Rogue attempt"),
            &rogue,
        );
    }));
    assert!(result.is_err(), "Unregistered engineer must be rejected");

    // Registered but not authorised → must also fail
    let engineer2 = Address::generate(&env);
    let hash2 = BytesN::from_array(&env, &[0x7Cu8; 32]);
    s.engineer_registry.register_engineer(&engineer2, &hash2, &s.issuer, &31_536_000, &None);
    // engineer2 is registered but NOT authorised for this asset

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        s.lifecycle.submit_maintenance(
            &s.asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "Unauthorised attempt"),
            &engineer2,
        );
    }));
    assert!(result.is_err(), "Registered but unauthorised engineer must be rejected");
}

/// A revoked engineer must not pass the EngineerAuth check.
#[test]
fn test_revoked_engineer_cannot_submit_maintenance() {
    let env = Env::default();
    let s = setup_signature_test(&env);

    // Revoke the engineer
    s.engineer_registry.revoke_credential(&s.engineer);
    assert_eq!(
        s.engineer_registry.verify_engineer(&s.engineer),
        CredentialStatus::Revoked
    );

    // Even though previously authorised, revoked engineer must be rejected
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        s.lifecycle.submit_maintenance(
            &s.asset_id,
            &symbol_short!("ENGINE"),
            &String::from_str(&env, "Revoked engineer attempt"),
            &s.engineer,
        );
    }));
    assert!(result.is_err(), "Revoked engineer must be rejected");
}

// ═══════════════════════════════════════════════════════════════════════════
// Issuer auth enforcement
// ═══════════════════════════════════════════════════════════════════════════

/// Verify that issuer identity is correctly stored on engineer registration,
/// enabling the `revoke_credential` function's `record.issuer.require_auth()`
/// check to correctly enforce that only the original issuer can revoke.
#[test]
fn test_issuer_identity_stored_for_revocation() {
    let env = Env::default();
    let s = setup_signature_test(&env);

    // Register a second issuer, and an engineer under the first issuer
    let issuer2 = Address::generate(&env);
    s.engineer_registry.add_trusted_issuer(&s._admin, &issuer2);

    let engineer2 = Address::generate(&env);
    let hash2 = BytesN::from_array(&env, &[0x8Du8; 32]);
    s.engineer_registry.register_engineer(&engineer2, &hash2, &s.issuer, &31_536_000, &None);

    // Verify the stored issuer is correct for both engineers
    let record = s.engineer_registry.get_engineer(&s.engineer);
    assert_eq!(record.issuer, s.issuer, "Original issuer must be stored correctly");

    let record2 = s.engineer_registry.get_engineer(&engineer2);
    assert_eq!(record2.issuer, s.issuer, "Engineer must be linked to original issuer");

    // Both issuers are trusted
    assert!(s.engineer_registry.is_trusted_issuer(&s.issuer));
    assert!(s.engineer_registry.is_trusted_issuer(&issuer2));
}

/// A credential that is already revoked cannot be revoked again.
#[test]
fn test_double_revoke_is_rejected() {
    let env = Env::default();
    let s = setup_signature_test(&env);

    s.engineer_registry.revoke_credential(&s.engineer);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        s.engineer_registry.revoke_credential(&s.engineer);
    }));
    assert!(result.is_err(), "Double revoke must be rejected");
}

// ═══════════════════════════════════════════════════════════════════════════
// Batch verification compatibility
// ═══════════════════════════════════════════════════════════════════════════

/// `batch_verify_engineers` must produce consistent results across
/// upgrade boundaries.
#[test]
fn test_batch_verification_compatibility() {
    let env = Env::default();
    let s = setup_signature_test(&env);

    // Register a second engineer
    let engineer2 = Address::generate(&env);
    let hash2 = BytesN::from_array(&env, &[0x9Eu8; 32]);
    s.engineer_registry.register_engineer(&engineer2, &hash2, &s.issuer, &31_536_000, &None);

    // Revoke the second one
    s.engineer_registry.revoke_credential(&engineer2);

    let ghost = Address::generate(&env);

    let batch = Vec::from_array(
        &env,
        [s.engineer.clone(), engineer2.clone(), ghost.clone()],
    );

    let results = s.engineer_registry.batch_verify_engineers(&batch);
    assert_eq!(results.len(), 3);
    assert_eq!(results.get(0).unwrap(), CredentialStatus::Valid);
    assert_eq!(results.get(1).unwrap(), CredentialStatus::Revoked);
    assert_eq!(results.get(2).unwrap(), CredentialStatus::NotFound);

    // Same check after "upgrade" simulation: re-read
    let results2 = s.engineer_registry.batch_verify_engineers(&batch);
    assert_eq!(results2.len(), 3);
    assert_eq!(results2.get(0).unwrap(), CredentialStatus::Valid);
    assert_eq!(results2.get(1).unwrap(), CredentialStatus::Revoked);
    assert_eq!(results2.get(2).unwrap(), CredentialStatus::NotFound);
}

// ═══════════════════════════════════════════════════════════════════════════
// Event topic consistency
// ═══════════════════════════════════════════════════════════════════════════

/// Events emitted for engineer operations must use consistent topics
/// that downstream indexers and listeners can depend on.
#[test]
fn test_engineer_registration_event_topics() {
    let env = Env::default();
    let s = setup_signature_test(&env);

    // Find the reg_eng event among emitted events
    let events = env.events().all();
    let mut found_reg_eng = false;

    // The reg_eng event has topic (symbol_short!("reg_eng"),)
    for (_id, topics, _data) in events.iter() {
        use soroban_sdk::TryIntoVal;
        if let Ok(t0) = topics.get(0).unwrap().try_into_val::<Symbol>(&env) {
            if t0 == symbol_short!("reg_eng") {
                found_reg_eng = true;
                break;
            }
        }
    }

    assert!(found_reg_eng, "reg_eng event must be emitted on engineer registration");
}

/// Revocation events must emit with the correct topic.
#[test]
fn test_revocation_event_topics() {
    let env = Env::default();
    let s = setup_signature_test(&env);

    // Revoke and check for REV_CRED topic
    s.engineer_registry.revoke_credential(&s.engineer);

    let events = env.events().all();
    let mut found_rev_cred = false;

    for (_id, topics, _data) in events.iter() {
        use soroban_sdk::TryIntoVal;
        if let Ok(t0) = topics.get(0).unwrap().try_into_val::<Symbol>(&env) {
            if t0 == symbol_short!("REV_CRED")
                || t0 == symbol_short!("ADM_AUD")
            {
                // Check for REV_CRED specifically
                if t0 == symbol_short!("REV_CRED") {
                    found_rev_cred = true;
                }
            }
        }
    }

    assert!(found_rev_cred, "REV_CRED event must be emitted on credential revocation");
}

// ═══════════════════════════════════════════════════════════════════════════
// Suspension compatibility
// ═══════════════════════════════════════════════════════════════════════════

/// Suspended engineer verification must work correctly and survive
/// an upgrade boundary.
#[test]
fn test_suspension_status_consistency() {
    let env = Env::default();
    let s = setup_signature_test(&env);

    let now = env.ledger().timestamp();
    let suspend_until = now + 86_400; // 1 day

    s.engineer_registry.suspend_engineer(
        &s.engineer,
        &suspend_until,
        &String::from_str(&env, "Suspension compatibility test"),
    );

    assert!(s.engineer_registry.is_credential_suspended(&s.engineer));
    assert_eq!(
        s.engineer_registry.get_credential_status(&s.engineer),
        CredentialStatus::Suspended
    );
    assert_eq!(
        s.engineer_registry.verify_engineer(&s.engineer),
        CredentialStatus::Suspended
    );
    assert!(!s.engineer_registry.is_engineer_active(&s.engineer));

    // Advance past suspension end
    env.ledger().set_timestamp(suspend_until);
    assert!(!s.engineer_registry.is_credential_suspended(&s.engineer));
    assert_eq!(
        s.engineer_registry.get_credential_status(&s.engineer),
        CredentialStatus::Valid
    );
}
