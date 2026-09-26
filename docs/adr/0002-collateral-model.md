# ADR-0002: Collateral Model for Asset-Backed Lending

**Status:** Accepted  
**Date:** 2026-09-26  
**Deciders:** Engineering Team, Risk Management, Lending Operations  
**Affected Components:** lending contracts, asset registry, collateral management  

## Context

Mainstay provides asset-backed loans secured by registered maintenance assets. To manage default risk, we needed a collateral valuation and eligibility model that:

- Ensures sufficient collateral coverage against loan value
- Reflects real-world asset depreciation
- Adapts to market conditions and asset type
- Can be verified on-chain without external oracles (initially)

## Decision

We implemented a **Loan-to-Value (LTV) Ratio Model** with collateral tiers:

1. **Collateral Eligibility:** Only registered, non-decommissioned assets qualify
2. **Base Valuation:** Assets valued at registration price (no dynamic pricing v1)
3. **LTV Ratio:** Loan amount ≤ LTV% × collateral value
4. **LTV Tiers by Asset Type:**
   - High-value assets (generators, compressors): 70% LTV
   - Medium-value assets (pumps, motors): 60% LTV
   - Low-value assets (sensors, controls): 40% LTV
5. **Reputation Discount:** Engineers with reputation ≥ 500 can access 5% higher LTV
6. **Haircut for Decommissioned:** Assets cannot serve as collateral once decommissioned

## Rationale

- **Conservative LTV:** 70% max ensures buffer against asset depreciation and liquidation losses
- **Asset Type Tiers:** Reflects real liquidity and value retention differences
- **Reputation Bonus:** Rewards long-term participants with slightly better terms
- **No Oracle Dependency:** Static valuations reduce attack surface (v2 will add oracles)

This model was chosen because it balances simplicity with practical risk management.

## Consequences

### Positive
- Conservative LTV ratios protect lenders from default cascades
- Asset type categorization reflects real-world value differences
- Reputation bonus incentivizes long-term engineer participation
- No oracle dependency reduces technical risk
- Collateral requirements are transparent and auditable

### Negative
- Static asset valuations don't reflect market changes (limited v1)
- All assets within a tier treated identically (no fine-grained pricing)
- LTV adjustments require contract upgrades (not dynamic)
- May disadvantage new engineers with limited reputation

## Alternatives Considered

### Dynamic Oracle-Based Valuation
Real-time asset prices via price feeds. Rejected for v1 due to oracle risk and added complexity; deferred to v2.

### Overcollateralization Multiple
All loans require 2x collateral regardless of asset type. Rejected because it's too conservative and ignores asset-specific risk profiles.

### Reputation-Only Lending
Unsecured loans based on reputation score alone. Rejected because default risk is unacceptable without collateral backing.

## Implementation Notes

- Collateral model: `contracts/lending/src/collateral.rs`
- LTV calculations: `validate_collateral_coverage()`
- Asset type mapping: `contracts/asset-registry/src/types.rs`
- Test coverage: `tests/test_collateral_model.rs`, `tests/test_is_collateral_eligible_decay_applied.rs`
- Decommissioned asset handling: checked in `is_collateral_eligible()`

## Related Decisions

- [ADR-0001: Reputation Score](./0001-reputation-score-calculation.md) — collateral requirements depend on reputation
- [ADR-0003: Lending Terms](./0003-lending-terms.md) — lending eligibility and terms

## References

- Traditional LTV models in mortgage lending
- DeFi collateral frameworks: Aave, Compound
- Asset depreciation tables: industry standards
