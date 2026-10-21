//! Optimal service-interval recommendation.
//!
//! For each candidate interval `T` (days), the expected cost per day is
//! `(service_cost + p_fail(T) * failure_cost) / T`, where `p_fail(T)` is the
//! predicted failure probability for an asset last serviced `T` days ago.
//! The interval minimising that rate is recommended.

use crate::failure::{AssetHistorySummary, FailurePredictor};

/// Expected cost per day of servicing every `interval_days` days.
pub fn expected_cost_per_day(
    predictor: &FailurePredictor,
    base: &AssetHistorySummary,
    interval_days: f64,
    service_cost: f64,
    failure_cost: f64,
) -> Option<f64> {
    if !interval_days.is_finite() || interval_days <= 0.0 {
        return None;
    }
    let state = AssetHistorySummary { days_since_last_service: interval_days, ..*base };
    let p_fail = predictor.failure_probability(&state)?;
    Some((service_cost + p_fail * failure_cost) / interval_days)
}

/// Recommend the candidate interval with the lowest expected cost per day.
/// Ties resolve to the earliest candidate. Returns `None` if no candidate is valid.
pub fn recommend_interval_days(
    predictor: &FailurePredictor,
    base: &AssetHistorySummary,
    candidates_days: &[f64],
    service_cost: f64,
    failure_cost: f64,
) -> Option<f64> {
    let mut best: Option<(f64, f64)> = None;
    for &t in candidates_days {
        if let Some(rate) = expected_cost_per_day(predictor, base, t, service_cost, failure_cost) {
            match best {
                Some((_, best_rate)) if rate >= best_rate => {}
                _ => best = Some((t, rate)),
            }
        }
    }
    best.map(|(t, _)| t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logistic::TrainConfig;

    fn base() -> AssetHistorySummary {
        AssetHistorySummary {
            days_since_last_service: 0.0,
            services_last_90d: 1.0,
            avg_cost_stroops: 500.0,
            dispute_count: 0.0,
        }
    }

    #[test]
    fn expensive_failures_shorten_the_interval() {
        // Risk rises steeply with time since service.
        let mut samples = Vec::new();
        for d in (0..400).step_by(10) {
            let fail = d >= 120;
            samples.push((AssetHistorySummary { days_since_last_service: d as f64, ..base() }, fail));
        }
        let p = FailurePredictor::train(&samples, TrainConfig::default()).unwrap();
        let candidates = [30.0, 90.0, 180.0, 360.0];

        let cheap_failures =
            recommend_interval_days(&p, &base(), &candidates, 100.0, 1.0).unwrap();
        let costly_failures =
            recommend_interval_days(&p, &base(), &candidates, 100.0, 1_000_000.0).unwrap();
        assert!(costly_failures <= cheap_failures);
        assert_eq!(costly_failures, 30.0);
    }

    #[test]
    fn invalid_candidates_are_ignored() {
        let samples = vec![
            (AssetHistorySummary { days_since_last_service: 1.0, ..base() }, false),
            (AssetHistorySummary { days_since_last_service: 500.0, ..base() }, true),
        ];
        let p = FailurePredictor::train(&samples, TrainConfig::default()).unwrap();
        assert_eq!(recommend_interval_days(&p, &base(), &[], 1.0, 1.0), None);
        assert_eq!(recommend_interval_days(&p, &base(), &[0.0, -5.0, f64::NAN], 1.0, 1.0), None);
    }
}
