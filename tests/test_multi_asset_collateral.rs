use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};
use lending::{LendingContract, LendingContractClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    token::StellarAssetClient,
    Address, BytesN, Env, String, Symbol, Vec,
};

fn serial_number(env: &Env, n: u64) -> String {
    let mut buf = [0u8; 20];
    buf[0] = b'S';
    buf[1] = b'N';
    buf[2] = b'-';
    let mut end = 20usize;
    let mut v = n;
    while v > 0 {
        end -= 1;
        buf[end] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    let start = end.min(3);
    let s = core::str::from_utf8(&buf[start..20]).unwrap_or("SN-0");
    String::from_str(env, s)
}

fn next_id() -> u64 {
    use core::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn setup_portfolio_env(
    env: &Env,
) -> (
    LifecycleClient,
    AssetRegistryClient,
    EngineerRegistryClient,
    LendingContractClient,
    Address,
    Address,
    Address,
    Address,
    Address,
) {
    env.mock_all_auths();

    let admin = Address::generate(env);
    let asset_admin = Address::generate(env);
    let issuer = Address::generate(env);
    let engineer = Address::generate(env);
    let deployer = Address::generate(env);

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());
    let lending_id = env.register(LendingContract, ());
    let lending_addr = lending_id.clone();

    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(env, &lifecycle_id);
    let lending = LendingContractClient::new(env, &lending_id);

    asset_registry.initialize_admin(&asset_admin, &asset_admin);
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("SOLAR"));
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("WIND"));
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("BATT"));
    asset_registry.add_asset_type(&asset_admin, &symbol_short!("HYDRO"));

    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);

    let credential_hash = BytesN::from_array(env, &[7u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000, &None);
    // Reputation 500 → each task contributes score_increment * (500 + 500) / 1000 = 5 points
    engineer_registry.update_reputation(&engineer, &500);

    lifecycle.initialize(
        &deployer,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &0,
    );

    let token_admin = Address::generate(env);
    let token_id = env.register_stellar_asset_contract(token_admin.clone());
    lending.initialize(&deployer, &admin, &token_id, &5000);

    let stellar_asset = StellarAssetClient::new(env, &token_id);
    stellar_asset.mint(&lending_addr, &10_000_000_000);

    (
        lifecycle,
        asset_registry,
        engineer_registry,
        lending,
        asset_admin,
        admin,
        engineer,
        issuer,
        token_id,
    )
}

fn register_asset(
    env: &Env,
    registry: &AssetRegistryClient,
    asset_type: Symbol,
    owner: &Address,
) -> u64 {
    let meta = String::from_str(env, "asset");
    let sn = serial_number(env, next_id());
    registry.register_asset(&asset_type, &meta, &sn, owner)
}

fn authorize_and_maintain(
    env: &Env,
    lifecycle: &LifecycleClient,
    _registry: &AssetRegistryClient,
    asset_id: u64,
    owner: &Address,
    engineer: &Address,
    task_type: Symbol,
    count: u32,
) {
    lifecycle.authorize_engineer(owner, &asset_id, engineer);
    for _ in 0..count {
        lifecycle.submit_maintenance(
            &asset_id,
            &task_type,
            &String::from_str(env, "Scheduled maintenance"),
            engineer,
            &None,
        );
    }
}

// ---------------------------------------------------------------------------
// Happy path: 5 diverse assets as collateral, loan, LTV calculation
// ---------------------------------------------------------------------------
#[test]
fn test_multi_asset_portfolio_happy_path() {
    let env = Env::default();
    let (
        lifecycle,
        registry,
        _eng_reg,
        lending,
        _asset_admin,
        _admin,
        engineer,
        _issuer,
        _token_id,
    ) = setup_portfolio_env(&env);

    let owner1 = Address::generate(&env);
    let owner2 = Address::generate(&env);
    let owner3 = Address::generate(&env);
    let owner4 = Address::generate(&env);
    let owner5 = Address::generate(&env);

    let id1 = register_asset(&env, &registry, symbol_short!("GENSET"), &owner1);
    let id2 = register_asset(&env, &registry, symbol_short!("SOLAR"), &owner2);
    let id3 = register_asset(&env, &registry, symbol_short!("WIND"), &owner3);
    let id4 = register_asset(&env, &registry, symbol_short!("BATT"), &owner4);
    let id5 = register_asset(&env, &registry, symbol_short!("HYDRO"), &owner5);

    let asset_ids: Vec<u64> = Vec::from_array(&env, [id1, id2, id3, id4, id5]);

    // Each task contributes 5 points (score_increment=5, reputation=500)
    // GENSET: 10 → 50, SOLAR: 2 → 10, WIND: 1 → 5, BATT: 8 → 40, HYDRO: 10 → 50
    authorize_and_maintain(&env, &lifecycle, &registry, id1, &owner1, &engineer, symbol_short!("ENGINE"), 10);
    authorize_and_maintain(&env, &lifecycle, &registry, id2, &owner2, &engineer, symbol_short!("INSPECT"), 2);
    authorize_and_maintain(&env, &lifecycle, &registry, id3, &owner3, &engineer, symbol_short!("OVERHAUL"), 1);
    authorize_and_maintain(&env, &lifecycle, &registry, id4, &owner4, &engineer, symbol_short!("FILTER"), 8);
    authorize_and_maintain(&env, &lifecycle, &registry, id5, &owner5, &engineer, symbol_short!("ENGINE"), 10);

    // Verify individual scores
    assert_eq!(lifecycle.get_collateral_score(&id1), 50);
    assert_eq!(lifecycle.get_collateral_score(&id2), 10);
    assert_eq!(lifecycle.get_collateral_score(&id3), 5);
    assert_eq!(lifecycle.get_collateral_score(&id4), 40);
    assert_eq!(lifecycle.get_collateral_score(&id5), 50);

    // Batch query
    let batch = lifecycle.get_collateral_score_batch(&asset_ids);
    assert_eq!(batch.len(), 5);
    let mut total_score: u32 = 0;
    for (_, score) in batch.iter() {
        total_score += score;
    }
    assert_eq!(total_score, 155);

    // Eligibility (threshold = 50): only GENSET(50) and HYDRO(50) qualify
    let eligible = lifecycle.batch_is_collateral_eligible(&asset_ids);
    assert!(eligible.get(0).unwrap());
    assert!(!eligible.get(1).unwrap());
    assert!(!eligible.get(2).unwrap());
    assert!(!eligible.get(3).unwrap());
    assert!(eligible.get(4).unwrap());

    // Create a loan
    let borrower = Address::generate(&env);
    let loan_amount: u64 = 100_000;
    lending.request_loan(&borrower, &loan_amount, &0u64);
    let loan = lending.get_loan(&borrower).unwrap();
    assert_eq!(loan.amount, loan_amount);
    assert_eq!(loan.borrower, borrower);

    // LTV = loan / (total_score * 1000) in bps → 100_000 / (155 * 1000) = 64.52%
    let ltv_bps = (loan_amount as u128) * 10_000 / ((total_score as u128) * 1000);
    assert_eq!(ltv_bps, 6451);

    // Repay
    lending.repay(&borrower);
    assert_eq!(lending.get_loan(&borrower).unwrap().status, lending::LoanStatus::Repaid);
}

// ---------------------------------------------------------------------------
// Score aggregation
// ---------------------------------------------------------------------------
#[test]
fn test_collateral_score_aggregation() {
    let env = Env::default();
    let (lifecycle, registry, _eng_reg, _lending, _aa, _admin, engineer, _issuer, _tok) =
        setup_portfolio_env(&env);

    let owner = Address::generate(&env);

    let id_a = register_asset(&env, &registry, symbol_short!("SOLAR"), &owner);
    let id_b = register_asset(&env, &registry, symbol_short!("WIND"), &owner);
    let id_c = register_asset(&env, &registry, symbol_short!("BATT"), &owner);

    // Fresh assets → score 0
    assert_eq!(lifecycle.get_collateral_score(&id_a), 0);
    assert_eq!(lifecycle.get_collateral_score(&id_b), 0);
    assert_eq!(lifecycle.get_collateral_score(&id_c), 0);

    // Maintain only id_a: 3 tasks × 5 = 15
    authorize_and_maintain(&env, &lifecycle, &registry, id_a, &owner, &engineer, symbol_short!("ENGINE"), 3);
    assert_eq!(lifecycle.get_collateral_score(&id_a), 15);
    assert_eq!(lifecycle.get_collateral_score(&id_b), 0);
    assert_eq!(lifecycle.get_collateral_score(&id_c), 0);

    // Batch sum = 15
    let ids = Vec::from_array(&env, [id_a, id_b, id_c]);
    let batch = lifecycle.get_collateral_score_batch(&ids);
    assert_eq!(batch.len(), 3);
    assert_eq!(batch.iter().fold(0, |acc, (_, s)| acc + s), 15);

    // None eligible yet (threshold 50)
    for ok in lifecycle.batch_is_collateral_eligible(&ids).iter() {
        assert!(!ok);
    }

    // Bring id_a to 50 with 7 more tasks (total 10 × 5 = 50)
    authorize_and_maintain(&env, &lifecycle, &registry, id_a, &owner, &engineer, symbol_short!("ENGINE"), 7);
    assert!(lifecycle.is_collateral_eligible(&id_a));
    assert!(!lifecycle.is_collateral_eligible(&id_b));
}

// ---------------------------------------------------------------------------
// Maintenance history independence
// ---------------------------------------------------------------------------
#[test]
fn test_multi_asset_maintenance_histories() {
    let env = Env::default();
    let (lifecycle, registry, _eng_reg, _lending, _aa, _admin, engineer, _issuer, _tok) =
        setup_portfolio_env(&env);

    let owner = Address::generate(&env);
    let id1 = register_asset(&env, &registry, symbol_short!("GENSET"), &owner);
    let id2 = register_asset(&env, &registry, symbol_short!("SOLAR"), &owner);

    authorize_and_maintain(&env, &lifecycle, &registry, id1, &owner, &engineer, symbol_short!("ENGINE"), 3);
    authorize_and_maintain(&env, &lifecycle, &registry, id2, &owner, &engineer, symbol_short!("INSPECT"), 5);

    assert_eq!(lifecycle.get_maintenance_history(&id1).len(), 3);
    assert_eq!(lifecycle.get_maintenance_history(&id2).len(), 5);
    for r in lifecycle.get_maintenance_history(&id1).iter() {
        assert_eq!(r.asset_id, id1);
    }
    for r in lifecycle.get_maintenance_history(&id2).iter() {
        assert_eq!(r.asset_id, id2);
    }

    let eng_hist = lifecycle.get_engineer_maintenance_history(&engineer);
    assert_eq!(eng_hist.len(), 2);
}

// ---------------------------------------------------------------------------
// Collateral score decay
// ---------------------------------------------------------------------------
#[test]
fn test_collateral_score_decay() {
    let env = Env::default();
    let (lifecycle, registry, _eng_reg, _lending, _aa, _admin, engineer, _issuer, _tok) =
        setup_portfolio_env(&env);

    let owner = Address::generate(&env);
    let asset_id = register_asset(&env, &registry, symbol_short!("GENSET"), &owner);

    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);
    lifecycle.submit_maintenance(
        &asset_id,
        &symbol_short!("ENGINE"),
        &String::from_str(&env, "Initial"),
        &engineer, &None,
    );
    // Single task with rep=500 → 5 points
    assert_eq!(lifecycle.get_collateral_score(&asset_id), 5);

    // Advance past one decay interval (2_592_000s) → stored score goes to 0,
    // but MIN_SCORE_WITH_HISTORY floor returns 1
    env.ledger().set_timestamp(env.ledger().timestamp() + 2_592_001);
    let s = lifecycle.get_collateral_score(&asset_id);
    assert_eq!(s, 1, "floor at MIN_SCORE_WITH_HISTORY = 1");

    assert_eq!(lifecycle.decay_score(&asset_id), 1);
}

// ---------------------------------------------------------------------------
// Error: unregistered asset
// ---------------------------------------------------------------------------
#[test]
fn test_unregistered_asset_errors() {
    let env = Env::default();
    let (lifecycle, _registry, _eng_reg, _lending, _aa, _admin, _eng, _issuer, _tok) =
        setup_portfolio_env(&env);

    let phantom: u64 = 9999;

    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lifecycle.get_collateral_score(&phantom);
    })).is_err());

    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lifecycle.is_collateral_eligible(&phantom);
    })).is_err());

    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lifecycle.get_maintenance_history(&phantom);
    })).is_err());

    assert_eq!(
        lifecycle.get_collateral_score_batch(&Vec::from_array(&env, [phantom])).len(),
        0,
    );
}

// ---------------------------------------------------------------------------
// Error: unauthorized engineer
// ---------------------------------------------------------------------------
#[test]
fn test_unauthorized_engineer_errors() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, registry, eng_reg, _lending, _aa, _admin, _engineer, issuer, _tok) =
        setup_portfolio_env(&env);

    let owner = Address::generate(&env);
    let asset_id = register_asset(&env, &registry, symbol_short!("GENSET"), &owner);
    let rogue = Address::generate(&env);

    // Unregistered
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lifecycle.submit_maintenance(&asset_id, &symbol_short!("ENGINE"), &String::from_str(&env, "Rogue"), &rogue, &None);
    })).is_err());

    // Registered but unauthorized
    let hash = BytesN::from_array(&env, &[9u8; 32]);
    eng_reg.register_engineer(&rogue, &hash, &issuer, &31_536_000, &None);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lifecycle.submit_maintenance(&asset_id, &symbol_short!("ENGINE"), &String::from_str(&env, "Unauth"), &rogue, &None);
    })).is_err());

    // Authorized → Ok
    lifecycle.authorize_engineer(&owner, &asset_id, &rogue);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lifecycle.submit_maintenance(&asset_id, &symbol_short!("ENGINE"), &String::from_str(&env, "Auth"), &rogue, &None);
    })).is_ok());
}

// ---------------------------------------------------------------------------
// Error: paused contract
// ---------------------------------------------------------------------------
#[test]
fn test_paused_contract_errors() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, registry, _eng_reg, _lending, _aa, admin, engineer, _issuer, _tok) =
        setup_portfolio_env(&env);

    let owner = Address::generate(&env);
    let asset_id = register_asset(&env, &registry, symbol_short!("GENSET"), &owner);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    lifecycle.pause(&admin);
    assert!(lifecycle.is_paused());

    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lifecycle.submit_maintenance(&asset_id, &symbol_short!("ENGINE"), &String::from_str(&env, "Paused"), &engineer, &None);
    })).is_err());

    lifecycle.unpause(&admin);
    assert!(!lifecycle.is_paused());
    lifecycle.submit_maintenance(&asset_id, &symbol_short!("ENGINE"), &String::from_str(&env, "Resumed"), &engineer, &None);
    assert_eq!(lifecycle.get_collateral_score(&asset_id), 5);
}

// ---------------------------------------------------------------------------
// Batch eligibility with mixed scores
// ---------------------------------------------------------------------------
#[test]
fn test_batch_eligibility_mixed() {
    let env = Env::default();
    let (lifecycle, registry, _eng_reg, _lending, _aa, _admin, engineer, _issuer, _tok) =
        setup_portfolio_env(&env);

    let owner = Address::generate(&env);
    let id_low = register_asset(&env, &registry, symbol_short!("SOLAR"), &owner);
    let id_high = register_asset(&env, &registry, symbol_short!("GENSET"), &owner);
    // 10 tasks × 5 = 50 → eligible
    authorize_and_maintain(&env, &lifecycle, &registry, id_high, &owner, &engineer, symbol_short!("ENGINE"), 10);

    let ids = Vec::from_array(&env, [id_low, id_high]);
    let eligible = lifecycle.batch_is_collateral_eligible(&ids);
    assert!(!eligible.get(0).unwrap());
    assert!(eligible.get(1).unwrap());
    assert!(!lifecycle.is_collateral_eligible(&id_low));
    assert!(lifecycle.is_collateral_eligible(&id_high));
}

// ---------------------------------------------------------------------------
// Multiple engineers
// ---------------------------------------------------------------------------
#[test]
fn test_multi_engineer_portfolio() {
    let env = Env::default();
    env.mock_all_auths();
    let (lifecycle, registry, eng_reg, _lending, _aa, admin, _eng, issuer, _tok) =
        setup_portfolio_env(&env);

    let owner = Address::generate(&env);
    let eng1 = Address::generate(&env);
    let eng2 = Address::generate(&env);

    let hash = BytesN::from_array(&env, &[1u8; 32]);
    eng_reg.register_engineer(&eng1, &hash, &issuer, &31_536_000, &None);
    eng_reg.update_reputation(&eng1, &500);
    let hash2 = BytesN::from_array(&env, &[2u8; 32]);
    eng_reg.register_engineer(&eng2, &hash2, &issuer, &31_536_000, &None);
    eng_reg.update_reputation(&eng2, &500);

    let id1 = register_asset(&env, &registry, symbol_short!("GENSET"), &owner);
    let id2 = register_asset(&env, &registry, symbol_short!("SOLAR"), &owner);

    lifecycle.authorize_engineer(&owner, &id1, &eng1);
    lifecycle.submit_maintenance(&id1, &symbol_short!("ENGINE"), &String::from_str(&env, "E1"), &eng1, &None);

    lifecycle.authorize_engineer(&owner, &id2, &eng2);
    lifecycle.submit_maintenance(&id2, &symbol_short!("ENGINE"), &String::from_str(&env, "E2"), &eng2, &None);

    assert_eq!(lifecycle.get_collateral_score(&id1), 5);
    assert_eq!(lifecycle.get_collateral_score(&id2), 5);

    let h1 = lifecycle.get_engineer_maintenance_history(&eng1);
    let h2 = lifecycle.get_engineer_maintenance_history(&eng2);
    assert_eq!(h1.len(), 1);
    assert_eq!(h1.get(0).unwrap(), id1);
    assert_eq!(h2.len(), 1);
    assert_eq!(h2.get(0).unwrap(), id2);
}

// ---------------------------------------------------------------------------
// Score boundaries: 0, floor, threshold 50, cap 100
// ---------------------------------------------------------------------------
#[test]
fn test_score_boundary_values() {
    let env = Env::default();
    let (lifecycle, registry, _eng_reg, _lending, _aa, _admin, engineer, _issuer, _tok) =
        setup_portfolio_env(&env);

    let owner = Address::generate(&env);

    // Score 0
    let id_zero = register_asset(&env, &registry, symbol_short!("SOLAR"), &owner);

    // Score floor (1): single maintenance, deep decay
    let id_one = register_asset(&env, &registry, symbol_short!("WIND"), &owner);
    lifecycle.authorize_engineer(&owner, &id_one, &engineer);
    lifecycle.submit_maintenance(&id_one, &symbol_short!("INSPECT"), &String::from_str(&env, "Minor"), &engineer, &None);
    env.ledger().set_timestamp(env.ledger().timestamp() + 2_592_000 * 10);

    // Score 50: threshold
    let id_fifty = register_asset(&env, &registry, symbol_short!("GENSET"), &owner);
    authorize_and_maintain(&env, &lifecycle, &registry, id_fifty, &owner, &engineer, symbol_short!("ENGINE"), 10);

    // Score 100: capped (20 tasks × 5 = 100)
    let id_max = register_asset(&env, &registry, symbol_short!("HYDRO"), &owner);
    authorize_and_maintain(&env, &lifecycle, &registry, id_max, &owner, &engineer, symbol_short!("ENGINE"), 20);

    assert_eq!(lifecycle.get_collateral_score(&id_zero), 0);
    let s1 = lifecycle.get_collateral_score(&id_one);
    assert!(s1 >= 1 && s1 <= 5, "decayed score should be near 1, got {s1}");
    assert_eq!(lifecycle.get_collateral_score(&id_fifty), 50);
    assert_eq!(lifecycle.get_collateral_score(&id_max), 100);

    assert!(!lifecycle.is_collateral_eligible(&id_zero));
    assert!(lifecycle.is_collateral_eligible(&id_fifty));
    assert!(lifecycle.is_collateral_eligible(&id_max));
}
