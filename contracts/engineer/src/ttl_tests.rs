#[cfg(test)]
mod ttl_extension_tests {
    use soroban_sdk::{testutils::Ledger, Env, String};
    use crate::EngineerContract;

    fn advance_ledger(env: &Env, sequences: u32) {
        env.ledger().with_mut(|l| l.sequence_number += sequences);
    }

    #[test]
    fn test_ttl_extended_after_register_engineer() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, EngineerContract);
        let client = crate::EngineerContractClient::new(&env, &contract_id);

        let eng_id = String::from_str(&env, "ENG-TTL-001");
        let name = String::from_str(&env, "Alice");
        let cert = String::from_str(&env, "ISO-9001");

        advance_ledger(&env, 150);
        let seq_before = env.ledger().sequence();

        client.register_engineer(&eng_id, &name, &cert);

        let ttl = env.storage().persistent().get_ttl(&soroban_sdk::symbol_short!("eng"));
        assert!(
            ttl > seq_before,
            "TTL ({}) must exceed ledger sequence at write time ({}) after register_engineer",
            ttl,
            seq_before
        );
    }

    #[test]
    fn test_engineer_record_survives_expected_ttl_window() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, EngineerContract);
        let client = crate::EngineerContractClient::new(&env, &contract_id);

        let eng_id = String::from_str(&env, "ENG-TTL-002");
        client.register_engineer(
            &eng_id,
            &String::from_str(&env, "Bob"),
            &String::from_str(&env, "CERT-42"),
        );

        advance_ledger(&env, 500);

        let engineer = client.get_engineer(&eng_id);
        assert_eq!(engineer.engineer_id, eng_id, "Engineer record must survive after 500 ledger sequences");
    }
}
