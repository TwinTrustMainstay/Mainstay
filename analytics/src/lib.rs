//! Predictive analytics for Mainstay maintenance planning (issue #1757).
//!
//! The on-chain lifecycle contract keeps rule-based scoring. This crate adds
//! trained models that run off-chain (in the indexer / backend), where
//! training is practical. Models are small, dependency-free, and fully
//! deterministic so that the same history always produces the same weights.
//!
//! Modules:
//! - [`logistic`]: standardised logistic regression trained with batch gradient descent.
//! - [`failure`]: failure-risk prediction for an asset from its maintenance history.
//! - [`schedule`]: optimal service-interval recommendation from predicted risk and cost.
//! - [`ranking`]: engineer performance ranking from historical job outcomes.
//! - [`serving`]: versioned model registry used as the model-serving API.
//! - [`metrics`]: accuracy, Brier score, rolling tracking, and PSI-based drift detection.

pub mod failure;
pub mod logistic;
pub mod metrics;
pub mod ranking;
pub mod schedule;
pub mod serving;

pub use logistic::{LogisticModel, TrainConfig, TrainError};
