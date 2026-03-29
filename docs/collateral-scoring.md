# Collateral Scoring Model

This document describes the collateral scoring model used in the Mainstay lifecycle contract to assess the value and eligibility of assets based on their maintenance history.

## Overview

The collateral scoring system is designed to provide a quantitative measure of an asset's maintenance quality and recency. Higher scores indicate well-maintained assets that are more suitable for use as collateral in financial transactions.

## Score Mechanics

### Score Range
- **Minimum Score**: 0 points
- **Maximum Score**: 100 points
- **Eligibility Threshold**: 50 points (default, configurable)

### Score Calculation

Scores are calculated based on:
1. **Maintenance Task Weight**: Different task types add different point values
2. **Time-Based Decay**: Scores decrease over time without maintenance
3. **Score Cap**: Total score never exceeds 100 points

## Task Type Weights

Maintenance tasks are categorized into three tiers with different point values:

### Minor Tasks (2 points)
- **OIL_CHG** - Oil changes
- **LUBE** - Lubrication services  
- **INSPECT** - General inspections

### Medium Tasks (5 points)
- **FILTER** - Filter replacements
- **TUNE_UP** - Engine tuning
- **BRAKE** - Brake system maintenance

### Major Tasks (10 points)
- **ENGINE** - Engine work/rebuilds
- **OVERHAUL** - Complete overhauls
- **REBUILD** - Major rebuilds

### Unknown Task Types
- **Default Weight**: 3 points
- Applied to any task type not explicitly categorized

## Time-Based Decay

### Default Decay Configuration
- **Decay Rate**: 5 points per interval
- **Decay Interval**: 2,592,000 seconds (30 days)
- **Effective Decay**: 5 points per 30 days without maintenance

### Decay Calculation
```
decay_intervals = time_elapsed / decay_interval
total_decay = decay_intervals * decay_rate
new_score = max(0, current_score - total_decay)
```

### Example
- Asset score: 60 points
- Time since last maintenance: 60 days
- Decay intervals: 60 / 30 = 2
- Total decay: 2 * 5 = 10 points
- New score: 60 - 10 = 50 points

## Score History Tracking

The system maintains a complete history of score changes:
- **Entry Format**: `(timestamp, score)` tuple
- **Trigger**: Recorded after each maintenance event
- **Purpose**: Enables trend analysis and audit trails

## Collateral Eligibility

### Default Threshold
- **Required Score**: 50 points
- **Purpose**: Minimum maintenance quality for collateral consideration

### Eligibility Check
```
is_eligible = current_score >= eligibility_threshold
```

### Use Cases
- **Loan Collateral**: Assets meeting threshold can secure financing
- **Insurance Premiums**: Higher scores may reduce insurance costs
- **Asset Valuation**: Score correlates with market value retention

## Configuration Parameters

All scoring parameters are configurable by contract administrators:

### Score Increment
- **Purpose**: Base points added per maintenance task
- **Default**: Not used (task weights take precedence)
- **Validation**: Must be > 0

### Decay Configuration
- **Decay Rate**: Points deducted per interval
- **Decay Interval**: Time between decay calculations (seconds)
- **Validation**: Interval must be > 0

### Eligibility Threshold
- **Purpose**: Minimum score for collateral eligibility
- **Default**: 50 points
- **Range**: 0-100 points

## Maintenance History Limits

### History Cap
- **Default Limit**: 200 records per asset
- **Purpose**: Prevents unlimited storage growth
- **Configurable**: Can be adjusted by administrators

### Pagination
- **Supported**: Yes
- **Parameters**: `offset` (start index), `limit` (max records)
- **Use Case**: UI display of large histories

## Score Examples

### Example 1: New Generator
```
Initial Score: 0
+ Oil Change (2 points): Score = 2
+ Filter Replacement (5 points): Score = 7  
+ Engine Overhaul (10 points): Score = 17
```

### Example 2: Aged Asset with Decay
```
Initial Score: 80
Time since last maintenance: 90 days
Decay intervals: 90 / 30 = 3
Total decay: 3 * 5 = 15 points
Final Score: 80 - 15 = 65 points
```

### Example 3: Score Cap
```
Current Score: 95
+ Major Rebuild (10 points): Score = 100 (capped at maximum)
```

## Integration with Contracts

### Asset Registry
- **Verification**: Asset existence validated before scoring
- **Events**: Score changes emit maintenance events

### Engineer Registry  
- **Verification**: Only verified engineers can submit maintenance
- **Authorization**: Ensures quality of maintenance records

## Best Practices

### For Asset Owners
- **Regular Maintenance**: Prevents score decay
- **Major Tasks**: Prioritize high-weight maintenance
- **Documentation**: Keep detailed maintenance records

### For Financial Institutions
- **Threshold Monitoring**: Set appropriate eligibility levels
- **Score Trends**: Analyze maintenance quality over time
- **Risk Assessment**: Use scores as part of comprehensive risk models

### For Engineers
- **Task Classification**: Use appropriate task types
- **Timely Updates**: Submit maintenance promptly
- **Quality Notes**: Provide detailed maintenance information

## Technical Implementation

### Storage Keys
- **Score**: `("SCORE", asset_id)`
- **Score History**: `("SCHIST", asset_id)`
- **Last Update**: `("LUPD", asset_id)`

### TTL Management
- **Extension**: All score-related entries extend TTL on updates
- **Duration**: 518,400 seconds (~6 days)
- **Purpose**: Prevents data loss

## Future Enhancements

### Potential Improvements
1. **Dynamic Weights**: Task weights based on asset type
2. **Quality Factors**: Multiplier based on engineer certification level
3. **Seasonal Adjustments**: Different decay rates for different seasons
4. **Predictive Scoring**: ML-based maintenance quality prediction

---

*This documentation is maintained alongside the Mainstay smart contract system. For the most current implementation details, refer to the source code in the lifecycle contract.*
