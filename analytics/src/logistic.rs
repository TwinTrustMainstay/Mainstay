//! Standardised logistic regression trained by deterministic batch gradient descent.

/// Errors raised when a model cannot be trained on the supplied data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainError {
    /// No samples, or no features.
    Empty,
    /// Feature rows have inconsistent lengths, or the row and label counts differ.
    DimensionMismatch,
    /// A feature value is NaN or infinite.
    NonFinite,
    /// All labels are identical, so there is nothing to discriminate.
    SingleClass,
}

/// Hyper-parameters for training. The defaults are tuned for the small,
/// standardised feature sets used by the maintenance models.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrainConfig {
    pub epochs: usize,
    pub learning_rate: f64,
    /// L2 regularisation strength applied to the weights (not the bias).
    pub l2: f64,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self { epochs: 500, learning_rate: 0.1, l2: 1e-3 }
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
}

impl LogisticModel {
    /// Train on rows `xs` (one feature vector per sample) and boolean labels `ys`.
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
        for _ in 0..cfg.epochs {
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
        }

        Ok(Self { means, scales, weights, bias })
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
