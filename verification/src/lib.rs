//! Executable mirror of `tla/MaintenanceRecords.tla` (issue #1756).
//!
//! The model uses the same variables, actions and invariants as the TLA+
//! spec. [`explore`] enumerates every reachable state breadth-first and checks
//! the invariants on each one. The test suite asserts the same distinct-state
//! count TLC reports, so a change to either side that is not mirrored in the
//! other fails CI.

pub mod model;

pub use model::{explore, Action, ExploreReport, Model, ModelError, Mutation, State, Violation};
