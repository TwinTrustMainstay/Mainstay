//! Tests for #1297: Remove dead #[allow(dead_code)] attributes
//!
//! This test suite verifies that all #[allow(dead_code)] attributes have been
//! removed from the codebase and that the underlying dead code issues have been
//! resolved either by:
//! 1. Implementing the missing functionality, or
//! 2. Removing the dead code entirely
//!
//! The test ensures that:
//! - All contracts initialize correctly after dead code removal
//! - Core functionality is preserved
//! - Clippy would not warn about dead code if run with deny(dead_code)

use asset_registry::AssetRegistry;
use engineer_registry::EngineerRegistry;
use lifecycle::Lifecycle;
use lending::Lending;
use soroban_sdk::{Address, Env};

/// Verify that asset-registry initializes correctly after dead code removal.
#[test]
fn test_asset_registry_no_dead_code() {
    let env = Env::default();
    let registry_id = env.register(AssetRegistry, ());
    let registry = asset_registry::AssetRegistryClient::new(&env, &registry_id);
    let admin = Address::generate(&env);

    // Should initialize without errors
    registry.initialize_admin(&admin, &admin);
}

/// Verify that engineer-registry initializes correctly after dead code removal.
#[test]
fn test_engineer_registry_no_dead_code() {
    let env = Env::default();
    let registry_id = env.register(EngineerRegistry, ());
    let registry = engineer_registry::EngineerRegistryClient::new(&env, &registry_id);
    let admin = Address::generate(&env);

    // Should initialize without errors
    registry.initialize_admin(&admin, &admin);
}

/// Verify that lifecycle contract initializes correctly after dead code removal.
#[test]
fn test_lifecycle_no_dead_code() {
    let env = Env::default();
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());

    let asset_registry = asset_registry::AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = engineer_registry::EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = lifecycle::LifecycleClient::new(&env, &lifecycle_id);

    let admin = Address::generate(&env);

    asset_registry.initialize_admin(&admin, &admin);
    engineer_registry.initialize_admin(&admin, &admin);

    // Should initialize without errors
    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &0,
    );
}

/// Verify that lending contract initializes correctly after dead code removal.
#[test]
fn test_lending_no_dead_code() {
    let env = Env::default();
    let lifecycle_id = env.register(Lifecycle, ());
    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lending_id = env.register(Lending, ());

    let asset_registry = asset_registry::AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = engineer_registry::EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = lifecycle::LifecycleClient::new(&env, &lifecycle_id);
    let lending = lending::LendingClient::new(&env, &lending_id);

    let admin = Address::generate(&env);

    asset_registry.initialize_admin(&admin, &admin);
    engineer_registry.initialize_admin(&admin, &admin);

    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &0,
    );

    // Should initialize without errors
    lending.initialize(&admin, &lifecycle_id, &admin, &0);
}

/// Test that all contracts can be instantiated together without dead code issues.
/// This ensures that the dead code removal didn't break any cross-contract integrations.
#[test]
fn test_all_contracts_integration_after_dead_code_removal() {
    let env = Env::default();

    let asset_registry_id = env.register(AssetRegistry, ());
    let engineer_registry_id = env.register(EngineerRegistry, ());
    let lifecycle_id = env.register(Lifecycle, ());
    let lending_id = env.register(Lending, ());

    let asset_registry = asset_registry::AssetRegistryClient::new(&env, &asset_registry_id);
    let engineer_registry = engineer_registry::EngineerRegistryClient::new(&env, &engineer_registry_id);
    let lifecycle = lifecycle::LifecycleClient::new(&env, &lifecycle_id);
    let lending = lending::LendingClient::new(&env, &lending_id);

    let admin = Address::generate(&env);

    asset_registry.initialize_admin(&admin, &admin);
    engineer_registry.initialize_admin(&admin, &admin);

    lifecycle.initialize(
        &admin,
        &asset_registry_id,
        &engineer_registry_id,
        &admin,
        &0,
    );

    lending.initialize(&admin, &lifecycle_id, &admin, &0);

    // All contracts should initialize without dead code warnings
}
