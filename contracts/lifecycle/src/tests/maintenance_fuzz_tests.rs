use super::*;
use soroban_sdk::symbol_short;

#[test]
fn maintenance_notes_corpus_does_not_panic() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, asset_registry, engineer_registry, _) = setup(&env, 0);
    let (asset_id, owner) = register_asset(&env, &asset_registry);
    let engineer = register_engineer(&env, &engineer_registry);
    client.authorize_engineer(&owner, &asset_id, &engineer);

    let cases = [
        "Routine check",
        "Unicode: Überprüfung",
        "Emoji: repair complete",
        "'; DROP TABLE maintenance; --",
        "<script>alert(1)</script>",
        "line one\nline two",
        "tab\tseparated",
        "   ",
    ];

    for notes in cases {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = client.try_submit_maintenance(
                &asset_id,
                &symbol_short!("OIL_CHG"),
                &Priority::Low,
                &String::from_str(&env, notes),
                &engineer,
                &None,
            );
        }));
        assert!(result.is_ok(), "maintenance submission panicked for {notes:?}");
    }
}
