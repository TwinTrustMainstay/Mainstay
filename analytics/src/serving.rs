//! Versioned model registry, used as the in-process model-serving API.
//!
//! Models are registered under a name and an integer version. Prediction
//! uses the requested version, or the latest registered one when none is given.

use std::collections::BTreeMap;

use crate::logistic::LogisticModel;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServingError {
    UnknownModel(String),
    UnknownVersion { model: String, version: u32 },
    VersionAlreadyRegistered { model: String, version: u32 },
    /// Feature vector length or values do not fit the model.
    InvalidFeatures,
}

/// Metadata describing one registered model version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub name: String,
    pub version: u32,
    pub feature_count: usize,
}

#[derive(Debug, Default)]
pub struct ModelRegistry {
    models: BTreeMap<String, BTreeMap<u32, LogisticModel>>,
}

impl ModelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new version. Versions are immutable once registered.
    pub fn register(&mut self, name: &str, version: u32, model: LogisticModel) -> Result<(), ServingError> {
        let versions = self.models.entry(name.to_string()).or_default();
        if versions.contains_key(&version) {
            return Err(ServingError::VersionAlreadyRegistered { model: name.to_string(), version });
        }
        versions.insert(version, model);
        Ok(())
    }

    /// Highest registered version for `name`.
    pub fn latest_version(&self, name: &str) -> Option<u32> {
        self.models.get(name)?.keys().next_back().copied()
    }

    /// Predict with `version` (or the latest when `None`).
    pub fn predict(&self, name: &str, version: Option<u32>, features: &[f64]) -> Result<f64, ServingError> {
        let versions = self
            .models
            .get(name)
            .ok_or_else(|| ServingError::UnknownModel(name.to_string()))?;
        let v = match version {
            Some(v) => v,
            None => *versions.keys().next_back().ok_or_else(|| ServingError::UnknownModel(name.to_string()))?,
        };
        let model = versions
            .get(&v)
            .ok_or_else(|| ServingError::UnknownVersion { model: name.to_string(), version: v })?;
        model.predict_proba(features).ok_or(ServingError::InvalidFeatures)
    }

    /// Describe every registered version, ordered by name then version.
    pub fn list(&self) -> Vec<ModelInfo> {
        self.models
            .iter()
            .flat_map(|(name, versions)| {
                versions.iter().map(move |(v, m)| ModelInfo {
                    name: name.clone(),
                    version: *v,
                    feature_count: m.feature_count(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logistic::TrainConfig;

    fn model() -> LogisticModel {
        let xs: Vec<Vec<f64>> = (0..20).map(|i| vec![i as f64]).collect();
        let ys: Vec<bool> = xs.iter().map(|x| x[0] >= 10.0).collect();
        LogisticModel::train(&xs, &ys, TrainConfig::default()).unwrap()
    }

    #[test]
    fn serves_latest_by_default_and_specific_versions_on_request() {
        let mut reg = ModelRegistry::new();
        reg.register("failure", 1, model()).unwrap();
        reg.register("failure", 2, model()).unwrap();
        assert_eq!(reg.latest_version("failure"), Some(2));
        assert!(reg.predict("failure", None, &[15.0]).unwrap() > 0.5);
        assert!(reg.predict("failure", Some(1), &[15.0]).is_ok());
        assert_eq!(reg.list().len(), 2);
    }

    #[test]
    fn errors_are_reported() {
        let mut reg = ModelRegistry::new();
        reg.register("failure", 1, model()).unwrap();
        assert_eq!(reg.predict("nope", None, &[1.0]), Err(ServingError::UnknownModel("nope".into())));
        assert_eq!(
            reg.predict("failure", Some(9), &[1.0]),
            Err(ServingError::UnknownVersion { model: "failure".into(), version: 9 })
        );
        assert_eq!(reg.predict("failure", None, &[1.0, 2.0]), Err(ServingError::InvalidFeatures));
        assert_eq!(
            reg.register("failure", 1, model()),
            Err(ServingError::VersionAlreadyRegistered { model: "failure".into(), version: 1 })
        );
    }
}
