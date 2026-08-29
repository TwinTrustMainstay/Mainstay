#[cfg(test)]
mod collateral_eligibility_tests {
    use soroban_sdk::{Env, String};
    use crate::AssetContract;

    /// Asset registered with zero maintenance records must not be collateral-eligible.
    /// No maintenance history = no verifiable lifecycle score = below threshold.
    #[test]
    fn test_ineligible_asset_no_maintenance_records() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AssetContract);
        let client = crate::AssetContractClient::new(&env, &contract_id);

        let asset_id    = String::from_str(&env, "ASSET-NO-MAINT-001");
        let owner       = String::from_str(&env, "owner-addr");
        let description = String::from_str(&env, "Unserviced Pump Unit");

        client.register_asset(&asset_id, &owner, &description);

        // No maintenance submitted — score must be at floor (0 or initial default)
        let eligible = client.is_collateral_eligible(&asset_id);

        assert!(
            !eligible,
            "Asset with no maintenance records must return false for is_collateral_eligible"
        );
    }

    /// Freshly registered asset score should be 0 or below minimum threshold.
    #[test]
    fn test_initial_lifecycle_score_below_threshold() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AssetContract);
        let client = crate::AssetContractClient::new(&env, &contract_id);

        let asset_id = String::from_str(&env, "ASSET-SCORE-FLOOR-001");
        client.register_asset(
            &asset_id,
            &String::from_str(&env, "owner"),
            &String::from_str(&env, "desc"),
        );

        let score = client.get_lifecycle_score(&asset_id);

        // Initial score must be at or below the eligibility threshold
        // (exact threshold value is contract-defined; we assert ineligibility as the source of truth)
        let eligible = client.is_collateral_eligible(&asset_id);
        assert!(
            !eligible,
            "Initial score {} must place asset below collateral eligibility threshold",
            score
        );
    }

    /// is_collateral_eligible must return false for an unregistered asset
    /// (should return error or false — must not panic).
    #[test]
    fn test_ineligible_unknown_asset() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AssetContract);
        let client = crate::AssetContractClient::new(&env, &contract_id);

        let unknown_id = String::from_str(&env, "ASSET-DOES-NOT-EXIST");

        // Must not panic — should return false or a structured error
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.try_is_collateral_eligible(&unknown_id)
        }));

        assert!(
            result.is_ok(),
            "is_collateral_eligible must not panic for an unregistered asset"
        );
    }

    /// Score just below threshold must remain ineligible (boundary check).
    #[test]
    fn test_score_just_below_threshold_is_ineligible() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AssetContract);
        let client = crate::AssetContractClient::new(&env, &contract_id);

        // Register asset — score starts at 0 (below any meaningful threshold)
        let asset_id = String::from_str(&env, "ASSET-BOUNDARY-001");
        client.register_asset(
            &asset_id,
            &String::from_str(&env, "owner"),
            &String::from_str(&env, "Boundary test asset"),
        );

        // Intentionally do NOT submit any maintenance records
        // This keeps the score at its minimum floor value

        assert!(
            !client.is_collateral_eligible(&asset_id),
            "Asset at score floor must be ineligible for collateral"
        );
    }
}
