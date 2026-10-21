use super::*;
use soroban_sdk::symbol_short;

#[test]
fn scores_increase_and_remain_stable_when_read_repeatedly() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry, engineer_registry, _) = setup(&env, 0);
    let (asset_id, owner) = register_asset(&env, &asset_registry);
    let engineer = register_engineer(&env, &engineer_registry);
    client.authorize_engineer(&owner, &asset_id, &engineer);

    let mut previous_score = client.get_collateral_score(&asset_id);
    let task_types = [
        symbol_short!("OIL_CHG"),
        symbol_short!("FILTER"),
        symbol_short!("ENGINE"),
    ];
    let notes = ["First service", "Second service", "Third service"];
    for (task_type, notes) in task_types.iter().zip(notes) {
        client.submit_maintenance(
            &asset_id,
            task_type,
            &Priority::Low,
            &String::from_str(&env, notes),
            &engineer,
            &None,
        );
        let score = client.get_collateral_score(&asset_id);
        assert!(score > previous_score, "score did not increase after maintenance");
        previous_score = score;
        env.ledger().with_mut(|ledger| ledger.timestamp += 1);
    }

    assert!(previous_score > 0);
    assert_eq!(
        client.get_collateral_score(&asset_id),
        client.get_collateral_score(&asset_id),
        "repeated reads of unchanged state must return the same score"
    );
}

#[test]
fn maintenance_scores_are_isolated_per_asset() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry, engineer_registry, _) = setup(&env, 0);
    let (maintained_asset, owner) = register_asset(&env, &asset_registry);
    let (unmaintained_asset, _) = register_asset(&env, &asset_registry);
    let engineer = register_engineer(&env, &engineer_registry);
    client.authorize_engineer(&owner, &maintained_asset, &engineer);

    client.submit_maintenance(
        &maintained_asset,
        &symbol_short!("OIL_CHG"),
        &Priority::Low,
        &String::from_str(&env, "Recorded service"),
        &engineer,
        &None,
    );

    assert!(
        client.get_collateral_score(&maintained_asset)
            > client.get_collateral_score(&unmaintained_asset),
        "maintenance for one asset must not affect another asset's score"
    );
}
