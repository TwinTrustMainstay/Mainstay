# ADR-0001: Reputation Score Calculation Model

**Status:** Accepted  
**Date:** 2026-09-26  
**Deciders:** Engineering Team, Risk Management  
**Affected Components:** reputation scoring system, lending module, engineer registry  

## Context

The Mainstay protocol requires engineers to build reputation over time through successful maintenance activities. The reputation score influences lending eligibility and collateral requirements. We needed a transparent, auditable calculation method that:

- Rewards consistent, quality work
- Penalizes failed or incomplete maintenance
- Adapts to engineer experience level
- Remains calculable on-chain within resource constraints

## Decision

We implemented a discrete reputation scoring model where:

1. **Base Score:** Engineers start at 0
2. **Maintenance Completion:** +10 points per successful maintenance record
3. **Score Decay:** 5% annual decay (applied per epoch) to reward recent activity
4. **Minimum Floor:** Score never falls below 0
5. **Maximum Ceiling:** Score capped at 1000 points

The score is recalculated on each maintenance submission and stored per engineer in the engineer registry.

## Rationale

- **Simplicity:** Linear scoring is auditable and verifiable on-chain
- **Fairness:** Rewards consistent work without favoring high-volume operations
- **Decay:** Recent activity matters more; old records lose influence over time
- **Caps:** Prevents score inflation and ensures bounded storage

This model was chosen over exponential scoring (too aggressive) and time-weighted averaging (too complex) because it balances transparency with meaningful differentiation.

## Consequences

### Positive
- Simple on-chain calculation minimizes gas costs
- Fully transparent: engineers can verify their score calculation
- Decay naturally rewards recent activity
- Capped scores prevent state bloat

### Negative
- Fixed +10 points/record may not reflect quality variation
- 5% annual decay may be too aggressive for dormant engineers
- Cap at 1000 points may disadvantage long-serving engineers
- No distinction between routine and complex maintenance

## Alternatives Considered

### Exponential Growth Model
Score += 10 * (1.05 ^ yearsActive). Rejected because growth becomes uncontrollable and cap becomes arbitrary.

### Time-Weighted Average
Score = weighted_sum(points) / weighted_count, with exponential time weights. Rejected because it's complex to compute on-chain and harder to verify.

### Quality-Adjusted Scoring
Points awarded based on maintenance complexity level. Rejected for now because it requires subjective complexity classification and additional state.

## Implementation Notes

- Score calculation: `contracts/engineer-registry/src/score.rs`
- Decay applied: per-epoch in `update_engineer_reputation()`
- Test coverage: `tests/test_reputation_score.rs`
- Score limits enforced: `score.clamped(0, 1000)`

## Related Decisions

- [ADR-0002: Collateral Model](./0002-collateral-model.md) — collateral requirements depend on reputation score
- [ADR-0003: Lending Terms](./0003-lending-terms.md) — lending eligibility thresholds based on score

## References

- Stellar documentation on on-chain computation constraints
- AAVE reputation model: https://aave.com/governance/
- MakerDAO stability mechanisms
