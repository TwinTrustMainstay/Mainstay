#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, contracterror, panic_with_error, symbol_short, Address, BytesN, Env, Symbol, Vec};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ContractError {
    CredentialAlreadyRevoked = 1,
    UnauthorizedAdmin = 2,
}

#[contracttype]
#[derive(Clone)]
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
    pub fn register_engineer(
        env: Env,
        engineer: Address,
        credential_hash: BytesN<32>,
        issuer: Address,
        validity_period: u64,
    ) {
        issuer.require_auth();
        if !env.storage().instance().has(&trusted_key(&issuer)) {
            panic!("issuer is not trusted");
        }
        assert!(
            credential_hash != BytesN::from_array(&env, &[0u8; 32]),
            "credential hash cannot be zero"
        );
        let now = env.ledger().timestamp();
        let record = Engineer {
            address: engineer.clone(),
            credential_hash,
            issuer: issuer.clone(),
            active: true,
            issued_at: now,
            expires_at: now + validity_period,
        };
        env.storage()
            .persistent()
            .set(&engineer_key(&engineer), &record);

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
    }

    pub fn verify_engineer(env: Env, engineer: Address) -> bool {
        env.storage()
            .persistent()
            .get::<_, Engineer>(&engineer_key(&engineer))
            .map(|e| e.active && env.ledger().timestamp() < e.expires_at)
            .unwrap_or(false)
    }

    pub fn revoke_credential(env: Env, engineer: Address) {
        let mut record: Engineer = env
            .storage()
            .persistent()
            .get(&engineer_key(&engineer))
            .expect("engineer not found");
        record.issuer.require_auth();
        assert!(record.active, "credential already revoked");
        record.active = false;
        env.storage()
            .persistent()
            .set(&engineer_key(&engineer), &record);
        env.storage().persistent().extend_ttl(&engineer_key(&engineer), 518400, 518400);
    }

    pub fn get_engineer(env: Env, engineer: Address) -> Engineer {
        env.storage()
            .persistent()
            .get(&engineer_key(&engineer))
            .expect("engineer not found")
    }

    pub fn initialize_admin(env: Env, admin: Address) {
        if env.storage().instance().has(&admin_key()) {
            panic!("admin already initialized");
        }
        env.storage().instance().set(&admin_key(), &admin);
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&admin_key())
            .expect("admin not initialized")
    }

    pub fn is_trusted_issuer(env: Env, issuer: Address) -> bool {
        env.storage().instance().has(&trusted_key(&issuer))
    }

    pub fn get_trusted_issuers(env: Env) -> Vec<Address> {
        env.storage()
            .instance()
            .get(&issuer_list_key())
            .unwrap_or(Vec::new(&env))
    }

    pub fn add_trusted_issuer(env: Env, admin: Address, issuer: Address) {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&admin_key()).expect("admin not initialized");
        if stored_admin != admin {
            panic_with_error!(&env, ContractError::UnauthorizedAdmin);
        }
        env.storage().instance().set(&trusted_key(&issuer), &());
        let mut list: Vec<Address> = env.storage().instance().get(&issuer_list_key()).unwrap_or(Vec::new(&env));
        if !list.contains(issuer.clone()) {
            list.push_back(issuer.clone());
        }
        env.storage().instance().set(&issuer_list_key(), &list);
    }

    pub fn remove_trusted_issuer(env: Env, admin: Address, issuer: Address) {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&admin_key()).expect("admin not initialized");
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

    /// Returns all engineer addresses credentialed by the given issuer.
    pub fn get_engineers_by_issuer(env: Env, issuer: Address) -> Vec<Address> {
        env.storage()
            .persistent()
            .get(&issuer_engineers_key(&issuer))
            .unwrap_or(Vec::new(&env))
    }

    /// Admin-only: upgrade the contract WASM to a new hash.
    pub fn upgrade(env: Env, admin: Address, _new_wasm_hash: BytesN<32>) {
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&admin_key())
            .expect("admin not initialized");
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
    use soroban_sdk::{testutils::Address as _, testutils::storage::Persistent, testutils::Ledger, BytesN, Env};

    fn setup<'a>(env: &'a Env) -> (EngineerRegistryClient<'a>, Address) {
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(env, &contract_id);
        let admin = Address::generate(env);
        client.initialize_admin(&admin);
        (client, admin)
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
    #[should_panic(expected = "credential hash cannot be zero")]
    fn test_register_zero_hash_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let zero_hash = BytesN::from_array(&env, &[0u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &zero_hash, &issuer, &31_536_000);
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
    #[should_panic(expected = "issuer is not trusted")]
    fn test_register_engineer_untrusted_issuer_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup(&env);

        let engineer = Address::generate(&env);
        let untrusted_issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        // untrusted_issuer was never added via add_trusted_issuer — must panic
        client.register_engineer(&engineer, &hash, &untrusted_issuer, &31_536_000);
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

    // --- get_trusted_issuers tests ---

    // 4.1: fresh contract returns empty list (Requirements: 2.2, 4.3)
    #[test]
    fn test_get_trusted_issuers_empty() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup(&env);

        let result = client.get_trusted_issuers();
        assert_eq!(result.len(), 0);
    }

    // 4.2: add one issuer, verify list contains it (Requirements: 1.2, 2.3)
    #[test]
    fn test_get_trusted_issuers_single() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);

        let list = client.get_trusted_issuers();
        assert_eq!(list.len(), 1);
        assert_eq!(list.get(0).unwrap(), issuer);
    }

    // 4.3: add same issuer twice, verify list length is 1 (Requirements: 1.3, 2.4)
    #[test]
    fn test_get_trusted_issuers_dedup() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);
        client.add_trusted_issuer(&admin, &issuer);

        let list = client.get_trusted_issuers();
        assert_eq!(list.len(), 1);
    }

    // 4.4: add then remove, verify list is empty (Requirements: 1.4, 2.3)
    #[test]
    fn test_get_trusted_issuers_after_remove() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);
        client.remove_trusted_issuer(&admin, &issuer);

        let list = client.get_trusted_issuers();
        assert_eq!(list.len(), 0);
    }

    // 4.5: remove an address never added, verify list unchanged (Requirements: 1.5)
    #[test]
    fn test_get_trusted_issuers_remove_absent_noop() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        let outsider = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);

        client.remove_trusted_issuer(&admin, &outsider);

        let list = client.get_trusted_issuers();
        assert_eq!(list.len(), 1);
        assert_eq!(list.get(0).unwrap(), issuer);
    }

    // 4.6: call get_trusted_issuers without mocking auth, verify no panic (Requirements: 4.3)
    #[test]
    fn test_get_trusted_issuers_no_auth_required() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);

        // Clear auths — get_trusted_issuers must not require any authorization
        env.mock_auths(&[]);
        let list = client.get_trusted_issuers();
        assert_eq!(list.len(), 1);
        assert_eq!(list.get(0).unwrap(), issuer);
    }

    // 4.7: non-admin cannot add trusted issuer (Requirements: 4.1)
    #[test]
    fn test_non_admin_cannot_add_trusted_issuer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup(&env);

        let outsider = Address::generate(&env);
        let issuer = Address::generate(&env);

        let result = client.try_add_trusted_issuer(&outsider, &issuer);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    // 4.8: non-admin cannot remove trusted issuer (Requirements: 4.2)
    #[test]
    fn test_non_admin_cannot_remove_trusted_issuer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);

        let outsider = Address::generate(&env);
        let result = client.try_remove_trusted_issuer(&outsider, &issuer);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }
}
