// tests/test_credential_renewal_extends_expiry.rs
//
// Issue #1041 — Add Test: engineer registry credential renewal extends
// expires_at correctly
//
// Verifies that renewing a credential moves `expires_at` forward relative to
// its state at renewal time, rather than leaving it pinned to a value that
// could be derived purely from the original registration.
//
// Per `renew_credential`'s documented behaviour (contracts/engineer-registry/
// src/lib.rs), a credential that has not yet hard-expired renews by adding
// the new validity period on top of its *current* `expires_at` — preserving
// whatever validity remained rather than resetting from "now". This test
// exercises exactly that path:
//
//   1. Register engineer with a 30-day credential.
//   2. Advance the ledger 20 days (10 days of the original period remain).
//   3. Renew the credential for another 30 days.
//   4. Assert the new `expires_at` is computed from the renewal-time state
//      (current `expires_at`), not simply reproducing the original
//      registration's `issued_at + 30 days` value untouched.

use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use soroban_sdk::{testutils::Address as _, testutils::Ledger, Address, BytesN, Env};

const DAY: u64 = 86_400;

#[test]
fn test_renew_credential_extends_expires_at_correctly() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(EngineerRegistry, ());
    let client = EngineerRegistryClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize_admin(&admin, &admin);

    let issuer = Address::generate(&env);
    client.add_trusted_issuer(&admin, &issuer);

    // 1. Register engineer with a 30-day credential.
    let engineer = Address::generate(&env);
    let credential_hash = BytesN::from_array(&env, &[0x11u8; 32]);
    let issued_at = env.ledger().timestamp();
    let initial_validity = 30 * DAY;
    client.register_engineer(&engineer, &credential_hash, &issuer, &initial_validity, &None);

    let original = client.get_engineer(&engineer);
    let original_expires_at = original.expires_at;
    assert_eq!(original_expires_at, issued_at + initial_validity);

    // 2. Advance ledger 20 days — credential is still valid (10 days remain).
    let elapsed = 20 * DAY;
    env.ledger().set_timestamp(issued_at + elapsed);
    let renewal_time = env.ledger().timestamp();
    assert_eq!(renewal_time, issued_at + elapsed);

    // 3. Renew credential for another 30 days.
    let new_validity = 30 * DAY;
    client.renew_credential(&engineer, &new_validity);

    // 4. The new expires_at must reflect the renewal — it is calculated
    //    relative to the credential's current state (its still-active
    //    expires_at) at renewal time, not a value that ignores the renewal
    //    entirely.
    let renewed = client.get_engineer(&engineer);

    // A broken renewal that failed to move the expiry forward at all would
    // leave expires_at exactly at the original registration's value; that
    // must not happen.
    assert_ne!(
        renewed.expires_at, original_expires_at,
        "renew_credential must change expires_at, not leave it at the pre-renewal value"
    );

    // Correct, documented behaviour: since the credential had not yet
    // expired, the new period stacks on the current expires_at.
    let expected_expires_at = original_expires_at + new_validity;
    assert_eq!(
        renewed.expires_at, expected_expires_at,
        "renewal must extend from the credential's current expires_at, not reset from renewal time"
    );

    // The renewed expiry must land strictly after "renewal_time + new_validity"
    // would give — confirming the remaining validity at renewal time was
    // preserved rather than discarded.
    assert!(
        renewed.expires_at > renewal_time + new_validity,
        "renewed expires_at ({}) must exceed a naive renewal_time + validity ({})",
        renewed.expires_at,
        renewal_time + new_validity
    );
}
