#! [no_std]
use soroban_sdk::{contract, contractimpl, contracttype, symbol_short, Address, BytesN, Env, Symbol};

#[contracttype]
#[derive(Clone)]
pub struct Engineer {
    pub address: Address,
    pub credential_hash: BytesN<32>,
    pub issuer: Address,
    pub active: bool,
    pub issued_at: u64,
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

#[contract]
pub struct EngineerRegistry;

#[contractimpl]
impl EngineerRegistry {
    pub fn register_engineer(
        env: Env,
        engineer: Address,
        credential_hash: BytesN<32>,
        issuer: Address,
    ) {
        issuer.require_auth();
        assert!(credential_hash != BytesN::from_array(&env, &[0u8; 32]), "credential hash cannot be zero");
        assert!(EngineerRegistry::is_trusted_issuer(env.clone(), issuer.clone()), "issuer not whitelisted");
        let record = Engineer {
            address: engineer.clone(),
            credential_hash,
            issuer,
            active: true,
            issued_at: env.ledger().timestamp(),
        };
        env.storage().persistent().set(&engineer_key(&engineer), &record);
        
        // Extend TTL for persistent storage entries to prevent data loss
        env.storage().persistent().extend_ttl(&engineer_key(&engineer), 518400, 518400); // 30 days
    }

    pub fn verify_engineer(env: Env, engineer: Address) -> bool {
        env.storage()
            .persistent()
            .get::<_, Engineer>(&engineer_key(&engineer))
            .map(|e| e.active)
            .unwrap_or(false)
    }

    pub fn revoke_credential(env: Env, engineer: Address) {
        let caller = env.invoker();
        let admin = get_admin(env.clone());
        let mut record: Engineer = env
            .storage()
            .persistent()
            .get(&engineer_key(&engineer))
            .expect("engineer not found");
        assert!(record.issuer == issuer, "not the issuer");
        assert!(record.active, "credential already revoked");
        record.active = false;
        env.storage().persistent().set(&engineer_key(&engineer), &record);
        // Extend TTL for persistent storage entries to prevent data loss
        env.storage().persistent().extend_ttl(&engineer_key(&engineer), 518400, 518400); // 30 days
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

    pub fn add_trusted_issuer(env: Env, issuer: Address) {
        let admin = get_admin(env.clone());
        if env.invoker() != admin {
            panic!("Only admin can add trusted issuers");
        }
        env.storage().instance().set(&trusted_key(&issuer), &());
        env.storage().instance().extend_ttl(&trusted_key(&issuer), 518400, 518400);
    }

    pub fn remove_trusted_issuer(env: Env, issuer: Address) {
        let admin = get_admin(env.clone());
        if env.invoker() != admin {
            panic!("Only admin can remove trusted issuers");
        }
        env.storage().instance().remove(&trusted_key(&issuer));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, BytesN, Env, Symbol};

    #[test]
    fn test_register_verify_revoke() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(&env, &contract_id);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let admin = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.initialize_admin(&admin);
        client.add_trusted_issuer(&issuer);
        client.register_engineer(&engineer, &hash, &issuer);
        assert!(client.verify_engineer(&engineer));

        client.revoke_credential(&engineer);
        assert!(!client.verify_engineer(&engineer));
    }

    #[test]
    #[should_panic(expected = "credential hash cannot be zero")]
    fn test_register_zero_hash_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(&env, &contract_id);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let admin = Address::generate(&env);
        let zero_hash = BytesN::from_array(&env, &[0u8; 32]);

        client.initialize_admin(&admin);
        client.add_trusted_issuer(&issuer);
        client.register_engineer(&engineer, &zero_hash, &issuer);
    }

    #[test]
    #[should_panic(expected = "credential already revoked")]
    fn test_double_revocation() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(&env, &contract_id);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.register_engineer(&engineer, &hash, &issuer);
        client.revoke_credential(&engineer, &issuer);
        
        // Attempting to revoke again should panic
        client.revoke_credential(&engineer, &issuer);
    }
}

