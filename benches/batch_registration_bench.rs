//! Criterion benchmarks for `batch_register_assets` with varying batch sizes.
//!
//! Tests batch sizes: 1, 10, 25, 50. Each batch size represents realistic
//! maintenance deployment scenarios for industrial equipment fleets.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _},
    Address, Env, String,
};


fn bench_batch_register_10(c: &mut Criterion) {
    c.bench_function("batch_register_assets/size_10", |b| {
        b.iter(|| {
            let env = Env::default();
            env.mock_all_auths();
            let registry = setup(&env);
            let owner = Address::generate(&env);
            let mut assets: soroban_sdk::Vec<AssetInput> = soroban_sdk::Vec::new(&env);
            for i in 0..10u32 {
                assets.push_back(AssetInput {
                    asset_type: symbol_short!("BENCH"),
                    metadata: String::from_str(&env, &format!("asset {}", i)),
                    serial_number: String::from_str(&env, &format!("SN-{:03}", i)),
                });
            }
            black_box(registry.batch_register_assets(&owner, &assets));
        });
    });
}

fn bench_batch_register_25(c: &mut Criterion) {
    c.bench_function("batch_register_assets/size_25", |b| {
        b.iter(|| {
            let env = Env::default();
            env.mock_all_auths();
            let registry = setup(&env);
            let owner = Address::generate(&env);
            let mut assets: soroban_sdk::Vec<AssetInput> = soroban_sdk::Vec::new(&env);
            for i in 0..25u32 {
                assets.push_back(AssetInput {
                    asset_type: symbol_short!("BENCH"),
                    metadata: String::from_str(&env, &format!("asset {}", i)),
                    serial_number: String::from_str(&env, &format!("SN-{:03}", i)),
                });
            }
            black_box(registry.batch_register_assets(&owner, &assets));
        });
    });
}

fn bench_batch_register_50(c: &mut Criterion) {
    c.bench_function("batch_register_assets/size_50", |b| {
        b.iter(|| {
            let env = Env::default();
            env.mock_all_auths();
            let registry = setup(&env);
            let owner = Address::generate(&env);
            let mut assets: soroban_sdk::Vec<AssetInput> = soroban_sdk::Vec::new(&env);
            for i in 0..50u32 {
                assets.push_back(AssetInput {
                    asset_type: symbol_short!("BENCH"),
                    metadata: String::from_str(&env, &format!("asset {}", i)),
                    serial_number: String::from_str(&env, &format!("SN-{:03}", i)),
                });
            }
            black_box(registry.batch_register_assets(&owner, &assets));
        });
    });
}

criterion_group!(
    batch_benches,
    bench_batch_register_1,
    bench_batch_register_10,
    bench_batch_register_25,
    bench_batch_register_50,
);
criterion_main!(batch_benches);
