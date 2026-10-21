# Differentially Private Analytics (`get_dp_stats`)

Closes #1759 and #1761.

## Purpose

Aggregate analytics can expose individual asset data. For example, a count of
assets of a rare type can reveal whether a specific asset exists. `get_dp_stats`
returns aggregate counts with calibrated random noise so the output does not
reveal whether any single asset is present.

## Mechanism

- Query: the number of assets of a given `asset_type`.
- Sensitivity: 1. Adding or removing one asset changes the count by at most 1.
- Noise: two-sided geometric noise, the discrete analogue of the Laplace
  mechanism. It is sampled as `G1 - G2`, where each `Gi` is geometric with
  parameter `q = e^(-epsilon)`.
- Output: `max(0, true_count + noise)`, clamped to non-negative values.

Soroban contracts cannot use floating point, so `e^(-epsilon)` is computed in
fixed point (scale 1e18) with a 60-term Taylor series. Geometric draws use the
contract PRNG with at most 10,000 Bernoulli trials per draw to bound gas.

## Privacy guarantee

For each call, the released noisy count satisfies **epsilon-differential
privacy** with respect to adding or removing one asset of the queried type,
where `epsilon = epsilon_milli / 1000`.

Limitations:
- The guarantee applies per query. Repeated queries spend budget, and the
  budget below enforces a total limit per asset type.
- The PRNG is the Soroban contract PRNG. Its quality is assumed to be
  adequate for this use. Do not treat it as a cryptographic guarantee against
  an adversary who can observe the chain's PRNG seed.
- The fixed-point sampler is an approximation. Its error is small but not zero.

## Privacy budget

- Each asset type has a budget in milli-epsilon. The default is
  `DEFAULT_DP_BUDGET_MILLI = 10_000` (epsilon 10 in total).
- Each query charges `epsilon_milli` to the budget for its asset type.
- A query is rejected with `DpBudgetExhausted` when the remaining budget is
  below its epsilon, and the budget is not reduced.
- Admins set budgets with `set_dp_budget`. Read them with `get_dp_budget`.
- Epsilon per query must be in `(0, 5000]` milli-units. Otherwise the call
  fails with `DpInvalidEpsilon`.

## Interface

```text
get_dp_stats(query: DpQuery { asset_type: Symbol, epsilon_milli: u32 }) -> NoisyStats
set_dp_budget(admin, asset_type, budget_milli: u64)
get_dp_budget(asset_type) -> u64
```

Use this endpoint for sensitive aggregate queries. Non-sensitive queries
should keep using the exact read functions.

## Utility

The expected absolute noise is `E|noise| = 2q / (1 - q^2)` with `q = e^(-epsilon)`.
At epsilon 1 this is about 0.85 counts, so counts above roughly 100 stay within
about 1% of the true value. Small counts are dominated by noise, which is the
intended privacy trade-off.
