#![cfg(test)]

use crate::{IncidentManagement, IncidentManagementClient, Severity, Status};
use soroban_sdk::{testutils::Address as _, Address, Env, String};

fn setup(env: &Env) -> (IncidentManagementClient<'_>, Address) {
    env.mock_all_auths();
    let id = env.register(IncidentManagement, ());
    let client = IncidentManagementClient::new(env, &id);
    let admin = Address::generate(env);
    client.initialize(&admin);
    (client, admin)
}

fn open(env: &Env, client: &IncidentManagementClient, sev: Severity) -> u64 {
    let reporter = Address::generate(env);
    client.open_incident(&reporter, &String::from_str(env, "Generator outage"), &sev)
}

#[test]
fn open_incident_assigns_sequential_ids() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    assert_eq!(client.get_incident_count(), 0);

    let a = open(&env, &client, Severity::Sev3);
    let b = open(&env, &client, Severity::Sev1);
    assert_eq!((a, b), (1, 2));
    assert_eq!(client.get_incident_count(), 2);

    let inc = client.get_incident(&b);
    assert_eq!(inc.status, Status::Open);
    assert_eq!(inc.severity, Severity::Sev1);
    assert_eq!(inc.resolved_at, 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn open_incident_rejects_empty_title() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let reporter = Address::generate(&env);
    client.open_incident(&reporter, &String::from_str(&env, ""), &Severity::Sev4);
}

#[test]
fn lifecycle_open_to_closed_for_minor_incident() {
    let env = Env::default();
    let (client, admin) = setup(&env);
    let id = open(&env, &client, Severity::Sev3);

    client.update_status(&admin, &id, &Status::Mitigated);
    client.update_status(&admin, &id, &Status::Resolved);
    assert_eq!(
        client.get_incident(&id).resolved_at,
        env.ledger().timestamp()
    );

    client.update_status(&admin, &id, &Status::Closed);
    assert_eq!(client.get_incident(&id).status, Status::Closed);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn invalid_transition_is_rejected() {
    let env = Env::default();
    let (client, admin) = setup(&env);
    let id = open(&env, &client, Severity::Sev3);
    // Open -> Closed skips Resolved.
    client.update_status(&admin, &id, &Status::Closed);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn sev1_cannot_close_without_postmortem() {
    let env = Env::default();
    let (client, admin) = setup(&env);
    let id = open(&env, &client, Severity::Sev1);
    client.update_status(&admin, &id, &Status::Resolved);
    client.update_status(&admin, &id, &Status::Closed);
}

#[test]
fn sev2_can_close_after_postmortem() {
    let env = Env::default();
    let (client, admin) = setup(&env);
    let id = open(&env, &client, Severity::Sev2);
    client.update_status(&admin, &id, &Status::Resolved);
    client.record_postmortem(&admin, &id, &String::from_str(&env, "Pump seal failed"));
    client.update_status(&admin, &id, &Status::Closed);

    let inc = client.get_incident(&id);
    assert_eq!(inc.status, Status::Closed);
    assert_eq!(inc.root_cause, String::from_str(&env, "Pump seal failed"));
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn postmortem_requires_resolved_incident() {
    let env = Env::default();
    let (client, admin) = setup(&env);
    let id = open(&env, &client, Severity::Sev2);
    client.record_postmortem(&admin, &id, &String::from_str(&env, "Too early"));
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn non_admin_cannot_update_status() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let id = open(&env, &client, Severity::Sev3);
    let intruder = Address::generate(&env);
    client.update_status(&intruder, &id, &Status::Mitigated);
}

#[test]
fn action_items_are_tracked_and_completed_by_owner() {
    let env = Env::default();
    let (client, admin) = setup(&env);
    let id = open(&env, &client, Severity::Sev2);
    let owner = Address::generate(&env);

    let item = client.add_action_item(
        &admin,
        &id,
        &String::from_str(&env, "Add seal pressure alert"),
        &owner,
    );
    assert_eq!(item, 0);

    client.complete_action_item(&owner, &id, &item);
    let items = client.get_action_items(&id);
    assert_eq!(items.len(), 1);
    assert!(items.get(0).unwrap().completed);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")]
fn only_item_owner_can_complete_action_item() {
    let env = Env::default();
    let (client, admin) = setup(&env);
    let id = open(&env, &client, Severity::Sev2);
    let owner = Address::generate(&env);
    let other = Address::generate(&env);
    let item = client.add_action_item(&admin, &id, &String::from_str(&env, "Review"), &owner);
    client.complete_action_item(&other, &id, &item);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn unknown_action_item_is_rejected() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let id = open(&env, &client, Severity::Sev2);
    let owner = Address::generate(&env);
    client.complete_action_item(&owner, &id, &99);
}
