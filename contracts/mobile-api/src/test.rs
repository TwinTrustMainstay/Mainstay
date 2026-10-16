use crate::{
    ContractError, MobileApi, MobileApiClient, CURRENT_API_VERSION, MAX_CHANGE_LOG_ENTRIES,
    MAX_PAGE_SIZE, MIN_API_VERSION,
};
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::{vec, Address, BytesN, Env, Symbol};

fn setup() -> (Env, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(MobileApi, ());
    let admin = Address::generate(&env);
    MobileApiClient::new(&env, &id).initialize(&admin);
    (env, id, admin)
}

fn hash(env: &Env, byte: u8) -> BytesN<32> {
    BytesN::from_array(env, &[byte; 32])
}

#[test]
fn negotiates_api_version_to_the_lower_of_client_and_server() {
    let (env, id, _admin) = setup();
    let client = MobileApiClient::new(&env, &id);
    let info = client.get_version_info();
    assert_eq!(info.current, CURRENT_API_VERSION);
    assert_eq!(info.minimum, MIN_API_VERSION);

    let old_app = Address::generate(&env);
    assert_eq!(client.register_client(&old_app, &MIN_API_VERSION), MIN_API_VERSION);

    let future_app = Address::generate(&env);
    assert_eq!(
        client.register_client(&future_app, &(CURRENT_API_VERSION + 10)),
        CURRENT_API_VERSION
    );
    assert_eq!(client.get_client(&future_app).unwrap().api_version, CURRENT_API_VERSION);
}

#[test]
fn rejects_versions_below_minimum() {
    let (env, id, _admin) = setup();
    let client = MobileApiClient::new(&env, &id);
    let app = Address::generate(&env);
    let res = client.try_register_client(&app, &0);
    assert_eq!(res, Err(Ok(ContractError::UnsupportedApiVersion)));
}

#[test]
fn sync_requires_registration() {
    let (env, id, _admin) = setup();
    let client = MobileApiClient::new(&env, &id);
    let stranger = Address::generate(&env);
    let res = client.try_sync(&stranger, &0, &10);
    assert_eq!(res, Err(Ok(ContractError::ClientNotRegistered)));
}

#[test]
fn publish_is_admin_only() {
    let (env, id, _admin) = setup();
    let client = MobileApiClient::new(&env, &id);
    let other = Address::generate(&env);
    let res = client.try_publish_change(&other, &Symbol::new(&env, "maint"), &1, &hash(&env, 1));
    assert!(res.is_err());
}

#[test]
fn sync_pages_through_changes_with_cursor() {
    let (env, id, admin) = setup();
    let client = MobileApiClient::new(&env, &id);
    let app = Address::generate(&env);
    client.register_client(&app, &CURRENT_API_VERSION);

    for i in 0..5u64 {
        let seq = client.publish_change(&admin, &Symbol::new(&env, "maint"), &i, &hash(&env, i as u8));
        assert_eq!(seq, i + 1);
    }

    let page1 = client.sync(&app, &0, &2);
    assert_eq!(page1.changes.len(), 2);
    assert_eq!(page1.changes.get(0).unwrap().seq, 1);
    assert_eq!(page1.next_seq, 2);
    assert_eq!(page1.latest_seq, 5);
    assert!(!page1.full_resync_required);

    let page2 = client.sync(&app, &page1.next_seq, &10);
    assert_eq!(page2.changes.len(), 3);
    assert_eq!(page2.changes.get(0).unwrap().seq, 3);
    assert_eq!(page2.next_seq, 5);

    // Caught up: an empty page keeps the cursor where it is.
    let page3 = client.sync(&app, &page2.next_seq, &10);
    assert_eq!(page3.changes.len(), 0);
    assert_eq!(page3.next_seq, 5);
}

#[test]
fn sync_rejects_out_of_range_limits() {
    let (env, id, _admin) = setup();
    let client = MobileApiClient::new(&env, &id);
    let app = Address::generate(&env);
    client.register_client(&app, &CURRENT_API_VERSION);
    assert_eq!(client.try_sync(&app, &0, &0), Err(Ok(ContractError::InvalidLimit)));
    assert_eq!(
        client.try_sync(&app, &0, &(MAX_PAGE_SIZE + 1)),
        Err(Ok(ContractError::InvalidLimit))
    );
}

#[test]
fn stale_cursor_after_pruning_requires_full_resync() {
    let (env, id, admin) = setup();
    let client = MobileApiClient::new(&env, &id);
    let app = Address::generate(&env);
    client.register_client(&app, &CURRENT_API_VERSION);

    let total = MAX_CHANGE_LOG_ENTRIES + 5;
    for i in 0..total {
        client.publish_change(&admin, &Symbol::new(&env, "asset"), &i, &hash(&env, 7));
    }

    // Records 1..=5 were pruned, so a cursor at 0 has lost changes.
    let stale = client.sync(&app, &0, &10);
    assert!(stale.full_resync_required);
    assert_eq!(stale.changes.len(), 0);
    assert_eq!(stale.next_seq, total);

    // A cursor inside the retained window still syncs normally.
    let fresh = client.sync(&app, &5, &3);
    assert!(!fresh.full_resync_required);
    assert_eq!(fresh.changes.get(0).unwrap().seq, 6);

    assert!(client.try_get_change(&1).is_err());
    assert_eq!(client.get_change(&total).seq, total);
}

#[test]
fn rate_limit_blocks_then_resets_after_window() {
    let (env, id, admin) = setup();
    let client = MobileApiClient::new(&env, &id);
    client.set_rate_limit(&admin, &3, &60);
    let app = Address::generate(&env);
    client.register_client(&app, &CURRENT_API_VERSION);

    // Registration is not counted; the first three reads succeed.
    client.sync(&app, &0, &1);
    client.sync(&app, &0, &1);
    client.sync(&app, &0, &1);
    assert_eq!(
        client.try_sync(&app, &0, &1),
        Err(Ok(ContractError::RateLimitExceeded))
    );

    env.ledger().with_mut(|l| l.timestamp += 61);
    assert!(client.try_sync(&app, &0, &1).is_ok());
}

#[test]
fn rate_limit_is_tracked_per_client() {
    let (env, id, admin) = setup();
    let client = MobileApiClient::new(&env, &id);
    client.set_rate_limit(&admin, &1, &60);
    let a = Address::generate(&env);
    let b = Address::generate(&env);
    client.register_client(&a, &CURRENT_API_VERSION);
    client.register_client(&b, &CURRENT_API_VERSION);

    client.sync(&a, &0, &1);
    assert!(client.try_sync(&a, &0, &1).is_err());
    assert!(client.try_sync(&b, &0, &1).is_ok());
}

#[test]
fn rate_limit_config_is_bounded() {
    let (env, id, admin) = setup();
    let client = MobileApiClient::new(&env, &id);
    assert_eq!(
        client.try_set_rate_limit(&admin, &0, &60),
        Err(Ok(ContractError::InvalidRateLimitConfig))
    );
    assert_eq!(
        client.try_set_rate_limit(&admin, &10, &0),
        Err(Ok(ContractError::InvalidRateLimitConfig))
    );
}

#[test]
fn notifications_only_return_subscribed_topics() {
    let (env, id, admin) = setup();
    let client = MobileApiClient::new(&env, &id);
    let app = Address::generate(&env);
    client.register_client(&app, &CURRENT_API_VERSION);

    assert_eq!(
        client.try_pull_notifications(&app, &0, &10),
        Err(Ok(ContractError::NoSubscription))
    );

    client.subscribe(&app, &vec![&env, Symbol::new(&env, "lien")]);
    client.publish_change(&admin, &Symbol::new(&env, "maint"), &1, &hash(&env, 1));
    client.publish_change(&admin, &Symbol::new(&env, "lien"), &2, &hash(&env, 2));
    client.publish_change(&admin, &Symbol::new(&env, "maint"), &3, &hash(&env, 3));
    client.publish_change(&admin, &Symbol::new(&env, "lien"), &4, &hash(&env, 4));

    let page = client.pull_notifications(&app, &0, &10);
    assert_eq!(page.changes.len(), 2);
    assert_eq!(page.changes.get(0).unwrap().asset_id, 2);
    assert_eq!(page.changes.get(1).unwrap().asset_id, 4);
    assert_eq!(page.next_seq, 4);

    client.unsubscribe(&app);
    assert!(client.get_subscription(&app).is_none());
}

#[test]
fn subscription_rejects_empty_and_oversized_topic_lists() {
    let (env, id, _admin) = setup();
    let client = MobileApiClient::new(&env, &id);
    let app = Address::generate(&env);
    client.register_client(&app, &CURRENT_API_VERSION);
    let empty: soroban_sdk::Vec<Symbol> = soroban_sdk::Vec::new(&env);
    assert_eq!(
        client.try_subscribe(&app, &empty),
        Err(Ok(ContractError::InvalidTopics))
    );
}
