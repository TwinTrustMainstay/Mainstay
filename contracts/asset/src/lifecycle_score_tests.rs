#[cfg(test)]
mod lifecycle_score_tests {
    use soroban_sdk::{Env, String};
    use crate::AssetContract;

    // ── Helpers ─────────────────────────────────────────────────────────

    fn register_engineers(env: &Env, client: &crate::AssetContractClient, count: u32) -> Vec<String> {
        (1..=count)
            .map(|i| {
                let id   = String::from_str(env, &format!("ENG-{i:03}"));
                let name = String::from_str(env, &format!("Engineer {i}"));
                let cert = String::from_str(env, &format!("CERT-{i}"));
                client.register_engineer(&id, &name, &cert);
                id
            })
            .collect()
    }

    fn submit_maintenance_records(
        env: &Env,
        client: &crate::AssetContractClient,
        asset_id: &String,
        engineer_ids: &[String],
        count: u32,
    ) {
        for i in 0..count {
            let eng = &engineer_ids[(i as usize) % engineer_ids.len()];
            let task  = String::from_str(env, if i % 2 == 0 { "inspection" } else { "repair" });
            let notes = String::from_str(env, &format!("Maintenance record {}", i + 1));
            client.submit_maintenance(asset_id, eng, &task, &notes);
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────

    /// Core case: score after 5 submissions by 3 engineers must be positive
    /// and consistent across repeated reads.
    #[test]
    fn test_score_positive_after_five_submissions() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AssetContract);
        let client = crate::AssetContractClient::new(&env, &contract_id);

        let asset_id = String::from_str(&env, "LIFECYCLE-SCORE-001");
        client.register_asset(
            &asset_id,
            &String::from_str(&env, "owner-001"),
            &String::from_str(&env, "Industrial Compressor A"),
        );

        let engineers = register_engineers(&env, &client, 3);
        submit_maintenance_records(&env, &client, &asset_id, &engineers, 5);

        let score = client.get_lifecycle_score(&asset_id);
        assert!(score > 0, "Score must be positive after 5 maintenance submissions, got {}", score);
    }

    /// Score must be strictly greater after more submissions (monotonic growth).
    #[test]
    fn test_score_increases_with_each_submission() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AssetContract);
        let client = crate::AssetContractClient::new(&env, &contract_id);

        let asset_id = String::from_str(&env, "LIFECYCLE-SCORE-002");
        client.register_asset(
            &asset_id,
            &String::from_str(&env, "owner-002"),
            &String::from_str(&env, "Pump Unit B"),
        );

        let engineers = register_engineers(&env, &client, 3);
        let eng_0 = engineers[0].clone();

        let score_0 = client.get_lifecycle_score(&asset_id);

        client.submit_maintenance(
            &asset_id,
            &eng_0,
            &String::from_str(&env, "inspection"),
            &String::from_str(&env, "First record"),
        );
        let score_1 = client.get_lifecycle_score(&asset_id);
        assert!(score_1 > score_0, "Score must increase after 1st submission: {} -> {}", score_0, score_1);

        submit_maintenance_records(&env, &client, &asset_id, &engineers, 4);
        let score_5 = client.get_lifecycle_score(&asset_id);
        assert!(score_5 > score_1, "Score must increase from 1 to 5 submissions: {} -> {}", score_1, score_5);
    }

    /// Score must be deterministic: same inputs → same score on repeated reads.
    #[test]
    fn test_score_is_deterministic() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AssetContract);
        let client = crate::AssetContractClient::new(&env, &contract_id);

        let asset_id = String::from_str(&env, "LIFECYCLE-SCORE-003");
        client.register_asset(
            &asset_id,
            &String::from_str(&env, "owner-003"),
            &String::from_str(&env, "Valve Array C"),
        );

        let engineers = register_engineers(&env, &client, 3);
        submit_maintenance_records(&env, &client, &asset_id, &engineers, 5);

        let score_a = client.get_lifecycle_score(&asset_id);
        let score_b = client.get_lifecycle_score(&asset_id);

        assert_eq!(score_a, score_b, "Score must be deterministic across repeated reads");
    }

    /// Scores for independent assets must not influence each other.
    #[test]
    fn test_scores_are_isolated_per_asset() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AssetContract);
        let client = crate::AssetContractClient::new(&env, &contract_id);

        let asset_a = String::from_str(&env, "LIFECYCLE-SCORE-A");
        let asset_b = String::from_str(&env, "LIFECYCLE-SCORE-B");

        for id in [&asset_a, &asset_b] {
            client.register_asset(id, &String::from_str(&env, "owner"), &String::from_str(&env, "desc"));
        }

        let engineers = register_engineers(&env, &client, 3);

        // Only submit for asset_a
        submit_maintenance_records(&env, &client, &asset_a, &engineers, 5);

        let score_a = client.get_lifecycle_score(&asset_a);
        let score_b = client.get_lifecycle_score(&asset_b);

        assert!(score_a > score_b, "Asset A (5 records) must outscore Asset B (0 records): {} vs {}", score_a, score_b);
    }

    /// Submissions by different engineers must each contribute to the score.
    #[test]
    fn test_all_three_engineers_contribute() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AssetContract);
        let client = crate::AssetContractClient::new(&env, &contract_id);

        let asset_id = String::from_str(&env, "LIFECYCLE-SCORE-MULTI-ENG");
        client.register_asset(
            &asset_id,
            &String::from_str(&env, "owner-multi"),
            &String::from_str(&env, "Turbine Delta"),
        );

        let engineers = register_engineers(&env, &client, 3);

        // One submission per engineer — verify each adds to the score
        let mut prev_score = client.get_lifecycle_score(&asset_id);
        for eng in &engineers {
            client.submit_maintenance(
                &asset_id,
                eng,
                &String::from_str(&env, "inspection"),
                &String::from_str(&env, "Engineer-specific record"),
            );
            let new_score = client.get_lifecycle_score(&asset_id);
            assert!(
                new_score > prev_score,
                "Score must increase after engineer {} submits: {} -> {}",
                eng,
                prev_score,
                new_score
            );
            prev_score = new_score;
        }
    }
}
