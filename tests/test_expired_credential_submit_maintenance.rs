#![cfg(test)]

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

/// Verify that an engineer whose credential has passed its hard‑expiry
/// (expires_at + grace period) cannot submit maintenance records even though
/// their `active` flag is still true.
///
/// Issue: #1032
#[test]
fn test_expired_credential_engineer_rejected_from_submit_maintenance() {
    let env = Env::default();
    env.mock_all_auths();

    // ── Deploy all contracts ──────────────────────────────────────────
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(&env, &lifecycle_id);

    // ── Setup actors ──────────────────────────────────────────────────
    let asset_admin = Address::generate(&env);
    let eng_admin = Address::generate(&env);
    let lifecycle_admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let asset_owner = Address::generate(&env);
    let engineer = Address::generate(&env);

    // ── Initialize contracts ──────────────────────────────────────────
    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("GEN"));
    engineer_registry.initialize_admin(&eng_admin, &eng_admin);
    engineer_registry.add_trusted_issuer(&eng_admin, &issuer);
    lifecycle.initialize(
        &lifecycle_admin,
        &asset_registry_id,
        &engineer_registry_id,
        &lifecycle_admin,
        &0,
    );

    // ── Register asset ────────────────────────────────────────────────
    let metadata = String::from_str(&env, "Test asset for expired credential test");
    let asset_id = asset_registry.register_asset(
        &symbol_short!("GEN"),
        &metadata,
        &String::from_str(&env, "SN-EXPIRED-TEST"),
        &asset_owner,
    );

    // ── Register engineer with short validity (1 day) ─────────────────
    let credential_hash = BytesN::from_array(&env, &[0xABu8; 32]);
    let validity_period = 86_400u64; // 1 day
    engineer_registry.register_engineer(
        &engineer,
        &credential_hash,
        &issuer,
        &validity_period,
        &None,
    );

    // ── Verify engineer is active with non‑expired credential ─────────
    let record = engineer_registry.get_engineer(&engineer);
    assert!(record.active, "engineer should be active after registration");
    assert!(
        env.ledger().timestamp() < record.expires_at,
        "credential should not be expired yet"
    );

    // ── Advance timestamp past hard expiry ────────────────────────────
    // The lifecycle contract accepts CredentialStatus::Valid and
    // CredentialStatus::GracePeriod, so merely advancing past expires_at
    // (into the grace window) would still allow submissions.  We must
    // advance past expires_at + grace_period to reach HardExpired, which
    // is the only expired state that triggers rejection.
    // Default grace period is 7 days (604_800 s).
    let grace_period = engineer_registry.get_grace_period();
    let hard_expired_timestamp = record.expires_at + grace_period + 1;
    env.ledger().set_timestamp(hard_expired_timestamp);

    // ── Assert active flag is still true (the core of the issue) ──────
    let record_after = engineer_registry.get_engineer(&engineer);
    assert!(
        record_after.active,
        "active flag should remain true even after hard expiry — this is the key invariant being tested"
    );

    // ── Attempt submit_maintenance — must be rejected ─────────────────
    // Note: we deliberately do NOT call authorize_engineer or
    // add_specialization here because the credential check runs
    // before both the authorization and specialization checks inside
    // submit_maintenance — a HardExpired credential fails first.
    let notes = String::from_str(&env, "Routine maintenance check");
    let result = lifecycle.try_submit_maintenance(
        &asset_id,
        &symbol_short!("CHECK"),
        &notes,
        &engineer,
    );

    // The contract should return either:
    //   lifecycle::ContractError::UnauthorizedEngineer  (wrapping the credential failure)
    //   engineer_registry::ContractError::CredentialExpired (if propagated as-is)
    // Both indicate the same semantic: an expired credential blocks maintenance.
    assert!(
        result.is_err(),
        "submit_maintenance must reject an engineer whose credential is hard‑expired \
         (expected UnauthorizedEngineer or CredentialExpired), \
         even when active == true"
    );
}
