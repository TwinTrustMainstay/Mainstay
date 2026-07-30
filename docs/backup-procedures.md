# Backup Procedures

This document describes the backup and restore workflow for Mainstay on-chain
asset metadata, engineer credentials, and maintenance history.

## Overview

Mainstay stores asset metadata on-chain (asset records, serial numbers,
maintenance history).  Off-chain metadata (photos, certificates, sensor logs)
is stored externally.  This backup system ensures that:

1. **On-chain data** can be exported as verifiable JSON snapshots.
2. **Backups** are encrypted at rest (AES-256 or KMS) and stored in a
   versioned S3 bucket.
3. **Restore** can rebuild on-chain state from a validated backup in the
   event of catastrophic data loss or a Stellar network issue.

## Backup Schedule

| Frequency | Trigger                     | Retention |
|-----------|-----------------------------|-----------|
| Daily     | Cron job (00:00 UTC)        | 30 days   |
| Weekly    | Cron job (Sunday 01:00 UTC) | 12 weeks  |
| Monthly   | Cron job (1st, 03:00 UTC)   | 12 months |
| On-demand | Before contract upgrades    | Permanent |

### Cron Configuration

```cron
# Daily backup at midnight UTC
0 0 * * * /path/to/Mainstay/scripts/backup.sh >> /var/log/mainstay-backup.log 2>&1

# Weekly backup Sunday 1am
0 1 * * 0 /path/to/Mainstay/scripts/backup.sh --output-dir /backups/weekly/$(date +\%Y\%W) >> /var/log/mainstay-backup-weekly.log 2>&1

# Monthly backup on 1st at 3am
0 3 1 * * /path/to/Mainstay/scripts/backup.sh --output-dir /backups/monthly/$(date +\%Y\%m) >> /var/log/mainstay-backup-monthly.log 2>&1
```

## Prerequisites

### 1. AWS S3 Bucket

Create an S3 bucket with versioning and encryption:

```bash
aws s3api create-bucket \
    --bucket mainstay-backups \
    --region us-east-1

aws s3api put-bucket-versioning \
    --bucket mainstay-backups \
    --versioning-configuration Status=Enabled

aws s3api put-bucket-encryption \
    --bucket mainstay-backups \
    --server-side-encryption-configuration '{
        "Rules": [{
            "ApplyServerSideEncryptionByDefault": {
                "SSEAlgorithm": "aws:kms",
                "KMSMasterKeyID": "alias/mainstay-backup-key"
            }
        }]
    }'
```

### 2. AWS IAM Policy

Attach this policy to the backup service account:

```json
{
    "Version": "2012-10-17",
    "Statement": [
        {
            "Effect": "Allow",
            "Action": [
                "s3:PutObject",
                "s3:GetObject",
                "s3:ListBucket",
                "s3:PutBucketVersioning"
            ],
            "Resource": [
                "arn:aws:s3:::mainstay-backups",
                "arn:aws:s3:::mainstay-backups/*"
            ]
        },
        {
            "Effect": "Allow",
            "Action": [
                "kms:Encrypt",
                "kms:Decrypt",
                "kms:GenerateDataKey"
            ],
            "Resource": "arn:aws:kms:us-east-1:<account-id>:key/<key-id>"
        }
    ]
}
```

### 3. Environment Variables

Set in `.env` or export before running:

```bash
# Stellar
export STELLAR_NETWORK=mainnet
export STELLAR_RPC_URL=https://mainnet.sorobanrpc.com

# Contracts
export CONTRACT_ASSET_REGISTRY=<asset-registry-contract-id>
export CONTRACT_ENGINEER_REGISTRY=<engineer-registry-contract-id>
export CONTRACT_LIFECYCLE=<lifecycle-contract-id>

# AWS
export AWS_ACCESS_KEY_ID=<your-key>
export AWS_SECRET_ACCESS_KEY=<your-secret>
export AWS_DEFAULT_REGION=us-east-1
export BACKUP_S3_BUCKET=mainstay-backups
export BACKUP_ENCRYPTION_KEY=arn:aws:kms:us-east-1:<account-id>:key/<key-id>
```

## Running a Backup

### Manual Backup (Local Only)

```bash
./scripts/backup.sh
```

Output is saved to `./backups/<timestamp>/`.

### Manual Backup with S3 Upload

```bash
# Ensure AWS env vars are set
./scripts/backup.sh
```

### Dry-Run (Preview Only)

```bash
./scripts/backup.sh --dry-run
```

### Backup to a Custom Directory

```bash
./scripts/backup.sh --output-dir /mnt/backups/mainstay-2026-07-28
```

## Backup Contents

Each backup directory contains:

```
backups/<timestamp>/
├── manifest.json              # Backup metadata (network, contract IDs, timestamp)
├── checksums.sha256           # SHA-256 hashes of all files
├── asset_count.json           # Total asset count at backup time
├── asset_export_summary.json  # Summary of exported assets
├── contracts_ids.md           # Human-readable contract ID reference
├── assets/
│   ├── 1.json                 # Asset ID 1 full record
│   ├── 2.json                 # Asset ID 2 full record
│   └── ...                    # One file per asset
├── maintenance/
│   ├── 1_history.json         # Maintenance history for asset 1
│   ├── 1_score.json           # Collateral score for asset 1
│   └── ...
└── engineers.json             # Engineer credential data
```

## Verifying a Backup

### Integrity Check

```bash
# Verify local backup
./scripts/restore.sh ./backups/20260728T000000Z --verify-only

# Verify S3 backup
./scripts/restore.sh s3://mainstay-backups/backups/20260728T000000Z/ --verify-only
```

### Manual Checksum Verification

```bash
cd backups/20260728T000000Z
sha256sum -c checksums.sha256
```

## Restoring from Backup

> **⚠️ WARNING:** Restoration writes to the live Stellar network.  Always test
> with `--dry-run` first and verify integrity with `--verify-only`.

### Dry-Run Restore

```bash
./scripts/restore.sh ./backups/20260728T000000Z --dry-run
```

### Full Restore

```bash
./scripts/restore.sh ./backups/20260728T000000Z \
    --admin-key /path/to/admin-secret-key
```

### Restore from a Specific Asset ID

```bash
./scripts/restore.sh s3://mainstay-backups/backups/20260728T000000Z/ \
    --admin-key /path/to/admin-secret-key \
    --start-asset-id 100
```

## Restoration Order

When restoring to a fresh deployment, follow this sequence:

1. Deploy contracts and initialize them (see [deployment-runbook.md](deployment-runbook.md)).
2. Re-add asset types to the allowlist.
3. Run `restore.sh` to re-register assets.
4. Re-register engineers and trusted issuers.
5. Re-submit maintenance records (manual step — maintenance records require engineer signatures).

## Backup Verification Tests

The project includes automated backup verification tests in `tests/test_backup_verify.rs`:

```bash
cargo test --test test_backup_verify
```

These tests validate:

- All assets up to `asset_count` are retrievable
- Metadata round-trip integrity after register → retrieve → update
- Monotonic asset count (deregistrations don't decrease the counter)
- Manifest field completeness

## Monitoring

Set up monitoring for the backup process:

1. **Cron job success**: Monitor that the daily backup script exits with code 0.
2. **S3 bucket size**: Track the total size of the backup bucket over time.
3. **Backup age**: Alert if the most recent backup is older than 26 hours.
4. **Versioning**: Periodically verify that bucket versioning is still enabled.

## Disaster Recovery Testing Schedule

| Frequency | Test                          |
|-----------|-------------------------------|
| Monthly   | Restore from latest backup to testnet |
| Quarterly | Full DR drill: deploy new contracts + restore all assets |
| Annually  | Restore from oldest retained monthly backup |

## Troubleshooting

### Backup fails with "contract invoke" errors

- Verify the Stellar RPC endpoint is reachable.
- Check that the contract IDs are correct.
- Ensure you're on the correct network (`STELLAR_NETWORK`).

### S3 upload fails

- Verify AWS credentials are valid and have the required permissions.
- Check that the S3 bucket exists and is accessible.
- Confirm the KMS key is in the same region as the bucket.

### Restore fails on duplicate assets

- Assets with existing serial numbers will fail.  Use `--start-asset-id` to
  skip already-restored assets, or deregister them first.
