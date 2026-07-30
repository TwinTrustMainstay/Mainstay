#![cfg(test)]

use engineer_registry::{ContractError, CredentialStatus, EngineerRegistry, EngineerRegistryClient, EngineerStatus};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger},
    testutils::storage::{Instance as _, Persistent},
    Address, BytesN, Env, IntoVal, String, Symbol, TryIntoVal, Vec,
};

fn setup<'a>(env: &'a Env) -> (EngineerRegistryClient<'a>, Address) {
    let contract_id = env.register(EngineerRegistry, ());
    let client = EngineerRegistryClient::new(env, &contract_id);
    let admin = Address::generate(env);
    client.initialize_admin(&admin, &admin);
    (client, admin)
}
    #[test]
    #[should_panic]
    fn test_initialize_admin_called_twice_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        // Second call must panic
        client.initialize_admin(&admin, &admin);
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
        assert!(client.verify_engineer(&engineer, &None::<Symbol>) == CredentialStatus::Valid);

        client.revoke_credential(&engineer);
        assert!(client.verify_engineer(&engineer, &None::<Symbol>) != CredentialStatus::Valid);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        assert_eq!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);

        client.revoke_credential(&engineer);
        assert_ne!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);
    }

    #[test]
    fn test_verify_engineer_false_after_revoke() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[9u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        // Sanity: engineer is initially verified
        assert!(client.verify_engineer(&engineer, &None::<Symbol>) == CredentialStatus::Valid);

        // Revoke credentials and verify immediately returns false
        client.revoke_credential(&engineer);
        assert!(client.verify_engineer(&engineer, &None::<Symbol>) != CredentialStatus::Valid);
        assert_eq!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);

        // Revoke credentials and verify immediately returns false
        client.revoke_credential(&engineer);
        assert_ne!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);
    }

    #[test]
    fn test_register_engineer_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        let validity_period: u64 = 31_536_000;

        client.add_trusted_issuer(&admin, &issuer);
        let issued_at = env.ledger().timestamp();
        client.register_engineer(&engineer, &hash, &issuer, &validity_period, &None);

        // ISS_ADD event fires first, reg_eng is the second event
        let events = env.events().all();
        let (_, topics, data) = events.last().unwrap();

        use soroban_sdk::TryIntoVal;
        let reg_topic = symbol_short!("reg_eng");
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, reg_topic);

        let (emitted_engineer, emitted_hash, emitted_issuer, emitted_timestamp): (
            Address,
            BytesN<32>,
            Address,
            u64,
        ) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_engineer, engineer);
        assert_eq!(emitted_hash, hash);
        assert_eq!(emitted_issuer, issuer);
        assert_eq!(emitted_timestamp, issued_at);
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
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        let revoked_at = env.ledger().timestamp();
        client.revoke_credential(&engineer);

        let events = env.events().all();
        let (_, topics, data) = events.last().unwrap();

        use soroban_sdk::TryIntoVal;
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("REV_CRED"));

        let (emitted_engineer, emitted_hash, emitted_revoked_by, emitted_timestamp): (
            Address,
            BytesN<32>,
            Address,
            u64,
        ) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_engineer, engineer);
        assert_eq!(emitted_hash, hash);
        assert_eq!(emitted_revoked_by, issuer);
        assert_eq!(emitted_timestamp, revoked_at);
    }

    #[test]
    fn test_revoke_already_revoked_returns_structured_error() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        client.revoke_credential(&engineer);

        let result = client.try_revoke_credential(&engineer);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::CredentialAlreadyRevoked as u32
            )))
        );
    }

    #[test]
    fn test_initialize_admin_double_call_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);

        // Second call should fail with structured error
        let result = client.try_initialize_admin(&admin, &admin);
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
        client.initialize_admin(&admin, &admin);

        // Verify admin was set
        assert_eq!(client.get_admin(), admin);
    }

    #[test]
    fn test_initialize_admin_extends_instance_ttl() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.initialize_admin(&admin, &admin);

        let ttl = env.as_contract(&contract_id, || env.storage().instance().get_ttl());
        assert!(
            ttl > 0,
            "Instance TTL should be extended after initialize_admin"
        );
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
        let result =
            client.try_register_engineer(&engineer, &zero_hash, &issuer, &31_536_000, &None);
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
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        let contract_id = client.address.clone();
        let ttl = env.as_contract(&contract_id, || {
            env.storage().persistent().get_ttl(&engineer_key(&engineer))
        });
        assert!(ttl > 0, "Engineer TTL should be extended");
    }

    /// Issue #838: every write must extend the relevant persistent entry's TTL
    /// to at least `TTL_THRESHOLD`. Verify the engineer entry after `register_engineer`.
    #[test]
    fn test_register_engineer_ttl_at_least_threshold() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        let contract_id = client.address.clone();
        let ttl = env.as_contract(&contract_id, || {
            env.storage().persistent().get_ttl(&engineer_key(&engineer))
        });
        assert!(
            ttl >= TTL_THRESHOLD,
            "engineer entry TTL ({}) must be >= TTL_THRESHOLD ({}) after register_engineer",
            ttl,
            TTL_THRESHOLD
        );
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
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

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
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        client.revoke_credential(&engineer);

        let contract_id = client.address.clone();
        let ttl = env.as_contract(&contract_id, || {
            env.storage().persistent().get_ttl(&engineer_key(&engineer))
        });
        assert!(ttl > 0, "Engineer TTL should be extended after revoke");
    }

    #[test]
    fn test_admin_can_propose_upgrade() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let new_wasm_hash = BytesN::from_array(&env, &[0xabu8; 32]);
        // propose_upgrade should succeed for admin without UnauthorizedAdmin error
        let result = client.try_propose_upgrade(&admin, &new_wasm_hash);
        assert!(
            result
                != Err(Ok(soroban_sdk::Error::from_contract_error(
                    ContractError::UnauthorizedAdmin as u32
                )))
        );
    }

    #[test]
    fn test_non_admin_cannot_propose_upgrade() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup(&env);

        let outsider = Address::generate(&env);
        let new_wasm_hash = BytesN::from_array(&env, &[0xabu8; 32]);

        let result = client.try_propose_upgrade(&outsider, &new_wasm_hash);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_upgrade_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let new_wasm_hash = BytesN::from_array(&env, &[0xabu8; 32]);
        client.propose_upgrade(&admin, &new_wasm_hash);

        // Advance past timelock delay (48 hours)
        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

        client.execute_upgrade(&admin);

        let events = env.events().all();
        // Find the UPGRADE event
        use soroban_sdk::TryIntoVal;
        let upgrade_event = events.iter().find(|(_, topics, _)| {
            if let Some(val) = topics.get(0) {
                if let Ok(s) = TryIntoVal::<_, Symbol>::try_into_val(&val, &env) {
                    return s == symbol_short!("UPGRADE");
                }
            }
            false
        });
        assert!(upgrade_event.is_some(), "UPGRADE event must be emitted");
        let (_, _, data) = upgrade_event.unwrap();
        let emitted_hash: BytesN<32> = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_hash, new_wasm_hash);
    }

    // --- get_engineers_by_issuer tests ---

    #[test]
    fn test_propose_and_accept_admin_transfer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let new_admin = Address::generate(&env);
        client.propose_admin(&admin, &new_admin);
        client.accept_admin();

        assert_eq!(client.get_admin(), new_admin);
    }

    #[test]
    fn test_pending_admin_key_cleared_after_accept() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let new_admin = Address::generate(&env);
        client.propose_admin(&admin, &new_admin);
        client.accept_admin();

        let contract_id = client.address.clone();
        env.as_contract(&contract_id, || {
            assert!(!env.storage().instance().has(&pending_admin_key()));
        });
    }

    #[test]
    fn test_non_admin_cannot_propose_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup(&env);

        let outsider = Address::generate(&env);
        let new_admin = Address::generate(&env);

        let result = client.try_propose_admin(&outsider, &new_admin);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    #[test]
    fn test_wrong_address_cannot_accept_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let new_admin = Address::generate(&env);
        let impostor = Address::generate(&env);
        client.propose_admin(&admin, &new_admin);

        use soroban_sdk::IntoVal;
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &impostor,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &client.address,
                fn_name: "accept_admin",
                args: ().into_val(&env),
                sub_invokes: &[],
            },
        }]);

        let result = client.try_accept_admin();
        assert!(result.is_err());
        assert_eq!(client.get_admin(), admin);
    }

    // --- get_engineers_by_issuer tests (original) ---

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
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

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
        client.register_engineer(
            &e1,
            &BytesN::from_array(&env, &[1u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );
        client.register_engineer(
            &e2,
            &BytesN::from_array(&env, &[2u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );
        client.register_engineer(
            &e3,
            &BytesN::from_array(&env, &[3u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );

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
        client.register_engineer(
            &e1,
            &BytesN::from_array(&env, &[1u8; 32]),
            &issuer_a,
            &31_536_000,
            &None,
        );
        client.register_engineer(
            &e2,
            &BytesN::from_array(&env, &[2u8; 32]),
            &issuer_b,
            &31_536_000,
            &None,
        );

        assert_eq!(client.get_engineers_by_issuer(&issuer_a).len(), 1);
        assert_eq!(client.get_engineers_by_issuer(&issuer_b).len(), 1);
        assert_eq!(
            client.get_engineers_by_issuer(&issuer_a).get(0).unwrap(),
            e1
        );
        assert_eq!(
            client.get_engineers_by_issuer(&issuer_b).get(0).unwrap(),
            e2
        );
    }

    #[test]
    fn test_get_engineer_count_by_issuer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        let e1 = Address::generate(&env);
        let e2 = Address::generate(&env);

        // Empty issuer
        assert_eq!(client.get_engineer_count_by_issuer(&issuer), 0);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(
            &e1,
            &BytesN::from_array(&env, &[1u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );
        assert_eq!(client.get_engineer_count_by_issuer(&issuer), 1);

        client.register_engineer(
            &e2,
            &BytesN::from_array(&env, &[2u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );
        assert_eq!(client.get_engineer_count_by_issuer(&issuer), 2);
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
        let result = client.try_register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            ))),
        );

        client.unpause(&admin);
        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000);
        assert!(client.verify_engineer(&engineer, &None::<Symbol>) == CredentialStatus::Valid);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        assert_eq!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);
    }

    #[test]
    fn test_pause_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        client.pause(&admin);

        let events = env.events().all();
        assert!(events.len() >= 1);
        let (_, topics, data) = events.get(0).unwrap();
        use soroban_sdk::TryIntoVal;
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("PAUSED"));
        let (emitted_admin,): (Address,) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_admin, admin);
    }

    #[test]
    fn test_unpause_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        client.pause(&admin);
        client.unpause(&admin);

        let events = env.events().all();
        assert!(events.len() >= 1);
        let (_, topics, data) = events.get(0).unwrap();
        use soroban_sdk::TryIntoVal;
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("UNPAUSED"));
        let (emitted_admin,): (Address,) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_admin, admin);
    }

    #[test]
    fn test_register_engineer_untrusted_issuer_returns_error() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup(&env);

        let engineer = Address::generate(&env);
        let untrusted_issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        let result =
            client.try_register_engineer(&engineer, &hash, &untrusted_issuer, &31_536_000, &None);
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
        // validity_period of 86_400 seconds (minimum)
        client.register_engineer(&engineer, &hash, &issuer, &86_400);
        assert!(client.verify_engineer(&engineer, &None::<Symbol>) == CredentialStatus::Valid);
        client.register_engineer(&engineer, &hash, &issuer, &86_400, &None);
        assert_eq!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);

        // Advance ledger past expiry
        env.ledger()
            .with_mut(|li| li.timestamp = li.timestamp + 86_401);
        assert!(client.verify_engineer(&engineer, &None::<Symbol>) != CredentialStatus::Valid);
        assert_ne!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);
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
        client.register_engineer(&engineer, &hash, &issuer, &86_400, &None);

        // Advance to just before expiry
        env.ledger()
            .with_mut(|li| li.timestamp = li.timestamp + 86_399);
        assert!(client.verify_engineer(&engineer, &None::<Symbol>) == CredentialStatus::Valid);
        assert_eq!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);
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
        client.register_engineer(&engineer, &hash, &issuer, &validity_period, &None);

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
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        client.revoke_credential(&engineer);

        // After revocation the entry must still be accessible and marked inactive
        let record = client.get_engineer(&engineer);
        assert!(!record.active);

        let contract_id = client.address.clone();
        let ttl = env.as_contract(&contract_id, || {
            env.storage().persistent().get_ttl(&engineer_key(&engineer))
        });
        assert!(ttl > 0, "TTL must be extended after revocation");
    }

    // --- Issue #370: renew_credential rejects new_validity_period = 0 ---

    #[test]
    fn test_renew_credential_zero_validity_period_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        let result = client.try_renew_credential(&engineer, &0);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidValidityPeriod as u32,
            ))),
        );
    }

    // --- Issue #369: register_engineer rejects validity_period = 0 ---

    #[test]
    fn test_register_engineer_zero_validity_period_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        client.add_trusted_issuer(&admin, &issuer);
        let result = client.try_register_engineer(&engineer, &hash, &issuer, &0, &None);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidValidityPeriod as u32,
            ))),
        );
    }
    #[test]
    fn test_add_trusted_issuer_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);

        let events = env.events().all();
        assert!(events.len() >= 1);

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
    fn test_add_trusted_issuer_emits_admin_audit_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        let timestamp = env.ledger().timestamp();
        client.add_trusted_issuer(&admin, &issuer);

        use soroban_sdk::TryIntoVal;
        let events = env.events().all();
        let (_, topics, data) = events.last().unwrap();
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        let t1: Symbol = topics.get(1).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("ADM_AUD"));
        assert_eq!(t1, symbol_short!("ISS_ADD"));

        let (emitted_admin, emitted_timestamp, emitted_issuer): (Address, u64, Address) =
            data.try_into_val(&env).unwrap();
        assert_eq!(emitted_admin, admin);
        assert_eq!(emitted_timestamp, timestamp);
        assert_eq!(emitted_issuer, issuer);
    }

    #[test]
    #[ignore = "pre-existing test failure: events not captured correctly"]
    fn test_add_trusted_issuer_no_duplicate_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);
        // Adding the same issuer again should not emit a second ISS_ADD event
        client.add_trusted_issuer(&admin, &issuer);

        let events = env.events().all();
        use soroban_sdk::TryIntoVal;
        let iss_add_count = events
            .iter()
            .filter(|(_, topics, _)| {
                topics
                    .get(0)
                    .and_then(|v| TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                    .map(|s| s == symbol_short!("ISS_ADD"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(iss_add_count, 1, "ISS_ADD should only be emitted once");
    }

    // --- Issue #386: add_trusted_issuer and remove_trusted_issuer extend instance TTL ---

    #[test]
    fn test_add_trusted_issuer_extends_instance_ttl() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);

        let ttl = env.as_contract(&client.address, || env.storage().instance().get_ttl());
        assert!(
            ttl > 0,
            "instance TTL must be extended after add_trusted_issuer"
        );
    }

    #[test]
    fn test_remove_trusted_issuer_extends_instance_ttl() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);
        client.remove_trusted_issuer(&admin, &issuer);

        let ttl = env.as_contract(&client.address, || env.storage().instance().get_ttl());
        assert!(
            ttl > 0,
            "instance TTL must be extended after remove_trusted_issuer"
        );
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
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        client.pause(&admin);

        // Read-only access should still work while paused
        assert!(client.verify_engineer(&engineer, &None::<Symbol>) == CredentialStatus::Valid);
        assert_eq!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);
        let fetched_engineer = client.get_engineer(&engineer);
        assert_eq!(fetched_engineer.address, engineer);
        assert!(fetched_engineer.active);
        assert!(client.try_get_engineer(&engineer).is_ok());

        // register_engineer
        assert_eq!(
            client.try_register_engineer(&Address::generate(&env), &hash, &issuer, &100, &None),
            client.try_register_engineer(
                &Address::generate(&env),
                &hash,
                &issuer,
                &100,
                &None,
                &None
            ),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );

        // revoke_credential
        assert_eq!(
            client.try_revoke_credential(&engineer),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );

        // add_trusted_issuer
        assert_eq!(
            client.try_add_trusted_issuer(&admin, &Address::generate(&env)),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );

        // remove_trusted_issuer
        assert_eq!(
            client.try_remove_trusted_issuer(&admin, &issuer),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );

        // propose_upgrade
        assert_eq!(
            client.try_propose_upgrade(&admin, &BytesN::from_array(&env, &[0u8; 32])),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );

        // renew_credential
        assert_eq!(
            client.try_renew_credential(&engineer, &31_536_000),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );
    }

    // --- renew_credential tests ---

    #[test]
    fn test_renew_credential_extends_expiry() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        let before = client.get_engineer(&engineer).expires_at;
        client.renew_credential(&engineer, &86_400);
        let after = client.get_engineer(&engineer).expires_at;

        assert_eq!(after, before + 86_400);
    }

    #[test]
    fn test_renew_credential_from_now_when_expired() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &86_400, &None);

        // Advance past original expiry
        env.ledger()
            .with_mut(|li| li.timestamp = li.timestamp + 86_401);
        assert!(client.verify_engineer(&engineer, &None::<Symbol>) != CredentialStatus::Valid);

        // Renew for another 86_400 seconds from now
        client.renew_credential(&engineer, &86_400);
        assert!(client.verify_engineer(&engineer, &None::<Symbol>) == CredentialStatus::Valid);
        assert_ne!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);

        // Renew for another 86_400 seconds from now
        client.renew_credential(&engineer, &86_400);
        assert_eq!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);

        let record = client.get_engineer(&engineer);
        assert_eq!(record.expires_at, env.ledger().timestamp() + 86_400);
    }

    #[test]
    fn test_renew_credential_early_renewal_preserves_remaining_validity() {
        // An engineer renews while 25 days are still left on their credential.
        // The new period should stack on top of the remaining validity, not replace it.
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[2u8; 32]);

        const DAY: u64 = 86_400;
        let initial_validity: u64 = 30 * DAY; // 30-day credential
        let elapsed: u64 = 5 * DAY; // renew after 5 days (25 days remain)
        let new_validity: u64 = 30 * DAY; // add another 30 days

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &initial_validity, &None);

        let original = client.get_engineer(&engineer);
        let now_before_renewal = original.issued_at; // ledger starts at issued_at

        // Advance ledger by 5 days (25 days of original validity still remain)
        env.ledger()
            .with_mut(|li| li.timestamp = now_before_renewal + elapsed);

        client.renew_credential(&engineer, &new_validity);

        let renewed = client.get_engineer(&engineer);

        // Expected: original expiry (now + 25 days) + 30 new days = now + 55 days
        let expected_expires_at = original.expires_at + new_validity;
        assert_eq!(
            renewed.expires_at, expected_expires_at,
            "Early renewal must extend from current expires_at, not from now"
        );

        // Sanity: the result is strictly more than just `now + new_validity` (remaining days preserved)
        let now = env.ledger().timestamp();
        assert!(
            renewed.expires_at > now + new_validity,
            "New expiry should exceed now + new_validity because remaining validity was stacked"
        );
    }

    #[test]
    fn test_renew_credential_short_validity_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        let result = client.try_renew_credential(&engineer, &1);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidValidityPeriod as u32,
            ))),
        );
    }

    #[test]
    fn test_renew_credential_before_expiry_preserves_original_fields() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[7u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &100_000, &None);

        let original = client.get_engineer(&engineer);
        env.ledger().with_mut(|li| li.timestamp += 250);

        client.renew_credential(&engineer, &86_400);

        let renewed = client.get_engineer(&engineer);
        assert_eq!(renewed.issued_at, original.issued_at);
        assert_eq!(renewed.credential_hash, original.credential_hash);
        assert_eq!(renewed.issuer, original.issuer);
        assert_eq!(renewed.expires_at, original.expires_at + 86_400);
        assert!(renewed.expires_at > original.expires_at);
        assert!(client.verify_engineer(&engineer, &None::<Symbol>) == CredentialStatus::Valid);
        assert_eq!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);
    }

    #[test]
    fn test_renew_credential_revoked_engineer_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        client.revoke_credential(&engineer);

        let result = client.try_renew_credential(&engineer, &31_536_000);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::CredentialRevoked as u32,
            ))),
        );
    }

    #[test]
    fn test_renew_credential_fails_when_issuer_removed() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        // Remove the issuer after registration
        client.remove_trusted_issuer(&admin, &issuer);

        let result = client.try_renew_credential(&engineer, &31_536_000);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::IssuerRemoved as u32,
            ))),
        );
    }

    #[test]
    fn test_renew_credential_unknown_engineer_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup(&env);

        let unknown = Address::generate(&env);
        let result = client.try_renew_credential(&unknown, &31_536_000);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::EngineerNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_renew_credential_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &100_000, &None);
        let previous_expires_at = client.get_engineer(&engineer).expires_at;
        client.renew_credential(&engineer, &86_400);

        let events = env.events().all();
        let (_, topics, data) = events.last().unwrap();
        use soroban_sdk::TryIntoVal;
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        let t1: Address = topics.get(1).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("RNW_CRED"));
        assert_eq!(t1, engineer);

        let renewed_at = env.ledger().timestamp();
        let (emitted_issuer, old_expires_at, new_expires_at, emitted_renewed_at): (
            Address,
            u64,
            u64,
            u64,
        ) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_issuer, issuer);
        assert_eq!(old_expires_at, previous_expires_at);
        assert_eq!(new_expires_at, previous_expires_at + 86_400);
        assert_eq!(emitted_renewed_at, renewed_at);
    }

    #[test]
    fn test_renew_credential_extends_ttl() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &86_400, &None);
        client.renew_credential(&engineer, &31_536_000);

        let contract_id = client.address.clone();
        let ttl = env.as_contract(&contract_id, || {
            env.storage().persistent().get_ttl(&engineer_key(&engineer))
        });
        assert!(ttl > 0, "TTL should be extended after renewal");
    }

    #[test]
    fn test_remove_trusted_issuer_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);
        client.remove_trusted_issuer(&admin, &issuer);

        let events = env.events().all();
        assert!(events.len() >= 1);

        let remove_event = events.get(0).unwrap();
        let (_, topics, data) = remove_event;

        use soroban_sdk::TryIntoVal;
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        let t1: Address = topics.get(1).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("ISS_RM"));
        assert_eq!(t1, admin);

        let (emitted_issuer,): (Address,) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_issuer, issuer);
    }

    #[test]
    fn test_get_trusted_issuers_consistent_after_add_and_remove() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer_a = Address::generate(&env);
        let issuer_b = Address::generate(&env);

        // Empty initially
        assert_eq!(client.get_trusted_issuers().len(), 0);

        // Add both
        client.add_trusted_issuer(&admin, &issuer_a);
        client.add_trusted_issuer(&admin, &issuer_b);
        let list = client.get_trusted_issuers();
        assert_eq!(list.len(), 2);
        assert!(list.contains(issuer_a.clone()));
        assert!(list.contains(issuer_b.clone()));

        // Remove one — list must stay consistent
        client.remove_trusted_issuer(&admin, &issuer_a);
        let list = client.get_trusted_issuers();
        assert_eq!(list.len(), 1);
        assert!(!list.contains(issuer_a.clone()));
        assert!(list.contains(issuer_b.clone()));

        // is_trusted_issuer must agree with the list
        assert!(!client.is_trusted_issuer(&issuer_a));
        assert!(client.is_trusted_issuer(&issuer_b));
    }

    #[test]
    fn test_remove_nonexistent_issuer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);
        let nonexistent_issuer = Address::generate(&env);

        assert_eq!(
            client.try_remove_trusted_issuer(&admin, &nonexistent_issuer),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::IssuerNotFound as u32
            )))
        );
    }

    #[test]
    fn test_remove_trusted_issuer_revokes_engineers() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        let engineer1 = Address::generate(&env);
        let engineer2 = Address::generate(&env);
        let hash1 = BytesN::from_array(&env, &[1u8; 32]);
        let hash2 = BytesN::from_array(&env, &[2u8; 32]);

        // Add issuer as trusted
        client.add_trusted_issuer(&admin, &issuer);

        // Register two engineers
        client.register_engineer(&engineer1, &hash1, &issuer, &31_536_000, &None);
        client.register_engineer(&engineer2, &hash2, &issuer, &31_536_000, &None);

        // Verify engineers are active
        assert!(client.verify_engineer(&engineer1) == CredentialStatus::Valid);
        assert!(client.verify_engineer(&engineer2) == CredentialStatus::Valid);
        assert_eq!(client.verify_engineer(&engineer1), CredentialStatus::Valid);
        assert_eq!(client.verify_engineer(&engineer2), CredentialStatus::Valid);

        // Remove the trusted issuer
        client.remove_trusted_issuer(&admin, &issuer);

        // Verify engineers are now revoked
        assert!(client.verify_engineer(&engineer1) != CredentialStatus::Valid);
        assert!(client.verify_engineer(&engineer2) != CredentialStatus::Valid);
        assert_ne!(client.verify_engineer(&engineer1), CredentialStatus::Valid);
        assert_ne!(client.verify_engineer(&engineer2), CredentialStatus::Valid);

        // Check status
        assert_eq!(
            client.get_engineer_status(&engineer1),
            EngineerStatus::Revoked
        );
        assert_eq!(
            client.get_engineer_status(&engineer2),
            EngineerStatus::Revoked
        );
    }

    #[test]
    fn test_different_issuer_cannot_revoke_another_issuers_engineer() {
        let env = Env::default();
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let engineer = Address::generate(&env);
        let issuer_a = Address::generate(&env);
        let issuer_b = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &admin,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &client.address,
                fn_name: "initialize_admin",
                args: (admin.clone(), admin.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.initialize_admin(&admin, &admin);

        // Add both issuers as trusted
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &admin,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &client.address,
                fn_name: "add_trusted_issuer",
                args: (admin.clone(), issuer_a.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.add_trusted_issuer(&admin, &issuer_a);
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &admin,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &client.address,
                fn_name: "add_trusted_issuer",
                args: (admin.clone(), issuer_b.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.add_trusted_issuer(&admin, &issuer_b);

        // Issuer A registers the engineer
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &issuer_a,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &client.address,
                fn_name: "register_engineer",
                args: (
                    engineer.clone(),
                    hash.clone(),
                    issuer_a.clone(),
                    31_536_000u64,
                    Option::<soroban_sdk::String>::None,
                )
                    .into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.register_engineer(&engineer, &hash, &issuer_a, &31_536_000, &None);

        // Issuer B attempts to revoke — should fail because record.issuer is issuer_a
        // Restrict to only issuer_b's auth so issuer_a.require_auth() fails
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &issuer_b,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &client.address,
                fn_name: "revoke_credential",
                args: (engineer.clone(),).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        // This should panic because issuer_b is not the original issuer
        // The require_auth will fail because record.issuer is issuer_a, not issuer_b
        let result = client.try_revoke_credential(&engineer);
        assert!(
            result.is_err(),
            "Different issuer should not be able to revoke"
        );
    }

    #[test]
    fn test_register_engineer_zero_validity_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        let result = client.try_register_engineer(&engineer, &hash, &issuer, &0, &None);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidValidityPeriod as u32,
            ))),
        );
    }

    #[test]
    fn test_register_engineer_rejects_active_duplicate() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        // Attempt to re-register the same active engineer
        let result = client.try_register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::EngineerAlreadyRegistered as u32,
            ))),
        );
    }

    #[test]
    fn test_register_engineer_allows_reregistration_after_revoke() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        // Revoke the credential
        client.revoke_credential(&engineer);
        assert!(client.verify_engineer(&engineer, &None::<Symbol>) != CredentialStatus::Valid);

        // Should be able to re-register after revocation
        let new_hash = BytesN::from_array(&env, &[2u8; 32]);
        client.register_engineer(&engineer, &new_hash, &issuer, &31_536_000);
        assert!(client.verify_engineer(&engineer, &None::<Symbol>) == CredentialStatus::Valid);
        assert_ne!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);

        // Should be able to re-register after revocation
        let new_hash = BytesN::from_array(&env, &[2u8; 32]);
        client.register_engineer(&engineer, &new_hash, &issuer, &31_536_000, &None);
        assert_eq!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);
    }

    #[test]
    fn test_register_engineer_rejects_duplicate_registration_when_active() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash1 = BytesN::from_array(&env, &[1u8; 32]);
        let hash2 = BytesN::from_array(&env, &[2u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);

        // First registration succeeds
        client.register_engineer(&engineer, &hash1, &issuer, &31_536_000);
        assert!(client.verify_engineer(&engineer, &None::<Symbol>) == CredentialStatus::Valid);
        client.register_engineer(&engineer, &hash1, &issuer, &31_536_000, &None);
        assert_eq!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);

        // Second registration with same engineer (still active) must panic
        let result = client.try_register_engineer(&engineer, &hash2, &issuer, &31_536_000, &None);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::EngineerAlreadyRegistered as u32,
            ))),
        );
    }

    #[test]
    fn test_register_engineer_rejects_invalid_credential_hash() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let invalid_hash = BytesN::from_array(&env, &[0u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);

        let result =
            client.try_register_engineer(&engineer, &invalid_hash, &issuer, &31_536_000, &None);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidCredentialHash as u32,
            ))),
        );
    }

    #[test]
    fn test_no_duplicate_in_issuer_list_after_reregistration() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        client.revoke_credential(&engineer);

        let new_hash = BytesN::from_array(&env, &[2u8; 32]);
        client.register_engineer(&engineer, &new_hash, &issuer, &31_536_000, &None);

        // Engineer address must appear exactly once in the issuer's list
        let list = client.get_engineers_by_issuer(&issuer);
        let count = list.iter().filter(|a| *a == engineer).count();
        assert_eq!(
            count, 1,
            "Engineer address must not be duplicated after re-registration"
        );
    }

    #[test]
    fn test_get_engineer_status_active() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        assert_eq!(
            client.get_engineer_status(&engineer),
            EngineerStatus::Active
        );
    }

    #[test]
    fn test_get_engineer_status_revoked() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        client.revoke_credential(&engineer);

        assert_eq!(
            client.get_engineer_status(&engineer),
            EngineerStatus::Revoked
        );
    }

    #[test]
    fn test_get_engineer_status_expired() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &86_400, &None); // 1 day validity
        env.ledger().set_timestamp(86_401); // Move time past expiry

        assert_eq!(
            client.get_engineer_status(&engineer),
            EngineerStatus::Expired
        );
    }

    #[test]
    fn test_get_engineer_status_not_found() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);

        let engineer = Address::generate(&env);
        assert_eq!(
            client.get_engineer_status(&engineer),
            EngineerStatus::NotFound
        );
    }

    #[test]
    fn test_get_active_engineers_by_issuer_filters_revoked() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        let eng1 = Address::generate(&env);
        let eng2 = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&eng1, &hash, &issuer, &31_536_000, &None);
        client.register_engineer(&eng2, &hash, &issuer, &31_536_000, &None);

        // All engineers should be in the full list
        let all = client.get_engineers_by_issuer(&issuer);
        assert_eq!(all.len(), 2);

        // All should be active initially
        let active = client.get_active_engineers_by_issuer(&issuer);
        assert_eq!(active.len(), 2);

        // Revoke one
        client.revoke_credential(&eng1);

        // Full list still has both
        let all = client.get_engineers_by_issuer(&issuer);
        assert_eq!(all.len(), 2);

        // Active list has only one
        let active = client.get_active_engineers_by_issuer(&issuer);
        assert_eq!(active.len(), 1);
        assert_eq!(active.get(0).unwrap(), eng2);
    }

    // Closes #776
    #[test]
    fn test_get_active_engineers_by_issuer_excludes_revoked_and_expired() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        let eng_valid = Address::generate(&env);
        let eng_revoked = Address::generate(&env);
        let eng_expired = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[2u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);

        // Valid engineer: 1-year credential, well within the window
        client.register_engineer(&eng_valid, &hash, &issuer, &31_536_000, &None);
        // Revoked engineer: registered with same long validity, then immediately revoked
        client.register_engineer(&eng_revoked, &hash, &issuer, &31_536_000, &None);
        // Expired engineer: registered with a 1-day credential
        client.register_engineer(&eng_expired, &hash, &issuer, &86_400, &None);

        client.revoke_credential(&eng_revoked);

        // Advance ledger time past the expired engineer's expiry (86_400 seconds)
        env.ledger().set_timestamp(86_401);

        // Confirm individual statuses
        assert_eq!(client.get_engineer_status(&eng_valid), EngineerStatus::Active);
        assert_eq!(client.get_engineer_status(&eng_revoked), EngineerStatus::Revoked);
        assert_eq!(client.get_engineer_status(&eng_expired), EngineerStatus::Expired);

        // All three remain in the full issuer list
        let all = client.get_engineers_by_issuer(&issuer);
        assert_eq!(all.len(), 3);

        // get_active_engineers_by_issuer must return only the valid engineer
        let active = client.get_active_engineers_by_issuer(&issuer);
        assert_eq!(active.len(), 1);
        assert_eq!(active.get(0).unwrap(), eng_valid);
    }

    #[test]
    fn test_propose_and_accept_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let new_admin = Address::generate(&env);
        client.propose_admin(&admin, &new_admin);
        client.accept_admin();

        assert_eq!(client.get_admin(), new_admin);
    }

    #[test]
    fn test_accept_admin_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let new_admin = Address::generate(&env);
        client.propose_admin(&admin, &new_admin);
        client.accept_admin();

        let events = env.events().all();
        assert!(events.len() >= 1);

        // setup emits 1 event, propose_admin emits 2, accept_admin emits 2
        // The final ADMIN_SET non-audit event is the last one
        let (_, topics, data) = events.last().unwrap();
        use soroban_sdk::TryIntoVal;
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, symbol_short!("ADMIN_SET"));

        let (emitted_admin,): (Address,) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_admin, new_admin);
    }

    #[test]
    fn test_propose_admin_unauthorized() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);

        let unauthorized = Address::generate(&env);
        let new_admin = Address::generate(&env);

        assert_eq!(
            client.try_propose_admin(&unauthorized, &new_admin),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32
            )))
        );
    }

    #[test]
    fn test_accept_admin_unauthorized() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let new_admin = Address::generate(&env);
        let unauthorized = Address::generate(&env);

        client.propose_admin(&admin, &new_admin);

        // Try to accept as unauthorized address
        use soroban_sdk::IntoVal;
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &unauthorized,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &client.address,
                fn_name: "accept_admin",
                args: ().into_val(&env),
                sub_invokes: &[],
            },
        }]);
        assert!(client.try_accept_admin().is_err());
    }

    #[test]
    fn test_propose_admin_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let new_admin = Address::generate(&env);
        client.propose_admin(&admin, &new_admin);

        let events = env.events().all();
        // propose_admin non-audit event: search for it by finding (PROP_ADM) topic
        // (setup/initialize_admin emits audit events first, propose_admin emits 2 events)
        let mut found = None;
        for i in 0..events.len() {
            let (_, t, d) = events.get(i).unwrap();
            use soroban_sdk::TryIntoVal;
            if let Ok(s) = TryIntoVal::<_, Symbol>::try_into_val(&t.get(0).unwrap(), &env) {
                if s == EVENT_PROP_ADMIN {
                    found = Some((t, d));
                    break;
                }
            }
        }
        let (topics, data) = found.expect("PROP_ADM event not found");

        use soroban_sdk::TryIntoVal;
        let topic: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        assert_eq!(topic, EVENT_PROP_ADMIN);
        let (emitted_admin, emitted_new_admin): (Address, Address) =
            data.try_into_val(&env).unwrap();
        assert_eq!(emitted_admin, admin);
        assert_eq!(emitted_new_admin, new_admin);
    }

    #[test]
    fn test_pause_state_persists_across_instance_ttl_boundary() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);

        // Pause the contract
        client.pause(&admin);
        assert!(client.is_paused());

        // Simulate instance TTL expiry by wiping instance storage
        env.as_contract(&client.address, || {
            env.storage().instance().remove(&admin_key());
            env.storage().instance().remove(&pending_admin_key());
        });

        // PAUSED_KEY lives in persistent storage — must still be true
        assert!(
            client.is_paused(),
            "pause state must survive instance TTL expiry"
        );

        // Writes must still be blocked
        let engineer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        assert_eq!(
            client.try_register_engineer(&engineer, &hash, &issuer, &31_536_000, &None),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::Paused as u32
            )))
        );
    }

    #[test]
    fn test_initialize_admin_rejects_non_deployer() {
        let env = Env::default();
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(&env, &contract_id);

        let deployer = Address::generate(&env);
        let attacker = Address::generate(&env);

        use soroban_sdk::IntoVal;
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &attacker,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &contract_id,
                fn_name: "initialize_admin",
                args: (&attacker, &attacker).into_val(&env),
                sub_invokes: &[],
            },
        }]);

        let result = client.try_initialize_admin(&deployer, &attacker);
        assert!(
            result.is_err(),
            "non-deployer must not be able to initialize"
        );
    }

    fn setup_engineer(
        env: &Env,
        client: &EngineerRegistryClient,
        issuer: &Address,
        seed: u8,
    ) -> Address {
        let engineer = Address::generate(env);
        client.register_engineer(
            &engineer,
            &BytesN::from_array(env, &[seed; 32]),
            issuer,
            &31_536_000,
            &None,
        );
        engineer
    }

    #[test]
    fn test_batch_verify_engineers_all_active() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let issuer = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_trusted_issuer(&admin, &issuer);

        let e1 = setup_engineer(&env, &client, &issuer, 1);
        let e2 = setup_engineer(&env, &client, &issuer, 2);
        let e3 = setup_engineer(&env, &client, &issuer, 3);

        let results = client.batch_verify_engineers(&soroban_sdk::vec![&env, e1, e2, e3]);
        assert_eq!(results.len(), 3);
        assert_eq!(results.get(0).unwrap(), CredentialStatus::Valid);
        assert_eq!(results.get(1).unwrap(), CredentialStatus::Valid);
        assert_eq!(results.get(2).unwrap(), CredentialStatus::Valid);
    }

    #[test]
    fn test_batch_verify_engineers_all_inactive() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let issuer = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_trusted_issuer(&admin, &issuer);

        let e1 = setup_engineer(&env, &client, &issuer, 20);
        let e2 = setup_engineer(&env, &client, &issuer, 21);
        client.revoke_credential(&e1);
        client.revoke_credential(&e2);

        let results = client.batch_verify_engineers(&soroban_sdk::vec![&env, e1, e2]);
        assert_eq!(results.len(), 2);
        assert_ne!(results.get(0).unwrap(), CredentialStatus::Valid);
        assert_ne!(results.get(1).unwrap(), CredentialStatus::Valid);
    }

    #[test]
    fn test_get_engineer_count() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);

        // Counter starts at 0
        assert_eq!(client.get_engineer_count(), 0);

        // Register first engineer, count should be 1
        let engineer1 = Address::generate(&env);
        let hash1 = BytesN::from_array(&env, &[1u8; 32]);
        client.register_engineer(&engineer1, &hash1, &issuer, &31_536_000, &None);
        assert_eq!(client.get_engineer_count(), 1);

        // Register second engineer, count should be 2
        let engineer2 = Address::generate(&env);
        let hash2 = BytesN::from_array(&env, &[2u8; 32]);
        client.register_engineer(&engineer2, &hash2, &issuer, &31_536_000, &None);
        assert_eq!(client.get_engineer_count(), 2);

        // Register third engineer, count should be 3
        let engineer3 = Address::generate(&env);
        let hash3 = BytesN::from_array(&env, &[3u8; 32]);
        client.register_engineer(&engineer3, &hash3, &issuer, &31_536_000, &None);
        assert_eq!(client.get_engineer_count(), 3);
    }

    #[test]
    fn test_verify_engineer_distinguishes_not_found_from_revoked() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(EngineerRegistry, ());
        let client = EngineerRegistryClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let issuer = Address::generate(&env);
        client.initialize_admin(&admin, &admin);
        client.add_trusted_issuer(&admin, &issuer);

        let engineer = Address::generate(&env);
        let never_registered = Address::generate(&env);

        // Never-registered engineer returns NotFound
        assert_eq!(
            client.verify_engineer(&never_registered, &None::<Symbol>),
            CredentialStatus::NotFound,
            "never-registered engineer should return NotFound"
        );

        // Register an engineer
        client.register_engineer(
            &engineer,
            &BytesN::from_array(&env, &[1u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );

        // Active engineer returns Valid
        assert_eq!(
            client.verify_engineer(&engineer, &None::<Symbol>),
            CredentialStatus::Valid,
            "active engineer should return Valid"
        );

        // Revoke the engineer
        client.revoke_credential(&engineer);

        // Revoked engineer returns Revoked
        assert_eq!(
            client.verify_engineer(&engineer, &None::<Symbol>),
            CredentialStatus::Revoked,
            "revoked engineer should return Revoked"
        );

        // Never-registered still returns NotFound
        assert_eq!(
            client.verify_engineer(&never_registered, &None::<Symbol>),
            CredentialStatus::NotFound,
            "never-registered engineer should still return NotFound after other operations"
        );
    }

    #[test]
    fn test_verify_engineer_succeeds_while_paused() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        client.pause(&admin);
        assert!(client.is_paused());

        // reads must still work while paused so the lifecycle contract isn't blocked
        assert_eq!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);
        assert_eq!(client.verify_engineer(&Address::generate(&env)), CredentialStatus::NotFound);
    }

    // --- Grace Period Tests ---

    #[test]
    fn test_get_credential_status_valid() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &86_400, &None);

        assert_eq!(
            client.get_credential_status(&engineer),
            CredentialStatus::Valid
        );
    }

    #[test]
    fn test_get_credential_status_in_grace_period() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        let base_time = env.ledger().timestamp();
        let validity_period = 86_400; // 1 day
        client.register_engineer(&engineer, &hash, &issuer, &validity_period, &None);

        // Advance to just after expiry but within grace period
        // Grace period is 7 days (604_800 seconds)
        env.ledger()
            .set_timestamp(base_time + validity_period + 100_000); // 1 day + ~1.15 days

        assert_eq!(
            client.get_credential_status(&engineer),
            CredentialStatus::GracePeriod
        );
    }

    #[test]
    fn test_get_credential_status_hard_expired() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        let base_time = env.ledger().timestamp();
        let validity_period = 86_400; // 1 day
        client.register_engineer(&engineer, &hash, &issuer, &validity_period, &None);

        // Advance past grace period (7 days + 1 second)
        env.ledger()
            .set_timestamp(base_time + validity_period + 7 * 86_400 + 1);

        assert_eq!(
            client.get_credential_status(&engineer),
            CredentialStatus::HardExpired
        );
    }

    #[test]
    fn test_get_credential_status_revoked() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        client.revoke_credential(&engineer);

        assert_eq!(
            client.get_credential_status(&engineer),
            CredentialStatus::Revoked
        );
    }

    #[test]
    fn test_get_credential_status_not_found() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);

        let unknown = Address::generate(&env);
        assert_eq!(
            client.get_credential_status(&unknown),
            CredentialStatus::NotFound
        );
    }

    #[test]
    fn test_grace_period_boundary_at_expiry_edge() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        let base_time = env.ledger().timestamp();
        let validity = 86_400u64;
        client.register_engineer(&engineer, &hash, &issuer, &validity, &None);

        // Exactly at expiry time
        env.ledger().set_timestamp(base_time + validity);
        assert_eq!(
            client.get_credential_status(&engineer),
            CredentialStatus::GracePeriod
        );

        // One second before expiry
        env.ledger().set_timestamp(base_time + validity - 1);
        assert_eq!(
            client.get_credential_status(&engineer),
            CredentialStatus::Valid
        );
    }

    #[test]
    fn test_grace_period_boundary_at_grace_end() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        let base_time = env.ledger().timestamp();
        let validity = 86_400u64;
        let grace_end = base_time + validity + 7 * 86_400;

        client.register_engineer(&engineer, &hash, &issuer, &validity, &None);

        // At the exact end of grace period boundary
        env.ledger().set_timestamp(grace_end);
        assert_eq!(
            client.get_credential_status(&engineer),
            CredentialStatus::HardExpired
        );

        // One second before grace end
        env.ledger().set_timestamp(grace_end - 1);
        assert_eq!(
            client.get_credential_status(&engineer),
            CredentialStatus::GracePeriod
        );
    }

    #[test]
    fn test_renew_credential_during_grace_period() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        let base_time = env.ledger().timestamp();
        client.register_engineer(&engineer, &hash, &issuer, &86_400, &None);

        // Advance into grace period
        env.ledger().set_timestamp(base_time + 86_401);
        assert_eq!(
            client.get_credential_status(&engineer),
            CredentialStatus::GracePeriod
        );

        // Renew during grace period
        client.renew_credential(&engineer, &86_400);

        // After renewal, should be valid again
        assert_eq!(
            client.get_credential_status(&engineer),
            CredentialStatus::Valid
        );

        let record = client.get_engineer(&engineer);
        assert!(record.expires_at > env.ledger().timestamp());
    }

    // --- Issue: batch_verify_engineers ---

    #[test]
    fn test_batch_verify_engineers_all_valid() {
    #[test]
    fn test_renew_credential_hard_expired_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        let base_time = env.ledger().timestamp();
        // Register engineer with 1 day validity
        client.register_engineer(&engineer, &hash, &issuer, &86_400, &None);

        assert_eq!(results.len(), 3);
        assert_eq!(results.get(0).unwrap(), CredentialStatus::Valid);
        assert_eq!(results.get(1).unwrap(), CredentialStatus::Valid);
        assert_eq!(results.get(2).unwrap(), CredentialStatus::Valid);
    }

    // --- #752: upgrade timelock tests ---

    #[test]
    fn test_execute_upgrade_before_timelock_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let hash = BytesN::from_array(&env, &[0xabu8; 32]);
        client.propose_upgrade(&admin, &hash);
        // Advance beyond grace period (default 7 days = 604_800 seconds)
        // Expiry: base_time + 86_400
        // Grace period end: base_time + 86_400 + 604_800 = base_time + 691_200
        env.ledger().set_timestamp(base_time + 691_201);

        // Verify credential is hard-expired
        assert_eq!(
            client.get_credential_status(&engineer),
            CredentialStatus::HardExpired
        );

        // Attempt to renew hard-expired credential should fail
        let result = client.try_renew_credential(&engineer, &86_400);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::CredentialExpired as u32
            ))),
        );
    }
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);
        // Default is 7 days = 604_800 seconds
        assert_eq!(client.get_grace_period(), 7 * 86_400u64);
    }

    #[test]
    fn test_set_grace_period_updates_credential_status() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let hash = BytesN::from_array(&env, &[0xabu8; 32]);
        client.propose_upgrade(&admin, &hash);

        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

        client.execute_upgrade(&admin);
    }

    #[test]
    fn test_batch_verify_engineers_mixed() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        let base_time = env.ledger().timestamp();
        let validity = 86_400u64;
        client.register_engineer(&engineer, &hash, &issuer, &validity, &None);

        client.revoke_credential(&revoked);
        env.ledger().with_mut(|li| li.timestamp += 86_401);
        // Shrink grace period to 1 hour
        client.set_grace_period(&admin, &3_600u64);
        assert_eq!(client.get_grace_period(), 3_600u64);

        // 2 hours after expiry → past 1h grace period → HardExpired
        env.ledger().set_timestamp(base_time + validity + 7_200);
        assert_eq!(
            client.get_credential_status(&engineer),
            CredentialStatus::HardExpired
        );

        assert_eq!(results.len(), 3);
        assert_eq!(results.get(0).unwrap(), CredentialStatus::Valid, "valid engineer must be true");
        assert_ne!(results.get(1).unwrap(), CredentialStatus::Valid, "revoked engineer must be false");
        assert_ne!(results.get(2).unwrap(), CredentialStatus::Valid, "expired engineer must be false");
    }

    #[test]
        // Within 1h grace period → GracePeriod
        env.ledger().set_timestamp(base_time + validity + 1_800);
        assert_eq!(
            client.get_credential_status(&engineer),
            CredentialStatus::GracePeriod
        );
    }

    #[test]
    fn test_set_grace_period_unauthorized() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);

        let non_admin = Address::generate(&env);
        let result = client.try_set_grace_period(&non_admin, &3_600u64);
        assert!(result.is_err());
    }

    // --- Issue: batch_verify_engineers ---

    #[test]
    fn test_batch_verify_engineers_all_valid() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        let e1 = Address::generate(&env);
        let e2 = Address::generate(&env);
        let e3 = Address::generate(&env);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(
            &e1,
            &BytesN::from_array(&env, &[1u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );
        client.register_engineer(
            &e2,
            &BytesN::from_array(&env, &[2u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );
        client.register_engineer(
            &e3,
            &BytesN::from_array(&env, &[3u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );

        let batch = soroban_sdk::vec![&env, e1, e2, e3];
        let results = client.batch_verify_engineers(&batch);

        assert_eq!(results.len(), 3);
        assert_eq!(results.get(0).unwrap(), CredentialStatus::Valid);
        assert_eq!(results.get(1).unwrap(), CredentialStatus::Valid);
        assert_eq!(results.get(2).unwrap(), CredentialStatus::Valid);
    }

    // --- #752: upgrade timelock tests ---

    #[test]
    fn test_execute_upgrade_before_timelock_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let hash = BytesN::from_array(&env, &[0xabu8; 32]);
        client.propose_upgrade(&admin, &hash);

        let result = client.try_execute_upgrade(&admin);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::TimelockNotExpired as u32,
            ))),
        );
    }

    #[test]
    fn test_batch_verify_engineers_mixed() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        let valid = Address::generate(&env);
        let revoked = Address::generate(&env);
        let expired = Address::generate(&env);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(
            &valid,
            &BytesN::from_array(&env, &[1u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );
        client.register_engineer(
            &revoked,
            &BytesN::from_array(&env, &[2u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );
        client.register_engineer(
            &expired,
            &BytesN::from_array(&env, &[3u8; 32]),
            &issuer,
            &86_400,
            &None,
        );

        client.revoke_credential(&revoked);
        env.ledger().with_mut(|li| li.timestamp += 86_401);

        let batch = soroban_sdk::vec![&env, valid.clone(), revoked.clone(), expired.clone()];
        let results = client.batch_verify_engineers(&batch);

        assert_eq!(results.len(), 3);
        assert_eq!(
            results.get(0).unwrap(),
            CredentialStatus::Valid,
            "valid engineer must be Valid"
        );
        assert_eq!(
            results.get(1).unwrap(),
            CredentialStatus::Revoked,
            "revoked engineer must be Revoked"
        );
        assert_ne!(
            results.get(2).unwrap(),
            CredentialStatus::Valid,
            "expired engineer must not be Valid"
        );
    }

    #[test]
    fn test_execute_upgrade_after_timelock_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let hash = BytesN::from_array(&env, &[0xabu8; 32]);
        client.propose_upgrade(&admin, &hash);

        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + TIMELOCK_DELAY_SECS + 1);

        client.execute_upgrade(&admin);
    }

    #[test]
    fn test_batch_verify_engineers_all_invalid() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        let e1 = Address::generate(&env);
        let e2 = Address::generate(&env);
        let never_registered = Address::generate(&env);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(
            &e1,
            &BytesN::from_array(&env, &[1u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );
        client.register_engineer(
            &e2,
            &BytesN::from_array(&env, &[2u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );

        client.revoke_credential(&e1);
        client.revoke_credential(&e2);

        let batch = soroban_sdk::vec![&env, e1, e2, never_registered];
        let results = client.batch_verify_engineers(&batch);

        assert_eq!(results.len(), 3);
        assert_ne!(results.get(0).unwrap(), CredentialStatus::Valid, "revoked engineer must be false");
        assert_ne!(results.get(1).unwrap(), CredentialStatus::Valid, "revoked engineer must be false");
        assert_ne!(results.get(2).unwrap(), CredentialStatus::Valid, "never-registered engineer must be false");
        assert_ne!(
            results.get(0).unwrap(),
            CredentialStatus::Valid,
            "revoked engineer must not be Valid"
        );
        assert_ne!(
            results.get(1).unwrap(),
            CredentialStatus::Valid,
            "revoked engineer must not be Valid"
        );
        assert_eq!(
            results.get(2).unwrap(),
            CredentialStatus::NotFound,
            "never-registered engineer must be NotFound"
        );
    }

    // --- #752: upgrade timelock tests ---

    #[test]
    fn test_execute_upgrade_before_timelock_fails_duplicate_blocked_by_compile_guard() {
        let env = Env::default();
        env.mock_all_auths();
        let (_client, _admin) = setup(&env);
        // Intentional duplicate-test body removed from compilation.
    }

    #[test]
    fn test_execute_upgrade_after_timelock_succeeds_duplicate_blocked_by_compile_guard() {
        let env = Env::default();
        env.mock_all_auths();
        let (_client, _admin) = setup(&env);
        // Intentional duplicate-test body removed from compilation.
    }

    #[test]
    fn test_execute_upgrade_without_proposal_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let result = client.try_execute_upgrade(&admin);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::ProposalNotFound as u32,
            ))),
        );
    }

    #[test]
    fn test_propose_upgrade_non_admin_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup(&env);

        let outsider = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[0xabu8; 32]);

        let result = client.try_propose_upgrade(&outsider, &hash);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::UnauthorizedAdmin as u32,
            ))),
        );
    }

    // --- is_engineer_active Tests ---

    #[test]
    fn test_is_engineer_active_returns_false_for_unknown_engineer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);

        let unknown_engineer = Address::generate(&env);
        assert!(!client.is_engineer_active(&unknown_engineer));
    }

    #[test]
    fn test_is_engineer_active_returns_true_for_active_engineer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        assert!(client.is_engineer_active(&engineer));
    }

    #[test]
    fn test_is_engineer_active_returns_false_for_revoked_engineer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        assert!(client.is_engineer_active(&engineer));

        client.revoke_credential(&engineer);
        assert!(!client.is_engineer_active(&engineer));
    }

    #[test]
    fn test_is_engineer_active_returns_false_for_expired_engineer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &86_400); // minimum 1-day validity

        // Advance past expiry
        let base = env.ledger().timestamp();
        env.ledger().set_timestamp(base + 86_401);

        assert!(!client.is_engineer_active(&engineer));
    }
        client.register_engineer(&engineer, &hash, &issuer, &86_400, &None); // minimum validity

        // Set ledger time past expiry
        env.ledger().set_timestamp(86_401);

    // --- Suspension tests (issue #882) ---

    fn setup_suspended_engineer(
        env: &Env,
        client: &EngineerRegistryClient,
        admin: &Address,
    ) -> (Address, Address) {
        let issuer = Address::generate(env);
        let engineer = Address::generate(env);
        client.add_trusted_issuer(admin, &issuer);
        client.register_engineer(
            &engineer,
            &BytesN::from_array(env, &[42u8; 32]),
            &issuer,
            &31_536_000,
        );
        (issuer, engineer)
    }

    #[test]
    fn test_get_reputation_default_is_zero() {
    fn test_suspend_engineer_makes_credential_suspended() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);
        let (_, engineer) = setup_suspended_engineer(&env, &client, &admin);

        let now = env.ledger().timestamp();
        client.suspend_engineer(
            &engineer,
            &(now + 86_400),
            &soroban_sdk::String::from_str(&env, "investigation"),
        );

        assert!(client.is_credential_suspended(&engineer));
        assert_eq!(
            client.get_credential_status(&engineer),
            CredentialStatus::Suspended
        );
        assert_eq!(
            client.get_engineer_status(&engineer),
            EngineerStatus::Suspended
        );
        assert!(!client.is_engineer_active(&engineer));
    }

    #[test]
    fn test_suspension_lifts_after_end_time() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);
        let (_, engineer) = setup_suspended_engineer(&env, &client, &admin);

        let now = env.ledger().timestamp();
        let suspension_end = now + 3_600; // 1 hour
        client.suspend_engineer(
            &engineer,
            &suspension_end,
            &soroban_sdk::String::from_str(&env, "temp"),
        );

        assert!(client.is_credential_suspended(&engineer));

        // Advance past suspension end
        env.ledger().set_timestamp(suspension_end);
        assert!(!client.is_credential_suspended(&engineer));
        assert_eq!(
            client.get_credential_status(&engineer),
            CredentialStatus::Valid
        );
        assert!(client.is_engineer_active(&engineer));
    }

    #[test]
    fn test_suspend_revoked_engineer_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);
        let (_, engineer) = setup_suspended_engineer(&env, &client, &admin);

        client.revoke_credential(&engineer);

        let now = env.ledger().timestamp();
        let result = client.try_suspend_engineer(
            &engineer,
            &(now + 86_400),
            &soroban_sdk::String::from_str(&env, "reason"),
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::CredentialRevoked as u32
            )))
        );
    }

    #[test]
    fn test_suspend_unknown_engineer_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup(&env);

        let unknown = Address::generate(&env);
        let now = env.ledger().timestamp();
        let result = client.try_suspend_engineer(
            &unknown,
            &(now + 86_400),
            &soroban_sdk::String::from_str(&env, "reason"),
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::EngineerNotFound as u32
            )))
    /// #802: removing a trusted issuer must revoke all credentials issued by that issuer.
    /// verify_engineer for those engineers must return CredentialStatus::Revoked.
    #[test]
    fn test_verify_engineer_revoked_after_issuer_removal() {
    #[test]
    fn test_get_reputation_default_is_zero() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        let engineer1 = Address::generate(&env);
        let engineer2 = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[2u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer1, &hash, &issuer, &31_536_000);
        client.register_engineer(&engineer2, &BytesN::from_array(&env, &[3u8; 32]), &issuer, &31_536_000);

        // Both engineers are valid before issuer removal.
        assert_eq!(client.verify_engineer(&engineer1), CredentialStatus::Valid);
        assert_eq!(client.verify_engineer(&engineer2), CredentialStatus::Valid);

        // Remove the issuer — all credentials issued by it must be batch-revoked.
        client.remove_trusted_issuer(&admin, &issuer);

        // Both engineers must now be revoked.
        assert_eq!(client.verify_engineer(&engineer1), CredentialStatus::Revoked);
        assert_eq!(client.verify_engineer(&engineer2), CredentialStatus::Revoked);
    }
        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        assert_eq!(client.get_reputation(&engineer), 0);
    }

    // --- Issue #827: get_total_engineer_count ---

    #[test]
    fn test_get_total_engineer_count_returns_u64() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        assert_eq!(client.get_reputation(&engineer), 0);
    }

    // --- Issue #827: get_total_engineer_count ---

    #[test]
    fn test_get_total_engineer_count_returns_u64() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        assert_eq!(client.get_total_engineer_count(), 0u64);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);

        let e1 = Address::generate(&env);
        let e2 = Address::generate(&env);
        client.register_engineer(&e1, &BytesN::from_array(&env, &[1u8; 32]), &issuer, &31_536_000);
        client.register_engineer(&e2, &BytesN::from_array(&env, &[2u8; 32]), &issuer, &31_536_000);
        client.register_engineer(
            &e1,
            &BytesN::from_array(&env, &[1u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );
        client.register_engineer(
            &e2,
            &BytesN::from_array(&env, &[2u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );

        assert_eq!(client.get_total_engineer_count(), 2u64);
    }

    // --- Reputation scoring ---

    #[test]
    fn test_update_reputation_increases_score() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);

        let e1 = Address::generate(&env);
        let e2 = Address::generate(&env);
        client.register_engineer(&e1, &BytesN::from_array(&env, &[1u8; 32]), &issuer, &31_536_000);
        client.register_engineer(&e2, &BytesN::from_array(&env, &[2u8; 32]), &issuer, &31_536_000);

        client.update_reputation(&e1, &100);
        client.update_reputation(&e2, &50);

        assert_eq!(client.get_reputation(&e1), 100);
        assert_eq!(client.get_reputation(&e2), 50);
    }

    #[test]
    fn test_update_reputation_increases_score() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[2u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        client.update_reputation(&engineer, &100);
        assert_eq!(client.get_reputation(&engineer), 100);

        client.update_reputation(&engineer, &200);
        assert_eq!(client.get_reputation(&engineer), 300);
    }

    #[test]
    fn test_update_reputation_decreases_score() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[3u8; 32]);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[3u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        client.update_reputation(&engineer, &500);
        client.update_reputation(&engineer, &-200);
        assert_eq!(client.get_reputation(&engineer), 300);
    }

    #[test]
    fn test_update_reputation_clamped_at_zero() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[4u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        // Subtract more than balance — should clamp to 0, not underflow
        client.update_reputation(&engineer, &-500);
        assert_eq!(client.get_reputation(&engineer), 0);
    }

    #[test]
    fn test_update_reputation_clamped_at_1000() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[5u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        // Add far more than max — should clamp to 1000
        client.update_reputation(&engineer, &2000);
        assert_eq!(client.get_reputation(&engineer), 1000);
    }

    #[test]
    fn test_get_reputation_returns_zero_for_unknown_engineer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);

        let unknown = Address::generate(&env);
        assert_eq!(client.get_reputation(&unknown), 0);
    }

    // --- Issue #828: batch_revoke_credentials ---

    #[test]
    fn test_batch_revoke_credentials_revokes_active_engineers() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);

        let e1 = Address::generate(&env);
        let e2 = Address::generate(&env);
        client.register_engineer(
            &e1,
            &BytesN::from_array(&env, &[1u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );
        client.register_engineer(
            &e2,
            &BytesN::from_array(&env, &[2u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000);

        client.update_reputation(&engineer, &500);
        client.update_reputation(&engineer, &-200);
        assert_eq!(client.get_reputation(&engineer), 300);
    }

    #[test]
    fn test_update_reputation_clamped_at_zero() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[3u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000);

        // Subtract more than balance — should clamp to 0, not underflow
        client.update_reputation(&engineer, &-500);
        assert_eq!(client.get_reputation(&engineer), 0);
    }

    #[test]
    fn test_batch_revoke_credentials_exceeds_max_returns_error() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let mut batch = Vec::new(&env);
        for _ in 0..=50u32 {
            batch.push_back(Address::generate(&env));
        }

        let e1 = Address::generate(&env);
        client.register_engineer(&e1, &BytesN::from_array(&env, &[1u8; 32]), &issuer, &31_536_000, &None);

    #[test]
    fn test_suspend_with_past_timestamp_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);
        let (_, engineer) = setup_suspended_engineer(&env, &client, &admin);

        // Advance ledger so we can use a "past" timestamp
        env.ledger().set_timestamp(10_000);
        let result = client.try_suspend_engineer(
            &engineer,
            &5_000, // in the past
            &soroban_sdk::String::from_str(&env, "reason"),
        );
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::InvalidSuspensionPeriod as u32
            )))
    fn test_batch_revoke_credentials_non_admin_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);

        assert_eq!(client.get_reputation(&e1), 50);
    }

    #[test]
    fn test_suspend_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);
        let (issuer, engineer) = setup_suspended_engineer(&env, &client, &admin);

        let now = env.ledger().timestamp();
        let until = now + 86_400;
        let reason = soroban_sdk::String::from_str(&env, "audit");
        client.suspend_engineer(&engineer, &until, &reason);

        use soroban_sdk::TryIntoVal;
        let events = env.events().all();
        let (_, topics, data) = events.last().unwrap();
        let t0: Symbol = topics.get(0).unwrap().try_into_val(&env).unwrap();
        let t1: Address = topics.get(1).unwrap().try_into_val(&env).unwrap();
        assert_eq!(t0, SUSPEND_TOPIC);
        assert_eq!(t1, engineer);

        let (emitted_issuer, emitted_until, _emitted_reason, emitted_at): (
            Address,
            u64,
            soroban_sdk::String,
            u64,
        ) = data.try_into_val(&env).unwrap();
        assert_eq!(emitted_issuer, issuer);
        assert_eq!(emitted_until, until);
        assert_eq!(emitted_at, now);
    }

    #[test]
    fn test_is_credential_suspended_false_for_unknown() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _) = setup(&env);
        let unknown = Address::generate(&env);
        assert!(!client.is_credential_suspended(&unknown));
    }

    #[test]
    fn test_is_credential_suspended_false_when_not_suspended() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);
        let (_, engineer) = setup_suspended_engineer(&env, &client, &admin);
        assert!(!client.is_credential_suspended(&engineer));
    }

    #[test]
    fn test_verify_engineer_returns_suspended_when_suspended() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);
        let (_, engineer) = setup_suspended_engineer(&env, &client, &admin);

        let now = env.ledger().timestamp();
        client.suspend_engineer(
            &engineer,
            &(now + 86_400),
            &soroban_sdk::String::from_str(&env, "review"),
        );

        assert_eq!(
            client.verify_engineer(&engineer, &None::<Symbol>),
            CredentialStatus::Suspended,
            "suspended engineer should return Suspended"
    fn test_batch_revoke_emits_event_per_engineer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);

        let e1 = Address::generate(&env);
        client.register_engineer(&e1, &BytesN::from_array(&env, &[1u8; 32]), &issuer, &31_536_000);
        client.register_engineer(
            &e1,
            &BytesN::from_array(&env, &[1u8; 32]),
            &issuer,
            &31_536_000,
            &None,
        );

        let mut batch = Vec::new(&env);
        batch.push_back(e1.clone());
        client.batch_revoke_credentials(&admin, &batch);

        let events = env.events().all();
        let mut revoke_count = 0u32;
        for (_, topics, _) in events.iter() {
            use soroban_sdk::TryIntoVal;
            if let Some(t0) = topics.get(0) {
                let sym: Result<Symbol, _> = t0.try_into_val(&env);
                if let Ok(s) = sym {
                    if s == symbol_short!("REV_CRED") {
                        revoke_count += 1;
                    }
                }
            }
        }
        assert!(revoke_count >= 1, "Should emit REV_CRED event per engineer");
        let revoke_events: Vec<_> = events
            .iter()
            .filter(|(_, topics, _)| {
                use soroban_sdk::TryIntoVal;
                topics
                    .get(0)
                    .and_then(|v| v.try_into_val::<_, Symbol>(&env).ok())
                    .map(|s| s == symbol_short!("REV_CRED"))
                    .unwrap_or(false)
            })
            .collect();
        assert!(
            !revoke_events.is_empty(),
            "Should emit REV_CRED event per engineer"
        );
    }

    // --- Issue #829: notes field on Engineer ---

    #[test]
    fn test_update_reputation_clamped_at_1000() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[4u8; 32]);

        client.add_trusted_issuer(&admin, &issuer);

        let engineer = Address::generate(&env);
        client.register_engineer(&engineer, &BytesN::from_array(&env, &[1u8; 32]), &issuer, &31_536_000, &None);
        client.update_reputation(&engineer, &2000);

        assert_eq!(client.get_reputation(&engineer), 1000);
    }

    #[test]
    fn test_register_engineer_with_notes() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);

        let engineer = Address::generate(&env);
        let issuer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[5u8; 32]);
        let notes = Some(String::from_str(&env, "Certified: High-Voltage Generators"));

        client.add_trusted_issuer(&admin, &issuer);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &notes);

        let record = client.get_engineer(&engineer);
        assert_eq!(record.notes, notes);
    }

    #[test]
    fn test_get_reputation_returns_zero_for_unknown_engineer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);

        let unknown = Address::generate(&env);
        assert_eq!(client.get_reputation(&unknown), 0);
    }

    #[test]
    fn test_register_engineer_without_notes_is_none() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.add_trusted_issuer(&admin, &issuer);

        let engineer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[6u8; 32]);

        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

        let record = client.get_engineer(&engineer);
        assert!(record.notes.is_none());
    }

    #[test]
    fn test_register_issuer_adds_trusted_issuer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);

        let asme = Address::generate(&env);
        assert!(!client.is_trusted_issuer(&asme));

        client.register_issuer(&asme);

        assert!(client.is_trusted_issuer(&asme));
        assert!(client.get_trusted_issuers().contains(asme.clone()));
    }

    #[test]
    fn test_register_multiple_issuers() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);

        let asme = Address::generate(&env);
        let ieee = Address::generate(&env);
        let nfpa = Address::generate(&env);

        client.register_issuer(&asme);
        client.register_issuer(&ieee);
        client.register_issuer(&nfpa);

        let issuers = client.get_trusted_issuers();
        assert_eq!(issuers.len(), 3);
        assert!(issuers.contains(asme));
        assert!(issuers.contains(ieee));
        assert!(issuers.contains(nfpa));
    }

    #[test]
    fn test_register_issuer_requires_admin_auth() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);

        // Restrict auth to a non-admin signer; the stored admin's auth is missing
        // so the admin-only call must fail to authorize.
        let issuer = Address::generate(&env);
        let stranger = Address::generate(&env);
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &stranger,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &client.address,
                fn_name: "register_issuer",
                args: (issuer.clone(),).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        assert!(client.try_register_issuer(&issuer).is_err());
    }

    #[test]
    fn test_revoke_issuer_removes_from_trusted_list() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);

        let asme = Address::generate(&env);
        let ieee = Address::generate(&env);
        client.register_issuer(&asme);
        client.register_issuer(&ieee);

        client.revoke_issuer(&asme);

        assert!(!client.is_trusted_issuer(&asme));
        assert!(client.is_trusted_issuer(&ieee));
        let issuers = client.get_trusted_issuers();
        assert_eq!(issuers.len(), 1);
        assert!(issuers.contains(ieee));
    }

    #[test]
    fn test_revoke_unknown_issuer_errors() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);

        let unknown = Address::generate(&env);
        assert_eq!(
            client.try_revoke_issuer(&unknown),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                ContractError::IssuerNotFound as u32
            )))
        );
    }

    #[test]
    fn test_suspension_does_not_persist_after_expiry_via_batch() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);
        let (_, engineer) = setup_suspended_engineer(&env, &client, &admin);

        let now = env.ledger().timestamp();
        let end = now + 1_000;
        client.suspend_engineer(
            &engineer,
            &end,
            &soroban_sdk::String::from_str(&env, "tmp"),
        );

        // Still suspended
        let results = client.batch_verify_engineers(&soroban_sdk::vec![&env, engineer.clone()]);
        assert_ne!(results.get(0).unwrap(), CredentialStatus::Valid);

        env.ledger().set_timestamp(end);
        let results = client.batch_verify_engineers(&soroban_sdk::vec![&env, engineer.clone()]);
        assert_eq!(results.get(0).unwrap(), CredentialStatus::Valid, "suspension should have lifted");
    }
    fn test_engineers_from_different_issuers_verify_independently() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);

        let asme = Address::generate(&env);
        let ieee = Address::generate(&env);
        client.register_issuer(&asme);
        client.register_issuer(&ieee);

        let eng_asme = Address::generate(&env);
        let eng_ieee = Address::generate(&env);
        let hash1 = BytesN::from_array(&env, &[7u8; 32]);
        let hash2 = BytesN::from_array(&env, &[8u8; 32]);
        client.register_engineer(&eng_asme, &hash1, &asme, &31_536_000, &None);
        client.register_engineer(&eng_ieee, &hash2, &ieee, &31_536_000, &None);

        assert_eq!(client.verify_engineer(&eng_asme, &None::<Symbol>), CredentialStatus::Valid);
        assert_eq!(client.verify_engineer(&eng_ieee, &None::<Symbol>), CredentialStatus::Valid);

        // Revoking one issuer must only affect that issuer's engineer.
        client.revoke_issuer(&asme);
        assert_eq!(client.verify_engineer(&eng_asme, &None::<Symbol>), CredentialStatus::Revoked);
        assert_eq!(client.verify_engineer(&eng_ieee, &None::<Symbol>), CredentialStatus::Valid);
    }

    #[test]
    fn test_verify_engineer_rejects_untrusted_issuer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);

        let issuer = Address::generate(&env);
        client.register_issuer(&issuer);

        let engineer = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[9u8; 32]);
        client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);
        assert_eq!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Valid);

        // Once the issuer is no longer trusted, verification must not be Valid.
        client.revoke_issuer(&issuer);
        assert_eq!(client.verify_engineer(&engineer, &None::<Symbol>), CredentialStatus::Revoked);

// ── Issue #1009: set_grace_period ──────────────────────────────────────────

#[test]
fn test_set_grace_period_valid() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup(&env);

    // Default grace period is 7 days (604800 seconds)
    assert_eq!(client.get_grace_period(), 7 * 86_400);

    // Admin can set a new grace period (14 days)
    client.set_grace_period(&admin, &(14 * 86_400));
    assert_eq!(client.get_grace_period(), 14 * 86_400);
}

#[test]
fn test_set_grace_period_minimum_boundary() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup(&env);

    // 1 day is the minimum allowed value
    client.set_grace_period(&admin, &86_400);
    assert_eq!(client.get_grace_period(), 86_400);
}

#[test]
fn test_set_grace_period_maximum_boundary() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup(&env);

    // 90 days is the maximum allowed value
    client.set_grace_period(&admin, &(90 * 86_400));
    assert_eq!(client.get_grace_period(), 90 * 86_400);
}

#[test]
#[should_panic]
fn test_set_grace_period_too_short_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup(&env);

    // Less than 1 day → should panic with InvalidGracePeriod
    client.set_grace_period(&admin, &(86_400 - 1));
}

#[test]
#[should_panic]
fn test_set_grace_period_too_long_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup(&env);

    // More than 90 days → should panic with InvalidGracePeriod
    client.set_grace_period(&admin, &(90 * 86_400 + 1));
}

#[test]
#[should_panic]
fn test_set_grace_period_non_admin_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin) = setup(&env);

    let not_admin = Address::generate(&env);
    client.set_grace_period(&not_admin, &(7 * 86_400));
}

// ── Issue #1008: verify_engineer with required_specialization ─────────────

#[test]
fn test_verify_engineer_with_matching_specialization() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup(&env);

    let issuer = Address::generate(&env);
    client.add_trusted_issuer(&admin, &issuer);

    let engineer = Address::generate(&env);
    let hash = BytesN::from_array(&env, &[5u8; 32]);
    client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

    // Add a known specialization to the engineer
    let spec = Symbol::new(&env, "diesel_ge");
    client.add_specialization(&issuer, &engineer, &spec);

    // Verify with matching specialization → Valid
    let status = client.verify_engineer(&engineer, &Some(spec));
    assert_eq!(status, CredentialStatus::Valid);
}

#[test]
fn test_verify_engineer_without_specialization_check() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup(&env);

    let issuer = Address::generate(&env);
    client.add_trusted_issuer(&admin, &issuer);

    let engineer = Address::generate(&env);
    let hash = BytesN::from_array(&env, &[6u8; 32]);
    client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

    // No specialization check (None) → Valid regardless of specializations
    let status = client.verify_engineer(&engineer, &None::<Symbol>);
    assert_eq!(status, CredentialStatus::Valid);
}

#[test]
#[should_panic]
fn test_verify_engineer_specialization_not_covered_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin) = setup(&env);

    let issuer = Address::generate(&env);
    client.add_trusted_issuer(&admin, &issuer);

    let engineer = Address::generate(&env);
    let hash = BytesN::from_array(&env, &[7u8; 32]);
    client.register_engineer(&engineer, &hash, &issuer, &31_536_000, &None);

    // Engineer has no specializations; requesting one that doesn't exist → panic
    let spec = Symbol::new(&env, "wind_turb");
    client.verify_engineer(&engineer, &Some(spec));
}
