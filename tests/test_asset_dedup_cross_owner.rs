// tests/test_asset_dedup_cross_owner.rs
//
// Issue #1036 — Add Test: asset registry deduplication rejects same serial number
// from different owners
//
// Verifies that two different owner addresses cannot register the same physical
// machine by supplying the same serial number.  The second registration attempt
// must fail with `ContractError::DuplicateAsset` (error code 2) regardless of
// whether the metadata description or owner address differ.
//
// Background
// ----------
// `register_asset` computes sha256(serial_number) and stores the resulting hash
// under a global `SN_DEDUP` key.  Because this key is owner-independent, any
// second attempt to register the same serial number — even from a different owner
// — is rejected with `DuplicateAsset`.

use asset_registry::{AssetRegistry, AssetRegistryClient};
use soroban_sdk::{
    symbol_short,
    testutils::Address as _,
    Address, Env, String,
};

/// `ContractError::DuplicateAsset` discriminant in `asset_registry`.
const DUPLICATE_ASSET_ERROR: u32 = 2;

#[test]
fn test_asset_dedup_rejects_same_serial_number_from_different_owners() {
    let env = Env::default();
    env.mock_all_auths();

    // ── Deploy and initialise the asset registry ──────────────────────────────
    let asset_registry_id = env.register(AssetRegistry, ());
    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);

    let admin = Address::generate(&env);
    asset_registry.initialize_admin(&admin, &admin);
    asset_registry.add_asset_type(&admin, &symbol_short!("GENSET"));

    // ── Two distinct owners ───────────────────────────────────────────────────
    let owner_a = Address::generate(&env);
    let owner_b = Address::generate(&env);

    // The shared serial number that identifies the physical machine.
    let serial_number = String::from_str(&env, "SN-PHYSICAL-MACHINE-X1");

    // ── Step 1: Owner A registers the asset successfully ─────────────────────
    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Heavy-duty generator owned by owner A"),
        &serial_number,
        &owner_a,
    );

    // Verify the registration succeeded and the asset belongs to owner A.
    let asset = asset_registry.get_asset(&asset_id);
    assert_eq!(
        asset.owner, owner_a,
        "registered asset should belong to owner A",
    );

    // ── Step 2: Owner B attempts to register the same serial number ───────────
    // Even with a different metadata description, the global serial-number
    // deduplication key must reject this as DuplicateAsset.
    let result = asset_registry.try_register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Same machine, different owner claim"),
        &serial_number, // <── identical serial number
        &owner_b,
    );

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            DUPLICATE_ASSET_ERROR,
        ))),
        "registering the same serial number from a different owner must return \
         DuplicateAsset (error {})",
        DUPLICATE_ASSET_ERROR,
    );

    // ── Step 3: Confirm that the original registration is untouched ───────────
    let asset_after = asset_registry.get_asset(&asset_id);
    assert_eq!(
        asset_after.owner, owner_a,
        "the original asset record must still belong to owner A after the failed \
         re-registration attempt",
    );
}
