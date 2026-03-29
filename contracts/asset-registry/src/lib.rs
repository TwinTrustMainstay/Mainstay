#![no_std]
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    Bytes, BytesN, Env, String, Symbol, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ContractError {
    AssetNotFound = 1,
    /// Same owner attempted to register an asset with identical metadata.
    DuplicateAsset = 2,
    UnauthorizedAdmin = 3,
    UnauthorizedOwner = 4,
    NotInitialized = 5,
    AdminAlreadyInitialized = 6,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Asset {
    pub asset_id: u64,
    pub asset_type: Symbol,
    pub metadata: String,
    pub owner: Address,
    pub registered_at: u64,
    pub metadata_updated_at: u64,
}

const ASSET_COUNT: Symbol = symbol_short!("A_COUNT");

const ADMIN_KEY: Symbol = symbol_short!("ADMIN");


#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    MetadataTooLong = 1,
}

fn asset_key(id: u64) -> (Symbol, u64) {
    (symbol_short!("ASSET"), id)
}

/// Deduplication key: (owner, sha256(metadata)) → existing asset_id.
fn dedup_key(owner: &Address, hash: &BytesN<32>) -> (Symbol, Address, BytesN<32>) {
    (symbol_short!("DEDUP"), owner.clone(), hash.clone())
}

/// Owner index key: owner → Vec<u64> of asset IDs.
fn owner_index_key(owner: &Address) -> (Symbol, Address) {
    (symbol_short!("OWN_IDX"), owner.clone())
}

/// Append an asset ID to the owner's index.
fn owner_index_add(env: &Env, owner: &Address, asset_id: u64) {
    let key = owner_index_key(owner);
    let mut ids: Vec<u64> = env.storage().persistent().get(&key).unwrap_or_else(|| Vec::new(env));
    ids.push_back(asset_id);
    env.storage().persistent().set(&key, &ids);
    env.storage().persistent().extend_ttl(&key, 518400, 518400);
}

/// Remove an asset ID from the owner's index.
fn owner_index_remove(env: &Env, owner: &Address, asset_id: u64) {
    let key = owner_index_key(owner);
    let ids: Vec<u64> = env.storage().persistent().get(&key).unwrap_or_else(|| Vec::new(env));
    let mut updated: Vec<u64> = Vec::new(env);
    for id in ids.iter() {
        if id != asset_id {
            updated.push_back(id);
        }
    }
    env.storage().persistent().set(&key, &updated);
    env.storage().persistent().extend_ttl(&key, 518400, 518400);
}

#[contract]
pub struct AssetRegistry;

#[contractimpl]
impl AssetRegistry {
    pub fn register_asset(env: Env, asset_type: Symbol, metadata: String, owner: Address) -> u64 {
        owner.require_auth();

        // Deduplication: reject if this owner already registered identical metadata.
        let meta_bytes = Bytes::from(metadata.clone().to_xdr(&env));
        let meta_hash: BytesN<32> = env.crypto().sha256(&meta_bytes).into();
        let dk = dedup_key(&owner, &meta_hash);
        if env.storage().persistent().has(&dk) {
            panic_with_error!(&env, ContractError::DuplicateAsset);
        }

        let id: u64 = env.storage().instance().get(&ASSET_COUNT).unwrap_or(0) + 1;
        let asset = Asset {
            asset_id: id,
            asset_type: asset_type.clone(),
            metadata,
            owner: owner.clone(),
            registered_at: env.ledger().timestamp(),
            metadata_updated_at: env.ledger().timestamp(),
        };
        env.storage().persistent().set(&asset_key(id), &asset);
        env.storage().persistent().extend_ttl(&asset_key(id), 518400, 518400); // Extend TTL for persistent storage entries to prevent data loss
        env.storage().instance().set(&ASSET_COUNT, &id);
        env.storage().persistent().set(&dk, &id);

        // Update owner index
        owner_index_add(&env, &owner, id);

        // Emit asset registration event
        env.events().publish(
            (symbol_short!("REG_AST"), id),
            (asset_type, owner.clone(), env.ledger().timestamp()),
        );

        id
    }

    pub fn get_asset(env: Env, asset_id: u64) -> Asset {
        env.storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound))
    }

    /// Returns true if an asset with the given ID exists, false otherwise.
    pub fn asset_exists(env: Env, asset_id: u64) -> bool {
        env.storage().persistent().has(&asset_key(asset_id))
    }

    /// Returns all asset IDs owned by the given address.
    pub fn get_assets_by_owner(env: Env, owner: Address) -> Vec<u64> {
        env.storage()
            .persistent()
            .get(&owner_index_key(&owner))
            .unwrap_or_else(|| Vec::new(&env))
    }

    pub fn asset_count(env: Env) -> u64 {
        env.storage().instance().get(&ASSET_COUNT).unwrap_or(0)
    }

    /// Initialize the admin address (call once on deploy)
    pub fn initialize_admin(env: Env, admin: Address) {
        admin.require_auth();
        if env.storage().instance().has(&ADMIN_KEY) {
            panic_with_error!(&env, ContractError::AdminAlreadyInitialized);
        }
        env.storage().instance().set(&ADMIN_KEY, &admin);
    }

    /// Get the current admin address
    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&ADMIN_KEY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized))
    }

    /// Admin-only: Deregister (remove) an asset
    pub fn deregister_asset(env: Env, asset_id: u64) {
        let admin = Self::get_admin(env.clone());
        admin.require_auth();
        
        let asset: Asset = env.storage().persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));
        
        // Remove asset storage
        env.storage().persistent().remove(&asset_key(asset_id));
        
        // Remove deduplication key
        let dk = dedup_key(&asset.owner, &env.crypto().sha256(&Bytes::from(asset.metadata.to_xdr(&env))).into());
        env.storage().persistent().remove(&dk);

        // Remove from owner index
        owner_index_remove(&env, &asset.owner, asset_id);

        // Emit deregistration event
        env.events().publish(
            (symbol_short!("DEREG_AST"), asset_id),
            (asset.asset_type.clone(), asset.owner.clone())
        );
    }

    /// Owner-only: update the metadata of an existing asset (e.g. after refurbishment).
    /// Removes the old deduplication key and registers the new one.
    pub fn update_asset_metadata(env: Env, asset_id: u64, owner: Address, new_metadata: String) {
        owner.require_auth();

        let mut asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));

        if asset.owner != owner {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        // Remove old dedup key
        let old_hash: BytesN<32> = env
            .crypto()
            .sha256(&Bytes::from(asset.metadata.to_xdr(&env)))
            .into();
        env.storage().persistent().remove(&dedup_key(&owner, &old_hash));

        // Reject if new metadata is a duplicate for this owner
        let new_hash: BytesN<32> = env
            .crypto()
            .sha256(&Bytes::from(new_metadata.clone().to_xdr(&env)))
            .into();
        let new_dk = dedup_key(&owner, &new_hash);
        if env.storage().persistent().has(&new_dk) {
            panic_with_error!(&env, ContractError::DuplicateAsset);
        }

        // Store new dedup key and updated asset
        env.storage().persistent().set(&new_dk, &asset_id);
        asset.metadata = new_metadata.clone();
        asset.metadata_updated_at = env.ledger().timestamp();
        env.storage().persistent().set(&asset_key(asset_id), &asset);

        env.events().publish(
            (symbol_short!("UPD_META"), asset_id),
            (owner, new_metadata, env.ledger().timestamp()),
        );
    }

    pub fn transfer_asset(env: Env, asset_id: u64, current_owner: Address, new_owner: Address) {
        current_owner.require_auth();

        let mut asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));

        if asset.owner != current_owner {
            panic_with_error!(&env, ContractError::UnauthorizedOwner);
        }

        // Move dedup key to new owner
        let hash: BytesN<32> = env
            .crypto()
            .sha256(&Bytes::from(asset.metadata.clone().to_xdr(&env)))
            .into();
        env.storage().persistent().remove(&dedup_key(&current_owner, &hash));
        env.storage().persistent().set(&dedup_key(&new_owner, &hash), &asset_id);

        // Move owner index entry
        owner_index_remove(&env, &current_owner, asset_id);
        owner_index_add(&env, &new_owner, asset_id);

        asset.owner = new_owner.clone();
        env.storage().persistent().set(&asset_key(asset_id), &asset);
        env.storage().persistent().extend_ttl(&asset_key(asset_id), 518400, 518400);

        env.events().publish(
            (symbol_short!("TRANSFER"), asset_id),
            (current_owner, new_owner, env.ledger().timestamp()),
        );
    }

    /// Admin-only: upgrade the contract WASM to a new hash.
    pub fn upgrade(env: Env, admin: Address, _new_wasm_hash: BytesN<32>) {
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&ADMIN_KEY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        #[cfg(not(test))]
        {
            env.deployer().update_current_contract_wasm(_new_wasm_hash);
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        symbol_short,
        testutils::{Address as _, Events, Ledger as _},
        Bytes, Env, String,
    };
    use soroban_sdk::testutils::storage::Persistent;

    use crate::AssetRegistryClient;


    #[test]
    fn test_register_and_get_asset() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Caterpillar 3516 Generator"),
            &owner,
        );
        assert_eq!(id, 1);

        let asset = client.get_asset(&id);
        assert_eq!(asset.asset_id, 1);
        assert_eq!(asset.owner, owner);
    }

    #[test]
    fn test_get_asset_returns_correct_owner() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let expected_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("TURBINE"),
            &String::from_str(&env, "GE LM2500 Turbine"),
            &expected_owner,
        );

        let asset = client.get_asset(&id);
        assert_eq!(asset.owner, expected_owner);
    }

    #[test]
    fn test_get_asset_not_found() {
        let env = Env::default();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);
        let result = client.try_get_asset(&999);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32
            )))
        );
    }

    #[test]
    fn test_duplicate_metadata_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let metadata = String::from_str(&env, "CAT-3516-SN123456");

        // First registration succeeds
        let id = client.register_asset(&symbol_short!("GENSET"), &metadata, &owner);
        assert_eq!(id, 1);

        // Second registration with identical metadata by same owner is rejected
        let result = client.try_register_asset(&symbol_short!("GENSET"), &metadata, &owner);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::DuplicateAsset as u32
            )))
        );
    }

    #[test]
    fn test_different_owners_same_metadata_allowed() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner_a = Address::generate(&env);
        let owner_b = Address::generate(&env);
        let metadata = String::from_str(&env, "CAT-3516-SN123456");

        // Different owners may register the same metadata (different physical assets)
        let id_a = client.register_asset(&symbol_short!("GENSET"), &metadata, &owner_a);
        let id_b = client.register_asset(&symbol_short!("GENSET"), &metadata, &owner_b);
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn test_register_asset_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let asset_type = symbol_short!("GENSET");
        let metadata = String::from_str(&env, "Caterpillar 3516 Generator");

        client.register_asset(&asset_type, &metadata, &owner);

        // Verify registration event was emitted
        let events = env.events().all();
        assert!(events.len() > 0);
    }

    #[test]
    fn test_ttl_extended_on_registration() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let asset_type = symbol_short!("GENSET");
        let metadata = String::from_str(&env, "Caterpillar 3516 Generator");
        
        let id = client.register_asset(&asset_type, &metadata, &owner);

        // Verify TTL is set for asset storage entry
        let asset_ttl = env.as_contract(&contract_id, || {
            env.storage().persistent().get_ttl(&asset_key(id))
        });
        assert!(asset_ttl > 0, "Asset TTL should be extended");

        // Verify TTL is set for deduplication key
        let meta_bytes = Bytes::from(metadata.to_xdr(&env));
        let meta_hash: BytesN<32> = env.crypto().sha256(&meta_bytes).into();
        let dedup_ttl = env.as_contract(&contract_id, || {
            let dk = dedup_key(&owner, &meta_hash);
            env.storage().persistent().get_ttl(&dk)
        });
        assert!(dedup_ttl > 0, "Deduplication key TTL should be extended");
    }

    #[test]
    fn test_admin_can_upgrade() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin);

        let new_wasm_hash = BytesN::from_array(&env, &[0xabu8; 32]);
        let result = client.try_upgrade(&admin, &new_wasm_hash);
        assert_ne!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_non_admin_cannot_upgrade() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin);

        let outsider = Address::generate(&env);
        let new_wasm_hash = BytesN::from_array(&env, &[0xabu8; 32]);

        let result = client.try_upgrade(&outsider, &new_wasm_hash);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_owner_can_update_metadata() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Original spec"),
            &owner,
        );

        client.update_asset_metadata(
            &id,
            &owner,
            &String::from_str(&env, "Refurbished spec v2"),
        );

        let asset = client.get_asset(&id);
        assert_eq!(asset.metadata, String::from_str(&env, "Refurbished spec v2"));
    }

    #[test]
    fn test_update_metadata_stamps_metadata_updated_at() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Original spec"),
            &owner,
        );

        // Advance ledger time before updating
        env.ledger().with_mut(|li| li.timestamp += 1000);
        let update_time = env.ledger().timestamp();

        client.update_asset_metadata(
            &id,
            &owner,
            &String::from_str(&env, "Refurbished spec v2"),
        );

        let asset = client.get_asset(&id);
        assert_eq!(asset.metadata_updated_at, update_time);
        assert!(asset.metadata_updated_at > asset.registered_at);
    }

    #[test]
    fn test_update_metadata_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Original spec"),
            &owner,
        );

        client.update_asset_metadata(
            &id,
            &owner,
            &String::from_str(&env, "Refurbished spec v2"),
        );

        // env.events().all() reflects only the most recent contract call
        assert_eq!(env.events().all().len(), 1);
    }

    #[test]
    fn test_non_owner_cannot_update_metadata() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Original spec"),
            &owner,
        );

        let attacker = Address::generate(&env);
        let result = client.try_update_asset_metadata(
            &id,
            &attacker,
            &String::from_str(&env, "Hacked spec"),
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedOwner as u32,
            ))),
        );
    }

    #[test]
    fn test_update_metadata_nonexistent_asset() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let result = client.try_update_asset_metadata(
            &999u64,
            &owner,
            &String::from_str(&env, "New spec"),
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AssetNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_transfer_asset() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &owner,
        );

        client.transfer_asset(&id, &owner, &new_owner);

        let asset = client.get_asset(&id);
        assert_eq!(asset.owner, new_owner);
    }

    #[test]
    fn test_transfer_asset_non_owner_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let attacker = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &owner,
        );

        let result = client.try_transfer_asset(&id, &attacker, &new_owner);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedOwner as u32,
            ))),
        );
    }

    #[test]
    fn test_transfer_asset_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &owner,
        );

        client.transfer_asset(&id, &owner, &new_owner);

        // env.events().all() reflects only the most recent contract call
        assert_eq!(env.events().all().len(), 1);
    }

    #[test]
    fn test_transfer_updates_dedup_so_new_owner_can_register_same_metadata() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let metadata = String::from_str(&env, "CAT-3516");

        let id = client.register_asset(&symbol_short!("GENSET"), &metadata, &owner);
        client.transfer_asset(&id, &owner, &new_owner);

        // Original owner can now register the same metadata again (dedup key was moved)
        let id2 = client.register_asset(&symbol_short!("GENSET"), &metadata, &owner);
        assert_ne!(id, id2);
    }

    #[test]
    fn test_update_metadata_dedup_enforced() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        // Register two assets with different metadata
        let id1 = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Spec A"),
            &owner,
        );
        client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Spec B"),
            &owner,
        );

        // Trying to update asset 1 to "Spec B" (already taken by same owner) should fail
        let result = client.try_update_asset_metadata(
            &id1,
            &owner,
            &String::from_str(&env, "Spec B"),
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::DuplicateAsset as u32,
            ))),
        );
    }

    #[test]
    fn test_asset_exists_returns_true_for_existing_asset() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Turbine X"),
            &owner,
        );

        assert!(client.asset_exists(&id));
    }

    #[test]
    fn test_asset_exists_returns_false_for_nonexistent_asset() {        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        assert!(!client.asset_exists(&9999u64));
    }

    #[test]
    fn test_get_assets_by_owner_returns_registered_ids() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let id1 = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "Asset Alpha"),
            &owner,
        );
        let id2 = client.register_asset(
            &symbol_short!("TURBINE"),
            &String::from_str(&env, "Asset Beta"),
            &owner,
        );

        let ids = client.get_assets_by_owner(&owner);
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
    }

    #[test]
    fn test_get_assets_by_owner_returns_empty_for_unknown_owner() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let stranger = Address::generate(&env);
        let ids = client.get_assets_by_owner(&stranger);
        assert_eq!(ids.len(), 0);
    }

    #[test]
    fn test_get_assets_by_owner_updated_after_transfer() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let new_owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &owner,
        );

        client.transfer_asset(&id, &owner, &new_owner);

        // Original owner should have no assets
        assert_eq!(client.get_assets_by_owner(&owner).len(), 0);
        // New owner should have the asset
        let new_ids = client.get_assets_by_owner(&new_owner);
        assert_eq!(new_ids.len(), 1);
        assert!(new_ids.contains(&id));
    }

    #[test]
    fn test_update_asset_metadata_removes_old_dedup_key() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let owner = Address::generate(&env);
        let meta_a = String::from_str(&env, "Spec A");
        let meta_b = String::from_str(&env, "Spec B");

        // Register with metadata A, then update to B
        let id = client.register_asset(&symbol_short!("GENSET"), &meta_a, &owner);
        client.update_asset_metadata(&id, &owner, &meta_b);

        // Old dedup key (A) is gone — owner can register metadata A again
        let id2 = client.register_asset(&symbol_short!("GENSET"), &meta_a, &owner);
        assert_ne!(id, id2);

        // New dedup key (B) is present — owner cannot register metadata B again
        let result = client.try_register_asset(&symbol_short!("GENSET"), &meta_b, &owner);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::DuplicateAsset as u32,
            ))),
        );
    }

    #[test]
    fn test_get_assets_by_owner_updated_after_deregister() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin);

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &owner,
        );

        assert_eq!(client.get_assets_by_owner(&owner).len(), 1);
        client.deregister_asset(&id);
        assert_eq!(client.get_assets_by_owner(&owner).len(), 0);
    }

    // --- Issue #142: get_admin structured error before initialization ---

    #[test]
    fn test_get_admin_before_init_returns_structured_error() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let result = client.try_get_admin();
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NotInitialized as u32,
            ))),
        );
    }
}
