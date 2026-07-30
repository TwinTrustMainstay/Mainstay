# Migration Guide: Paper-to-Chain

This guide helps asset owners migrate from paper or spreadsheet-based
maintenance records to Mainstay's on-chain system.  It covers data cleaning,
batch import, validation, and cost estimation.

## Table of Contents

1. [Overview](#overview)
2. [Data Cleaning Requirements](#data-cleaning-requirements)
3. [Prerequisites](#prerequisites)
4. [Batch Import Script](#batch-import-script)
5. [Validation Checklist](#validation-checklist)
6. [Migration Workflow](#migration-workflow)
7. [Cost Estimates](#cost-estimates)
8. [FAQ](#faq)

---

## Overview

Migrating from paper or spreadsheet records to Mainstay involves:

1. **Data cleaning** — standardising engineer names, dates, and descriptions.
2. **Asset registration** — assigning each physical machine a unique on-chain ID.
3. **Engineer onboarding** — registering engineers and issuing credentials.
4. **Historical import** — batch-importing past maintenance records.
5. **Validation** — verifying imported data matches source records.

### What Can and Cannot Be Imported

| Can be imported | Cannot be imported |
|-----------------|--------------------|
| Asset metadata (type, serial number, owner) | Physical inspection verifications |
| Maintenance task types and dates | Engineer signatures (requires live sign) |
| Maintenance notes/descriptions | Photos and attachments (off-chain only) |
| Engineer credentials (via trusted issuers) | Expired certifications |

---

## Data Cleaning Requirements

Before importing, clean your data to match Mainstay's validation rules.

### Asset Data

| Field | Rule | Example |
|-------|------|---------|
| `asset_type` | Alphanumeric + underscore only, ≤ 64 chars | `GENSET`, `DIESEL_PUMP` |
| `serial_number` | Non-empty, ≤ 64 chars, unique per asset | `CAT-3516-2024-001` |
| `metadata` | Non-empty, ≤ 256 chars | `Caterpillar 3516B diesel generator, 2250 kVA` |

**Cleaning steps:**

1. **Remove special characters** from asset types:
   ```bash
   # Strip non-alphanumeric/underscore characters from asset type column
   sed 's/[^A-Za-z0-9_]//g' records.csv > cleaned.csv
   ```

2. **Deduplicate serial numbers**: Each physical machine can be registered only
   once.  Use a spreadsheet to identify duplicates:
   ```
   =COUNTIF(B:B, B2) > 1
   ```

3. **Truncate metadata** to 256 characters.

### Engineer Data

| Field | Rule | Example |
|-------|------|---------|
| Name | ASCII printable, ≤ 128 chars | `John Doe` |
| Certification ID | Non-empty, unique | `CERT-2024-12345` |
| Validity | Unix timestamp in the future | `1735689600` (2025-01-01) |

**Cleaning steps:**

1. **Standardize names**: Convert to title case, remove extra whitespace.
2. **Validate dates**: No future certification issue dates; validity must be
   in the future.
3. **Remove orphaned engineers**: Every engineer must be associated with at
   least one maintenance record.

### Maintenance Records

| Field | Rule | Example |
|-------|------|---------|
| `asset_id` | Must exist in asset registry | `42` |
| `engineer` | Must be a registered engineer | `GBR3...` |
| `task_type` | Must be in the allowed task types | `OIL_CHG`, `ENGINE` |
| `timestamp` | Unix seconds, not in the future | `1719705600` |
| `notes` | 1–256 chars | `Changed oil and inspected filters` |

**Cleaning steps:**

1. **Remove future dates**:
   ```bash
   awk -F',' '{if ($4 <= '$(date +%s)') print}' records.csv
   ```
2. **Map free-text task types** to Mainstay's predefined types:
   - `"oil change"` → `OIL_CHG`
   - `"engine rebuild"` → `ENGINE`
   - `"annual inspection"` → `INSPECT`
   - `"brake replacement"` → `BRAKE`
3. **Remove orphaned records**: Records referencing assets or engineers not
   in the cleaned asset/engineer lists.
4. **Check notes length**: Truncate to 256 characters.

---

## Prerequisites

### For Asset Owners

- A funded Stellar account with enough XLM for transaction fees.
- The Stellar CLI installed and configured.
- Contract IDs for the deployed Mainstay contracts.
- At least one trusted issuer that will register your engineers.

### For Engineers

- Each engineer needs a Stellar account.
- Engineers must provide their certification documents to a trusted issuer.
- The trusted issuer registers the engineer on-chain and issues a credential.

---

## Batch Import Script

The `scripts/batch-import.sh` script automates importing historical
maintenance records from a CSV file.

### Input CSV Format

```csv
asset_serial_number,asset_type,metadata,engineer_name,engineer_address,task_type,timestamp,notes
CAT-3516-001,GENSET,"Caterpillar 3516B, 2250 kVA",John Doe,GDR37XYZ...,OIL_CHG,1704067200,"Changed oil and filter"
```

### Usage

```bash
# Dry-run: validate CSV without submitting to chain
./scripts/batch-import.sh records.csv --dry-run

# Full import with admin key
./scripts/batch-import.sh records.csv --admin-key /path/to/admin-key

# Import only specific rows (e.g., rows 100-200)
./scripts/batch-import.sh records.csv --start-row 100 --end-row 200

# Skip asset registration (if assets are already registered)
./scripts/batch-import.sh records.csv --skip-asset-registration
```

### What the Script Does

1. Validates CSV format and data constraints.
2. Registers any new assets not already on-chain (deduplicates by serial number).
3. Skips maintenance records that reference unregistered assets or engineers.
4. Submits maintenance records in batches (up to 50 per transaction).
5. Produces a summary report of successes, failures, and skipped records.

---

## Validation Checklist

Use this checklist to verify the migration before marking it complete.

### Pre-Import Validation

- [ ] All asset serial numbers are unique in the source data.
- [ ] All engineer certification IDs are unique.
- [ ] No maintenance record has a future date.
- [ ] All task types map to valid Mainstay symbols.
- [ ] No asset is referenced without being in the asset list.
- [ ] No engineer is referenced without being in the engineer list.
- [ ] All notes are ≤ 256 characters.

### Post-Import Validation

- [ ] `get_asset_count()` matches the number of unique assets imported.
- [ ] `get_assets_by_owner(owner)` returns all expected asset IDs.
- [ ] Each asset's `get_maintenance_history()` returns the expected records.
- [ ] Each asset's `get_collateral_score()` returns a non-zero value.
- [ ] Spot-check 5% of imported records against source data.
- [ ] No duplicate asset errors occurred during registration.
- [ ] All engineers can be verified via `verify_engineer`.

### Reconciliation

For each asset, compare:

| Source Data Field | On-Chain Equivalent | Match? |
|-------------------|---------------------|--------|
| Serial Number | `asset.serial_number` | `[ ]` |
| Asset Type | `asset.asset_type` | `[ ]` |
| Maintenance Date | `record.timestamp` | `[ ]` |
| Task Type | `record.task_type` | `[ ]` |
| Notes | `record.notes` | `[ ]` |

---

## Migration Workflow

### Phase 1: Preparation (1–3 days)

1. Export existing records into CSV format.
2. Run data cleaning scripts.
3. Prepare validation workbook.
4. Set up Stellar test accounts for a dry-run on testnet.

### Phase 2: Testnet Dry-Run (1–2 days)

1. Deploy Mainstay contracts on testnet.
2. Register all asset types via admin.
3. Onboard engineers (issuer registers credentials on testnet).
4. Run `batch-import.sh` with `--dry-run` first, then full import.
5. Validate all records.
6. Document any failures or edge cases.

### Phase 3: Mainnet Migration (1 day)

1. Ensure all prerequisites are met (funded accounts, contracts deployed).
2. Register asset types on mainnet.
3. Onboard all engineers via trusted issuers.
4. Run the batch import script in chunks:
   ```bash
   # Import 500 records at a time
   ./scripts/batch-import.sh records.csv --start-row 1 --end-row 500
   ./scripts/batch-import.sh records.csv --start-row 501 --end-row 1000
   ```
5. Validate each chunk before proceeding to the next.
6. Run the full validation checklist.

### Phase 4: Verification (1 day)

1. Run the full validation checklist.
2. Export on-chain data and compare with original CSV.
3. Have engineers submit a new maintenance record to verify the workflow.
4. Confirm collateral scores are calculated correctly.
5. Sign off on migration completion.

---

## Cost Estimates

### Stellar Network Fees

Transaction fees on Stellar are very low.  The following estimates assume the
Stellar base fee of 100 stroops (0.00001 XLM) per operation:

| Operation | Fee per unit | Typical units | Estimated XLM |
|-----------|-------------|---------------|---------------|
| Register asset | 0.00001 XLM | 1 | 0.00001 XLM |
| Batch register (50 assets) | 0.0005 XLM | 1 batch | 0.0005 XLM |
| Register engineer | 0.00001 XLM | 1 | 0.00001 XLM |
| Submit maintenance | 0.00001 XLM | 1 | 0.00001 XLM |
| Batch submit (50 records) | 0.0005 XLM | 1 batch | 0.0005 XLM |

### Example: Migrating 1,000 Assets with 10,000 Records

| Item | Calculation | XLM Cost |
|------|-------------|----------|
| Asset registrations (in 20 batches of 50) | 20 × 0.0005 | 0.01 XLM |
| Engineer registrations (50 engineers) | 50 × 0.00001 | 0.0005 XLM |
| Maintenance records (in 200 batches of 50) | 200 × 0.0005 | 0.1 XLM |
| **Total** | | **~0.11 XLM** |

At approximately $0.10 USD per XLM, the total network cost is < **$0.02 USD**.

### Operational Costs

| Activity | Estimated Duration | Labor (if outsourced) |
|----------|-------------------|-----------------------|
| Data cleaning | 1–3 days | $500–$1,500 |
| Testnet dry-run | 1–2 days | $300–$1,000 |
| Mainnet migration | 1 day | $300–$800 |
| Verification & sign-off | 1 day | $300–$800 |
| **Total** | **4–7 days** | **$1,400–$4,100** |

### Engineer Credentialing Costs

If using a third-party credential issuer, factor in per-engineer fees
(typically $5–$50 per credential depending on verification depth).

### Ongoing Storage Costs (S3 for Off-Chain Metadata)

| Data Volume | Monthly S3 Cost (Standard IA) | Monthly Glacier Cost |
|-------------|-------------------------------|---------------------|
| 10 GB | ~$0.13 | ~$0.04 |
| 100 GB | ~$1.25 | ~$0.40 |
| 1 TB | ~$12.50 | ~$4.00 |

---

## FAQ

### Q: Can I import maintenance records without engineer signatures?

Yes, for historical records that predate engineer registration, an admin
or authorized issuer can submit records on behalf of engineers.  However,
all live (post-migration) records must be signed by the actual engineer.

### Q: What happens if my CSV has duplicate serial numbers?

The batch import script will skip the duplicate and log a warning.  You must
resolve the duplicate in your source data before re-importing.

### Q: Can I update already-imported records?

No, maintenance records are immutable.  If you find an error after import,
you can submit a corrective maintenance record with notes explaining the
correction.

### Q: How do I handle assets that change ownership during migration?

Complete the migration under the current owner, then use the standard
`transfer_asset` function to transfer ownership on-chain.

### Q: What if an engineer leaves before credentials are issued?

Only engineers with active credentials can submit maintenance records.
If an engineer has departed, ensure their historical records are imported
under a current engineer's name or a designated "system" engineer.

---

## Related Documents

- [Backup Procedures](backup-procedures.md)
- [Disaster Recovery Plan](disaster-recovery-plan.md)
- [Deployment Runbook](deployment-runbook.md)
- [Architecture Overview](architecture.md)
