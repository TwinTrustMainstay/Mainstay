//! Model accuracy tracking and drift detection.

use std::collections::VecDeque;

/// Fraction of predictions whose thresholded class matches the label.
/// Returns `None` for empty or mismatched inputs.
pub fn accuracy(probs: &[f64], labels: &[bool], threshold: f64) -> Option<f64> {
    if probs.is_empty() || probs.len() != labels.len() {
        return None;
    }
    let correct = probs
        .iter()
        .zip(labels)
        .filter(|(p, &y)| (**p >= threshold) == y)
        .count();
    Some(correct as f64 / probs.len() as f64)
}

/// Mean squared error between predicted probabilities and outcomes (lower is better).
pub fn brier_score(probs: &[f64], labels: &[bool]) -> Option<f64> {
    if probs.is_empty() || probs.len() != labels.len() {
        return None;
    }
    let total: f64 = probs
        .iter()
        .zip(labels)
        .map(|(p, &y)| {
            let t = if y { 1.0 } else { 0.0 };
            (p - t).powi(2)
        })
        .sum();
    Some(total / probs.len() as f64)
}

/// Rolling window of recent (probability, outcome) pairs for live accuracy tracking.
#[derive(Debug, Clone)]
pub struct AccuracyTracker {
    capacity: usize,
    window: VecDeque<(f64, bool)>,
}

impl AccuracyTracker {
    pub fn new(capacity: usize) -> Self {
        Self { capacity: capacity.max(1), window: VecDeque::new() }
    }

    /// Record one resolved prediction, evicting the oldest once the window is full.
    pub fn record(&mut self, probability: f64, outcome: bool) {
        if self.window.len() == self.capacity {
            self.window.pop_front();
        }
        self.window.push_back((probability, outcome));
    }

    pub fn len(&self) -> usize {
        self.window.len()
    }

    pub fn is_empty(&self) -> bool {
        self.window.is_empty()
    }

    /// Accuracy over the window at threshold 0.5.
    pub fn accuracy(&self) -> Option<f64> {
        let (probs, labels): (Vec<f64>, Vec<bool>) = self.window.iter().copied().unzip();
        accuracy(&probs, &labels, 0.5)
    }

    /// Brier score over the window.
    pub fn brier(&self) -> Option<f64> {
        let (probs, labels): (Vec<f64>, Vec<bool>) = self.window.iter().copied().unzip();
        brier_score(&probs, &labels)
    }
}

/// Population Stability Index (PSI) drift monitor over a single feature or score.
///
/// Bins are fixed from the training-time reference sample. A PSI above roughly
/// 0.2 is conventionally treated as significant drift.
#[derive(Debug, Clone)]
pub struct DriftMonitor {
    min: f64,
    width: f64,
    bins: usize,
    reference_fractions: Vec<f64>,
}

impl DriftMonitor {
    /// Build from a reference sample using `bins` equal-width buckets. Returns `None`
    /// for an empty or non-finite reference or zero bins.
    pub fn new(reference: &[f64], bins: usize) -> Option<Self> {
        if reference.is_empty() || bins == 0 || reference.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let min = reference.iter().copied().fold(f64::INFINITY, f64::min);
        let max = reference.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let width = if max > min { (max - min) / bins as f64 } else { 1.0 };
        let mut monitor = Self { min, width, bins, reference_fractions: vec![0.0; bins] };
        monitor.reference_fractions = monitor.fractions(reference);
        Some(monitor)
    }

    /// Population Stability Index of `live` against the reference.
    pub fn psi(&self, live: &[f64]) -> Option<f64> {
        if live.is_empty() || live.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let live_fractions = self.fractions(live);
        let eps = 1e-6;
        Some(
            self.reference_fractions
                .iter()
                .zip(&live_fractions)
                .map(|(r, l)| {
                    let r = r.max(eps);
                    let l = l.max(eps);
                    (l - r) * (l / r).ln()
                })
                .sum(),
        )
    }

    /// True when `live` has drifted beyond `threshold` PSI (default convention 0.2).
    pub fn has_drifted(&self, live: &[f64], threshold: f64) -> Option<bool> {
        self.psi(live).map(|p| p > threshold)
    }

    fn fractions(&self, values: &[f64]) -> Vec<f64> {
        let mut counts = vec![0usize; self.bins];
        for &v in values {
            let idx = (((v - self.min) / self.width).floor() as isize)
                .clamp(0, self.bins as isize - 1) as usize;
            counts[idx] += 1;
        }
        counts.iter().map(|&c| c as f64 / values.len() as f64).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accuracy_and_brier_basic() {
        let probs = [0.9, 0.2, 0.7, 0.4];
        let labels = [true, false, false, false];
        assert_eq!(accuracy(&probs, &labels, 0.5), Some(0.75));
        let b = brier_score(&probs, &labels).unwrap();
        assert!((b - (0.01 + 0.04 + 0.49 + 0.16) / 4.0).abs() < 1e-12);
        assert_eq!(accuracy(&[], &[], 0.5), None);
        assert_eq!(brier_score(&[0.5], &[true, false]), None);
    }

    #[test]
    fn tracker_keeps_only_recent_window() {
        let mut t = AccuracyTracker::new(2);
        t.record(0.9, false); // wrong, will be evicted
        t.record(0.9, true);
        t.record(0.1, false);
        assert_eq!(t.len(), 2);
        assert_eq!(t.accuracy(), Some(1.0));
    }

    #[test]
    fn drift_detects_shifted_distribution() {
        let reference: Vec<f64> = (0..100).map(|i| i as f64).collect();
        let m = DriftMonitor::new(&reference, 10).unwrap();
        assert_eq!(m.has_drifted(&reference, 0.2), Some(false));
        let shifted: Vec<f64> = (0..100).map(|i| (i as f64) * 0.1).collect();
        assert_eq!(m.has_drifted(&shifted, 0.2), Some(true));
        assert!(m.psi(&reference).unwrap() < 1e-9);
    }

    #[test]
    fn drift_monitor_rejects_bad_input() {
        assert!(DriftMonitor::new(&[], 5).is_none());
        assert!(DriftMonitor::new(&[1.0], 0).is_none());
        assert!(DriftMonitor::new(&[f64::NAN], 5).is_none());
        let m = DriftMonitor::new(&[1.0, 1.0], 3).unwrap();
        assert!(m.psi(&[]).is_none());
    }
}
