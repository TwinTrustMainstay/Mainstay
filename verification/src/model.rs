//! Transition system and explicit-state explorer.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

/// Variables of the TLA+ spec.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct State {
    pub registered: BTreeSet<u8>,
    pub ever: BTreeSet<u8>,
    pub balance: BTreeMap<u8, u32>,
    pub pool: u32,
    pub records: Vec<u8>,
}

/// The actions of the spec's `Next`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Register { a: u8, v: u32 },
    Decommission { a: u8 },
    Submit { a: u8 },
    Transfer { a: u8, b: u8, amt: u32 },
}

/// Guard failures. A guard failure means the action is not enabled in that state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelError {
    UnknownAsset,
    IdAlreadyUsed,
    InsufficientPool,
    NotRegistered,
    HistoryFull,
    SameAsset,
    InvalidAmount,
    InsufficientBalance,
}

/// Deliberately broken variants used to prove the invariants are not vacuous.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutation {
    None,
    /// Decommission drops the asset's value instead of returning it to the pool.
    LeakOnDecommission,
    /// Submit is allowed for assets that were never registered.
    SubmitUnregistered,
}

/// Model instance. Mirrors the constants `AssetIds`, `TotalValue`, `MaxRecords`.
#[derive(Clone, Debug)]
pub struct Model {
    pub asset_ids: Vec<u8>,
    pub total_value: u32,
    pub max_records: usize,
    pub mutation: Mutation,
}

impl Model {
    pub fn new(asset_ids: Vec<u8>, total_value: u32, max_records: usize) -> Self {
        Self { asset_ids, total_value, max_records, mutation: Mutation::None }
    }

    pub fn with_mutation(mut self, mutation: Mutation) -> Self {
        self.mutation = mutation;
        self
    }

    /// `Init`.
    pub fn init(&self) -> State {
        State {
            registered: BTreeSet::new(),
            ever: BTreeSet::new(),
            balance: self.asset_ids.iter().map(|a| (*a, 0)).collect(),
            pool: self.total_value,
            records: Vec::new(),
        }
    }

    /// Every action whose parameters lie in the model's finite domains (`Next`'s quantifiers).
    pub fn candidate_actions(&self) -> Vec<Action> {
        let mut out = Vec::new();
        for &a in &self.asset_ids {
            for v in 0..=self.total_value {
                out.push(Action::Register { a, v });
            }
            out.push(Action::Decommission { a });
            out.push(Action::Submit { a });
        }
        for &a in &self.asset_ids {
            for &b in &self.asset_ids {
                for amt in 1..=self.total_value {
                    out.push(Action::Transfer { a, b, amt });
                }
            }
        }
        out
    }

    /// Apply one action. Returns the successor state, or the first failing guard.
    pub fn step(&self, s: &State, action: Action) -> Result<State, ModelError> {
        let mut next = s.clone();
        match action {
            Action::Register { a, v } => {
                if !self.asset_ids.contains(&a) {
                    return Err(ModelError::UnknownAsset);
                }
                if s.ever.contains(&a) {
                    return Err(ModelError::IdAlreadyUsed);
                }
                if v > s.pool {
                    return Err(ModelError::InsufficientPool);
                }
                next.registered.insert(a);
                next.ever.insert(a);
                next.balance.insert(a, v);
                next.pool = s.pool - v;
            }
            Action::Decommission { a } => {
                if !s.registered.contains(&a) {
                    return Err(ModelError::NotRegistered);
                }
                let value = s.balance[&a];
                next.registered.remove(&a);
                next.balance.insert(a, 0);
                if self.mutation != Mutation::LeakOnDecommission {
                    next.pool = s.pool + value;
                }
            }
            Action::Submit { a } => {
                if self.mutation != Mutation::SubmitUnregistered && !s.registered.contains(&a) {
                    return Err(ModelError::NotRegistered);
                }
                if !self.asset_ids.contains(&a) {
                    return Err(ModelError::UnknownAsset);
                }
                if s.records.len() >= self.max_records {
                    return Err(ModelError::HistoryFull);
                }
                next.records.push(a);
            }
            Action::Transfer { a, b, amt } => {
                if !s.registered.contains(&a) || !s.registered.contains(&b) {
                    return Err(ModelError::NotRegistered);
                }
                if a == b {
                    return Err(ModelError::SameAsset);
                }
                if amt == 0 {
                    return Err(ModelError::InvalidAmount);
                }
                let from = s.balance[&a];
                if amt > from {
                    return Err(ModelError::InsufficientBalance);
                }
                next.balance.insert(a, from - amt);
                *next.balance.get_mut(&b).expect("registered asset has a balance entry") += amt;
            }
        }
        Ok(next)
    }

    /// Check every invariant of the spec on `s`. Returns the name of the first violated one.
    pub fn check_invariants(&self, s: &State) -> Result<(), &'static str> {
        // TypeOK
        if !s.registered.is_subset(&s.ever)
            || !s.ever.iter().all(|a| self.asset_ids.contains(a))
            || s.pool > self.total_value
            || s.records.len() > self.max_records
            || s.records.iter().any(|a| !self.asset_ids.contains(a))
        {
            return Err("TypeOK");
        }
        // Conservation: pool + sum(balance) = TotalValue
        let held: u32 = s.balance.values().sum();
        if s.pool + held != self.total_value {
            return Err("Conservation");
        }
        // UnregisteredHoldsNoValue
        if self
            .asset_ids
            .iter()
            .any(|a| !s.registered.contains(a) && s.balance[a] != 0)
        {
            return Err("UnregisteredHoldsNoValue");
        }
        // NoOrphanedRecords
        if s.records.iter().any(|a| !s.ever.contains(a)) {
            return Err("NoOrphanedRecords");
        }
        Ok(())
    }
}

/// A counterexample: the violated invariant and the action trace that reaches it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub invariant: &'static str,
    pub trace: Vec<Action>,
}

/// Result of an exhaustive exploration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExploreReport {
    /// Number of distinct reachable states (comparable to TLC's "distinct states").
    pub distinct_states: usize,
    /// Length of the longest shortest-path from `Init` to any reachable state.
    pub max_depth: usize,
    pub violation: Option<Violation>,
}

/// Exhaustive breadth-first exploration. Stops at the first invariant violation.
pub fn explore(model: &Model) -> ExploreReport {
    let init = model.init();
    let actions = model.candidate_actions();

    // Each entry: (state, parent index, action that led here). Index 0 is Init.
    let mut nodes: Vec<(State, Option<usize>, Option<Action>)> = vec![(init.clone(), None, None)];
    let mut index: HashMap<State, usize> = HashMap::new();
    index.insert(init.clone(), 0);
    let mut depth_of: Vec<usize> = vec![0];
    let mut queue: VecDeque<usize> = VecDeque::from([0]);
    let mut max_depth = 0;

    let trace_to = |nodes: &Vec<(State, Option<usize>, Option<Action>)>, mut i: usize| {
        let mut trace = Vec::new();
        while let Some(parent) = nodes[i].1 {
            trace.push(nodes[i].2.expect("non-root node has an action"));
            i = parent;
        }
        trace.reverse();
        trace
    };

    if let Err(invariant) = model.check_invariants(&init) {
        return ExploreReport {
            distinct_states: 1,
            max_depth: 0,
            violation: Some(Violation { invariant, trace: vec![] }),
        };
    }

    while let Some(i) = queue.pop_front() {
        let current = nodes[i].0.clone();
        for &action in &actions {
            let Ok(next) = model.step(&current, action) else { continue };
            if index.contains_key(&next) {
                continue;
            }
            let j = nodes.len();
            index.insert(next.clone(), j);
            nodes.push((next.clone(), Some(i), Some(action)));
            let d = depth_of[i] + 1;
            depth_of.push(d);
            max_depth = max_depth.max(d);
            if let Err(invariant) = model.check_invariants(&next) {
                return ExploreReport {
                    distinct_states: nodes.len(),
                    max_depth,
                    violation: Some(Violation { invariant, trace: trace_to(&nodes, j) }),
                };
            }
            queue.push_back(j);
        }
    }

    ExploreReport { distinct_states: nodes.len(), max_depth, violation: None }
}
