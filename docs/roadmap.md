# Mainstay Roadmap

This document expands on the high-level roadmap summarized in the [README](../README.md#-roadmap).

## v1.0 (Current)
- Asset registry
- Engineer credentialing (federated trusted-issuer model)
- Basic maintenance records and collateral scoring

## v1.1
- Collateral scoring engine refinements (recency-weighted dual-model scoring)
- DeFi lender API for querying collateral scores and maintenance history

## v2.0
- **IoT sensor integration** (automated maintenance triggers). Sensor-triggered
  maintenance records will allow equipment to flag maintenance needs
  automatically between engineer visits, without weakening the trust
  guarantees credentialed engineers currently provide.
  See [docs/iot-integration-design.md](iot-integration-design.md) for the
  full interface specification, trust model, and on-chain data format
  proposed for this feature — feedback from IoT integrators and fleet
  operators is welcome before implementation begins.
- Scalable history indexing for large portfolios
- On-chain weight proposals for scoring configuration

## v3.0
- Frontend dashboard with wallet integration

## v4.0
- Mobile app for field engineers
- Multi-asset portfolio view

## Related Documents
- [docs/iot-integration-design.md](iot-integration-design.md) — IoT sensor integration design (v2.0)
- [docs/credentialing.md](credentialing.md) — engineer credentialing system
- [docs/threat-model.md](threat-model.md) — security threat model
- [docs/deployment-runbook.md](deployment-runbook.md) — deployment and operations guide
