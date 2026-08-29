use soroban_sdk::{Env, String as SorobanString};

pub const DEFAULT_MAX_STRING_LEN: u32 = 256;

pub fn require_non_empty_string(value: &SorobanString, field: &str) {
    if value.is_empty() {
        panic!("validation: {} must not be empty", field);
    }
}

pub fn require_string_length(value: &SorobanString, field: &str, max: u32) {
    require_non_empty_string(value, field);
    if value.len() > max {
        panic!("validation: {} exceeds maximum length", field);
    }
}

/// Panics if `value` is an empty vector.
///
/// **Element validation is the caller's responsibility.** This function only
/// checks that the collection has at least one entry. Callers must separately
/// validate individual elements (e.g. reject zero-value or null-equivalent
/// entries) according to their own domain rules.
pub fn require_non_empty_vec<T>(value: &soroban_sdk::Vec<T>, field: &str) {
    if value.is_empty() {
        panic!("validation: {} must not be empty", field);
    }
}

pub fn require_positive_u64(value: u64, field: &str) {
    if value == 0 {
        panic!("validation: {} must be positive", field);
    }
}

pub fn require_positive_u32(value: u32, field: &str) {
    if value == 0 {
        panic!("validation: {} must be positive", field);
    }
}

pub fn require_within_bounds(value: u64, min: u64, max: u64, field: &str) {
    if value < min || value > max {
        panic!("validation: {} must be within bounds", field);
    }
}

pub fn require_env(_env: &Env) {}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{Env, String as SorobanString, Vec};

    #[test]
    #[should_panic(expected = "validation: asset_id must not be empty")]
    fn empty_string_rejected() {
        let env = Env::default();
        let value = SorobanString::from_str(&env, "");
        require_non_empty_string(&value, "asset_id");
    }

    #[test]
    #[should_panic(expected = "validation: description exceeds maximum length")]
    fn long_string_rejected() {
        let env = Env::default();
        let value = SorobanString::from_str(&env, &"x".repeat(300));
        require_string_length(&value, "description", 256);
    }

    #[test]
    fn valid_string_passes() {
        let env = Env::default();
        let value = SorobanString::from_str(&env, "valid");
        require_string_length(&value, "field", 256);
    }

    #[test]
    #[should_panic(expected = "validation: amount must be positive")]
    fn zero_amount_rejected() {
        require_positive_u64(0, "amount");
    }

    #[test]
    #[should_panic(expected = "validation: items must not be empty")]
    fn empty_vec_rejected() {
        let env = Env::default();
        let items: Vec<u64> = Vec::new(&env);
        require_non_empty_vec(&items, "items");
    }

    /// #805: require_non_empty_vec passes a vec containing zero-value entries —
    /// element validation is the caller's responsibility, not this helper's.
    #[test]
    fn vec_with_zero_entries_passes_non_empty_check() {
        let env = Env::default();
        let mut items: Vec<u64> = Vec::new(&env);
        items.push_back(0u64);
        items.push_back(0u64);
        // Should NOT panic — the vec is non-empty; callers own per-element validation.
        require_non_empty_vec(&items, "items");
    }
}
