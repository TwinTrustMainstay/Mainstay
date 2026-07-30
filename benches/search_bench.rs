//! Criterion benchmarks for `search_assets` with large result sets.
//!
//! Registers 1000 assets of various types and benchmarks search queries
//! that return progressively larger result sets.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _},
    Address, Env, String,
};

use asset_registry::{
    AssetInput, AssetRegistry, AssetRegistryClient, SearchFilter, SearchPage,
};

/// Build a registry with `n` assets evenly distributed across 5 asset types.
fn setup_large_dataset(env: &Env, n: u32) -> AssetRegistryClient {
    let admin = Address::generate(env);
    let owner = Address::generate(env);
    let asset_registry_id = env.register(AssetRegistry, ());
    let registry = AssetRegistryClient::new(env, &asset_registry_id);

    registry.initialize_admin(&admin, &admin);

    let types = [
        symbol_short!("GENSET"),
        symbol_short!("TURBINE"),
        symbol_short!("COMPR"),
        symbol_short!("PUMP"),
        symbol_short!("MOTOR"),
    ];

    for t in &types {
        registry.add_asset_type(&admin, t);
    }

    let mut assets: soroban_sdk::Vec<AssetInput> = soroban_sdk::Vec::new(env);
    for i in 0..n {
        let t = &types[(i % 5) as usize];
        assets.push_back(AssetInput {
            asset_type: t.clone(),
            metadata: String::from_str(env, &format!("search bench asset {}", i)),
            serial_number: String::from_str(env, &format!("SN-SRCH-{:05}", i)),
        });

        if assets.len() >= 50 {
            registry.batch_register_assets(&owner, &assets);
            assets = soroban_sdk::Vec::new(env);
        }
    }
    if !assets.is_empty() {
        registry.batch_register_assets(&owner, &assets);
    }

    registry
}

fn bench_search_all_types(c: &mut Criterion) {
    c.bench_function("search_assets/all_1000_no_filter", |b| {
        let env = Env::default();
        env.mock_all_auths();
        let registry = setup_large_dataset(&env, 1000);
        let filter = SearchFilter {
            asset_type: None,
            manufacturer: None,
            min_age_months: None,
            max_age_months: None,
            sort: None,
            lifecycle_contract: None,
        };
        b.iter(|| {
            black_box(registry.search_assets(&filter));
        });
    });
}

fn bench_search_single_type(c: &mut Criterion) {
    c.bench_function("search_assets/single_type_200", |b| {
        let env = Env::default();
        env.mock_all_auths();
        let registry = setup_large_dataset(&env, 1000);
        let filter = SearchFilter {
            asset_type: Some(symbol_short!("GENSET")),
            manufacturer: None,
            min_age_months: None,
            max_age_months: None,
            sort: None,
            lifecycle_contract: None,
        };
        b.iter(|| {
            black_box(registry.search_assets(&filter));
        });
    });
}

fn bench_search_by_owner(c: &mut Criterion) {
    c.bench_function("get_assets_by_owner/1000", |b| {
        let env = Env::default();
        env.mock_all_auths();
        let registry = setup_large_dataset(&env, 1000);
        let owner = Address::generate(&env);
        // Register one batch as this owner
        let mut assets: soroban_sdk::Vec<AssetInput> = soroban_sdk::Vec::new(&env);
        for i in 0..50u32 {
            assets.push_back(AssetInput {
                asset_type: symbol_short!("GENSET"),
                metadata: String::from_str(&env, &format!("owner-search-{}", i)),
                serial_number: String::from_str(&env, &format!("SN-OWN-{:05}", i)),
            });
        }
        registry.batch_register_assets(&owner, &assets);
        b.iter(|| {
            black_box(registry.get_assets_by_owner(&owner));
        });
    });
}

criterion_group!(
    search_benches,
    bench_search_all_types,
    bench_search_single_type,
    bench_search_by_owner,
);
criterion_main!(search_benches);
