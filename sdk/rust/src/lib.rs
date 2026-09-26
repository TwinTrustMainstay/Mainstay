//! Typed client helpers for the Mainstay Asset Registry contract.

pub use asset_registry::{
    Asset, AssetRegistryClient, FacetCount, SearchFilter, SearchPage, SortOrder,
};
use soroban_sdk::{Address, Env, String, Symbol};

/// A convenient, typed facade over the generated Soroban client.
pub struct MainstayClient<'a> {
    registry: AssetRegistryClient<'a>,
}

impl<'a> MainstayClient<'a> {
    /// Construct a client for an already deployed Asset Registry contract.
    pub fn new(env: &'a Env, contract_id: &'a Address) -> Self {
        Self { registry: AssetRegistryClient::new(env, contract_id) }
    }

    /// Register an asset using the contract's canonical argument order.
    pub fn register_asset(
        &self,
        asset_type: &Symbol,
        metadata: &String,
        serial_number: &String,
        owner: &Address,
    ) -> u64 {
        self.registry.register_asset(asset_type, metadata, serial_number, owner)
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
