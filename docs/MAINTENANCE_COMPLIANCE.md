# Maintenance Compliance Standard Registry

This document describes the on-chain maintenance compliance standard registry system, which enables fleet operators and asset financiers to define and enforce maintenance standards for different asset types.

## Overview

The maintenance compliance standard registry is a critical feature for asset financing platforms that need to ensure assets are maintained according to specific standards. The system allows:

1. **Operators** to register maintenance standards for asset types
2. **DeFi platforms** to validate that maintenance history complies with registered standards
3. **Auditors** to verify compliance programmatically
4. **Integration** with lending protocols for collateral validation

## Core Concepts

### Asset Type
A symbol identifying the category of asset (e.g., `ENGINE`, `PUMP`, `COMPRESSOR`, `GENERATOR`). Each asset type can have a unique maintenance standard.

### Maintenance Standard
A structured definition of maintenance requirements for an asset type, encoded as a hash. The standard typically includes:
- Required maintenance intervals (e.g., every 500 operating hours)
- Specific maintenance tasks (e.g., oil change, filter replacement, calibration)
- Documentation requirements
- Cost thresholds
- Engineer qualifications

### Compliance Proof
A hash of maintenance documentation that proves an asset's maintenance history complies with its registered standard. This enables off-chain computation while maintaining verifiable on-chain evidence.

## Smart Contract Functions

### 0. Generate an Audit Report

`generate_compliance_report(asset_id, compliance_proof_hash)` returns an
auditable summary containing the registered-standard status, total and
compliant record counts, compliance percentage, total recorded cost, and
maintenance hash-chain integrity. The report is deterministic for the
provided proof and can be archived by compliance systems as an audit result.

### 1. Register Maintenance Standard

Register a maintenance standard for a specific asset type.

```rust
pub fn register_standard(
    env: Env,
    admin: Address,           // Must be contract admin
    asset_type: Symbol,       // Asset type identifier
    standard_definition_hash: Bytes,  // Hash of standard specification
)
```

**Requirements:**
- Caller must be the contract admin
- Each asset type can only have one standard registered
- Attempting to re-register a standard for the same type raises `StandardAlreadyRegistered`

**Events:** Emits `(REG_STD, asset_type)` event with the standard hash

**Example:**
```rust
let admin = Address::generate(&env);
let asset_type = symbol_short!("ENGINE");

// Hash of the standard definition (e.g., JSON spec serialized and hashed)
let standard_hash = Bytes::from_slice(&env, &[/* 32-byte hash */]);

lifecycle.register_standard(&admin, &asset_type, &standard_hash);
```

### 2. Validate Maintenance Compliance

Check if a maintenance record complies with the registered standard for an asset.

```rust
pub fn validate_maintenance_compliance(
    env: Env,
    asset_id: u64,              // Asset to validate
    task_type: Symbol,          // Type of maintenance task
    compliance_proof_hash: Bytes, // Hash of compliance documentation
) -> bool
```

**Returns:**
- `true` if the proof matches the registered standard for the asset's type
- `false` if no standard is registered or proof doesn't match

**Logic:**
1. Retrieves asset from registry to determine its type
2. Looks up the registered standard for that asset type
3. Compares the provided compliance proof hash with the registered standard
4. Returns true only if they match exactly

**Example:**
```rust
let asset_id = 42u64;
let task_type = symbol_short!("OIL_CHG");
let proof = Bytes::from_slice(&env, &[/* compliance proof hash */]);

let is_compliant = lifecycle.validate_maintenance_compliance(
    &asset_id,
    &task_type,
    &proof
);

if is_compliant {
    println!("Maintenance is compliant!");
}
```

### 3. Retrieve Maintenance Standard

Get the registered maintenance standard for a specific asset type.

```rust
pub fn get_maintenance_standard(
    env: Env,
    asset_type: Symbol,  // Asset type to query
) -> Bytes              // Standard definition hash (empty if not registered)
```

**Returns:**
- The standard hash if registered for this asset type
- Empty `Bytes` if no standard is registered

**Example:**
```rust
let asset_type = symbol_short!("PUMP");
let standard = lifecycle.get_maintenance_standard(&asset_type);

if standard.len() > 0 {
    println!("Standard found: {:?}", standard);
} else {
    println!("No standard registered for this asset type");
}
```

## Implementation Patterns

### Pattern 1: Offline Standard Definition with On-Chain Verification

This pattern separates the complex standard definition from the on-chain system:

```
Off-Chain (Trusted Authority)
├── Define maintenance standard in JSON/YAML
│   ├── Required intervals
│   ├── Specific tasks
│   ├── Cost limits
│   └── Engineer certifications
├── Serialize and hash the definition
└── Distribute hash to fleet operators

On-Chain Smart Contract
├── Register hash via register_standard()
├── Operators submit maintenance with compliance proof hash
└── Contract validates proof matches registered hash
```

**Example Standard Definition:**
```json
{
  "asset_type": "ENGINE",
  "version": "1.0",
  "effective_date": "2024-01-01",
  "maintenance_intervals": [
    {
      "task": "oil_change",
      "interval_hours": 500,
      "duration_minutes": 30,
      "required_parts": ["oil_filter", "synthetic_oil"],
      "estimated_cost_stroops": 50000000
    },
    {
      "task": "filter_replacement",
      "interval_hours": 1000,
      "duration_minutes": 15,
      "estimated_cost_stroops": 10000000
    }
  ],
  "required_engineer_certifications": ["ENGINE_MECHANIC", "INDUSTRIAL_CERT"],
  "documentation_requirements": [
    "work_order",
    "parts_receipt",
    "engineer_signature",
    "before_after_photos"
  ]
}
```

Hash this JSON and register on-chain:
```python
import hashlib
import json

standard_json = json.dumps(standard_dict)
standard_hash = hashlib.sha256(standard_json.encode()).digest()

lifecycle.register_standard(admin, symbol_short!("ENGINE"), standard_hash)
```

### Pattern 2: DeFi Lending Protocol Integration

Integrate compliance validation into lending workflows:

```rust
// In lending contract
pub fn initiate_loan(
    env: Env,
    borrower: Address,
    asset_id: u64,
    amount: i128,
    lifecycle_contract: Address,
) -> u64 {
    let lifecycle = LifecycleClient::new(&env, &lifecycle_contract);
    
    // Verify asset is healthy
    let score = lifecycle.get_collateral_score(&asset_id);
    assert!(score >= MIN_COLLATERAL_SCORE);
    
    // For engines, verify compliance with registered standard
    let asset_type = get_asset_type(&env, &asset_id);
    
    if asset_type == symbol_short!("ENGINE") {
        let standard = lifecycle.get_maintenance_standard(&asset_type);
        
        // Require proof in loan request
        if !standard.is_empty() {
            // Loan origination UI should include compliance proof upload
            // We verify it on-chain before disbursement
        }
    }
    
    // ... continue loan issuance
}
```

### Pattern 3: Audit Trail and Compliance Reporting

Query and report on compliance:

```python
def build_compliance_report(asset_id, lifecycle_client):
    """Generate compliance report for an asset."""
    
    # Get asset and its type
    asset = asset_registry.get_asset(asset_id)
    
    # Get registered standard
    standard = lifecycle_client.get_maintenance_standard(asset.asset_type)
    
    if not standard:
        return {
            "asset_id": asset_id,
            "compliant": None,
            "status": "No standard registered"
        }
    
    # Get maintenance history
    history = lifecycle_client.get_maintenance_history(asset_id)
    
    # For each maintenance record, check compliance
    compliant_records = 0
    total_records = 0
    
    for record in history:
        total_records += 1
        
        # Reconstruct compliance proof from record
        compliance_proof = hash_compliance_proof(record)
        
        # Verify against standard
        is_compliant = lifecycle_client.validate_maintenance_compliance(
            asset_id,
            record.task_type,
            compliance_proof
        )
        
        if is_compliant:
            compliant_records += 1
    
    return {
        "asset_id": asset_id,
        "asset_type": asset.asset_type,
        "compliant_records": compliant_records,
        "total_records": total_records,
        "compliance_rate": compliant_records / total_records if total_records > 0 else 0,
        "standard_hash": standard.hex(),
        "last_verified": datetime.now().isoformat()
    }
```

## Best Practices

### 1. Standard Definition Management

**Do:**
- Version your standards (include version number in JSON)
- Document effective dates for transitions
- Use clear, unambiguous task descriptions
- Include cost estimates for budgeting
- Specify required engineer certifications

**Don't:**
- Change standards without versioning
- Register multiple standards for the same asset type
- Use vague interval descriptions (e.g., "regular" instead of "500 hours")

### 2. Compliance Proof Generation

**Secure Approach:**
```python
def generate_compliance_proof(maintenance_record, standard_spec):
    """Generate verifiable compliance proof."""
    
    # Include all relevant fields
    proof_data = {
        "asset_id": maintenance_record.asset_id,
        "task_type": maintenance_record.task_type,
        "timestamp": maintenance_record.timestamp,
        "engineer": maintenance_record.engineer.id,
        "parts_used": maintenance_record.parts,
        "duration_minutes": maintenance_record.duration,
        "cost": maintenance_record.cost,
        "photos": [photo_hash for photo_hash in maintenance_record.photos],
        "work_order": maintenance_record.work_order_id,
        # Include standard version to ensure consistency
        "standard_version": standard_spec["version"]
    }
    
    # Serialize deterministically
    proof_json = json.dumps(proof_data, sort_keys=True)
    
    # Hash for on-chain verification
    proof_hash = hashlib.sha256(proof_json.encode()).digest()
    
    return proof_hash, proof_json  # Return both for audit trail
```

### 3. Migration Strategy

When upgrading standards:

```python
def register_new_standard(asset_type, new_standard_hash):
    """
    Register a new standard version.
    
    Note: Current implementation only supports one standard per type.
    For version migration, coordinate with your governance process.
    """
    
    # In the future, contract could support:
    # - register_standard_version()
    # - deprecate_standard()
    # - query_standard_history()
    
    pass
```

## Integration Examples

### TypeScript/JavaScript Integration

```typescript
import { LifecycleClient } from './lifecycle-client';

async function verifyCompliance(
  assetId: number,
  complianceProof: Buffer
): Promise<boolean> {
  const client = new LifecycleClient(contractId, provider);
  
  return await client.validateMaintenanceCompliance(
    assetId,
    Symbol.shorthand("OIL_CHG"),  // task type
    complianceProof
  );
}

async function registerStandard(
  assetType: string,
  standardHash: Buffer
) {
  const admin = getCurrentAdmin();
  const client = new LifecycleClient(contractId, provider);
  
  try {
    await client.registerStandard(
      admin,
      Symbol.shorthand(assetType),
      standardHash
    );
    console.log(`Registered standard for ${assetType}`);
  } catch (error) {
    if (error.code === 'StandardAlreadyRegistered') {
      console.warn(`Standard already registered for ${assetType}`);
    } else {
      throw error;
    }
  }
}
```

### Python Integration

```python
from stellar_sdk import Server, Network, TransactionBuilder
from mainstay_client import LifecycleClient

def setup_compliance_system(asset_types_and_specs):
    """Initialize compliance standards for all asset types."""
    
    client = LifecycleClient(CONTRACT_ID, provider)
    
    for asset_type, spec in asset_types_and_specs.items():
        # Prepare standard hash
        spec_json = json.dumps(spec, sort_keys=True)
        standard_hash = hashlib.sha256(spec_json.encode()).digest()
        
        # Register on-chain
        try:
            client.register_standard(
                admin_address,
                asset_type,
                standard_hash
            )
            print(f"✓ Registered {asset_type}")
        except AlreadyRegisteredError:
            print(f"✗ {asset_type} already registered")
            
            # Verify the registered hash matches our spec
            on_chain_hash = client.get_maintenance_standard(asset_type)
            if on_chain_hash == standard_hash:
                print(f"  (Standard matches our spec)")
            else:
                print(f"  (WARNING: Standard hash mismatch!)")
```

## Testing

### Unit Tests

The Mainstay test suite includes comprehensive tests:

```bash
cargo test test_register_and_validate_compliance_standard
cargo test test_validate_compliance_with_unregistered_standard
cargo test test_get_maintenance_standard
```

### Running Compliance Tests

```bash
./scripts/test.sh --filter maintenance_compliance
```

## Troubleshooting

### Standard Already Registered

```
Error: StandardAlreadyRegistered
```

**Cause:** Attempted to register a standard for an asset type that already has one.

**Solution:** Either:
1. Query the existing standard first: `get_maintenance_standard(asset_type)`
2. Use admin governance process to decommission old standard and register new one
3. Create a new asset type if different maintenance rules apply

### Compliance Validation Failing

**Issue:** `validate_maintenance_compliance()` returns false even though proof should match

**Debugging:**
```python
# 1. Verify asset exists and get its type
asset = asset_registry.get_asset(asset_id)
print(f"Asset type: {asset.asset_type}")

# 2. Check if standard is registered
standard = lifecycle.get_maintenance_standard(asset.asset_type)
print(f"Standard registered: {standard.hex() if standard else 'NO'}")

# 3. Verify proof hash matches
expected_hash = hashlib.sha256(compliance_proof_json).digest()
print(f"Computed hash: {expected_hash.hex()}")
print(f"Stored hash:   {standard.hex()}")

# 4. Check for encoding issues
# Ensure JSON serialization is deterministic (sorted keys)
```

## Related Documentation

- [Maintenance Records](./MAINTENANCE.md) - Recording maintenance
- [Backup and Recovery](./BACKUP_RECOVERY.md) - Data preservation
- [DeFi Integration](./DEFI_INTEGRATION.md) - Lending platform integration
- [Monitoring and Alerting](./monitoring/PROMETHEUS_METRICS.md) - Compliance monitoring
