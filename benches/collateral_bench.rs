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
);
criterion_main!(collateral_benches);
