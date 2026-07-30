# Engineer Training Records

This document describes the training record tracking system in Mainstay's engineer registry. Training records provide verifiable proof of continuing education and certification renewals for credentialed engineers.

## Overview

The training system extends the engineer credentialing framework to include a complete history of training completions. Each training event is recorded on-chain by the engineer's original credential issuer, creating an immutable audit trail that regulators, auditors, and asset owners can verify.

## Motivation

- **Audit Compliance**: Auditors can verify engineers have completed required continuing education
- **Renewal Tracking**: Credential renewals can reference training history
- **Reputation Building**: Engineers accumulate verifiable training records over time
- **Regulatory Alignment**: Supports industries with mandatory training hour requirements

## Data Structure

### TrainingRecord

```rust
pub struct TrainingRecord {
    pub training_type: Symbol,       // Type of training completed
    pub completion_date: u64,        // Unix timestamp of completion
    pub certificate_hash: BytesN<32>, // SHA-256 hash of training certificate
    pub issuer: Address,             // Issuer who recorded the training
}
```

### Storage

Training records are stored per-engineer under the key `(TRAIN, engineer_address) -> Vec<TrainingRecord>`. Records are appended chronologically and never removed, providing a permanent audit trail.

## Training Types

The `training_type` field is a `Symbol` that identifies the category of training. Below are the recommended training types:

| Symbol | Description |
|--------|-------------|
| `SAFETY` | Workplace safety and hazard prevention |
| `TECHNICAL` | Equipment-specific technical training |
| `CERT_RENEW` | Certification renewal examination |
| `CONT_ED` | Continuing education credits |
| `OEM_TRAIN` | Original equipment manufacturer training |
| `COMPLIANCE` | Regulatory compliance training |
| `SPECIALTY` | Specialized skill development |
| `REFRESHER` | Periodic refresher course |

### Custom Training Types

Issuers can use any `Symbol` value for custom training types. It is recommended to use short, uppercase identifiers consistent with the above convention.

## API Operations

### Record Training (Issuer-Only)

```rust
record_training(
    engineer: Address,
    training_type: Symbol,
    completion_date: u64,
    certificate_hash: BytesN<32>,
)
```

**Authorization**: Must be called by the engineer's original credential issuer. The issuer must still be in the trusted issuers list.

**Panics**:
- `EngineerNotFound` — no engineer record exists for the given address
- `UntrustedIssuer` — caller is not the engineer's issuer or issuer has been removed

**Events**: Emits `REC_TRAIN` event with topics `(engineer_address)` and data `(training_type, completion_date, certificate_hash)`.

### Get Training History

```rust
get_training_history(engineer: Address) -> Vec<TrainingRecord>
```

Returns the complete chronological training history for an engineer. Returns an empty vector if no training records exist.

## Integration Examples

### Recording a Training Completion (Shell)

```bash
stellar contract invoke \
  --id $ENGINEER_REGISTRY_ID \
  --source $ISSUER_KEY \
  --network testnet \
  -- \
  record_training \
  --engineer "$ENGINEER_ADDRESS" \
  --training_type "SAFETY" \
  --completion_date "$(date +%s)" \
  --certificate_hash "$CERT_HASH"
```

### Querying Training History (Shell)

```bash
stellar contract invoke \
  --id $ENGINEER_REGISTRY_ID \
  --network testnet \
  -- \
  get_training_history \
  --engineer "$ENGINEER_ADDRESS"
```

## Best Practices

### For Issuers
- **Verify Completion**: Confirm training was actually completed before recording
- **Store Certificates**: Keep the original certificate corresponding to each `certificate_hash` off-chain
- **Timely Recording**: Record training promptly after completion
- **Consistent Typing**: Use standardized training type symbols across your organization

### For Engineers
- **Track Your Records**: Query your training history regularly
- **Verify Accuracy**: Ensure all training completions are properly recorded
- **Plan Renewals**: Use training history to demonstrate continuing education for credential renewals

### For Auditors
- **Verify On-Chain**: All training records are immutable once written
- **Cross-Reference**: Match `certificate_hash` against off-chain certificate documents
- **Track Timelines**: Verify training was completed within required time windows

## Security Considerations

- **Issuer-Only**: Only the original credential issuer can record training, preventing fraudulent self-reporting
- **Trusted Issuer Check**: Training is rejected if the issuer has been removed from the trusted list
- **Immutable**: Training records are append-only and cannot be deleted or modified
- **Auditable**: All records are publicly verifiable on-chain

## TTL Strategy

Training records use persistent storage with automatic TTL extension on every write:
- **Extension Threshold**: 518,400 ledgers (~30 days)
- **Extension Target**: 518,400 ledgers (~30 days)
- **Trigger**: Extended on every `record_training` call

---

*For general credentialing documentation, see [credentialing.md](credentialing.md).*
