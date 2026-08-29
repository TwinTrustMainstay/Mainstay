#[cfg(test)]
mod ttl_extension_tests {
    use soroban_sdk::{testutils::Ledger, Env, String};
    use crate::AssetContract;

    /// Helper: advance ledger sequence so TTL assertions are meaningful
    fn advance_ledger(env: &Env, sequences: u32) {
        env.ledger().with_mut(|l| l.sequence_number += sequences);
    }

    #[test]
    fn test_ttl_extended_after_register_asset() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AssetContract);
        let client = crate::AssetContractClient::new(&env, &contract_id);

        let asset_id = String::from_str(&env, "ASSET-TTL-001");
        let owner = String::from_str(&env, "owner-addr");
        let description = String::from_str(&env, "Turbine Unit Alpha");

        // Advance ledger before write so we get a non-trivial baseline
        advance_ledger(&env, 100);
        let seq_before = env.ledger().sequence();

        client.register_asset(&asset_id, &owner, &description);

        // TTL for persistent storage should have been extended
        // Soroban sets ttl = current_seq + extension; verify storage survives past seq_before
        let ttl = env.storage().persistent().get_ttl(&soroban_sdk::symbol_short!("asset"));
        // TTL must be greater than the ledger sequence at the time of write
        assert!(
            ttl > seq_before,
            "TTL ({}) must exceed ledger sequence at write time ({})",
            ttl,
            seq_before
        );
    }

    #[test]
    fn test_register_asset_data_survives_expected_ttl_window() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AssetContract);
        let client = crate::AssetContractClient::new(&env, &contract_id);

        let asset_id = String::from_str(&env, "ASSET-TTL-002");
        client.register_asset(
            &asset_id,
            &String::from_str(&env, "owner"),
            &String::from_str(&env, "desc"),
        );

        // Advance by a substantial number of ledgers — data should still be accessible
        advance_ledger(&env, 500);

        // If TTL was NOT extended this would panic / return None
        let retrieved = client.get_asset(&asset_id);
        assert_eq!(retrieved.asset_id, asset_id, "Asset data must survive after 500 ledger sequences");
    }
}
