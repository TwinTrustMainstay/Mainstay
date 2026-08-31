# IoT Sensor Integration — Design Document (v2.0 Roadmap Item)

**Status:** Draft — proposal for community and integrator feedback.
**Target release:** v2.0 (see [docs/roadmap.md](roadmap.md)).

## 1. Motivation

Today, maintenance records are submitted manually by credentialed engineers through `submit_maintenance` on the Lifecycle contract. This works well for scheduled maintenance but does not capture continuous, automated signals from equipment itself — e.g., a vibration sensor detecting bearing wear, a pressure sensor flagging a leak, or a runtime-hours counter crossing a service threshold. This document proposes a design for **sensor-triggered maintenance records**: a way for IoT devices attached to physical assets to feed data into Mainstay's on-chain maintenance history and collateral scoring, without compromising the trust model that credentialed engineers currently provide.

This is a **design proposal**, not an implementation spec. Nothing here is final — see [Section 6](#6-feedback--open-questions) for how to weigh in before this becomes an implementation task.

## 2. Goals and Non-Goals

**Goals:**
- Allow sensor data to trigger or supplement maintenance records on-chain.
- Preserve the existing trust guarantees: no unverified party should be able to inflate a collateral score.
- Keep gas/storage costs bounded — sensors can produce high-frequency data; only meaningful events should reach chain.
- Interoperate with the existing credentialing and specialization system rather than replacing it.

**Non-Goals (for this design):**
- Real-time on-chain ingestion of raw sensor telemetry (too expensive; out of scope).
- Replacing engineer credentialing — sensors augment, they do not replace, the human-verified maintenance trail.
- Specifying a single hardware vendor or protocol — this design is transport-agnostic.

## 3. Interface Specification

### 3.1 Off-Chain Components
- **Sensor Gateway**: A device or service co-located with the physical asset (or its fleet operator's infrastructure) that collects raw sensor telemetry (vibration, temperature, pressure, runtime hours, GPS/geofence, etc.) and evaluates it against configurable thresholds.
- **Oracle Relay**: A signing service, operated by a role introduced in this design — the **IoT Attestor** — that watches for threshold-crossing events from registered gateways, packages them into a canonical on-chain payload, and submits a transaction to Lifecycle. The Oracle Relay is the only off-chain component that needs chain-signing capability; gateways themselves never hold contract-signing keys.

### 3.2 On-Chain Entry Point (proposed)
A new Lifecycle function, tentatively:

```rust
pub fn submit_sensor_event(
    env: Env,
    attestor: Address,       // must be a registered IoT Attestor
    asset_id: u64,
    sensor_id: BytesN<32>,   // hash identifying the physical sensor/device
    event_type: Symbol,      // e.g. VIBRATION, PRESSURE, RUNTIME_HRS, GEOFENCE
    reading_hash: BytesN<32>,// hash of the raw reading payload (kept off-chain)
    severity: u32,           // normalized 0-100 severity/confidence score
    timestamp: u64,
)
```

This mirrors `submit_maintenance`'s shape deliberately, so Lifecycle's existing history, decay, and scoring machinery can treat sensor events as a distinct `record_type` within the same append-only history rather than a parallel system.

### 3.3 Registration Flow (proposed)
1. Admin (or, in quorum mode, the admin multisig — see [docs/threat-model.md](threat-model.md#di-03-admin-key-compromise--contract-upgrade-attack)) registers an IoT Attestor address, analogous to how `add_trusted_issuer` registers a credential issuer today.
2. Asset owners opt in per-asset by associating one or more `sensor_id`s with their `asset_id`, so an attestor cannot attribute sensor events to assets the owner hasn't linked.
3. The Oracle Relay submits `submit_sensor_event` whenever a gateway-reported threshold crossing occurs.

## 4. Trust Model

Sensor-triggered records introduce a new trust boundary that must be weaker than credentialed-engineer trust, not equal to it:

| Actor | Trust level | Rationale |
|---|---|---|
| Credentialed Engineer | High — can post maintenance records that directly increase collateral score | Vetted by a trusted issuer; identity and qualification verified |
| IoT Attestor | Medium — can post sensor events that flag *risk* or *required maintenance*, but do **not** directly increase collateral score | Attestors relay hardware signals, not human judgment; hardware can be spoofed, tampered with, or miscalibrated |
| Raw sensor/gateway | Low — never signs transactions directly | Physical devices are the most exposed link in the chain (theft, tampering, replay) |

**Key trust decisions:**
- **Sensor events do not unilaterally raise collateral scores.** A sensor event that indicates *good* condition (e.g., "runtime hours within normal range") should not itself increase score — only credentialed engineer maintenance does that, preserving the existing accountability model documented in [docs/credentialing.md](credentialing.md).
- **Sensor events can lower scores or raise flags.** A sensor event indicating a fault condition (high vibration, leak detected) can apply a scoring penalty or set a `NEEDS_MAINTENANCE` flag on the asset, since a false *negative* (missing a real fault) is a bigger safety/financial risk than a false *positive* being later corrected by an engineer inspection.
- **Attestor compromise is bounded.** Because a compromised attestor can at most flag assets as needing maintenance (denial-of-service-like griefing against an owner's score) rather than fabricate positive maintenance history, the blast radius of a single compromised attestor key is much smaller than a compromised engineer credential or admin key.
- **Attestor removal mirrors issuer removal.** `remove_trusted_issuer`-style admin revocation should apply equally to attestors, and existing sensor events they've posted remain on the append-only history for audit, matching the credential revocation model.

## 5. On-Chain Data Format

Sensor events are proposed to live in the same `HIST` history structure Lifecycle already uses for maintenance records, tagged with a `record_type` discriminant so existing indexing/pagination logic requires minimal change:

```rust
pub struct SensorEvent {
    pub asset_id: u64,
    pub attestor: Address,
    pub sensor_id: BytesN<32>,   // opaque device identifier hash, not PII
    pub event_type: Symbol,      // VIBRATION | PRESSURE | RUNTIME_HRS | GEOFENCE | ...
    pub reading_hash: BytesN<32>,// SHA-256 of the raw off-chain reading payload
    pub severity: u32,           // 0-100
    pub timestamp: u64,
    pub score_delta: i32,        // signed adjustment applied to collateral score, if any
}
```

Design notes:
- **Raw readings stay off-chain.** Only a hash is stored on-chain (`reading_hash`), consistent with how `credential_hash` keeps qualification documents off-chain in the credentialing system. Full readings can be published to IPFS/Arweave or a fleet operator's own storage, with the hash serving as the on-chain integrity anchor.
- **`severity` is normalized**, not raw sensor units, so scoring logic doesn't need per-sensor-type calibration on-chain — normalization happens at the gateway/oracle layer, where it can be updated without a contract upgrade.
- **`score_delta` is bounded** (e.g., capped magnitude per event, similar to how `task_weights` bounds engineer-submitted maintenance impact) to prevent a single sensor event from catastrophically swinging a collateral score.

## 6. Feedback & Open Questions

This design intentionally leaves several questions open for IoT integrators and fleet operators to weigh in on before implementation begins:

1. **Attestor decentralization**: should there be a federated attestor model (like trusted issuers) from day one, or start with a single admin-run attestor and federate later?
2. **Threshold configuration**: should severity thresholds and score-delta caps be global contract config, or per-asset-type/per-owner configurable?
3. **Replay/liveness**: how should the design handle a gateway going offline (no news vs. bad news) — is silence itself ever a signal worth recording on-chain?
4. **Standard protocols**: is there community interest in supporting a specific existing IoT messaging standard (e.g., MQTT-based gateways, LoRaWAN) at the Oracle Relay layer, or should that be left entirely to integrators?

**How to submit feedback:** Open a GitHub issue against this repository tagged `iot-integration` and `design-feedback`, referencing this document. Feedback from integrators building fleet-management or sensor-gateway software is especially valuable before this design is finalized into an implementation plan for v2.0.

## 7. Relationship to Existing Systems

- Builds on [docs/credentialing.md](credentialing.md)'s specialization system: an asset requiring `HV_GEN`-specialized engineer sign-off could still accept `VIBRATION` sensor flags from an attestor without requiring the attestor to hold any specialization itself.
- Extends the append-only history model documented in the STRIDE analysis (T-LC-02 in [docs/threat-model.md](threat-model.md)) — sensor events are immutable once posted, same as engineer-submitted maintenance.
- Subject to the same rate-limiting principles as [docs/deployment-runbook.md](deployment-runbook.md#45-post-deploy-configuration-max_submissions_per_hour)'s `max_submissions_per_hour`; high-frequency sensor gateways will need an analogous per-attestor rate limit to prevent storage exhaustion, to be specified during implementation.
