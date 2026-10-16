//! Failure-risk prediction from an asset's maintenance history.

use crate::logistic::{LogisticModel, TrainConfig, TrainError};

/// Features summarising an asset's maintenance history at a point in time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AssetHistorySummary {
    /// Days since the most recent maintenance record.
    pub days_since_last_service: f64,
    /// Number of maintenance records in the trailing 90 days.
    pub services_last_90d: f64,
    /// Mean maintenance cost in stroops over the history (0 if none).
    pub avg_cost_stroops: f64,
    /// Number of disputed records on the asset.
    pub dispute_count: f64,
}

impl AssetHistorySummary {
    fn to_features(self) -> Vec<f64> {
        vec![
            self.days_since_last_service,
            self.services_last_90d,
            self.avg_cost_stroops,
            self.dispute_count,
        ]
    }
}

/// Predicts the probability that an asset will fail (or need unscheduled
/// repair) in the next maintenance window.
#[derive(Debug, Clone, PartialEq)]
pub struct FailurePredictor {
    model: LogisticModel,
}

impl FailurePredictor {
    /// Train from labelled history: `true` means a failure followed the summarised state.
    pub fn train(
        samples: &[(AssetHistorySummary, bool)],
        cfg: TrainConfig,
    ) -> Result<Self, TrainError> {
        let xs: Vec<Vec<f64>> = samples.iter().map(|(s, _)| s.to_features()).collect();
        let ys: Vec<bool> = samples.iter().map(|(_, y)| *y).collect();
        Ok(Self { model: LogisticModel::train(&xs, &ys, cfg)? })
    }

    /// Failure probability for the given history summary.
    pub fn failure_probability(&self, summary: &AssetHistorySummary) -> Option<f64> {
        self.model.predict_proba(&summary.to_features())
    }

    /// The underlying trained model (for serving or persistence).
    pub fn model(&self) -> &LogisticModel {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(days: f64, dispute: f64) -> AssetHistorySummary {
        AssetHistorySummary {
            days_since_last_service: days,
            services_last_90d: 1.0,
            avg_cost_stroops: 500.0,
            dispute_count: dispute,
        }
    }

    #[test]
    fn long_gaps_and_disputes_predict_higher_risk() {
        // Failures follow long service gaps with disputes.
        let mut samples = Vec::new();
        for d in 0..20 {
            samples.push((summary(10.0 + d as f64, 0.0), false));
            samples.push((summary(200.0 + d as f64, 2.0), true));
        }
        let p = FailurePredictor::train(&samples, TrainConfig::default()).unwrap();
        let low = p.failure_probability(&summary(15.0, 0.0)).unwrap();
        let high = p.failure_probability(&summary(210.0, 2.0)).unwrap();
        assert!(high > low, "expected {high} > {low}");
        assert!(high > 0.5 && low < 0.5);
    }
}
