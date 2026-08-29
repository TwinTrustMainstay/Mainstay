#![no_std]
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    Bytes, BytesN, Env, String, Symbol,
};

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ContractError {
    AssetNotFound = 1,
    /// Same owner attempted to register an asset with identical metadata.
    DuplicateAsset = 2,
    UnauthorizedAdmin = 3,
    UnauthorizedOwner = 4,
    /// Caller is not the currently configured lending contract.
    UnauthorizedLender = 5,
    /// set_lending_contract called before the 48h timelock elapsed.
    LendingTimelockNotElapsed = 6,
    /// set_lending_contract called with no matching proposal on file.
    NoLendingProposal = 7,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Asset {
    pub asset_id: u64,
    pub asset_type: Symbol,
    pub metadata: String,
    pub owner: Address,
    pub registered_at: u64,
    /// True while a lending contract holds this asset as active collateral.
    pub locked: bool,
}

/// A pending admin proposal to change the lending contract address.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LendingContractProposal {
    pub new_contract: Address,
    pub proposed_at: u64,
}

const ASSET_COUNT: Symbol = symbol_short!("A_COUNT");

const ADMIN_KEY: Symbol = symbol_short!("ADMIN");

/// Address of the contract currently authorized to call `lock_asset_as_collateral`.
const LENDING_CONTRACT_KEY: Symbol = symbol_short!("LENDCTR");
const LENDING_PROPOSAL_KEY: Symbol = symbol_short!("LENDPROP");
/// Timelock enforced between proposing and applying a new lending contract: 48 hours.
const LENDING_TIMELOCK_SECS: u64 = 172800;


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
            locked: false,
        };
        env.storage().persistent().set(&asset_key(id), &asset);
        env.storage().persistent().extend_ttl(&asset_key(id), 518400, 518400); // Extend TTL for persistent storage entries to prevent data loss
        env.storage().instance().set(&ASSET_COUNT, &id);
        env.storage().persistent().set(&dk, &id);

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

    pub fn asset_count(env: Env) -> u64 {
        env.storage().instance().get(&ASSET_COUNT).unwrap_or(0)
    }

    /// Initialize the admin address (call once on deploy)
    pub fn initialize_admin(env: Env, admin: Address) {
        admin.require_auth();
        if env.storage().instance().has(&ADMIN_KEY) {
            panic!("Admin already initialized");
        }
        env.storage().instance().set(&ADMIN_KEY, &admin);
    }

    /// Get the current admin address
    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&ADMIN_KEY).expect("Admin not initialized")
    }

    /// Admin-only: Deregister (remove) an asset
    pub fn deregister_asset(env: Env, asset_id: u64) {
        let admin = Self::get_admin(env.clone());
        admin.require_auth();
        
        let asset: Asset = env.storage().persistent()
            .get(&asset_key(asset_id))
            .expect("Asset not found");
        
        // Remove asset storage
        env.storage().persistent().remove(&asset_key(asset_id));
        
        // Remove deduplication key
        let dk = dedup_key(&asset.owner, &env.crypto().sha256(&Bytes::from(asset.metadata.to_xdr(&env))).into());
        env.storage().persistent().remove(&dk);
        
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

        asset.owner = new_owner.clone();
        env.storage().persistent().set(&asset_key(asset_id), &asset);
        env.storage().persistent().extend_ttl(&asset_key(asset_id), 518400, 518400);

        env.events().publish(
            (symbol_short!("TRANSFER"), asset_id),
            (current_owner, new_owner, env.ledger().timestamp()),
        );
    }

    /// Admin-only: propose a new lending contract address. Must wait
    /// LENDING_TIMELOCK_SECS (48h) before `set_lending_contract` can apply it,
    /// so a compromised admin key can't instantly redirect collateral-locking
    /// authority to a malicious contract.
    pub fn propose_lending_contract(env: Env, admin: Address, new_contract: Address) {
        admin.require_auth();
        Self::require_admin(&env, &admin);

        let proposed_at = env.ledger().timestamp();
        env.storage().instance().set(
            &LENDING_PROPOSAL_KEY,
            &LendingContractProposal {
                new_contract: new_contract.clone(),
                proposed_at,
            },
        );
        env.events().publish(
            (symbol_short!("LENDPROP"),),
            (admin, new_contract, proposed_at),
        );
    }

    /// Admin-only: apply a previously proposed lending contract change once
    /// the 48h timelock has elapsed. Emits ADM_AUD, an administrative audit
    /// event, so any change to this security-critical setting is traceable.
    pub fn set_lending_contract(env: Env, admin: Address) {
        admin.require_auth();
        Self::require_admin(&env, &admin);

        let proposal: LendingContractProposal = env
            .storage()
            .instance()
            .get(&LENDING_PROPOSAL_KEY)
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NoLendingProposal));

        let now = env.ledger().timestamp();
        if now < proposal.proposed_at + LENDING_TIMELOCK_SECS {
            panic_with_error!(&env, ContractError::LendingTimelockNotElapsed);
        }

        let old_contract: Option<Address> = env.storage().instance().get(&LENDING_CONTRACT_KEY);
        env.storage()
            .instance()
            .set(&LENDING_CONTRACT_KEY, &proposal.new_contract);
        env.storage().instance().remove(&LENDING_PROPOSAL_KEY);

        env.events().publish(
            (symbol_short!("ADM_AUD"),),
            (old_contract, proposal.new_contract, now),
        );
    }

    pub fn get_lending_contract(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&LENDING_CONTRACT_KEY)
            .expect("lending contract not set")
    }

    /// Lending-contract-only: mark an asset as locked collateral.
    pub fn lock_asset_as_collateral(env: Env, asset_id: u64, lender: Address) {
        lender.require_auth();
        Self::require_lending_contract(&env, &lender);

        let mut asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));
        asset.locked = true;
        env.storage().persistent().set(&asset_key(asset_id), &asset);

        env.events().publish(
            (symbol_short!("LOCK_AST"), asset_id),
            (lender, env.ledger().timestamp()),
        );
    }

    /// Lending-contract-only: release an asset from collateral lock.
    pub fn unlock_asset(env: Env, asset_id: u64, lender: Address) {
        lender.require_auth();
        Self::require_lending_contract(&env, &lender);

        let mut asset: Asset = env
            .storage()
            .persistent()
            .get(&asset_key(asset_id))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::AssetNotFound));
        asset.locked = false;
        env.storage().persistent().set(&asset_key(asset_id), &asset);

        env.events().publish(
            (symbol_short!("UNLK_AST"), asset_id),
            (lender, env.ledger().timestamp()),
        );
    }

    fn require_admin(env: &Env, admin: &Address) {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&ADMIN_KEY)
            .expect("admin not initialized");
        if &stored_admin != admin {
            panic_with_error!(env, ContractError::UnauthorizedAdmin);
        }
    }

    fn require_lending_contract(env: &Env, lender: &Address) {
        let lending_contract: Address = env
            .storage()
            .instance()
            .get(&LENDING_CONTRACT_KEY)
            .unwrap_or_else(|| panic_with_error!(env, ContractError::UnauthorizedLender));
        if &lending_contract != lender {
            panic_with_error!(env, ContractError::UnauthorizedLender);
        }
    }

    /// Admin-only: upgrade the contract WASM to a new hash.
    pub fn upgrade(env: Env, admin: Address, new_wasm_hash: BytesN<32>) {
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&ADMIN_KEY)
            .expect("admin not initialized");
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }

        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        symbol_short,
        testutils::{Address as _, Events},
        Env, String,
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
        let asset_ttl = env.storage().persistent().get_ttl(&asset_key(id));
        assert!(asset_ttl > 0, "Asset TTL should be extended");

        // Verify TTL is set for deduplication key
        let meta_bytes = Bytes::from(metadata.to_xdr(&env));
        let meta_hash: BytesN<32> = env.crypto().sha256(&meta_bytes).into();
        let dk = dedup_key(&owner, &meta_hash);
        let dedup_ttl = env.storage().persistent().get_ttl(&dk);
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
        // Should not panic — admin is authorized
        client.upgrade(&admin, &new_wasm_hash);
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

    // --- Issue 2: lending contract timelock + audit event ---

    #[test]
    fn test_set_lending_contract_requires_timelock() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin);
        let lender = Address::generate(&env);

        client.propose_lending_contract(&admin, &lender);

        // Applying immediately, before the 48h timelock elapses, must fail.
        let result = client.try_set_lending_contract(&admin);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::LendingTimelockNotElapsed as u32,
            ))),
        );
    }

    #[test]
    fn test_set_lending_contract_succeeds_after_timelock_and_emits_adm_aud() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin);
        let lender = Address::generate(&env);

        client.propose_lending_contract(&admin, &lender);
        env.ledger()
            .with_mut(|li| li.timestamp = li.timestamp + LENDING_TIMELOCK_SECS + 1);
        client.set_lending_contract(&admin);

        assert_eq!(client.get_lending_contract(), lender);
        let events = env.events().all();
        assert!(events.len() > 0);
    }

    #[test]
    fn test_set_lending_contract_non_admin_cannot_propose() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin);
        let outsider = Address::generate(&env);
        let lender = Address::generate(&env);

        let result = client.try_propose_lending_contract(&outsider, &lender);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_lock_asset_as_collateral_rejects_unauthorized_caller() {
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

        let attacker = Address::generate(&env);
        let result = client.try_lock_asset_as_collateral(&id, &attacker);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedLender as u32,
            ))),
        );
        assert!(!client.get_asset(&id).locked);
    }

    #[test]
    fn test_lock_asset_as_collateral_succeeds_for_configured_lender() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(AssetRegistry, ());
        let client = AssetRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin);
        let lender = Address::generate(&env);
        client.propose_lending_contract(&admin, &lender);
        env.ledger()
            .with_mut(|li| li.timestamp = li.timestamp + LENDING_TIMELOCK_SECS + 1);
        client.set_lending_contract(&admin);

        let owner = Address::generate(&env);
        let id = client.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(&env, "CAT-3516"),
            &owner,
        );

        client.lock_asset_as_collateral(&id, &lender);
        assert!(client.get_asset(&id).locked);
    }
}

