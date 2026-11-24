//! Integration test for the engineer credential expiry state machine.
//!
//! Verifies the complete transition path:
//!     Valid → GracePeriod → HardExpired
//!
//! Tasks covered:
//!   1. Register engineer with short validity period.
//!   2. Advance time to the grace period boundary and assert `GracePeriod` status.
//!   3. Advance past the hard expiry boundary and assert `HardExpired` status.
//!   4. Call `verify_engineer` for a `HardExpired` credential and assert failure.

use engineer_registry::{
    CredentialStatus, EngineerRegistry, EngineerRegistryClient, EngineerStatus,
};
use soroban_sdk::{testutils::Address as _, testutils::Ledger, Address, BytesN, Env, Symbol};

#[test]
fn test_engineer_credential_expiry_states() {
    let env = Env::default();
    env.mock_all_auths();

    // Bootstrap an EngineerRegistry with an admin and a trusted issuer.
    let contract_id = env.register(EngineerRegistry, ());
    let client = EngineerRegistryClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize_admin(&admin, &admin);

    let issuer = Address::generate(&env);
    client.add_trusted_issuer(&admin, &issuer);

    // Sanity-check: default grace period is 7 days. We rely on this in the test.
    assert_eq!(client.get_grace_period(), 7 * 86_400);

    // 1. Register engineer with the shortest permitted validity period (one day).
    let engineer = Address::generate(&env);
    let credential_hash = BytesN::from_array(&env, &[0xabu8; 32]);
    let base_time = env.ledger().timestamp();
    let validity = 86_400u64;
    client.register_engineer(&engineer, &credential_hash, &issuer, &validity, &None);

    let record = client.get_engineer(&engineer);
    assert_eq!(record.expires_at, base_time + validity);
    let hard_expired_at = record.expires_at + 7 * 86_400;

    // -------- Valid state --------
    // Right after registration: detailed status is Valid, verify_engineer agrees.
    assert_eq!(
        client.get_credential_status(&engineer),
        CredentialStatus::Valid
    );
    assert_eq!(
        client.verify_engineer(&engineer, &None::<Symbol>),
        CredentialStatus::Valid
    );

    // One second before expiry: still Valid.
    env.ledger().set_timestamp(record.expires_at - 1);
    assert_eq!(
        client.get_credential_status(&engineer),
        CredentialStatus::Valid
    );

    // 2. Advance to the exact expiry boundary. The credential is technically
    //    expired but still inside the configurable grace window.
    env.ledger().set_timestamp(record.expires_at);
    assert_eq!(
        client.get_credential_status(&engineer),
        CredentialStatus::GracePeriod
    );

    // Mid-grace: well inside the grace window, still GracePeriod.
    env.ledger().set_timestamp(record.expires_at + 7 * 86_400 / 2);
    assert_eq!(
        client.get_credential_status(&engineer),
        CredentialStatus::GracePeriod
    );

    // One second before grace ends: still GracePeriod.
    env.ledger().set_timestamp(hard_expired_at - 1);
    assert_eq!(
        client.get_credential_status(&engineer),
        CredentialStatus::GracePeriod
    );

    // 3. Advance past the hard expiry boundary → HardExpired.
    env.ledger().set_timestamp(hard_expired_at);
    assert_eq!(
        client.get_credential_status(&engineer),
        CredentialStatus::HardExpired
    );
    env.ledger().set_timestamp(hard_expired_at + 86_400);
    assert_eq!(
        client.get_credential_status(&engineer),
        CredentialStatus::HardExpired
    );

    // 4. verify_engineer must reflect failure for a HardExpired credential.
    //    The credential is no longer Valid; is_engineer_active is false; the
    //    bounded legacy status (`get_engineer_status`) reports Expired.
    let status_via_verify = client.verify_engineer(&engineer, &None::<Symbol>);
    assert_ne!(status_via_verify, CredentialStatus::Valid);
    assert_ne!(status_via_verify, CredentialStatus::GracePeriod);
    assert_eq!(client.is_engineer_active(&engineer), false);
    assert_eq!(
        client.get_engineer_status(&engineer),
        EngineerStatus::Expired
    );
}
