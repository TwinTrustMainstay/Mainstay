# Architecture Decision Records (ADRs)

This directory contains a comprehensive record of significant architectural and design decisions made in the Mainstay protocol. Each ADR documents the context, decision, rationale, and consequences of a major choice.

## Purpose

ADRs serve as:
- **Decision History:** Understanding why particular choices were made
- **Knowledge Sharing:** Onboarding new team members quickly
- **Reference:** Linking code to design intent
- **Evolution Tracking:** Recording how architectural thinking evolves over time

## ADR Format

All ADRs follow the [template](./0000-template.md). Each record includes:
- **Status:** Proposed, Accepted, Superseded, or Deprecated
- **Date:** When the decision was recorded
- **Deciders:** Who made the decision
- **Context:** Problem statement and constraints
- **Decision:** The chosen solution
- **Rationale:** Why this choice over alternatives
- **Consequences:** Positive and negative trade-offs
- **Alternatives:** What was considered and rejected

## ADR Index

| ID | Title | Status | Topic |
|---|---|---|---|
| [0001](./0001-reputation-score-calculation.md) | Reputation Score Calculation Model | Accepted | Scoring & Reputation |
| [0002](./0002-collateral-model.md) | Collateral Model for Asset-Backed Lending | Accepted | Lending & Collateral |
| [0003](./0003-lending-terms.md) | Lending Terms and Interest Rate Model | Accepted | Lending & Interest |

## Key Decision Areas

### Reputation & Scoring
- **ADR-0001:** How engineer reputation is calculated and decayed

### Lending & Collateral
- **ADR-0002:** What assets qualify as collateral and their valuation
- **ADR-0003:** Borrowing terms, interest rates, and default handling

## Decision Making Process

When proposing a new significant architectural decision:

1. **Identify the Decision:** What problem needs solving?
2. **Gather Context:** What constraints apply?
3. **Propose Solutions:** What are the reasonable alternatives?
4. **Evaluate Trade-offs:** What are the pros and cons?
5. **Make Decision:** Choose and document in an ADR
6. **Implement:** Execute the decision in code
7. **Link Code:** Reference the ADR from relevant source files

## Linking ADRs to Code

When implementing a decision documented in an ADR, add a comment in the relevant code:

```rust
// See docs/adr/0001-reputation-score-calculation.md for the design rationale
fn calculate_reputation_score(engineer: &Engineer) -> u32 {
    // implementation
}
```

## Related Documentation

- [Architecture Overview](../architecture.md) — System design and component breakdown
- [Deployment Guide](../deployment-runbook.md) — How to deploy and upgrade contracts
- [Test Documentation](../testing.md) — Testing strategies and coverage

## How to Contribute

1. Create a new ADR file: `docs/adr/NNNN-kebab-case-title.md`
2. Use the [template](./0000-template.md) structure
3. Follow the status workflow: Proposed → Accepted/Rejected
4. Get stakeholder agreement before marking as Accepted
5. Update this README index
6. Link from related code and documentation

## Questions?

For questions about specific decisions, refer to the linked code examples and test files in each ADR.
