# ADR-0003: Lending Terms and Interest Rate Model

**Status:** Accepted  
**Date:** 2026-09-26  
**Deciders:** Engineering Team, Risk Management, Product  
**Affected Components:** lending contract, interest calculation, repayment management  

## Context

The Mainstay lending system needs to balance borrower affordability with lender returns. We required:

- Clear eligibility criteria based on engineer reputation
- Competitive interest rates that adjust to credit quality
- Predictable repayment schedules
- On-chain interest calculation without external price feeds

## Decision

We implemented a **Reputation-Tiered Interest Rate Model** with two lending tiers:

### Eligibility Criteria
- **Tier 1 (Premium):** Reputation score ≥ 500, can borrow up to 100,000 XLM
- **Tier 2 (Standard):** Reputation score ≥ 100, can borrow up to 50,000 XLM
- **Below 100:** Ineligible (insufficient reputation)

### Interest Rates
- **Tier 1 (Premium):** 3% APR base rate
- **Tier 2 (Standard):** 8% APR base rate
- **Late Payment Penalty:** +2% APR on overdue balances
- **Compound Frequency:** Daily (365 days/year)

### Repayment Terms
- **Loan Duration:** 12 months standard (adjustable by admin)
- **Minimum Repayment:** 5% of principal per quarter
- **Grace Period:** 7 days after due date before penalty applies
- **Full Prepayment:** Allowed without penalty

### Default and Liquidation
- **Default:** Failure to pay 30 days past due date triggers liquidation
- **Collateral Seizure:** Automatic after default period; collateral transferred to lender address
- **Deficiency:** Any shortfall after collateral liquidation triggers bad debt accounting

## Rationale

- **Reputation Tiers:** Incentivizes engineers to build reputation while reflecting credit risk
- **3% Base (Tier 1):** Competitive rate reflects low default risk from proven engineers
- **8% Base (Tier 2):** Higher rate compensates lender for higher default risk
- **Late Penalties:** Incentivizes on-time payment
- **Daily Compounding:** Standard in DeFi; transparent and auditable
- **12-Month Duration:** Balances cash flow for engineers with reasonable lender commitment
- **Quarterly Minimums:** Forces gradual repayment while allowing flexibility

This model was chosen to strike a balance between accessibility (lower rates for proven engineers) and risk management (penalties for late payment, default mechanisms).

## Consequences

### Positive
- Reputation-based tiers reward long-term participation
- Tier 1 rates (3% APR) are competitive and attract quality borrowers
- Clear default and liquidation rules prevent ambiguity
- Daily compounding is standard and auditable
- Grace period balances borrower hardship with lender protection

### Negative
- Tier 2 rate (8% APR) may be discouraging for new engineers
- 12-month minimum duration is inflexible (early repayment bonus would help)
- No rate adjustment for collateral quality variation (all tiers use same LTV)
- Late penalties don't cap, potentially creating debt spirals for struggling borrowers
- Automatic collateral liquidation may be too harsh for temporary hardship

## Alternatives Considered

### Fixed 5% Rate for All
Single rate regardless of reputation. Rejected because it doesn't differentiate risk and doesn't incentivize reputation building.

### Variable Rate Based on Utilization
Rates increase as more capital is borrowed (like Aave). Rejected for v1 due to complexity and requires tracking total outstanding loans.

### Performance-Based Bonuses
Reputation bonus that reduces interest (already implemented as LTV bonus in ADR-0002). Rejected extending to interest rates due to state complexity.

### Graduated Default Period
Default period shortens with reputation (e.g., 60 days for Tier 2, 30 for Tier 1). Rejected for simplicity; deferred to v2.

## Implementation Notes

- Lending terms: `contracts/lending/src/terms.rs`
- Interest calculation: `calculate_interest()` with daily compounding
- Tier eligibility: `check_borrower_eligibility(reputation_score)`
- Default/liquidation: `check_default_status()`, `liquidate_collateral()`
- Test coverage:
  - `tests/test_lending_terms.rs` — term calculations
  - `tests/test_full_loan_lien_lifecycle_e2e.rs` — end-to-end repayment flow
  - `tests/test_lending_error_variants.rs` — error handling

## Related Decisions

- [ADR-0001: Reputation Score](./0001-reputation-score-calculation.md) — borrower eligibility based on score
- [ADR-0002: Collateral Model](./0002-collateral-model.md) — collateral requirements for loans

## References

- SOFR (Secured Overnight Financing Rate) interest models
- Aave interest rate strategies: https://aave.com/governance/
- MakerDAO stability fee mechanism
- Traditional lending APR conventions
