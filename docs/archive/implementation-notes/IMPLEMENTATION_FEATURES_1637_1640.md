# Implementation Summary: Issues #1637-#1640

This document describes the implementation of four major scoring enhancements to the Mainstay lifecycle contract.

## Overview

These four features work together to create a more robust, secure, and insightful asset scoring system:

1. **Cross-Contract Score Consensus (Issue #1637)**: Multiple independent scoring nodes improve security through consensus
2. **Degradation Curves per Category (Issue #1638)**: Different asset types degrade differently based on their nature
3. **Score Spike Detection (Issue #1639)**: Anomaly detection flags suspicious score movements for investigation
4. **Peer Comparison Scoring (Issue #1640)**: Relative scoring shows how assets compare to their peers

## Issue #1637: Cross-Contract Score Consensus

### Overview
Instead of relying on a single scoring contract, multiple independent scoring providers submit scores that are combined using median consensus. This improves security by preventing a single compromised or faulty scorer from significantly affecting collateral valuations.

### Key Functions

#### `register_score_provider(env, admin, provider)`
- Registers a new external score provider
- Only callable by contract admin
- Prevents duplicate registrations
- Stores provider in persistent storage with TTL extension

**Parameters:**
- `env: Env` - Soroban environment
- `admin: Address` - Admin caller (must be authenticated)
- `provider: Address` - Address of the provider to register

#### `remove_score_provider(env, admin, provider)`
- Removes a provider from the registry
- Only callable by contract admin
- Filters out the provider and updates storage

#### `submit_external_score(env, asset_id, score, provider)`
- Provider submits a score for an asset
- Only authorized providers can submit
- Each submission includes:
  - The score value (u32)
  - Provider reputation at submission time
  - Timestamp of submission
- Stored in persistent storage per asset

**Parameters:**
- `env: Env` - Soroban environment
- `asset_id: u64` - ID of the asset being scored
- `score: u32` - The score value (0-100)
- `provider: Address` - Provider address (must be registered)

#### `get_consensus_score(env, asset_id) -> u32`
- Computes consensus score using median of all submitted external scores
- Collects weighted scores from all providers
- Sorts scores and calculates median
- Returns the median score (handles both even/odd counts)

**Algorithm:**
```
1. Collect all external scores for the asset
2. Extract score values from ExternalScoreEntry structs
3. Sort scores in ascending order
4. If odd number of scores: return middle score
   If even number of scores: return average of two middle scores
```

#### `update_provider_reputation(env, admin, provider, reputation)`
- Adjusts a provider's reputation score (0-100)
- Higher reputation = more trustworthy
- Used to weight provider scores in future consensus calculations
- Capped at maximum of 100

### Data Structures

**ExternalScoreEntry**
```rust
pub struct ExternalScoreEntry {
    pub provider: Address,      // Provider address
    pub score: u32,            // Score value
    pub timestamp: u64,        // Submission timestamp
    pub provider_reputation: u32, // Provider's reputation at submission
}
```

### Storage Keys
- `ExternalScores(asset_id)`: Vec of ExternalScoreEntry for each asset
- `ScoreProviders`: Vec of authorized provider addresses
- `ProviderReputation`: Map of Address -> u32 (reputation scores)

### Use Cases

1. **Improved Collateral Valuation**: DeFi lending protocols can rely on median consensus instead of single scorer
2. **Dispute Resolution**: If one provider submits an outlier score, median prevents overweighting
3. **Gradual Provider Evaluation**: Reputation system allows tracking provider accuracy over time

---

## Issue #1638: Score Degradation Curves per Asset Category

### Overview
Different asset types degrade at different rates. A software component might degrade quickly, while a building structure degrades slowly. Degradation curves define category-specific parameters.

### Key Functions

#### `register_degradation_curve(env, admin, category, initial_rate, acceleration, floor)`
- Registers or updates a degradation curve for an asset category
- Automatically increments version on update
- Stores both in central map and individual lookup key

**Parameters:**
- `category: Symbol` - Asset category (e.g., "MECHANICAL", "SOFTWARE", "STRUCTURAL")
- `initial_rate: u32` - Base degradation per interval
- `acceleration: u32` - Additional degradation that increases with time
- `floor: u32` - Minimum score that cannot be degraded further

**Example:**
```
Mechanical equipment: initial_rate=5, acceleration=1, floor=15
Software licenses: initial_rate=10, acceleration=2, floor=5
Building structure: initial_rate=2, acceleration=0, floor=20
```

#### `get_category_degradation_curve(env, category) -> Option<DegradationCurve>`
- Retrieves the degradation curve for a specific asset category
- Returns None if no curve defined for that category

#### `get_all_degradation_curves(env) -> Map<Symbol, DegradationCurve>`
- Retrieves all registered degradation curves
- Useful for audit and reporting

#### `apply_category_degradation(env, asset_id, base_score, category) -> u32`
- Helper function to apply degradation to a score
- Respects the category's degradation curve
- Enforces floor constraint

**Algorithm:**
```
if base_score <= curve.floor:
    return curve.floor
else:
    degradation = (curve.initial_rate + curve.acceleration)
                  .min(base_score - curve.floor)
    return base_score - degradation (at least curve.floor)
```

### Data Structures

**DegradationCurve**
```rust
pub struct DegradationCurve {
    pub category: Symbol,       // Asset category identifier
    pub initial_rate: u32,     // Base degradation rate
    pub acceleration: u32,     // Rate increase over time
    pub floor: u32,            // Minimum non-degradable score
    pub version: u32,          // Version tracking
    pub created_at: u64,       // Curve creation timestamp
}
```

### Storage Keys
- `DegradationCurve(category)`: Individual curve for quick lookup
- `DegradationCurves`: Map of all curves for bulk operations

### Use Cases

1. **Asset Type Appropriate Scoring**: Different assets degrade at appropriate rates
2. **Regulatory Compliance**: Curves can be tuned to meet regulatory requirements
3. **Market Realism**: Reflects real-world degradation patterns

---

## Issue #1639: Score Spike Detection for Anomalies

### Overview
Large score changes can indicate data quality issues, fraud, or system problems. Anomaly detection flags suspicious score movements (spikes > 2 standard deviations) for investigation.

### Key Functions

#### `get_score_anomalies(env, asset_id) -> Vec<ScoreAnomaly>`
- Retrieves all detected anomalies for an asset
- Returns empty vector if no anomalies recorded

#### `record_score_anomaly(env, admin, asset_id, baseline_score, observed_score, stdev_multiple)`
- Records a newly detected anomaly
- Only callable by admin
- Marks initial status as "PENDING"

**Parameters:**
- `baseline_score: u32` - Expected score based on moving average
- `observed_score: u32` - Actual score observed
- `stdev_multiple: u32` - How many standard deviations away from baseline

#### `update_anomaly_status(env, admin, asset_id, anomaly_index, status)`
- Updates investigation status of a specific anomaly
- Allows admin to mark as "RESOLVED" or "FALSE_POSITIVE"
- Only callable by admin

#### `check_score_anomaly(env, asset_id, new_score) -> Option<(u32, u32)>`
- Helper function that detects anomalies automatically
- Returns (stdev_multiple, baseline_score) if anomaly detected
- Uses moving average of last 20 scores as baseline

**Algorithm:**
```
1. Calculate mean of baseline scores
2. Calculate standard deviation
3. Calculate distance from mean to new score
4. If distance > 2 * stdev: return anomaly
5. Otherwise: return None
```

#### `record_score_baseline(env, asset_id, score)`
- Records a score in the moving average baseline
- Maintains sliding window of last 20 scores
- Used for anomaly detection

### Data Structures

**ScoreAnomaly**
```rust
pub struct ScoreAnomaly {
    pub asset_id: u64,                      // Asset with anomaly
    pub timestamp: u64,                     // Detection timestamp
    pub baseline_score: u32,                // Expected score
    pub observed_score: u32,                // Actual score
    pub standard_deviation_multiple: u32,   // How many stdev away
    pub investigation_status: Symbol,       // PENDING|RESOLVED|FALSE_POSITIVE
}
```

### Storage Keys
- `ScoreBaseline(asset_id)`: Vec<u64> of last 20 scores (moving average)
- `ScoreAnomalies(asset_id)`: Vec of detected anomalies

### Helper Functions

#### `clear_old_anomalies(env, admin, asset_id, age_seconds)`
- Removes anomalies older than specified age
- Helps manage storage and performance
- Only callable by admin

### Use Cases

1. **Data Quality Monitoring**: Detect suspicious score changes
2. **Fraud Detection**: Flag unusual movements for investigation
3. **System Health**: Identify potential issues with scoring calculations
4. **Audit Trail**: Maintain record of all anomalies and their status

---

## Issue #1640: Peer Comparison Scoring

### Overview
Absolute scores (e.g., "50") are hard to interpret. Peer comparison shows how an asset scores relative to similar assets in its peer group.

### Key Functions

#### `get_peer_group(env, asset_id) -> Option<PeerGroup>`
- Retrieves the peer group an asset belongs to
- Peer groups are defined by asset type and age range
- Returns None if no peer group defined

#### `compute_percentile_rank(env, asset_id) -> u32`
- Calculates percentile rank (0-100) within peer group
- Considers all assets in the same type/age category
- 0 = worst in group, 100 = best in group

**Algorithm:**
```
1. Get asset's collateral score
2. Get all assets of same type and age range
3. Count how many have score <= this asset's score
4. percentile = (count * 100) / total_in_group
5. Return percentile clamped to 0-100
```

#### `get_similar_assets(env, asset_id, top_n) -> Vec<u64>`
- Returns up to top_n similar assets for comparison
- Similar = same type and age range
- Useful for market comparisons

**Parameters:**
- `top_n: u32` - Maximum number of similar assets to return

#### `register_peer_group(env, admin, group_id, asset_type, age_range_min, age_range_max, member_count, mean_score, median_score, stdev)`
- Registers or updates peer group statistics
- Precomputed for efficiency
- Only callable by admin

**Parameters:**
- `group_id: Symbol` - Unique identifier for this peer group
- `asset_type: Symbol` - Type of assets in group
- `age_range_min/max: u64` - Age range in seconds
- `member_count: u32` - Number of assets in group
- `mean_score: u32` - Average score in group
- `median_score: u32` - Median score in group
- `stdev: u32` - Standard deviation in group

### Data Structures

**PeerGroup**
```rust
pub struct PeerGroup {
    pub group_id: Symbol,       // Unique group identifier
    pub asset_type: Symbol,     // Type of assets
    pub age_range_min: u64,     // Min age (seconds)
    pub age_range_max: u64,     // Max age (seconds)
    pub member_count: u32,      // Number of members
    pub mean_score: u32,        // Average score
    pub median_score: u32,      // Median score
    pub stdev: u32,             // Standard deviation
}
```

### Storage Keys
- `PeerGroup(group_id)`: Precomputed peer group statistics

### Use Cases

1. **Relative Valuation**: Compare asset to peers
2. **Market Positioning**: Understand where asset stands
3. **Risk Assessment**: High/low percentile rank indicates risk
4. **Portfolio Balancing**: Identify overweighted/underweighted assets

---

## Integration Scenarios

### Scenario 1: Full Scoring Pipeline
```
1. Multiple providers submit external scores
2. Consensus score calculated via median
3. Score checked against degradation curve for asset category
4. Score compared to moving average for anomaly detection
5. Percentile rank calculated within peer group
6. All values stored and available for queries
```

### Scenario 2: Anomaly Investigation
```
1. System detects spike > 2 stdev from baseline
2. Records ScoreAnomaly with PENDING status
3. Admin reviews anomaly details and similar assets
4. Admin checks consensus scores from multiple providers
5. If fraud suspected: updates status to RESOLVED
6. If data quality issue: triggers remediation
7. Old anomalies cleaned up automatically
```

### Scenario 3: Regulatory Reporting
```
1. Retrieve all degradation curves per category
2. Show how scores degrade over time
3. Report anomaly detection and resolution rate
4. Show percentile ranks for each asset
5. Demonstrate peer comparison methodology
```

---

## Admin Operations

### Maintenance Functions
All require admin authentication:

- `register_score_provider()` - Add provider to consensus
- `remove_score_provider()` - Remove provider from consensus
- `update_provider_reputation()` - Adjust provider trust level
- `register_degradation_curve()` - Define category-specific curves
- `record_score_anomaly()` - Record detected anomaly
- `update_anomaly_status()` - Update anomaly investigation status
- `register_peer_group()` - Register peer group statistics
- `clear_old_external_scores()` - Clean up old scores
- `clear_old_anomalies()` - Clean up old anomalies

### Query Functions
No authentication required:

- `get_score_providers()` - List all providers
- `get_provider_reputation()` - Get provider trust score
- `get_external_scores()` - Get all scores for asset
- `get_consensus_score()` - Get median consensus score
- `get_category_degradation_curve()` - Get curve for category
- `get_all_degradation_curves()` - Get all curves
- `get_score_anomalies()` - Get anomalies for asset
- `get_peer_group()` - Get peer group info
- `compute_percentile_rank()` - Get percentile in peer group
- `get_similar_assets()` - Get comparable assets

---

## Error Handling

All functions properly handle:

1. **Uninitialized Contract**: Check CONFIG is set
2. **Unauthorized Access**: Enforce admin authentication
3. **Data Not Found**: Return Option/empty Vec appropriately
4. **Paused Contract**: Respect global pause flag
5. **Overflow**: Use saturating arithmetic

---

## Gas and Storage Optimization

### Efficient Operations
- Median calculation: O(n log n) sorting
- Percentile rank: O(n) single pass
- Standard deviation: O(n) two passes
- TTL extensions: Extend all persistent keys appropriately

### Storage Pruning
- Baseline scores: Limited to 20-score moving window
- Anomalies: Configurable cleanup by age
- External scores: Configurable cleanup by age

---

## Testing

Comprehensive test coverage includes:

- Provider registration and removal
- Consensus score calculation with various inputs
- Reputation tracking and updates
- Degradation curve application and floor enforcement
- Anomaly detection with statistical validation
- Percentile rank calculations
- Peer group management
- Integration tests across all features

See `tests/test_new_scoring_features.rs` for full test suite.

---

## Future Enhancements

Potential improvements for future versions:

1. **Weighted Consensus**: Use provider reputation to weight scores
2. **Custom Anomaly Thresholds**: Configurable per asset type
3. **Predictive Degradation**: Use ML to forecast degradation curves
4. **Dynamic Peer Groups**: Auto-group assets based on similarity
5. **Real-time Percentile Updates**: Continuous percentile calculation
6. **Provider Slashing**: Penalize providers with poor reputation
7. **Composite Scores**: Combine multiple scoring methodologies

---

## Security Considerations

1. **Authorization**: All admin functions require explicit authentication
2. **Reentrancy**: Use of persistent storage prevents reentrancy attacks
3. **Overflow**: All arithmetic uses saturating operations
4. **Input Validation**: All external inputs validated before use
5. **Storage TTL**: All persistent data has appropriate TTL extensions

---

## Deployment Checklist

Before deploying to production:

- [ ] All tests pass
- [ ] Gas usage optimized
- [ ] Admin accounts configured
- [ ] Initial provider set registered
- [ ] Degradation curves configured for all asset types
- [ ] Peer groups precomputed and registered
- [ ] Monitoring and alerting configured
- [ ] Governance framework defined for admin operations

---

## References

- Issue #1637: Cross-Contract Score Consensus
- Issue #1638: Score Degradation Curves per Asset Category
- Issue #1639: Score Spike Detection for Anomalies
- Issue #1640: Peer Comparison Scoring

All issues include links to GitHub for full requirements and discussion.
