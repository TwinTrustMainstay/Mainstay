//! Criterion benchmarks for `get_collateral_score` with varying
//! maintenance history sizes.
//!
//! Methodology: For each history size (10, 100, 1000 records), we submit that
//! many maintenance records and then time how long `get_collateral_score` takes.
//! This exposes the algorithmic cost scaling relative to history length.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, String,
};

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{Lifecycle, LifecycleClient};

/// Bootstrap the full contract stack and return a fully-wired lifecycle client
/// plus the asset id of a pre-registered asset.
fn setup(env: &Env, history_size: u32) -> (LifecycleClient, u64) {
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(env, &lifecycle_id);

    let admin = Address::generate(env);
    let owner = Address::generate(env);
    let engineer = Address::generate(env);
    let issuer = Address::generate(env);

    asset_registry.initialize_admin(&admin, &admin);
    asset_registry.add_asset_type(&admin, &symbol_short!("BENCH"));
    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);
    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &(history_size.max(200)),
    );

    let asset_id = asset_registry.register_asset(
        &symbol_short!("BENCH"),
        &String::from_str(env, "Benchmark asset"),
        &String::from_str(env, "SN-BENCH-001"),
        &owner,
    );

    let credential_hash = BytesN::from_array(env, &[1u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // Pre-populate maintenance history
    for i in 0..history_size {
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &String::from_str(env, &format!("bench record {}", i)),
            &engineer,
        );
        // Bump timestamp to avoid dedup
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 1);
    }

    (lifecycle, asset_id)
}

/// Bootstrap the full contract stack at the default `max_history` (200) and
/// pre-populate exactly 200 maintenance records — the maximum history size —
/// of which most are additionally flagged as duplicates.
///
/// After submission the ledger is advanced well beyond `MAX_AGE_LEDGERS` so
/// every recency-weight contribution collapses to zero. This matters because
/// `compute_decay` short-circuits as soon as the accumulated score reaches the
/// `MAX_COLLATERAL_SCORE` cap. By ageing all records out we force the scan to
/// visit every record and, for each one, walk the populated duplicate list —
/// the true O(n*m) worst case this benchmark is meant to cover.
fn setup_max_history_with_duplicates(env: &Env) -> (LifecycleClient, u64) {
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
    let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
    let lifecycle = LifecycleClient::new(env, &lifecycle_id);

    let admin = Address::generate(env);
    let owner = Address::generate(env);
    let engineer = Address::generate(env);
    let issuer = Address::generate(env);

    asset_registry.initialize_admin(&admin, &admin);
    asset_registry.add_asset_type(&admin, &symbol_short!("BENCH"));
    engineer_registry.initialize_admin(&admin, &admin);
    engineer_registry.add_trusted_issuer(&admin, &issuer);
    // 200 == DEFAULT_MAX_HISTORY: exercise the worst-case history size.
    lifecycle.initialize(&admin, &asset_registry_id, &engineer_registry_id, &admin, &200);

    let asset_id = asset_registry.register_asset(
        &symbol_short!("BENCH"),
        &String::from_str(env, "Benchmark asset"),
        &String::from_str(env, "SN-BENCH-001"),
        &owner,
    );

    let credential_hash = BytesN::from_array(env, &[1u8; 32]);
    engineer_registry.register_engineer(&engineer, &credential_hash, &issuer, &31_536_000);
    lifecycle.authorize_engineer(&owner, &asset_id, &engineer);

    // Pre-populate the maximum number of maintenance records.
    for i in 0..200u32 {
        lifecycle.submit_maintenance(
            &asset_id,
            &symbol_short!("INSPECT"),
            &String::from_str(env, &format!("bench record {}", i)),
            &engineer,
        );
        // Bump timestamp to avoid dedup
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + 1);
    }

    // Populate DuplicateRecords: mark ~3 out of every 4 records as duplicates
    // so the inner duplicate scan in compute_decay runs over a large list.
    let history = lifecycle.get_maintenance_history(&asset_id);
    for (idx, record) in history.iter().enumerate() {
        if idx % 4 != 0 {
            lifecycle.mark_maintenance_as_duplicate(&admin, &asset_id, &0u64, &record.timestamp);
        }
    }

    // Age all records out so the score contribution is zero and the scan cannot
    // short-circuit at the score cap (see doc comment above).
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 2_592_001); // > MAX_AGE_LEDGERS * 5 ≈ 30 days

    (lifecycle, asset_id)
}

fn bench_get_collateral_score_max_history_duplicates(c: &mut Criterion) {
    c.bench_function("get_collateral_score/max_history_200_with_duplicates", |b| {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, asset_id) = setup_max_history_with_duplicates(&env);
        b.iter(|| {
            black_box(lifecycle.get_collateral_score(&asset_id));
        });
    });
}

fn bench_get_collateral_score_10(c: &mut Criterion) {
    c.bench_function("get_collateral_score/10_records", |b| {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, asset_id) = setup(&env, 10);
        b.iter(|| {
            black_box(lifecycle.get_collateral_score(&asset_id));
        });
    });
}

fn bench_get_collateral_score_100(c: &mut Criterion) {
    c.bench_function("get_collateral_score/100_records", |b| {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, asset_id) = setup(&env, 100);
        b.iter(|| {
            black_box(lifecycle.get_collateral_score(&asset_id));
        });
    });
}

fn bench_get_collateral_score_1000(c: &mut Criterion) {
    c.bench_function("get_collateral_score/1000_records", |b| {
        let env = Env::default();
        env.mock_all_auths();
        let (lifecycle, asset_id) = setup(&env, 1000);
        b.iter(|| {
            black_box(lifecycle.get_collateral_score(&asset_id));
        });
    });
}

criterion_group!(
    collateral_benches,
    bench_get_collateral_score_10,
    bench_get_collateral_score_100,
    bench_get_collateral_score_1000,
    bench_get_collateral_score_max_history_duplicates,
);
criterion_main!(collateral_benches);
