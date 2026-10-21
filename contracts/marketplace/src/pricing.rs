//! Supply/demand price adjustment.
//!
//! All functions here are pure arithmetic so they can be unit-tested without an
//! `Env`. Multipliers and ratios are expressed in basis points (`BPS` = 1.0x).

use crate::{MarketState, PricingConfig};

/// Basis points representing a 1.0x multiplier (and 100%).
pub const BPS: i128 = 10_000;

/// Upper bound on the sensitivity setting (10x the imbalance response).
const MAX_SENSITIVITY_BPS: u32 = 100_000;

/// Returns `true` when a pricing configuration is internally consistent.
///
/// The neutral 1.0x multiplier must lie inside `[min, max]`, so that the
/// starting price is always reachable, and the per-update step must be non-zero
/// so that the price can actually move.
pub fn is_valid_config(cfg: &PricingConfig) -> bool {
    cfg.base_price > 0
        && cfg.min_multiplier_bps > 0
        && (cfg.min_multiplier_bps as i128) <= BPS
        && (cfg.max_multiplier_bps as i128) >= BPS
        && cfg.max_multiplier_bps >= cfg.min_multiplier_bps
        && cfg.max_step_bps > 0
        && cfg.sensitivity_bps > 0
        && cfg.sensitivity_bps <= MAX_SENSITIVITY_BPS
}

/// Multiplier (bps) implied by the current supply and demand, clamped to the
/// configured surge floor and cap.
///
/// Imbalance is `(demand - supply) / supply`. With no supply the denominator is
/// treated as 1 so the result saturates at the surge cap instead of dividing by
/// zero.
pub fn target_multiplier_bps(cfg: &PricingConfig, supply: u32, demand: u32) -> u32 {
    let denominator = supply.max(1) as i128;
    let imbalance_bps = (demand as i128 - supply as i128) * BPS / denominator;
    let target = BPS + imbalance_bps * cfg.sensitivity_bps as i128 / BPS;
    target.clamp(
        cfg.min_multiplier_bps as i128,
        cfg.max_multiplier_bps as i128,
    ) as u32
}

/// Move `current` toward `target` by at most `max_step` bps.
///
/// This is the surge safeguard: a single observation can never move the price
/// by more than `max_step_bps`, however large the imbalance.
pub fn step_towards(current: u32, target: u32, max_step: u32) -> u32 {
    if target > current {
        current.saturating_add(max_step).min(target)
    } else {
        current.saturating_sub(max_step).max(target)
    }
}

/// Price for a given multiplier. Saturates at `u64::MAX` rather than wrapping.
pub fn price_for(base_price: u64, multiplier_bps: u32) -> u64 {
    let scaled = base_price as u128 * multiplier_bps as u128 / BPS as u128;
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

/// Estimated price elasticity of demand between two observations, in bps.
///
/// Computed as `%change in demand / %change in price`. A negative value means
/// demand falls as price rises. Returns `0` when the estimate is undefined
/// (previous demand or price was zero, or the price did not change).
pub fn elasticity_bps(prev: &MarketState, curr: &MarketState) -> i64 {
    if prev.demand == 0 || prev.price == 0 {
        return 0;
    }
    let pct_demand = (curr.demand as i128 - prev.demand as i128) * BPS / prev.demand as i128;
    let pct_price = (curr.price as i128 - prev.price as i128) * BPS / prev.price as i128;
    if pct_price == 0 {
        return 0;
    }
    (pct_demand * BPS / pct_price).clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> PricingConfig {
        PricingConfig {
            base_price: 1_000,
            min_multiplier_bps: 5_000,
            max_multiplier_bps: 20_000,
            max_step_bps: 1_000,
            sensitivity_bps: 10_000,
            min_update_interval_secs: 0,
        }
    }

    #[test]
    fn balanced_market_is_neutral() {
        assert_eq!(target_multiplier_bps(&cfg(), 10, 10), 10_000);
    }

    #[test]
    fn excess_demand_raises_target_and_is_capped() {
        // demand = 2x supply -> +100% imbalance -> 20_000 bps (cap).
        assert_eq!(target_multiplier_bps(&cfg(), 10, 20), 20_000);
        // demand = 10x supply would be 110_000 bps before clamping.
        assert_eq!(target_multiplier_bps(&cfg(), 1, 10), 20_000);
    }

    #[test]
    fn excess_supply_lowers_target_and_is_floored() {
        // demand = 0 -> -100% imbalance -> 0 bps, clamped up to floor.
        assert_eq!(target_multiplier_bps(&cfg(), 10, 0), 5_000);
    }

    #[test]
    fn zero_supply_saturates_at_cap_without_dividing_by_zero() {
        assert_eq!(target_multiplier_bps(&cfg(), 0, 5), 20_000);
        assert_eq!(target_multiplier_bps(&cfg(), 0, 0), 10_000);
    }

    #[test]
    fn step_limits_single_move() {
        assert_eq!(step_towards(10_000, 20_000, 1_000), 11_000);
        assert_eq!(step_towards(10_000, 5_000, 1_000), 9_000);
        // Target closer than one step: land exactly on it.
        assert_eq!(step_towards(10_000, 10_400, 1_000), 10_400);
    }

    #[test]
    fn price_scales_with_multiplier() {
        assert_eq!(price_for(1_000, 10_000), 1_000);
        assert_eq!(price_for(1_000, 15_000), 1_500);
        assert_eq!(price_for(u64::MAX, 20_000), u64::MAX);
    }

    #[test]
    fn config_validation() {
        assert!(is_valid_config(&cfg()));
        let mut bad = cfg();
        bad.min_multiplier_bps = 12_000; // floor above neutral
        assert!(!is_valid_config(&bad));
        let mut bad = cfg();
        bad.max_step_bps = 0;
        assert!(!is_valid_config(&bad));
        let mut bad = cfg();
        bad.base_price = 0;
        assert!(!is_valid_config(&bad));
    }

    #[test]
    fn elasticity_is_negative_when_demand_falls_as_price_rises() {
        let a = MarketState {
            timestamp: 0,
            supply: 10,
            demand: 100,
            multiplier_bps: 10_000,
            price: 1_000,
        };
        let b = MarketState {
            timestamp: 1,
            supply: 10,
            demand: 80,
            multiplier_bps: 12_000,
            price: 1_200,
        };
        // %demand = -20%, %price = +20% -> elasticity = -1.0 = -10_000 bps.
        assert_eq!(elasticity_bps(&a, &b), -10_000);
    }

    #[test]
    fn elasticity_undefined_cases_return_zero() {
        let a = MarketState {
            timestamp: 0,
            supply: 10,
            demand: 0,
            multiplier_bps: 10_000,
            price: 1_000,
        };
        let b = MarketState {
            timestamp: 1,
            supply: 10,
            demand: 5,
            multiplier_bps: 12_000,
            price: 1_200,
        };
        assert_eq!(elasticity_bps(&a, &b), 0);
        let c = MarketState {
            timestamp: 1,
            supply: 10,
            demand: 50,
            multiplier_bps: 10_000,
            price: 1_000,
        };
        let d = MarketState {
            timestamp: 0,
            supply: 10,
            demand: 10,
            multiplier_bps: 10_000,
            price: 1_000,
        };
        assert_eq!(elasticity_bps(&d, &c), 0);
    }
}
