#[cfg(test)]
mod ttl_extension_tests {
    use soroban_sdk::{testutils::Ledger, Env, String};
    use crate::MaintenanceContract;

    fn advance_ledger(env: &Env, sequences: u32) {
        env.ledger().with_mut(|l| l.sequence_number += sequences);
    }

    #[test]
    fn test_ttl_extended_after_submit_maintenance() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, MaintenanceContract);
        let client = crate::MaintenanceContractClient::new(&env, &contract_id);

        let asset_id = String::from_str(&env, "ASSET-MAINT-TTL-001");
        let engineer_id = String::from_str(&env, "ENG-001");
        let task_type = String::from_str(&env, "inspection");
        let notes = String::from_str(&env, "Routine check complete");

        advance_ledger(&env, 200);
        let seq_before = env.ledger().sequence();

        client.submit_maintenance(&asset_id, &engineer_id, &task_type, &notes);

        let ttl = env.storage().persistent().get_ttl(&soroban_sdk::symbol_short!("maint"));
        assert!(
            ttl > seq_before,
            "TTL ({}) must exceed ledger sequence at write time ({}) after submit_maintenance",
            ttl,
            seq_before
        );
    }

    #[test]
    fn test_maintenance_record_persists_after_ttl_window() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, MaintenanceContract);
        let client = crate::MaintenanceContractClient::new(&env, &contract_id);

        let asset_id = String::from_str(&env, "ASSET-MAINT-TTL-002");
        client.submit_maintenance(
            &asset_id,
            &String::from_str(&env, "ENG-001"),
            &String::from_str(&env, "repair"),
            &String::from_str(&env, "Fixed seal"),
        );

        advance_ledger(&env, 500);

        // Data must survive — TTL extension on write is the guard
        let records = client.get_maintenance_records(&asset_id);
        assert!(!records.is_empty(), "Maintenance records must persist after 500 ledger sequences");
    }
}
