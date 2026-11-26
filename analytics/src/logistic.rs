//! Standardised logistic regression trained by deterministic batch gradient descent.
//! Training stops early once the loss change falls below [`TrainConfig::tolerance`], and the
//! model reports whether it converged.

/// Errors raised when a model cannot be trained on the supplied data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainError {
    /// No samples, or no features.
    Empty,
    /// Feature rows have inconsistent lengths, or the row and label counts differ.
    DimensionMismatch,
    /// A feature value or the computed loss is NaN or infinite.
    NonFinite,
    /// All labels are identical, so there is nothing to discriminate.
    SingleClass,
}

/// Optimisation state reported alongside a trained model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConvergenceReport {
    /// Whether training stopped because the loss change fell below the tolerance.
    pub converged: bool,
    /// Number of gradient-descent epochs completed.
    pub epochs: usize,
    /// Objective value after the last completed epoch.
    pub final_loss: f64,
    /// Absolute change in the objective over the last epoch.
    pub loss_change: f64,
}

/// Hyper-parameters for training. The defaults are tuned for the small,
/// standardised feature sets used by the maintenance models.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrainConfig {
    pub epochs: usize,
    pub learning_rate: f64,
    /// L2 regularisation strength applied to the weights (not the bias).
    pub l2: f64,
    /// Stop training when the absolute loss change between epochs is below this value.
    pub tolerance: f64,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self { epochs: 500, learning_rate: 0.1, l2: 1e-3, tolerance: 1e-4 }
    }
}

/// A trained binary classifier. Features are standardised with the means and
/// scales learned at training time, so inputs must be in the raw feature space.
#[derive(Debug, Clone, PartialEq)]
pub struct LogisticModel {
    means: Vec<f64>,
    scales: Vec<f64>,
    weights: Vec<f64>,
    bias: f64,
    /// Optimisation state captured when training stopped.
    pub convergence: ConvergenceReport,
}

impl LogisticModel {
    /// Train on rows `xs` (one feature vector per sample) and boolean labels `ys`.
    ///
    /// Training stops when the loss change falls below `cfg.tolerance` or the `cfg.epochs`
    /// budget is exhausted. The returned model reports which happened in
    /// [`LogisticModel::convergence`].
    pub fn train(xs: &[Vec<f64>], ys: &[bool], cfg: TrainConfig) -> Result<Self, TrainError> {
        if xs.is_empty() {
            return Err(TrainError::Empty);
        }
        if xs.len() != ys.len() {
            return Err(TrainError::DimensionMismatch);
        }
        let d = xs[0].len();
        if d == 0 {
            return Err(TrainError::Empty);
        }
        if xs.iter().any(|x| x.len() != d) {
            return Err(TrainError::DimensionMismatch);
        }
        if xs.iter().flatten().any(|v| !v.is_finite()) {
            return Err(TrainError::NonFinite);
        }
        let positives = ys.iter().filter(|&&y| y).count();
        if positives == 0 || positives == ys.len() {
            return Err(TrainError::SingleClass);
        }

        let n = xs.len() as f64;
        let means: Vec<f64> = (0..d)
            .map(|j| xs.iter().map(|x| x[j]).sum::<f64>() / n)
            .collect();
        let scales: Vec<f64> = (0..d)
            .map(|j| {
                let var = xs.iter().map(|x| (x[j] - means[j]).powi(2)).sum::<f64>() / n;
                let sd = var.sqrt();
                if sd < 1e-12 { 1.0 } else { sd }
            })
            .collect();
        let zs: Vec<Vec<f64>> = xs
            .iter()
            .map(|x| (0..d).map(|j| (x[j] - means[j]) / scales[j]).collect())
            .collect();

        let mut weights = vec![0.0; d];
        let mut bias = 0.0;
        let mut previous_loss = loss(&zs, ys, &weights, bias, cfg.l2);
        if !previous_loss.is_finite() {
            return Err(TrainError::NonFinite);
        }
        let mut final_loss = previous_loss;
        let mut loss_change = 0.0;
        let mut completed_epochs = 0;
        let mut converged = false;

        for epoch in 1..=cfg.epochs {
            let mut grad_w = vec![0.0; d];
            let mut grad_b = 0.0;
            for (z, &y) in zs.iter().zip(ys) {
                let target = if y { 1.0 } else { 0.0 };
                let err = sigmoid(dot(&weights, z) + bias) - target;
                for (g, zj) in grad_w.iter_mut().zip(z) {
                    *g += err * zj;
                }
                grad_b += err;
            }
            for (w, g) in weights.iter_mut().zip(&grad_w) {
                *w -= cfg.learning_rate * (g / n + cfg.l2 * *w);
            }
            bias -= cfg.learning_rate * grad_b / n;

            final_loss = loss(&zs, ys, &weights, bias, cfg.l2);
            if !final_loss.is_finite() {
                return Err(TrainError::NonFinite);
            }
            loss_change = (previous_loss - final_loss).abs();
            completed_epochs = epoch;
            if loss_change < cfg.tolerance {
                converged = true;
                break;
            }
            previous_loss = final_loss;
        }

        let convergence = ConvergenceReport {
            converged,
            epochs: completed_epochs,
            final_loss,
            loss_change,
        };
        Ok(Self { means, scales, weights, bias, convergence })
    }

    /// Probability that the positive class applies. `None` if `x` has the wrong length
    /// or contains a non-finite value.
    pub fn predict_proba(&self, x: &[f64]) -> Option<f64> {
        if x.len() != self.weights.len() || x.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let z: Vec<f64> = x
            .iter()
            .enumerate()
            .map(|(j, v)| (v - self.means[j]) / self.scales[j])
            .collect();
        Some(sigmoid(dot(&self.weights, &z) + self.bias))
    }

    /// Number of input features the model expects.
    pub fn feature_count(&self) -> usize {
        self.weights.len()
    }

    /// Optimisation state captured when training stopped.
    pub fn convergence(&self) -> ConvergenceReport {
        self.convergence
    }

    /// Whether training stopped because the loss settled below the tolerance.
    pub fn is_converged(&self) -> bool {
        self.convergence.converged
    }
}

fn loss(zs: &[Vec<f64>], ys: &[bool], weights: &[f64], bias: f64, l2: f64) -> f64 {
    let data_loss = zs
        .iter()
        .zip(ys)
        .map(|(z, &y)| {
            let logit = dot(weights, z) + bias;
            softplus(logit) - if y { logit } else { 0.0 }
        })
        .sum::<f64>()
        / zs.len() as f64;
    let reg = 0.5 * l2 * weights.iter().map(|w| w * w).sum::<f64>();
    data_loss + reg
}

fn softplus(z: f64) -> f64 {
    if z > 40.0 { z } else { (1.0 + z.exp()).ln() }
}

fn sigmoid(z: f64) -> f64 {
    1.0 / (1.0 + (-z).exp())
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn separable() -> (Vec<Vec<f64>>, Vec<bool>) {
        // Label is true when the first feature is large.
        let xs: Vec<Vec<f64>> = (0..40).map(|i| vec![i as f64, (i % 7) as f64]).collect();
        let ys: Vec<bool> = xs.iter().map(|x| x[0] >= 20.0).collect();
        (xs, ys)
    }

    fn non_separable() -> (Vec<Vec<f64>>, Vec<bool>) {
        let xs: Vec<Vec<f64>> = (0..40).map(|i| vec![i as f64]).collect();
        let ys: Vec<bool> = (0..40).map(|i| i % 4 < 2).collect();
        (xs, ys)
    }

    #[test]
    fn separable_data_converges_before_the_epoch_budget() {
        let (xs, ys) = separable();
        let cfg = TrainConfig::default();
        let m = LogisticModel::train(&xs, &ys, cfg).unwrap();
        let report = m.convergence();

        assert!(m.is_converged());
        assert_eq!(m.convergence(), report);
        assert!(report.converged);
        assert!(report.epochs > 0 && report.epochs < cfg.epochs);
        assert!(report.loss_change < cfg.tolerance);
        assert!(report.final_loss.is_finite());

        let correct = xs
            .iter()
            .zip(&ys)
            .filter(|(x, &y)| (m.predict_proba(x).unwrap() >= 0.5) == y)
            .count();
        assert!(correct as f64 / xs.len() as f64 >= 0.95, "accuracy too low: {correct}");
    }

    #[test]
    fn non_separable_data_reports_both_convergence_states() {
        let (xs, ys) = non_separable();
        let short_cfg = TrainConfig { epochs: 1, ..TrainConfig::default() };
        let short_model = LogisticModel::train(&xs, &ys, short_cfg).unwrap();
        let short_report = short_model.convergence();

        assert!(!short_model.is_converged());
        assert!(!short_report.converged);
        assert_eq!(short_report.epochs, short_cfg.epochs);
        assert!(short_report.loss_change >= short_cfg.tolerance);

        let cfg = TrainConfig::default();
        let m = LogisticModel::train(&xs, &ys, cfg).unwrap();
        let report = m.convergence();

        assert!(m.is_converged());
        assert!(report.epochs > 0 && report.epochs < cfg.epochs);
        assert!(report.loss_change < cfg.tolerance);
        assert!(report.final_loss.is_finite());
        assert!(xs
            .iter()
            .all(|x| m.predict_proba(x).unwrap().is_finite()));
    }

    #[test]
    fn zero_epoch_budget_reports_an_unconverged_model() {
        let (xs, ys) = separable();
        let cfg = TrainConfig { epochs: 0, ..TrainConfig::default() };
        let m = LogisticModel::train(&xs, &ys, cfg).unwrap();

        let report = m.convergence();
        assert!(!report.converged);
        assert_eq!(report.epochs, 0);
        assert_eq!(report.loss_change, 0.0);
        assert!((report.final_loss - (2.0f64).ln()).abs() < 1e-12);
    }

    #[test]
    fn learns_separable_data() {
        let (xs, ys) = separable();
        let m = LogisticModel::train(&xs, &ys, TrainConfig::default()).unwrap();
        let correct = xs
            .iter()
            .zip(&ys)
            .filter(|(x, &y)| (m.predict_proba(x).unwrap() >= 0.5) == y)
            .count();
        assert!(correct as f64 / xs.len() as f64 >= 0.95, "accuracy too low: {correct}");
    }

    #[test]
    fn training_is_deterministic() {
        let (xs, ys) = separable();
        let a = LogisticModel::train(&xs, &ys, TrainConfig::default()).unwrap();
        let b = LogisticModel::train(&xs, &ys, TrainConfig::default()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn rejects_bad_input() {
        let cfg = TrainConfig::default();
        assert_eq!(LogisticModel::train(&[], &[], cfg), Err(TrainError::Empty));
        assert_eq!(
            LogisticModel::train(&[vec![1.0]], &[true, false], cfg),
            Err(TrainError::DimensionMismatch)
        );
        assert_eq!(
            LogisticModel::train(&[vec![1.0], vec![1.0, 2.0]], &[true, false], cfg),
            Err(TrainError::DimensionMismatch)
        );
        assert_eq!(
            LogisticModel::train(&[vec![f64::NAN], vec![1.0]], &[true, false], cfg),
            Err(TrainError::NonFinite)
        );
        assert_eq!(
            LogisticModel::train(&[vec![1.0], vec![2.0]], &[true, true], cfg),
            Err(TrainError::SingleClass)
        );
    }

    #[test]
    fn predict_rejects_wrong_arity_and_non_finite() {
        let (xs, ys) = separable();
        let m = LogisticModel::train(&xs, &ys, TrainConfig::default()).unwrap();
        assert_eq!(m.feature_count(), 2);
        assert!(m.predict_proba(&[1.0]).is_none());
        assert!(m.predict_proba(&[f64::INFINITY, 1.0]).is_none());
    }
}
