//! Engineer performance ranking from historical job outcomes.

use crate::logistic::{LogisticModel, TrainConfig, TrainError};

/// Per-engineer features aggregated over their history.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineerFeatures {
    /// Fraction of assigned jobs completed (0..=1).
    pub completion_rate: f64,
    /// Mean hours from assignment to completion.
    pub avg_turnaround_hours: f64,
    /// Fraction of completed jobs that were later disputed (0..=1).
    pub dispute_rate: f64,
    /// Total completed jobs.
    pub jobs_completed: f64,
}

impl EngineerFeatures {
    fn to_features(self) -> Vec<f64> {
        vec![self.completion_rate, self.avg_turnaround_hours, self.dispute_rate, self.jobs_completed]
    }
}

/// Scores engineers by the predicted probability that their next job is
/// delivered without dispute.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineerRanker {
    model: LogisticModel,
}

impl EngineerRanker {
    /// Train from past jobs: `true` means the job completed without dispute.
    pub fn train(
        outcomes: &[(EngineerFeatures, bool)],
        cfg: TrainConfig,
    ) -> Result<Self, TrainError> {
        let xs: Vec<Vec<f64>> = outcomes.iter().map(|(f, _)| f.to_features()).collect();
        let ys: Vec<bool> = outcomes.iter().map(|(_, y)| *y).collect();
        Ok(Self { model: LogisticModel::train(&xs, &ys, cfg)? })
    }

    /// Score one engineer, or `None` if the features are malformed.
    pub fn score(&self, features: &EngineerFeatures) -> Option<f64> {
        self.model.predict_proba(&features.to_features())
    }

    /// Rank `(engineer_id, features)` pairs, best first. Ties break on the lower id so
    /// the order is stable. Engineers with malformed features are dropped.
    pub fn rank(&self, engineers: &[(u64, EngineerFeatures)]) -> Vec<(u64, f64)> {
        let mut scored: Vec<(u64, f64)> = engineers
            .iter()
            .filter_map(|(id, f)| self.score(f).map(|s| (*id, s)))
            .collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        scored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(completion: f64, dispute: f64, jobs: f64) -> EngineerFeatures {
        EngineerFeatures {
            completion_rate: completion,
            avg_turnaround_hours: 24.0,
            dispute_rate: dispute,
            jobs_completed: jobs,
        }
    }

    #[test]
    fn reliable_engineers_rank_first() {
        let mut outcomes = Vec::new();
        for i in 0..15 {
            outcomes.push((f(0.95, 0.0, 50.0 + i as f64), true));
            outcomes.push((f(0.5, 0.4, 3.0 + i as f64), false));
        }
        let ranker = EngineerRanker::train(&outcomes, TrainConfig::default()).unwrap();
        let ranked = ranker.rank(&[(7, f(0.5, 0.4, 4.0)), (3, f(0.95, 0.0, 60.0))]);
        assert_eq!(ranked[0].0, 3);
        assert_eq!(ranked[1].0, 7);
    }

    #[test]
    fn ties_break_on_lower_id_and_bad_features_are_dropped() {
        let mut outcomes = Vec::new();
        for i in 0..10 {
            outcomes.push((f(0.9, 0.0, 20.0 + i as f64), true));
            outcomes.push((f(0.2, 0.5, 2.0 + i as f64), false));
        }
        let ranker = EngineerRanker::train(&outcomes, TrainConfig::default()).unwrap();
        let same = f(0.9, 0.0, 25.0);
        let bad = EngineerFeatures { completion_rate: f64::NAN, ..same };
        let ranked = ranker.rank(&[(9, same), (2, same), (5, bad)]);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].0, 2);
        assert_eq!(ranked[1].0, 9);
    }
}
