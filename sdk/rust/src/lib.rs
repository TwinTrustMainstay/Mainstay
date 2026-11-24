//! Typed client helpers for Mainstay Soroban contracts.

use asset_registry_bindings::AssetRegistryBindings;
use engineer_registry_bindings::EngineerRegistryBindings;
use lending_bindings::LendingBindings;
use lifecycle_bindings::LifecycleBindings;
use soroban_sdk::{contractclient, contracttype, Address, BytesN, Env, String, Symbol, Vec};

mod asset_registry_bindings {
    use super::*;

    #[contracttype]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum DeprecationStatus {
        Active = 0,
        Deprecated = 1,
        Decommissioned = 2,
    }

    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Asset {
        pub asset_id: u64,
        pub asset_type: Symbol,
        pub metadata: String,
        pub serial_number: String,
        pub owner: Address,
        pub registered_at: u64,
        pub metadata_updated_at: u64,
        pub metadata_version: u32,
        pub deprecation_status: DeprecationStatus,
        pub is_locked: bool,
        pub lender: Option<Address>,
        pub loan_id: Option<u64>,
        pub deprecated_at: Option<u64>,
        pub co_owners: Vec<(Address, u32)>,
    }

    #[contracttype]
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub enum SortOrder {
        ByCollateralScore = 0,
        ByMaintenanceDate = 1,
    }

    #[contracttype]
    #[derive(Clone, Debug)]
    pub struct SearchFilter {
        pub asset_type: Option<Symbol>,
        pub manufacturer: Option<String>,
        pub min_age_months: Option<u32>,
        pub max_age_months: Option<u32>,
        pub sort: Option<SortOrder>,
        pub lifecycle_contract: Option<Address>,
    }

    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct FacetCount {
        pub asset_type: Symbol,
        pub count: u32,
    }

    #[contracttype]
    #[derive(Clone, Debug)]
    pub struct SearchPage {
        pub assets: Vec<Asset>,
        pub total: u32,
        pub facets: Vec<FacetCount>,
    }

    #[allow(dead_code)]
    #[contractclient(name = "AssetRegistryBindings")]
    pub trait AssetRegistry {
        fn register_asset(
            env: Env,
            asset_type: Symbol,
            metadata: String,
            serial_number: String,
            owner: Address,
        ) -> u64;
        fn get_asset(env: Env, asset_id: u64) -> Asset;
        fn search_assets(env: Env, filter: SearchFilter) -> SearchPage;
    }
}

mod engineer_registry_bindings {
    use super::*;

    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum CredentialStatus {
        Valid = 0,
        GracePeriod = 1,
        HardExpired = 2,
        Revoked = 3,
        NotFound = 4,
        Suspended = 5,
        Expired = 6,
    }

    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum EngineerStatus {
        Active = 0,
        Revoked = 1,
        Expired = 2,
        NotFound = 3,
        Suspended = 4,
    }

    #[allow(dead_code)]
    #[contractclient(name = "EngineerRegistryBindings")]
    pub trait EngineerRegistry {
        fn register_engineer(
            env: Env,
            engineer: Address,
            credential_hash: BytesN<32>,
            issuer: Address,
            validity_period: u64,
            notes: Option<String>,
        );
        fn verify_engineer(
            env: Env,
            engineer: Address,
            required_specialization: Option<Symbol>,
        ) -> CredentialStatus;
        fn get_engineer_status(env: Env, engineer: Address) -> EngineerStatus;
        fn get_credential_status(env: Env, engineer: Address) -> CredentialStatus;
        fn revoke_credential(env: Env, caller: Address, engineer: Address);
    }
}

mod lifecycle_bindings {
    use super::*;

    #[contracttype]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Priority {
        Low = 0,
        Medium = 1,
        High = 2,
        Critical = 3,
    }

    #[allow(dead_code)]
    #[contractclient(name = "LifecycleBindings")]
    pub trait Lifecycle {
        fn submit_maintenance(
            env: Env,
            asset_id: u64,
            task_type: Symbol,
            priority: Priority,
            notes: String,
            engineer: Address,
            cost: Option<u64>,
            fee: u64,
        );
        fn get_collateral_score(env: Env, asset_id: u64) -> u32;
        fn is_collateral_eligible(env: Env, asset_id: u64) -> bool;
        fn get_min_collateral_score(env: Env) -> u32;
    }
}

mod lending_bindings {
    use super::*;

    #[contracttype]
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub enum LoanStatus {
        Active = 0,
        Repaid = 1,
        Defaulted = 2,
        None = 3,
    }

    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Loan {
        pub borrower: Address,
        pub amount: u64,
        pub status: LoanStatus,
        pub deadline: u64,
        pub id: u64,
    }

    #[allow(dead_code)]
    #[contractclient(name = "LendingBindings")]
    pub trait Lending {
        fn request_loan(env: Env, borrower: Address, amount: u64, asset_id: u64);
        fn repay(env: Env, borrower: Address);
        fn get_loan(env: Env, borrower: Address) -> Option<Loan>;
        fn get_collateral_value(env: Env, asset_base_value: u64, score: u32, ltv_bps: u32) -> u64;
    }
}

pub use asset_registry_bindings::AssetRegistryBindings as AssetRegistryClient;
pub use asset_registry_bindings::{
    Asset, DeprecationStatus, FacetCount, SearchFilter, SearchPage, SortOrder,
};
pub use engineer_registry_bindings::{CredentialStatus, EngineerStatus};
pub use lending_bindings::{Loan, LoanStatus};
pub use lifecycle_bindings::Priority;

/// Convenient typed facade over a deployed Asset Registry contract.
pub struct MainstayClient<'a> {
    registry: AssetRegistryBindings<'a>,
}

impl<'a> MainstayClient<'a> {
    /// Construct a client for an already deployed Asset Registry contract.
    pub fn new(env: &'a Env, contract_id: &'a Address) -> Self {
        Self {
            registry: AssetRegistryBindings::new(env, contract_id),
        }
    }

    /// Register an asset using the contract's canonical argument order.
    pub fn register_asset(
        &self,
        asset_type: &Symbol,
        metadata: &String,
        serial_number: &String,
        owner: &Address,
    ) -> u64 {
        self.registry
            .register_asset(asset_type, metadata, serial_number, owner)
    }

    /// Retrieve an asset by its on-chain ID.
    pub fn get_asset(&self, asset_id: &u64) -> Asset {
        self.registry.get_asset(asset_id)
    }

    /// Search assets and return type facets calculated on-chain.
    pub fn search_assets(&self, filter: &SearchFilter) -> SearchPage {
        self.registry.search_assets(filter)
    }
}

/// Typed client for the engineer credential registry.
pub struct EngineerRegistryClient<'a> {
    inner: EngineerRegistryBindings<'a>,
}

impl<'a> EngineerRegistryClient<'a> {
    pub fn new(env: &'a Env, contract_id: &'a Address) -> Self {
        Self {
            inner: EngineerRegistryBindings::new(env, contract_id),
        }
    }

    pub fn register_engineer(
        &self,
        engineer: &Address,
        credential_hash: &BytesN<32>,
        issuer: &Address,
        validity_period: &u64,
        notes: &Option<String>,
    ) {
        self.inner
            .register_engineer(engineer, credential_hash, issuer, validity_period, notes)
    }

    pub fn verify_engineer(
        &self,
        engineer: &Address,
        required_specialization: &Option<Symbol>,
    ) -> CredentialStatus {
        self.inner
            .verify_engineer(engineer, required_specialization)
    }

    pub fn get_engineer_status(&self, engineer: &Address) -> EngineerStatus {
        self.inner.get_engineer_status(engineer)
    }

    pub fn get_credential_status(&self, engineer: &Address) -> CredentialStatus {
        self.inner.get_credential_status(engineer)
    }

    pub fn revoke_credential(&self, caller: &Address, engineer: &Address) {
        self.inner.revoke_credential(caller, engineer)
    }
}

/// Typed client for maintenance submission and lifecycle collateral scoring.
pub struct LifecycleClient<'a> {
    inner: LifecycleBindings<'a>,
}

impl<'a> LifecycleClient<'a> {
    pub fn new(env: &'a Env, contract_id: &'a Address) -> Self {
        Self {
            inner: LifecycleBindings::new(env, contract_id),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn submit_maintenance(
        &self,
        asset_id: &u64,
        task_type: &Symbol,
        priority: &Priority,
        notes: &String,
        engineer: &Address,
        cost: &Option<u64>,
        fee: &u64,
    ) {
        self.inner
            .submit_maintenance(asset_id, task_type, priority, notes, engineer, cost, fee)
    }

    pub fn get_collateral_score(&self, asset_id: &u64) -> u32 {
        self.inner.get_collateral_score(asset_id)
    }

    pub fn is_collateral_eligible(&self, asset_id: &u64) -> bool {
        self.inner.is_collateral_eligible(asset_id)
    }

    pub fn get_min_collateral_score(&self) -> u32 {
        self.inner.get_min_collateral_score()
    }
}

/// Typed client for loan requests, repayments, and collateral calculations.
pub struct LendingClient<'a> {
    inner: LendingBindings<'a>,
}

impl<'a> LendingClient<'a> {
    pub fn new(env: &'a Env, contract_id: &'a Address) -> Self {
        Self {
            inner: LendingBindings::new(env, contract_id),
        }
    }

    pub fn request_loan(&self, borrower: &Address, amount: &u64, asset_id: &u64) {
        self.inner.request_loan(borrower, amount, asset_id)
    }

    pub fn repay(&self, borrower: &Address) {
        self.inner.repay(borrower)
    }

    pub fn get_loan(&self, borrower: &Address) -> Option<Loan> {
        self.inner.get_loan(borrower)
    }

    pub fn get_collateral_value(&self, asset_base_value: &u64, score: &u32, ltv_bps: &u32) -> u64 {
        self.inner
            .get_collateral_value(asset_base_value, score, ltv_bps)
    }
}
