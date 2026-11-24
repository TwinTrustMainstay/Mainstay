// tests/test_full_loan_lien_lifecycle_e2e.rs
//
// Issue: no test exercises the full loan + lien lifecycle end-to-end:
// register asset → place lien → repay loan → verify lien released →
// verify asset is unlocked → verify a new loan can be taken against the
// same asset after release.
//
// This test wires the asset-registry and lending contracts together the
// same way the existing focused unit tests do (asset-registry's
// `set_lending_contract` + lending's `record_lien`/`release_lien`), but
// walks the *entire* collateral lifecycle in one place instead of testing
// each half in isolation.

#![cfg(test)]

use asset_registry::{AssetRegistry, AssetRegistryClient};
use lending::{LendingContract, LendingContractClient, LoanStatus};
use soroban_sdk::{
    symbol_short,
    testutils::Address as _,
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env, String,
};

#[test]
fn test_full_loan_lien_lifecycle_register_lien_repay_release_unlock_relend() {
    let env = Env::default();
    env.mock_all_auths();

    // ── Deploy contracts ───────────────────────────────────────────────────
    let asset_registry_id = env.register(AssetRegistry, ());
    let lending_id = env.register(LendingContract, ());

    let asset_registry = AssetRegistryClient::new(&env, &asset_registry_id);
    let lending = LendingContractClient::new(&env, &lending_id);

    let asset_admin = Address::generate(&env);
    let lending_deployer = Address::generate(&env);
    let lending_admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let owner = Address::generate(&env);
    let borrower = owner.clone();
    // Identity used as the "lending contract" registered with asset-registry —
    // mirrors the pattern used by asset-registry's own lock/unlock tests,
    // where the registered lender is an authorized address, not necessarily
    // the lending contract instance itself.
    let lender = Address::generate(&env);

    // ── Token setup ────────────────────────────────────────────────────────
    let token_id = env.register_stellar_asset_contract(token_admin.clone());
    let token_client = TokenClient::new(&env, &token_id);
    let stellar_asset_client = StellarAssetClient::new(&env, &token_id);
    stellar_asset_client.mint(&lending_id, &1_000_000_000);

    // ── Initialize contracts ──────────────────────────────────────────────
    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));
    asset_registry.set_lending_contract(&asset_admin, &lender);

    lending.initialize(&lending_deployer, &lending_admin, &token_id, &0);

    // ── Step 1: register asset ────────────────────────────────────────────
    let asset_id = asset_registry.register_asset(
        &symbol_short!("GENSET"),
        &String::from_str(&env, "Industrial Generator — full lifecycle test"),
        &String::from_str(&env, "SN-LIEN-LIFECYCLE-001"),
        &owner,
    );
    let asset_before = asset_registry.get_asset(&asset_id);
    assert!(!asset_before.is_locked, "freshly registered asset must not be locked");

    // ── Step 2: borrower takes out a loan and a lien is placed ────────────
    let loan_amount: u64 = 100_000;
    lending.request_loan(&borrower, &loan_amount, &asset_id);
    let loan = lending.get_loan(&borrower).expect("loan must exist after request_loan");
    assert_eq!(loan.status, LoanStatus::Active);
    let loan_id_1 = loan.id;

    lending.record_lien(&lending_admin, &asset_id, &lender, &loan_id_1, &loan_amount);
    asset_registry.lock_asset_as_collateral(&lender, &asset_id, &loan_id_1);

    let asset_locked = asset_registry.get_asset(&asset_id);
    assert!(asset_locked.is_locked, "asset must be locked once collateralized");
    let liens_after_lock = lending.get_liens(&asset_id);
    assert_eq!(liens_after_lock.len(), 1, "one lien must be recorded against the asset");

    // ── Step 3: repay the loan ─────────────────────────────────────────────
    // Borrower must hold the token balance to cover repayment (amount + yield).
    stellar_asset_client.mint(&borrower, &1_000_000);
    let _ = token_client.balance(&borrower);
    lending.repay(&borrower);

    let repaid_loan = lending.get_loan(&borrower).expect("loan record must still exist after repay");
    assert_eq!(repaid_loan.status, LoanStatus::Repaid);

    // ── Step 4: release the lien and unlock the asset ─────────────────────
    lending.release_lien(&lending_admin, &asset_id, &lender, &loan_id_1);
    asset_registry.unlock_asset_from_collateral(&lender, &asset_id, &loan_id_1);

    let liens_after_release = lending.get_liens(&asset_id);
    assert!(
        liens_after_release.is_empty(),
        "lien must be released after loan repayment"
    );

    let asset_after_release = asset_registry.get_asset(&asset_id);
    assert!(
        !asset_after_release.is_locked,
        "asset is_locked must be false after lien release"
    );

    // ── Step 5: a new loan can be taken against the same asset ────────────
    let second_loan_amount: u64 = 50_000;
    lending.request_loan(&borrower, &second_loan_amount, &asset_id);
    let second_loan = lending
        .get_loan(&borrower)
        .expect("second loan must exist after re-requesting against the released asset");
    assert_eq!(second_loan.status, LoanStatus::Active);
    assert_ne!(
        second_loan.id, loan_id_1,
        "the second loan must be assigned a fresh loan id"
    );

    lending.record_lien(&lending_admin, &asset_id, &lender, &second_loan.id, &second_loan_amount);
    asset_registry.lock_asset_as_collateral(&lender, &asset_id, &second_loan.id);

    let asset_relocked = asset_registry.get_asset(&asset_id);
    assert!(
        asset_relocked.is_locked,
        "asset must be lockable again as collateral for the new loan"
    );
}
