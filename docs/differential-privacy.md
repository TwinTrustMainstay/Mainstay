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

## Threat model and composition (#1761)

This section covers the privacy-guarantee documentation task from #1761, which
duplicates #1759.

**What an attacker learns from one query.** The released count reveals at most
`epsilon` in log-likelihood ratio about whether one asset of the queried type
exists. This holds for any observer, including one who knows every other asset.

**Repeated queries.** Each call is a separate mechanism. Under basic sequential
composition, `k` queries with epsilons `e1..ek` together give
`sum(ei)`-differential privacy. The per-type budget caps this sum. The default
of 10 units therefore bounds the total leakage for a type across all queries,
however they are issued.

**What the budget does not protect.**
- Non-DP reads, such as `get_assets_by_type`, are exact and leak the count
  completely. The DP endpoint protects only callers who use it. Sensitive
  queries must not be answered through the exact read functions.
- Attributes other than the count are not covered. A query about asset
  attributes needs its own mechanism and budget.
- Budget reset is an admin action (`set_dp_budget`). An admin who resets the
  budget resets the privacy accounting. Treat budget changes as sensitive.
- The guarantee applies to the count as a function of asset membership. It does
  not hide that the registry has a given asset type's category at all.

**Utility.** At epsilon 1 the expected noise is about 0.85 counts. Small counts
are dominated by noise, and that is intended.
