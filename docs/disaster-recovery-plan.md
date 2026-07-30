# Disaster Recovery Plan

This document describes the disaster recovery (DR) procedures for the Mainstay
smart contract system on Stellar Soroban.  It covers recovery from data loss,
contract bugs, Stellar network issues, and off-chain metadata failures.

## Table of Contents

1. [Scope](#scope)
2. [Backup Strategy for Off-Chain Metadata](#backup-strategy-for-off-chain-metadata)
3. [Recovery Procedures for Contract State](#recovery-procedures-for-contract-state)
4. [Rollback to Previous Contract Version](#rollback-to-previous-contract-version)
5. [Stellar Network Outage Recovery](#stellar-network-outage-recovery)
6. [DR Testing Schedule](#dr-testing-schedule)
7. [Incident Response Checklist](#incident-response-checklist)
8. [Emergency Contacts](#emergency-contacts)

---

## Scope

This DR plan covers the following failure scenarios:

| Scenario | Impact | RTO | RPO |
|----------|--------|-----|-----|
| Stellar network outage (1+ hours) | Read/write operations blocked | 4 hours | 1 ledger |
| Contract data corruption / TTL expiry | Asset/engineer records lost | 8 hours | 24 hours |
| Off-chain metadata storage failure | Photos, certificates unavailable | 4 hours | Real-time |
| Contract bug requiring rollback | Incorrect state, blocked functions | 24 hours | Varies |
| Admin key compromise | Unauthorized contract changes | 2 hours | Immediate |
| S3 backup storage failure | Historical backups lost | 24 hours | Daily |

**RTO** = Recovery Time Objective (target time to restore service)
**RPO** = Recovery Point Objective (maximum acceptable data loss)

---

## Backup Strategy for Off-Chain Metadata

### What Is Covered

Off-chain metadata includes:
- Asset photos and certification documents
- Engineer credential documents (PDFs of certifications)
- Maintenance logs with attached media (photos, videos, sensor data)
- External API configuration and secrets

### Storage Architecture

```
┌──────────────────┐     ┌─────────────────────┐     ┌──────────────────┐
│   Application    │────►│   S3 (Versioned +    │────►│  Cross-region    │
│   Layer          │     │   KMS-encrypted)     │     │  replication     │
└──────────────────┘     └─────────────────────┘     └──────────────────┘
```

### Backup Schedule

| Data Type | Frequency | Retention | Storage |
|-----------|-----------|-----------|---------|
| On-chain state export | Daily | 30 days rolling | S3 + local |
| Off-chain metadata | Continuous (replication) | 90 days versioned | S3 cross-region |
| Credential documents | Weekly | 12 months | S3 Glacier IA |
| Configuration backups | On every change | 12 months | Git + S3 |

### Implementation

1. **S3 bucket** with versioning enabled and KMS encryption (SSE-KMS).
2. **Cross-region replication** configured for the backup bucket to a secondary
   AWS region (e.g., `us-east-1` → `us-west-2`).
3. **Immutable backups**: Use S3 Object Lock in compliance mode for weekly
   backups to prevent accidental or malicious deletion.
4. **Lifecycle policies**: Move backups older than 30 days to Glacier, older
   than 365 days to Deep Archive or delete.

```bash
# Enable cross-region replication
aws s3api put-bucket-replication --bucket mainstay-backups \
    --replication-configuration '{
        "Role": "arn:aws:iam::<account-id>:role/s3-crr-role",
        "Rules": [{
            "Status": "Enabled",
            "Priority": 1,
            "DeleteMarkerReplication": {"Status": "Disabled"},
            "Destination": {
                "Bucket": "arn:aws:s3:::mainstay-backups-dr",
                "EncryptionConfiguration": {
                    "ReplicaKmsKeyID": "arn:aws:kms:us-west-2:<account-id>:key/<key-id>"
                }
            }
        }]
    }'
```

### Verification

- Run `scripts/backup.sh` daily and validate the manifest.
- Verify S3 bucket versioning is enabled weekly via the AWS Console or CLI.
- Perform a test restore to a staging environment monthly.

---

## Recovery Procedures for Contract State

### Scenario: Data Loss Due to TTL Expiry

If persistent contract storage entries expire due to prolonged inactivity:

1. **Assess the damage**: Query each contract to list which entries remain.
   ```bash
   stellar contract invoke --id $CONTRACT_ASSET_REGISTRY --network mainnet -- get_asset_count
   ```
2. **Restore from backup**: Use the restore script:
   ```bash
   ./scripts/restore.sh s3://mainstay-backups/backups/<latest>/ --admin-key /path/to/key
   ```
3. **Extend TTL on all restored entries**: After restore, run a script that
   reads every asset to trigger TTL extension:
   ```bash
   for id in $(seq 1 $(get_asset_count)); do
     stellar contract invoke --id $CONTRACT_ASSET_REGISTRY --network mainnet -- get_asset --asset_id $id
   done
   ```
4. **Validate**: Run the backup verification tests and check dashboard metrics.

### Scenario: Mass Asset Deregistration (Security Incident)

1. **Pause all contracts immediately**:
   ```bash
   stellar contract invoke --id $CONTRACT_ASSET_REGISTRY --network mainnet --source admin -- pause --admin <admin>
   stellar contract invoke --id $CONTRACT_ENGINEER_REGISTRY --network mainnet --source admin -- pause --admin <admin>
   stellar contract invoke --id $CONTRACT_LIFECYCLE --network mainnet --source admin -- pause --admin <admin>
   ```
2. **Investigate**: Review contract events and transaction history.
3. **Rotate admin keys**: Initiate the 2-step admin transfer process.
4. **Restore from pre-incident backup** using `scripts/restore.sh`.
5. **Unpause contracts** after verification.

### Scenario: Engineer Credential Loss

1. **Query**: Get the list of engineers from a recent backup.
2. **Re-register**: Have a trusted issuer re-register each engineer:
   ```bash
   stellar contract invoke --id $CONTRACT_ENGINEER_REGISTRY --source admin -- register_engineer \
     --engineer <address> --credential_hash <hash> --issuer <issuer> --validity_period 31536000
   ```
3. **Notify engineers**: Email affected engineers to confirm their credentials.

---

## Rollback to Previous Contract Version

### When to Roll Back

- A contract upgrade introduces a critical bug.
- A configuration change causes unexpected behavior.
- A security vulnerability is discovered in the current WASM.

### Rollback Procedure

1. **Identify the previous WASM**: Recover it from the deployment history
   (stored in S3 under `backups/wasm/<contract>-<timestamp>.wasm`).

2. **Pause the current contract** (if possible):
   ```bash
   stellar contract invoke --id $CONTRACT --network mainnet --source admin -- pause --admin <admin>
   ```

3. **Deploy the previous version** as a new contract:
   ```bash
   stellar contract deploy \
     --wasm backups/wasm/asset-registry-20260701.wasm \
     --network mainnet \
     --source admin
   ```

4. **Initialize the new deployment**, binding to existing registries.

5. **Restore state** from the most recent backup taken before the upgrade:
   ```bash
   ./scripts/restore.sh s3://mainstay-backups/backups/20260701T000000Z/ --admin-key /path/to/key
   ```

6. **Update all integrations** (dApps, monitoring, backup scripts) to point to
   the new contract ID.

7. **Validate** by running the full test suite against the restored state.

### Version History Tracking

Maintain a `contract-versions.md` file in the repository:

| Contract | Version | WASM Hash | Deployed | Status |
|----------|---------|-----------|----------|--------|
| asset-registry | v1.2.0 | `a1b2c3...` | 2026-07-01 | Active |
| asset-registry | v1.1.0 | `d4e5f6...` | 2026-06-01 | Rollback target |
| lifecycle | v1.2.0 | `g7h8i9...` | 2026-07-01 | Active |

---

## Stellar Network Outage Recovery

### During an Outage

1. **Confirm the outage**: Check [Stellar Status](https://status.stellar.org/)
   or Stellar Developer Discord for announcements.
2. **Notify stakeholders**: Post to the operations channel.
3. **Queue operations**: Any attempted writes during the outage will fail.
   Applications should retry with exponential backoff once the network recovers.
4. **Monitor TTL**: If the outage lasts more than 25 days, plan to extend TTL
   on all persistent entries immediately after recovery.

### After Recovery

1. **Verify**: Run the backup exporter and check that all contracts are
   reachable.
2. **Extend TTL**: Run the TTL extension script to ensure no entries expire:
   ```bash
   for ID in $CONTRACT_ASSET_REGISTRY $CONTRACT_ENGINEER_REGISTRY $CONTRACT_LIFECYCLE; do
     stellar contract storage extend --id $ID --network mainnet --durability persistent --ledgers-to-extend 518400
     stellar contract storage extend --id $ID --network mainnet --durability instance --ledgers-to-extend 518400
   done
   ```
3. **Replay queued transactions** from application logs.
4. **Record the incident** in the operations log with start time, end time,
   impact, and resolution.

---

## DR Testing Schedule

| Frequency | Test | Owner | Success Criteria |
|-----------|------|-------|------------------|
| **Monthly** | Restore latest backup to testnet | Operations | All assets recoverable, scores intact |
| **Quarterly** | Full DR drill: deploy contracts from scratch + restore state | Engineering | Complete restoration in < 4 hours |
| **Semi-annually** | Simulated admin key compromise rotation | Security | Keys rotated, no unauthorized access |
| **Annually** | Restore from oldest retained monthly backup | Operations | 12-month-old backup fully valid |
| **Annually** | Simulated Stellar network outage | Engineering | TTL extended, no data loss after 48h |

### DR Test Template

Each DR test should be documented with:

```markdown
## DR Test: [Scenario] — [Date]

**Objective**: [What we're testing]
**Start time**: [HH:MM UTC]
**End time**: [HH:MM UTC]
**Duration**: [X hours]
**Participants**: [Names]

**Steps**:
1. [Step description]
2. ...

**Results**:
- [ ] Backup verified
- [ ] Assets restored correctly
- [ ] Scores match pre-backup values
- [ ] Engineers re-verifiable

**Issues found**: [List issues]

**Action items**:
- [ ] [Action from issues found]
```

---

## Incident Response Checklist

Use this checklist when responding to any Mainstay incident:

### Triage (First 15 minutes)

- [ ] Confirm the incident (check monitoring dashboards, alerts)
- [ ] Determine if contracts need to be paused immediately
- [ ] Notify the incident response channel (Slack `#mainstay-incidents`)
- [ ] Assign an incident commander

### Containment (First hour)

- [ ] Pause contracts if the incident involves unauthorized writes
- [ ] Take a snapshot backup before any remediation
- [ ] Identify the scope: which contracts, which assets, which time window

### Recovery

- [ ] Execute the appropriate recovery procedure from this document
- [ ] Verify restoration with automated tests
- [ ] Confirm monitoring metrics return to normal

### Post-Incident

- [ ] Write a post-mortem document
- [ ] Update this DR plan with lessons learned
- [ ] Schedule any follow-up fixes or improvements

---

## Emergency Contacts

| Role | Contact | Availability |
|------|---------|-------------|
| Contract Admin | [admin@example.com](mailto:admin@example.com) | 24/7 |
| Lead Engineer | [engineering@example.com](mailto:engineering@example.com) | Business hours |
| Security | [security@example.com](mailto:security@example.com) | 24/7 |
| Infrastructure | [infra@example.com](mailto:infra@example.com) | 24/7 |

---

## Related Documents

- [Backup Procedures](backup-procedures.md)
- [Deployment Runbook](deployment-runbook.md)
- [Monitoring Guide](monitoring-guide.md)
- [Architecture Overview](architecture.md)
- [TTL Strategy](ttl-strategy.md)
