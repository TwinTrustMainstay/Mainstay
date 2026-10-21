# Backup Procedures

> **Last updated:** 2026-07-28 · **Doc version:** 0.1.0

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

The bucket, its versioning configuration, and its encryption are managed by
Terraform in `infra/backup-bucket/`.  Versioning is a **separate bootstrap
resource** that must be applied before the first backup runs — `backup.sh`
checks `get-bucket-versioning` before uploading and aborts if versioning is
not `Enabled`.

```bash
# One-time bootstrap (creates the bucket, enables versioning, sets encryption)
terraform -chdir=infra/backup-bucket init
terraform -chdir=infra/backup-bucket apply
```

The equivalent manual commands are:

```bash
aws s3api create-bucket \
    --bucket mainstay-backups \
    --region us-east-1

# Must complete before the first upload; backup.sh fails otherwise.
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

#### Optional: S3 Object Lock

For tamper-evident backups, enable S3 Object Lock on the bucket (it can only
be enabled at bucket creation time).  With Object Lock in governance or
compliance mode, backup objects cannot be deleted or overwritten for the
retention period, even by a compromised credential.  Set
`object_lock_enabled = true` and a retention period in the Terraform module,
and grant `s3:PutObjectRetention` to the backup role.

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
                "s3:GetBucketVersioning"
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
                "kms:GenerateDataKey",
                "kms:Sign",
                "kms:Verify"
            ],
            "Resource": "arn:aws:kms:us-east-1:<account-id>:key/<key-id>"
        }
    ]
}
```

`kms:Sign`/`kms:Verify` are required for manifest signing (see
[Manifest Signing](#manifest-signing)).  If you use minisign instead, no KMS
signing permissions are needed.

### 3. Environment Variables

Set in `.env` or export before running:

```bash
# Stellar
export STELLAR_NETWORK=mainnet
source scripts/load-network-config.sh

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

# Manifest signing (choose one)
export BACKUP_SIGNING_KEY=arn:aws:kms:us-east-1:<account-id>:key/<signing-key-id>
# or, for minisign:
# export BACKUP_MINISIGN_SECRET_KEY=/etc/mainstay/minisign.key
# export BACKUP_MINISIGN_PUBLIC_KEY=/etc/mainstay/minisign.pub
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
├── manifest.json.sig          # Signature over manifest.json (KMS or minisign)
├── checksums.sha256           # SHA-256 hashes of every other file, incl. manifest.json
├── failures.json              # IDs that could not be read after retries (empty list = complete)
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
└── engineers.json             # Engineer records and credential state
```

### Read Failures and `failures.json`

Every on-chain read (asset records, maintenance history, collateral scores,
engineer records) is retried with exponential backoff.  If a read still fails
after the retries are exhausted, the failing ID is recorded in
`failures.json`:

```json
{
  "assets": [42, 107],
  "maintenance": [{ "asset_id": 42, "kind": "history" }],
  "engineers": ["GABC..."]
}
```

When `failures.json` is non-empty the backup is **incomplete**: the summary
counts only successful reads, and `backup.sh` exits non-zero so the cron job
and monitoring alert.  A partial backup is still uploaded (for forensics), but
it must not be treated as a valid restore source.  Re-run the backup once the
RPC endpoint is healthy.

### Engineer Export

`engineers.json` is exported from the engineer registry's engineer-listing
view, which returns each engineer's address, credential state (active,
suspended, revoked), and trusted-issuer relationships.  An empty
`engineers.json` means no engineers were exported; `restore.sh` reports this
as a partial recovery rather than silently skipping engineer restoration.

## Verifying a Backup

### Integrity Check

```bash
# Verify local backup
./scripts/restore.sh ./backups/20260728T000000Z --verify-only

# Verify S3 backup
./scripts/restore.sh s3://mainstay-backups/backups/20260728T000000Z/ --verify-only
```

`--verify-only` checks the manifest signature, then verifies every file listed
in `checksums.sha256` — including `manifest.json` itself.  Only
`checksums.sha256` is excluded from the checksum list (it cannot hash itself);
its integrity is covered by the signed manifest.

### Manual Checksum Verification

```bash
cd backups/20260728T000000Z
sha256sum -c checksums.sha256
```

### Manifest Signing

`manifest.json` is signed after it is written, and every file (including
`manifest.json`) is hashed afterwards.  Signing uses either AWS KMS
asymmetric keys or minisign:

```bash
# KMS
aws kms sign \
    --key-id "$BACKUP_SIGNING_KEY" \
    --message fileb://manifest.json \
    --message-type RAW \
    --signing-algorithm RSASSA_PSS_SHA_256 \
    --output json > manifest.json.sig

# minisign
minisign -Sm manifest.json -s "$BACKUP_MINISIGN_SECRET_KEY"
```

`restore.sh` verifies `manifest.json.sig` against the manifest before
restoring, and aborts if the signature is missing or invalid.

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
   A non-zero exit may indicate read failures (see `failures.json`).
2. **Read failures**: Alert whenever `failures.json` is non-empty; the backup
   is incomplete and must be re-run.
3. **S3 bucket size**: Track the total size of the backup bucket over time.
4. **Backup age**: Alert if the most recent backup is older than 26 hours.
5. **Versioning**: Periodically verify that bucket versioning is still enabled
   (`aws s3api get-bucket-versioning --bucket mainstay-backups`).
6. **Signature verification**: Alert if `restore.sh --verify-only` fails on the
   latest backup.

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

### Backup exits non-zero with read failures

- Inspect `failures.json` in the backup directory to see which IDs failed.
- Check RPC health and re-run the backup; do not restore from an incomplete
  backup.

### Restore reports partial recovery

- `restore.sh` reports partial recovery when `engineers.json` is empty or when
  `failures.json` lists missing records.  Re-export the missing data from a
  complete backup before declaring the restore finished.

### Upload fails with "bucket versioning is not enabled"

- Apply the Terraform bootstrap (`terraform -chdir=infra/backup-bucket apply`)
  or run `put-bucket-versioning` manually, then re-run the backup.
