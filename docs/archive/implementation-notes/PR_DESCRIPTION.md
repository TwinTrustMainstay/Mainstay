# Add operational analytics for lenders, engineers, finance, and fleet management

## Summary

Mainstay now exposes deterministic, typed lifecycle analytics views for the four identified visibility gaps:

- **Collateral portfolio health:** Aggregates current owner assets, collateral scores, eligibility, and locked collateral for lender risk review.
- **Engineer productivity:** Reports an engineer's maintained asset count, maintenance volume, recorded cost, average cost, and latest activity, with timestamp filtering.
- **Cost analysis and forecasting:** Summarizes historical maintenance costs and estimates future-period cost from the observed retained cost rate.
- **Fleet performance:** Aggregates fleet service coverage, maintenance volume, maintenance cost, collateral score, locked assets, and decommissioned assets.

All calculations use existing on-chain registry, lifecycle, maintenance, and ownership data. No new mutable state or authorization path was introduced.

## Commits

1. `Add collateral portfolio health analytics`
2. `Add engineer productivity analytics`
3. `Add maintenance cost forecasting analytics`
4. `Add fleet performance analytics`

## Validation

- `git diff --check` passes.
- Rust tests and formatting could not be run in this environment because `cargo` is not installed.
