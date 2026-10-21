//! Verification tests for the maintenance-record ledger (issue #1756).
//!
//! The instance below is the one in `verification/tla/MaintenanceRecords.cfg`.
//! `TLC_DISTINCT_STATES` and `TLC_DEPTH` are the numbers TLC reported for it. If
//! the spec or this model changes, update both sides together.

use verification::{explore, Action, Model, ModelError, Mutation};

const TLC_DISTINCT_STATES: usize = 228;
const TLC_DEPTH: usize = 8;

fn spec_instance() -> Model {
    Model::new(vec![1, 2], 2, 3)
}

#[test]
fn exhaustive_check_passes_and_matches_tlc() {
    let report = explore(&spec_instance());
    assert_eq!(report.violation, None, "unexpected invariant violation");
    assert_eq!(report.distinct_states, TLC_DISTINCT_STATES);
    // TLC counts states along the longest path (Init included); `max_depth` counts transitions.
    assert_eq!(report.max_depth + 1, TLC_DEPTH);
}

#[test]
fn mutant_leaking_value_violates_conservation() {
    let model = spec_instance().with_mutation(Mutation::LeakOnDecommission);
    let v = explore(&model).violation.expect("mutant must be caught");
    assert_eq!(v.invariant, "Conservation");
    assert!(!v.trace.is_empty());
}

#[test]
fn mutant_submitting_for_unregistered_violates_no_orphans() {
    let model = spec_instance().with_mutation(Mutation::SubmitUnregistered);
    let v = explore(&model).violation.expect("mutant must be caught");
    assert_eq!(v.invariant, "NoOrphanedRecords");
}

#[test]
fn counterexample_trace_replays_to_violation() {
    let model = spec_instance().with_mutation(Mutation::LeakOnDecommission);
    let v = explore(&model).violation.unwrap();
    let mut s = model.init();
    for action in &v.trace {
        s = model.step(&s, *action).expect("trace action must be enabled");
    }
    assert_eq!(model.check_invariants(&s), Err("Conservation"));
}

// --- Edge cases and error conditions ---------------------------------------

fn state_with(model: &Model, registered: &[(u8, u32)]) -> verification::State {
    let mut s = model.init();
    for &(a, v) in registered {
        s = model.step(&s, Action::Register { a, v }).unwrap();
    }
    s
}

#[test]
fn register_rejects_reused_id() {
    let m = spec_instance();
    let s = state_with(&m, &[(1, 0)]);
    let s = model_step_ok(&m, &s, Action::Decommission { a: 1 });
    assert_eq!(m.step(&s, Action::Register { a: 1, v: 0 }), Err(ModelError::IdAlreadyUsed));
}

#[test]
fn register_rejects_value_beyond_pool() {
    let m = spec_instance();
    let s = m.init();
    assert_eq!(m.step(&s, Action::Register { a: 1, v: 3 }), Err(ModelError::InsufficientPool));
}

#[test]
fn register_rejects_unknown_asset_id() {
    let m = spec_instance();
    assert_eq!(m.step(&m.init(), Action::Register { a: 9, v: 0 }), Err(ModelError::UnknownAsset));
}

#[test]
fn decommission_returns_value_to_pool() {
    let m = spec_instance();
    let s = state_with(&m, &[(1, 2)]);
    assert_eq!(s.pool, 0);
    let s = model_step_ok(&m, &s, Action::Decommission { a: 1 });
    assert_eq!(s.pool, 2);
    assert!(!s.registered.contains(&1));
}

#[test]
fn decommission_of_unregistered_asset_is_rejected() {
    let m = spec_instance();
    assert_eq!(m.step(&m.init(), Action::Decommission { a: 1 }), Err(ModelError::NotRegistered));
}

#[test]
fn submit_rejected_for_unregistered_and_when_history_full() {
    let m = spec_instance();
    assert_eq!(m.step(&m.init(), Action::Submit { a: 1 }), Err(ModelError::NotRegistered));

    let mut s = state_with(&m, &[(1, 0)]);
    for _ in 0..3 {
        s = model_step_ok(&m, &s, Action::Submit { a: 1 });
    }
    assert_eq!(m.step(&s, Action::Submit { a: 1 }), Err(ModelError::HistoryFull));
}

#[test]
fn transfer_edge_cases() {
    let m = spec_instance();
    let s = state_with(&m, &[(1, 2)]);
    assert_eq!(
        m.step(&s, Action::Transfer { a: 1, b: 2, amt: 1 }),
        Err(ModelError::NotRegistered),
        "transfer to unregistered asset"
    );
    let s = state_with(&m, &[(1, 1), (2, 1)]);
    assert_eq!(m.step(&s, Action::Transfer { a: 1, b: 1, amt: 1 }), Err(ModelError::SameAsset));
    assert_eq!(m.step(&s, Action::Transfer { a: 1, b: 2, amt: 0 }), Err(ModelError::InvalidAmount));
    assert_eq!(
        m.step(&s, Action::Transfer { a: 1, b: 2, amt: 2 }),
        Err(ModelError::InsufficientBalance)
    );
    let moved = model_step_ok(&m, &s, Action::Transfer { a: 1, b: 2, amt: 1 });
    assert_eq!(moved.balance[&1], 0);
    assert_eq!(moved.balance[&2], 2);
}

#[test]
fn initial_state_satisfies_invariants() {
    let m = spec_instance();
    assert_eq!(m.check_invariants(&m.init()), Ok(()));
}

fn model_step_ok(m: &Model, s: &verification::State, a: Action) -> verification::State {
    m.step(s, a).unwrap_or_else(|e| panic!("action {a:?} should be enabled: {e:?}"))
}
