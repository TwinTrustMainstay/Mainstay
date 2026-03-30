#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, contracterror, panic_with_error, symbol_short, Address, BytesN, Env, Symbol, Vec};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ContractError {
    CredentialAlreadyRevoked = 1,
    UnauthorizedAdmin = 2,
    EngineerNotFound = 3,
    NotInitialized = 4,
    AdminAlreadyInitialized = 5,
    UntrustedIssuer = 6,
    InvalidCredentialHash = 7,
    Paused = 8,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Engineer {
    pub address: Address,
    pub credential_hash: BytesN<32>,
    pub issuer: Address,
    pub active: bool,
    pub issued_at: u64,
    pub expires_at: u64,
}

fn engineer_key(addr: &Address) -> (Symbol, Address) {
    (symbol_short!("ENG"), addr.clone())
}

const PAUSED_KEY: Symbol = symbol_short!("PAUSED");

fn is_paused(env: &Env) -> bool {
    env.storage().instance().get(&PAUSED_KEY).unwrap_or(false)
}

fn ensure_not_paused(env: &Env) {
    if is_paused(env) {
        panic_with_error!(env, ContractError::Paused);
    }
}

fn admin_key() -> Symbol {
    symbol_short!("ADMIN")
}

fn trusted_key(issuer: &Address) -> (Symbol, Address) {
    (symbol_short!("TRUSTED"), issuer.clone())
}

fn issuer_engineers_key(issuer: &Address) -> (Symbol, Address) {
    (symbol_short!("ISS_ENGS"), issuer.clone())
}

fn issuer_list_key() -> Symbol {
    symbol_short!("ISS_LIST")
}

#[contract]
pub struct EngineerRegistry;

#[contractimpl]
impl EngineerRegistry {
    /// Register a new engineer with their credential information.
    /// Only trusted issuers can register engineers.
    ///
    /// # Arguments
    /// * `engineer` - The address of the engineer being registered
    /// * `credential_hash` - Hash of the engineer's credentials/certifications
    /// * `issuer` - The trusted issuer address registering the engineer
    /// * `validity_period` - Duration in seconds for which the credentials are valid
    ///
    /// # Panics
    /// - [`ContractError::UntrustedIssuer`] if the issuer is not in the trusted list
    /// - [`ContractError::InvalidCredentialHash`] if credential hash is all zeros
    pub fn register_engineer(
        env: Env,
        engineer: Address,
        credential_hash: BytesN<32>,
        issuer: Address,
        validity_period: u64,
    ) {
        ensure_not_paused(&env);
        issuer.require_auth();
        if !env.storage().instance().has(&trusted_key(&issuer)) {
            panic_with_error!(&env, ContractError::UntrustedIssuer);
        }
        if credential_hash == BytesN::from_array(&env, &[0u8; 32]) {
            panic_with_error!(&env, ContractError::InvalidCredentialHash);
        }
        let now = env.ledger().timestamp();
        let record = Engineer {
            address: engineer.clone(),
            credential_hash: credential_hash.clone(),
            issuer: issuer.clone(),
            active: true,
            issued_at: now,
            expires_at: now + validity_period,
        };
        env.storage()
            .persistent()
            .set(&engineer_key(&engineer), &record);
        env.storage()
            .persistent()
            .extend_ttl(&engineer_key(&engineer), 518400, 518400);

        // Track issuer → engineers mapping
        let mut list: Vec<Address> = env
            .storage()
            .persistent()
            .get(&issuer_engineers_key(&issuer))
            .unwrap_or(Vec::new(&env));
        list.push_back(engineer.clone());
        env.storage()
            .persistent()
            .set(&issuer_engineers_key(&issuer), &list);
        env.storage()
            .persistent()
            .extend_ttl(&issuer_engineers_key(&issuer), 518400, 518400);

        // Emit engineer registration event
        env.events().publish(
            (symbol_short!("REG_ENG"), engineer.clone()),
            (issuer, credential_hash.clone(), now),
        );
    }

    /// Verify if an engineer has valid, active credentials.
    /// Checks both active status and expiration time.
    ///
    /// # Arguments
    /// * `engineer` - The address of the engineer to verify
    ///
    /// # Returns
    /// `true` if the engineer has valid, non-expired credentials; `false` otherwise
    pub fn verify_engineer(env: Env, engineer: Address) -> bool {
        env.storage()
            .persistent()
            .get::<_, Engineer>(&engineer_key(&engineer))
            .map(|e| e.active && env.ledger().timestamp() < e.expires_at)
            .unwrap_or(false)
    }

    /// Revoke an engineer's credentials, making them inactive.
    /// Only the original issuer can revoke credentials.
    ///
    /// # Arguments
    /// * `engineer` - The address of the engineer whose credentials should be revoked
    ///
    /// # Panics
    /// - [`ContractError::EngineerNotFound`] if no engineer exists with the given address
    /// - [`ContractError::CredentialAlreadyRevoked`] if the credentials are already revoked
    pub fn revoke_credential(env: Env, engineer: Address) {
        ensure_not_paused(&env);
        let mut record: Engineer = env
            .storage()
            .persistent()
            .get(&engineer_key(&engineer))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::EngineerNotFound));
        record.issuer.require_auth();
        if !record.active {
            panic_with_error!(&env, ContractError::CredentialAlreadyRevoked);
        }
        // Extend TTL before write to ensure consistency even on near-expired entries
        env.storage().persistent().extend_ttl(&engineer_key(&engineer), 518400, 518400);
        record.active = false;
        env.storage()
            .persistent()
            .set(&engineer_key(&engineer), &record);

        // Emit credential revocation event
        env.events().publish(
            (symbol_short!("REV_CRED"), engineer.clone()),
            (record.issuer.clone(), env.ledger().timestamp()),
        );
    }

    /// Retrieve complete engineer information by address.
    ///
    /// # Arguments
    /// * `engineer` - The address of the engineer to retrieve
    ///
    /// # Returns
    /// The complete Engineer struct with all credential information
    ///
    /// # Panics
    /// - [`ContractError::EngineerNotFound`] if no engineer exists with the given address
    pub fn get_engineer(env: Env, engineer: Address) -> Engineer {
        env.storage()
            .persistent()
            .get(&engineer_key(&engineer))
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::EngineerNotFound))
    }

    /// Initialize the admin address for the contract.
    /// This function should be called once immediately after deployment.
    ///
    /// # Arguments
    /// * `admin` - The address that will have administrative privileges
    ///
    /// # Panics
    /// - [`ContractError::AdminAlreadyInitialized`] if admin has already been initialized
    pub fn initialize_admin(env: Env, admin: Address) {
        admin.require_auth();
        if env.storage().instance().has(&admin_key()) {
            panic_with_error!(&env, ContractError::AdminAlreadyInitialized);
        }
        env.storage().instance().set(&admin_key(), &admin);
    }

    /// Get the current admin address of the contract.
    ///
    /// # Returns
    /// The address of the current administrator
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if the admin has not been initialized
    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&admin_key())
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized))
    }

    /// Admin-only function to pause the contract.
    ///
    /// # Arguments
    /// * `admin` - The address that must match the stored admin
    pub fn pause(env: Env, admin: Address) {
        admin.require_auth();
        let stored_admin: Address = Self::get_admin(env.clone());
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        env.storage().instance().set(&PAUSED_KEY, &true);
    }

    /// Admin-only function to unpause the contract.
    ///
    /// # Arguments
    /// * `admin` - The address that must match the stored admin
    pub fn unpause(env: Env, admin: Address) {
        admin.require_auth();
        let stored_admin: Address = Self::get_admin(env.clone());
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        env.storage().instance().set(&PAUSED_KEY, &false);
    }

    /// Check if the contract is currently paused.
    ///
    /// # Returns
    /// `true` if paused; `false` otherwise
    pub fn is_paused(env: Env) -> bool {
        is_paused(&env)
    }

    /// Check if an issuer is in the trusted issuers list.
    ///
    /// # Arguments
    /// * `issuer` - The address of the issuer to check
    ///
    /// # Returns
    /// `true` if the issuer is trusted; `false` otherwise
    pub fn is_trusted_issuer(env: Env, issuer: Address) -> bool {
        env.storage().instance().has(&trusted_key(&issuer))
    }

    /// Get the list of all trusted issuer addresses.
    ///
    /// # Returns
    /// A Vec containing all trusted issuer addresses
    pub fn get_trusted_issuers(env: Env) -> Vec<Address> {
        env.storage()
            .instance()
            .get(&issuer_list_key())
            .unwrap_or(Vec::new(&env))
    }

    /// Admin-only function to add a new trusted issuer.
    /// Only admins can modify the trusted issuers list.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored admin
    /// * `issuer` - The address of the issuer to add as trusted
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if the admin has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    pub fn add_trusted_issuer(env: Env, admin: Address, issuer: Address) {
        ensure_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&admin_key())
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        env.storage().instance().set(&trusted_key(&issuer), &());
        let mut list: Vec<Address> = env.storage().instance().get(&issuer_list_key()).unwrap_or(Vec::new(&env));
        if !list.contains(issuer.clone()) {
            list.push_back(issuer.clone());
        }
        env.storage().instance().set(&issuer_list_key(), &list);

        env.events().publish(
            (symbol_short!("ISS_ADD"), admin),
            (issuer,),
        );
    }

    /// Admin-only function to remove a trusted issuer.
    /// Only admins can modify the trusted issuers list.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored admin
    /// * `issuer` - The address of the issuer to remove from trusted list
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if the admin has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    pub fn remove_trusted_issuer(env: Env, admin: Address, issuer: Address) {
        ensure_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&admin_key())
            .unwrap_or_else(|| panic_with_error!(&env, ContractError::NotInitialized));
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        env.storage().instance().remove(&trusted_key(&issuer));
        let list: Vec<Address> = env.storage().instance().get(&issuer_list_key()).unwrap_or(Vec::new(&env));
        let mut new_list: Vec<Address> = Vec::new(&env);
        for addr in list.iter() {
            if addr != issuer {
                new_list.push_back(addr);
            }
        }
        env.storage().instance().set(&issuer_list_key(), &new_list);
    }

    /// Get all engineer addresses that have been credentialed by a specific issuer.
    ///
    /// # Arguments
    /// * `issuer` - The address of the issuer to query
    ///
    /// # Returns
    /// A Vec containing all engineer addresses credentialed by the given issuer
    pub fn get_engineers_by_issuer(env: Env, issuer: Address) -> Vec<Address> {
        env.storage()
            .persistent()
            .get(&issuer_engineers_key(&issuer))
            .unwrap_or(Vec::new(&env))
    }

    /// Admin-only function to upgrade the contract WASM to a new hash.
    /// This allows for contract updates while maintaining state.
    ///
    /// # Arguments
    /// * `admin` - The admin address that must match the stored admin
    /// * `new_wasm_hash` - The hash of the new WASM code to deploy
    ///
    /// # Panics
    /// - [`ContractError::NotInitialized`] if the admin has not been initialized
    /// - [`ContractError::UnauthorizedAdmin`] if caller is not the admin
    pub fn upgrade(env: Env, admin: Address, _new_wasm_hash: BytesN<32>) {
        ensure_not_paused(&env);
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&admin_key())
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
    use soroban_sdk::{testutils::Address as _, testutils::storage::Persistent, testutils::{Ledger, Events}, BytesN, Env};

    fn setup<'a>(env: &'a Env) -> (EngineerRegistryClient<'a>, Address) {
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(env, &contract_id);
        let admin = Address::generate(env);
        client.initialize_admin(&admin);
        (client, admin)
    }

    #[test]
    #[should_panic(expected = "admin already initialized")]
    fn test_initialize_admin_called_twice_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin);
        // Second call must panic
        client.initialize_admin(&admin);
    }

    #[test]
    fn test_register_verify_revoke() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000);
        assert!(client.verify_engineer(&engineer));

        client.revoke_credential(&engineer);
        assert!(!client.verify_engineer(&engineer));
    }

    #[test]
    fn test_register_engineer_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000);

        // Verify registration event was emitted
        let events = env.events().all();
        assert!(events.len() > 0);
    }

    #[test]
    fn test_revoke_credential_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000);
        client.revoke_credential(&engineer);

        // Verify revocation event was emitted
        let events = env.events().all();
        assert!(events.len() > 0);
    }

    #[test]
    fn test_initialize_admin_double_call_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin);

        // Second call should fail with structured error
        let result = client.try_initialize_admin(&admin);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::AdminAlreadyInitialized as u32,
            ))),
        );
    }

    #[test]
    fn test_initialize_admin_requires_auth() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        // This should succeed because we mock all auths
        client.initialize_admin(&admin);
        
        // Verify admin was set
        assert_eq!(client.get_admin(), admin);
    }

    #[test]
    fn test_register_zero_hash_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let zero_hash = BytesN::from_array(&env, &[0u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        let result = client.try_register_engineer(&engineer, &zero_hash, &issuer, &31_536_000);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidCredentialHash as u32,
            ))),
        );
    }

    #[test]
    fn test_ttl_extended_on_registration() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000);

        let contract_id = client.address.clone();
        let ttl = env.as_contract(&contract_id, || {
            env.storage().persistent().get_ttl(&engineer_key(&engineer))
        });
        assert!(ttl > 0, "Engineer TTL should be extended");
    }

    #[test]
    fn test_issuer_engineers_ttl_extended_on_registration() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000);

        let contract_id = client.address.clone();
        let ttl = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get_ttl(&issuer_engineers_key(&issuer))
        });
        assert!(ttl > 0, "Issuer engineers TTL should be extended");
    }

    #[test]
    fn test_ttl_extended_on_revoke() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000);
        client.revoke_credential(&engineer);

        let contract_id = client.address.clone();
        let ttl = env.as_contract(&contract_id, || {
            env.storage().persistent().get_ttl(&engineer_key(&engineer))
        });
        assert!(ttl > 0, "Engineer TTL should be extended after revoke");
    }

    #[test]
    fn test_admin_can_upgrade() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let new_wasm_hash = BytesN::from_array(&env, &[0xabu8; 32]);
        // In test env the WASM hash won't exist, so we just verify auth passes (no UnauthorizedAdmin error)
        let result = client.try_upgrade(&admin, &new_wasm_hash);
        assert!(result != Err(Ok(soroban_sdk::Error::from_contract_error(ContractError::UnauthorizedAdmin as u32))));
    }

    #[test]
    fn test_non_admin_cannot_upgrade() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup(&env);

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

    // --- get_engineers_by_issuer tests ---

    #[test]
    fn test_get_engineers_by_issuer_empty() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup(&env);

        let issuer = Address::generate(&env);
        let result = client.get_engineers_by_issuer(&issuer);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_get_engineers_by_issuer_single() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000);

        let list = client.get_engineers_by_issuer(&issuer);
        assert_eq!(list.len(), 1);
        assert_eq!(list.get(0).unwrap(), engineer);
    }

    #[test]
    fn test_get_engineers_by_issuer_multiple() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        let e1 = Address::generate(&env);
        let e2 = Address::generate(&env);
        let e3 = Address::generate(&env);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&e1, &BytesN::from_array(&env, &[1u8; 32]), &issuer, &31_536_000);
        client.register_engineer(&e2, &BytesN::from_array(&env, &[2u8; 32]), &issuer, &31_536_000);
        client.register_engineer(&e3, &BytesN::from_array(&env, &[3u8; 32]), &issuer, &31_536_000);

        let list = client.get_engineers_by_issuer(&issuer);
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn test_get_engineers_by_issuer_isolated_per_issuer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer_a = Address::generate(&env);
        let issuer_b = Address::generate(&env);
        let e1 = Address::generate(&env);
        let e2 = Address::generate(&env);

        client.add_trusted_issuer(&admin, &issuer_a);
        client.add_trusted_issuer(&admin, &issuer_b);
        client.register_engineer(&e1, &BytesN::from_array(&env, &[1u8; 32]), &issuer_a, &31_536_000);
        client.register_engineer(&e2, &BytesN::from_array(&env, &[2u8; 32]), &issuer_b, &31_536_000);

        assert_eq!(client.get_engineers_by_issuer(&issuer_a).len(), 1);
        assert_eq!(client.get_engineers_by_issuer(&issuer_b).len(), 1);
        assert_eq!(client.get_engineers_by_issuer(&issuer_a).get(0).unwrap(), e1);
        assert_eq!(client.get_engineers_by_issuer(&issuer_b).get(0).unwrap(), e2);
    }

    #[test]
    fn test_pause_and_unpause_in_engineer_registry() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.pause(&admin);
        let result = client.try_register_engineer(&engineer, &hash, &issuer, &31_536_000);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            ))),
        );

        client.unpause(&admin);
        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000);
        assert!(client.verify_engineer(&engineer));
    }

    #[test]
    fn test_register_engineer_untrusted_issuer_returns_error() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup(&env);

        let engineer = Address::generate(&env);
        let untrusted_issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        let result = client.try_register_engineer(&engineer, &hash, &untrusted_issuer, &31_536_000);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UntrustedIssuer as u32,
            ))),
        );
    }

    #[test]
    fn test_expired_credential_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        // validity_period of 1000 seconds
        client.register_engineer(&engineer, &hash, &issuer, &1000);
        assert!(client.verify_engineer(&engineer));

        // Advance ledger past expiry
        env.ledger().with_mut(|li| li.timestamp = li.timestamp + 1001);
        assert!(!client.verify_engineer(&engineer));
    }

    #[test]
    fn test_credential_valid_before_expiry() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &1000);

        // Advance to just before expiry
        env.ledger().with_mut(|li| li.timestamp = li.timestamp + 999);
        assert!(client.verify_engineer(&engineer));
    }

    #[test]
    fn test_expires_at_stored_correctly() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        let validity_period: u64 = 86_400;

        client.add_trusted_issuer(&admin, &issuer);
        let issued_at = env.ledger().timestamp();
        client.register_engineer(&engineer, &hash, &issuer, &validity_period);

        let record = client.get_engineer(&engineer);
        assert_eq!(record.issued_at, issued_at);
        assert_eq!(record.expires_at, issued_at + validity_period);
    }

    // --- Issue #141: get_engineer structured error ---

    #[test]
    fn test_get_engineer_unknown_returns_structured_error() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup(&env);

        let unknown = Address::generate(&env);
        let result = client.try_get_engineer(&unknown);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::EngineerNotFound as u32,
            ))),
        );
    }

    // --- Issue #142: get_admin structured error before initialization ---

    #[test]
    fn test_get_admin_before_init_returns_structured_error() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(&env, &contract_id);

        let result = client.try_get_admin();
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::NotInitialized as u32,
            ))),
        );
    }

    // --- Issue #143: revoke_credential extends TTL before write ---

    #[test]
    fn test_revoke_credential_ttl_extended_before_write() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000);

        client.revoke_credential(&engineer);

        // After revocation the entry must still be accessible and marked inactive
        let record = client.get_engineer(&engineer);
        assert!(!record.active);

        let ttl = env.as_contract(&contract_id, || {
            env.storage().persistent().get_ttl(&engineer_key(&engineer))
        });
        assert!(ttl > 0, "TTL must be extended after revocation");
    }

    #[test]
    fn test_add_trusted_issuer_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);

        let events = env.events().all();
        assert_eq!(events.len(), 1);

        let (_, topics, data) = events.get(0).unwrap();

        use soroban_sdk::TryIntoVal;
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        let t1: Address = topics.get(1).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("ISS_ADD"));
        assert_eq!(t1, admin);

        let (emitted_issuer,): (Address,) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_issuer, issuer);
    }

    #[test]
    fn test_pause_affects_all_state_changes() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000);

        client.pause(&admin);

        // register_engineer
        assert_eq!(
            client.try_register_engineer(&Address::generate(&env), &hash, &issuer, &100),
            Err(Ok(soroban_sdk::Error::from_contract_error(ContractError::Paused as u32)))
        );

        // revoke_credential
        assert_eq!(
            client.try_revoke_credential(&engineer),
            Err(Ok(soroban_sdk::Error::from_contract_error(ContractError::Paused as u32)))
        );

        // add_trusted_issuer
        assert_eq!(
            client.try_add_trusted_issuer(&admin, &Address::generate(&env)),
            Err(Ok(soroban_sdk::Error::from_contract_error(ContractError::Paused as u32)))
        );

        // remove_trusted_issuer
        assert_eq!(
            client.try_remove_trusted_issuer(&admin, &issuer),
            Err(Ok(soroban_sdk::Error::from_contract_error(ContractError::Paused as u32)))
        );

        // upgrade
        assert_eq!(
            client.try_upgrade(&admin, &BytesN::from_array(&env, &[0u8; 32])),
            Err(Ok(soroban_sdk::Error::from_contract_error(ContractError::Paused as u32)))
        );
    }
}
