// tests/test_issues_1204_1205_1206_1207.rs
//
// Issue #1204 — asset transfer invalidates existing engineer authorizations
// Issue #1205 — record_transfer sentinel carries ownership_start_ledger
// Issue #1206 — anchor_history_to_snapshot emits an ADM_AUD event
// Issue #1207 — batch_submit_maintenance enforces rate limit across the batch

use asset_registry::{AssetRegistry, AssetRegistryClient};
use engineer_registry::{EngineerRegistry, EngineerRegistryClient};
use lifecycle::{BatchRecord, Lifecycle, LifecycleClient};
use soroban_sdk::{
    symbol_short, testutils::Address as _, Address, BytesN, Env, String, Symbol, TryIntoVal,
};

/// lifecycle: ContractError::EngineerNotAuthorized = 16
const LIFECYCLE_ENGINEER_NOT_AUTHORIZED: u32 = 16;
/// lifecycle: ContractError::RateLimitExceeded = 37
const LIFECYCLE_RATE_LIMIT_EXCEEDED: u32 = 37;

struct Setup<'a> {
    env: &'a Env,
    asset_registry: AssetRegistryClient<'a>,
    lifecycle: LifecycleClient<'a>,
    lc_admin: Address,
    asset_id: u64,
    asset_owner: Address,
    engineer: Address,
}

impl<'a> Setup<'a> {
    fn new(env: &'a Env) -> Self {
        env.mock_all_auths();

        let asset_registry_id = env.register(AssetRegistry, ());
        let engineer_registry_id = env.register(EngineerRegistry, ());
        let lifecycle_id = env.register(Lifecycle, ());

        let asset_registry = AssetRegistryClient::new(env, &asset_registry_id);
        let engineer_registry = EngineerRegistryClient::new(env, &engineer_registry_id);
        let lifecycle = LifecycleClient::new(env, &lifecycle_id);

        let asset_admin = Address::generate(env);
        let eng_admin = Address::generate(env);
        let lc_admin = Address::generate(env);
        let issuer = Address::generate(env);
        let asset_owner = Address::generate(env);
        let engineer = Address::generate(env);

        asset_registry.initialize_admin(&asset_admin, &asset_admin);
        asset_registry.add_asset_type(&asset_admin, &symbol_short!("GENSET"));

        engineer_registry.initialize_admin(&eng_admin, &eng_admin);
        engineer_registry.add_trusted_issuer(&eng_admin, &issuer);

        lifecycle.initialize(
            &lc_admin,
            &asset_registry_id,
            &engineer_registry_id,
            &lc_admin,
            &0,
        );

        let asset_id = asset_registry.register_asset(
            &symbol_short!("GENSET"),
            &String::from_str(env, "Industrial generator"),
            &String::from_str(env, "SN-ISSUES-001"),
            &asset_owner,
        );

        let credential_hash = BytesN::from_array(env, &[7u8; 32]);
        engineer_registry.register_engineer(
            &engineer,
            &credential_hash,
            &issuer,
            &31_536_000,
            &None,
        );

        lifecycle.authorize_engineer(&asset_owner, &asset_id, &engineer);

        Setup {
            env,
            asset_registry,
            lifecycle,
            lc_admin,
            asset_id,
            asset_owner,
            engineer,
        }
    }
}

/// Issue #1204 — engineer auth from the previous owner must be rejected after
/// an ownership transfer + `record_transfer`.
#[test]
fn test_transfer_invalidates_prior_engineer_authorization() {
    let env = Env::default();
    let s = Setup::new(&env);
    let new_owner = Address::generate(&env);

    s.asset_registry
        .transfer_asset(&s.asset_id, &s.asset_owner, &new_owner);
    s.lifecycle
        .record_transfer(&s.asset_id, &s.asset_owner, &new_owner);

    assert!(
        s.lifecycle.get_authorized_engineers(&s.asset_id).is_empty(),
        "authorized engineer list must be cleared after transfer"
    );

    let result = s.lifecycle.try_submit_maintenance(
        &s.asset_id,
        &symbol_short!("OIL_CHG"),
        &String::from_str(&env, "attempt after transfer"),
        &s.engineer,
    );

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_ENGINEER_NOT_AUTHORIZED,
        ))),
        "previously authorized engineer must be rejected after ownership transfer"
    );
}

/// Issue #1205 — the XFER sentinel record must carry the ledger sequence at
/// which the new ownership period started.
#[test]
fn test_record_transfer_sentinel_has_ownership_start_ledger() {
    let env = Env::default();
    let s = Setup::new(&env);
    let new_owner = Address::generate(&env);

    s.asset_registry
        .transfer_asset(&s.asset_id, &s.asset_owner, &new_owner);
    let expected_ledger = env.ledger().sequence();
    s.lifecycle
        .record_transfer(&s.asset_id, &s.asset_owner, &new_owner);

    let history = s.lifecycle.get_maintenance_history(&s.asset_id);
    let sentinel = history.get(history.len() - 1).unwrap();
    assert_eq!(sentinel.task_type, symbol_short!("XFER"));
    assert_eq!(
        sentinel.ownership_start_ledger,
        Some(expected_ledger),
        "XFER sentinel must carry the ownership_start_ledger of the new period"
    );
}

/// Issue #1206 — `anchor_history_to_snapshot` must emit an ADM_AUD event.
#[test]
fn test_anchor_history_to_snapshot_emits_admin_audit_event() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.lifecycle.take_health_snapshot(&s.asset_id);
    s.lifecycle
        .anchor_history_to_snapshot(&s.lc_admin, &s.asset_id, &0);

    let events = env.events().all();
    let audit_event = events.iter().find(|(_, topics, _)| {
        topics.len() == 2
            && topics
                .get(0)
                .and_then(|v| TryIntoVal::<_, Symbol>::try_into_val(&v, &env).ok())
                .map(|sym| sym == symbol_short!("ADM_AUD"))
                .unwrap_or(false)
    });

    assert!(audit_event.is_some(), "expected an ADM_AUD event");
    let (_, _, data) = audit_event.unwrap();
    let (admin, asset_id, _timestamp): (Address, u64, u64) = data.try_into_val(&env).unwrap();
    assert_eq!(admin, s.lc_admin);
    assert_eq!(asset_id, s.asset_id);
}

/// Issue #1207 — a single oversized batch must not bypass the per-engineer
/// hourly submission cap.
#[test]
fn test_batch_submit_maintenance_enforces_rate_limit_across_batch() {
    let env = Env::default();
    let s = Setup::new(&env);

    s.lifecycle
        .update_max_submissions_per_hour(&s.lc_admin, &5);

    let mut records = soroban_sdk::Vec::new(&env);
    for _ in 0..6 {
        records.push_back(BatchRecord {
            task_type: symbol_short!("OIL_CHG"),
            notes: String::from_str(&env, "Routine oil change"),
        });
    }

    let result = s
        .lifecycle
        .try_batch_submit_maintenance(&s.asset_id, &records, &s.engineer);

    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            LIFECYCLE_RATE_LIMIT_EXCEEDED,
        ))),
        "a batch exceeding the hourly cap must be rejected in full"
    );
    assert_eq!(
        s.lifecycle.get_maintenance_history(&s.asset_id).len(),
        0,
        "no records from the rejected batch must be persisted"
    );
}
