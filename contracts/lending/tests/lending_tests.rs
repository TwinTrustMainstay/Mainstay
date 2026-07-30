#![cfg(test)]

use lending::{ContractError, LendingContract, LendingContractClient, LoanStatus};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env, String,
};

fn setup_contract_and_token(env: &Env) -> (Address, Address, Address, Address) {
    let admin = Address::generate(env);
    let deployer = Address::generate(env);
    let token_admin = Address::generate(env);

    // Register and initialize token
    let token_id = env.register_stellar_asset_contract(token_admin.clone());
    let token_client = TokenClient::new(env, &token_id);

    // Register lending contract
    let contract_id = env.register(LendingContract, ());
    let client = LendingContractClient::new(env, &contract_id);

    // Initialize lending contract
    client.initialize(&deployer, &admin, &token_id, &0);

    (contract_id, token_id, admin, token_admin)
}

#[test]
fn test_self_vouch_rejection() {
    let env = Env::default();
    env.mock_all_auths();

    let (contract_id, token_id, _admin, token_admin) = setup_contract_and_token(&env);
    let client = LendingContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token_id);

    let borrower = Address::generate(&env);
    let stellar_asset_client = StellarAssetClient::new(&env, &token_id);

    // Mint tokens to borrower
    stellar_asset_client.mint(&borrower, &10_000_000);

    // Attempt to vouch for themselves should fail with DuplicateVouch
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.vouch(&borrower, &borrower, &100);
    }));

    assert!(result.is_err(), "Self-vouch should be rejected");
}

#[test]
fn test_vouch_with_active_loan_rejection() {
    let env = Env::default();
    env.mock_all_auths();

    let (contract_id, token_id, admin, token_admin) = setup_contract_and_token(&env);
    let client = LendingContractClient::new(&env, &contract_id);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);
    let stellar_asset_client = StellarAssetClient::new(&env, &token_id);

    // Mint tokens to contract and voucher
    stellar_asset_client.mint(&env.current_contract_address(), &1_000_000_000);
    stellar_asset_client.mint(&voucher, &10_000_000);

    // Request a loan first
    client.request_loan(&borrower, &100_000, &0u64);

    // Verify loan is active
    let loan = client.get_loan(&borrower);
    assert!(loan.is_some());
    assert_eq!(loan.unwrap().status, LoanStatus::Active);

    // Attempt to vouch after loan is active should fail with LoanAlreadyActive
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.vouch(&borrower, &voucher, &100);
    }));

    assert!(result.is_err(), "Vouch with active loan should be rejected");
}

#[test]
fn test_insufficient_contract_balance_rejection() {
    let env = Env::default();
    env.mock_all_auths();

    let (contract_id, token_id, _admin, _token_admin) = setup_contract_and_token(&env);
    let client = LendingContractClient::new(&env, &contract_id);
    let stellar_asset_client = StellarAssetClient::new(&env, &token_id);

    let borrower = Address::generate(&env);

    // Mint only 50 tokens to contract (less than requested loan)
    stellar_asset_client.mint(&env.current_contract_address(), &50);

    // Attempt to request loan of 100 should fail with InsufficientFunds
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.request_loan(&borrower, &100, &0u64);
    }));

    assert!(
        result.is_err(),
        "Loan request with insufficient contract balance should fail"
    );
}

#[test]
fn test_withdraw_vouch_success() {
    let env = Env::default();
    env.mock_all_auths();

    let (contract_id, token_id, _admin, token_admin) = setup_contract_and_token(&env);
    let client = LendingContractClient::new(&env, &contract_id);
    let stellar_asset_client = StellarAssetClient::new(&env, &token_id);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    // Mint tokens to voucher
    stellar_asset_client.mint(&voucher, &10_000_000);

    // Vouch for borrower
    client.vouch(&borrower, &voucher, &1_000);

    // Verify vouch was recorded
    let vouches = client.get_vouches(&borrower);
    assert_eq!(vouches.len(), 1);
    assert_eq!(vouches.get(0).unwrap().stake, 1_000);

    // Withdraw vouch
    client.withdraw_vouch(&borrower, &voucher);

    // Verify vouch was removed
    let vouches_after = client.get_vouches(&borrower);
    assert_eq!(vouches_after.len(), 0);
}

#[test]
fn test_withdraw_vouch_with_active_loan_rejection() {
    let env = Env::default();
    env.mock_all_auths();

    let (contract_id, token_id, _admin, token_admin) = setup_contract_and_token(&env);
    let client = LendingContractClient::new(&env, &contract_id);
    let stellar_asset_client = StellarAssetClient::new(&env, &token_id);

    let borrower = Address::generate(&env);
    let voucher = Address::generate(&env);

    // Mint tokens
    stellar_asset_client.mint(&voucher, &10_000_000);
    stellar_asset_client.mint(&env.current_contract_address(), &1_000_000_000);

    // Vouch for borrower
    client.vouch(&borrower, &voucher, &1_000);

    // Request a loan
    client.request_loan(&borrower, &100_000, &0u64);

    // Attempt to withdraw vouch with active loan should fail
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.withdraw_vouch(&borrower, &voucher);
    }));

    assert!(
        result.is_err(),
        "Withdraw vouch with active loan should fail"
    );
}

#[test]
fn test_loan_disbursal_transfers_funds() {
    let env = Env::default();
    env.mock_all_auths();

    let (contract_id, token_id, _admin, token_admin) = setup_contract_and_token(&env);
    let client = LendingContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token_id);
    let stellar_asset_client = StellarAssetClient::new(&env, &token_id);

    let borrower = Address::generate(&env);

    // Mint tokens to contract
    stellar_asset_client.mint(&env.current_contract_address(), &1_000_000);

    // Check initial balance
    let initial_balance = token_client.balance(&borrower);
    assert_eq!(initial_balance, 0);

    // Request loan
    client.request_loan(&borrower, &100_000, &0u64);

    // Check borrower received the loan amount
    let final_balance = token_client.balance(&borrower);
    assert_eq!(final_balance, 100_000);
}
