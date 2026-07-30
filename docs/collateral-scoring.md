# Collateral Scoring Model

This document describes the collateral scoring model used in the Mainstay lifecycle contract to assess the value and eligibility of assets based on their maintenance history.

## Overview

The collateral scoring system is designed to provide a quantitative measure of an asset's maintenance quality and recency. Higher scores indicate well-maintained assets that are more suitable for use as collateral in financial transactions.

## Score Floor

Assets with at least one verified maintenance record are guaranteed a minimum score of **1** (`MIN_SCORE_WITH_HISTORY`), even if time-based decay would otherwise reduce the raw score to 0.

This prevents a legitimately-maintained asset from becoming indistinguishable from one with no history at all.

| Maintenance records | Minimum score returned |
|---------------------|------------------------|
| 0                   | 0                      |
| ≥ 1                 | 1 (floor)              |

### Minimum records for non-zero score

With default configuration (`score_increment = 5`):

- **1 record** → raw score = 5 (non-zero immediately after submission)
- After sufficient decay the raw score reaches 0, but the floor ensures `get_collateral_score` returns **1**

To reach the default **eligibility threshold of 50**, an asset needs at least **10 records** at the default increment of 5 points each (or fewer records using higher-weight task types such as `ENGINE` / `OVERHAUL` at 10 points each).

## Score Mechanics

### Score Range
- **Minimum Score**: 0 points (no maintenance history), or **1 point** (at least one record — see [Score Floor](#score-floor))
- **Maximum Score**: 100 points
- **Eligibility Threshold**: 50 points (default, configurable)

### Score Calculation

Scores are calculated based on:
1. **Maintenance Task Weight**: Different task types add different point values
2. **Time-Based Decay**: Scores decrease over time without maintenance
3. **Score Cap**: Total score never exceeds 100 points

### Score Formula

The Lifecycle contract computes collateral score using a combination of weighted maintenance events and time decay:

```text
raw_score = sum(weight(task_i) * recency_weight(task_i))
decay = floor((current_timestamp - last_update) / decay_interval) * decay_rate
score = clamp(max(raw_score - decay, 0), 0, 100)
return if has_history && score == 0 { 1 } else { score }
```

### Score Diagram

```
Maintenance history  -->  Weighted event points  -->  Time decay  -->  Score cap (100)
      task weights          recency calc             decay rate         clamp to [0,100]
```

The eligibility threshold is checked after this score is computed: the asset is collateral-eligible only if the final score is greater than or equal to the configured threshold.

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

### Default weights and rationale

| Task type | Weight | Rationale |
|-----------|-------:|-----------|
| `OIL_CHG` | 2 | Routine consumable service; low cost, frequent cadence, minimal impact on asset longevity per event. |
| `LUBE` | 2 | Preventive, low-cost, high-frequency task with a small marginal effect on collateral value. |
| `INSPECT` | 2 | Diagnostic only — verifies condition but does not itself extend asset life, so it is weighted like other minor tasks. |
| `FILTER` | 5 | Protects core components (engine/fuel system) from contamination; moderate cost and moderate impact on reliability. |
| `TUNE_UP` | 5 | Restores performance and efficiency; broader scope than a single-part swap, so weighted above minor tasks. |
| `BRAKE` | 5 | Safety-critical system service; weighted at the medium tier to reflect both cost and risk mitigation. |
| `ENGINE` | 10 | High cost, directly extends the useful life of the highest-value component on most DePIN hardware. |
| `OVERHAUL` | 10 | Comprehensive multi-system rebuild; the strongest signal of long-term asset care available. |
| `REBUILD` | 10 | Equivalent scope/cost to an overhaul — restores the asset close to as-new condition. |
| *(unrecognized)* | 3 | Default for task types outside the allowlist; deliberately between minor and medium so unknown work is never over- or under-credited. |

### Worked Example: 5 Maintenance Records Over 90 Days

A minimal example isolating just the decay/score interaction, using the documented defaults (`decay_rate = 5`, `decay_interval = 30 days`, `score_increment` via task weights):

| # | Day | Task | Weight | Days since prior | Decay before event | Score after decay | Score after event |
|--:|----:|------|-------:|------------------:|--------------------:|-------------------:|--------------------:|
| — | 0 (registration) | — | — | — | — | — | **0** |
| 1 | 0 | `ENGINE` | 10 | 0 | 0 | 0 | **10** |
| 2 | 18 | `FILTER` | 5 | 18 | `floor(18/30)*5 = 0` | 10 | **15** |
| 3 | 40 | `BRAKE` | 5 | 22 | `floor(22/30)*5 = 0` | 15 | **20** |
| 4 | 65 | `OVERHAUL` | 10 | 25 | `floor(25/30)*5 = 0` | 20 | **30** |
| 5 | 90 | `TUNE_UP` | 5 | 25 | `floor(25/30)*5 = 0` | 30 | **35** |

At registration (day 0, before any record) the score is **0** — no maintenance history yet. After the 5th record on day 90, the score is **35**.

**After 60 further days of inactivity** (day 150, no new records):

```text
elapsed_time = 60 days
decay_intervals = floor(60 / 30) = 2
total_decay = 2 * 5 = 10
new_score = max(35 - 10, 0) = 25
```

Score at day 150: **25** (still above the score floor of 1 since the asset has maintenance history, and below the default eligibility threshold of 50 — this asset would not be collateral-eligible at that point).

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

## Worked Example: Generator with 12 Months of Maintenance

The table below walks through a realistic generator maintenance schedule over roughly 12 months using the documented defaults:

- `decay_rate = 5`
- `decay_interval = 30 days`
- `eligibility_threshold = 50`
- task weights: minor = 2, medium = 5, major = 10

Assumption: the score is checked immediately before each maintenance event, so any elapsed-time decay is applied first and the new maintenance points are then added.

| Event | Approx. date | Task | Weight | Days since prior event | Decay applied before event | Score after decay | Score after event |
|------:|--------------|------|-------:|-----------------------:|---------------------------:|------------------:|------------------:|
| 1 | Jan 1 | `ENGINE` | 10 | — | 0 | 0 | 10 |
| 2 | Jan 21 | `FILTER` | 5 | 20 | `floor(20 / 30) * 5 = 0` | 10 | 15 |
| 3 | Feb 20 | `BRAKE` | 5 | 30 | `floor(30 / 30) * 5 = 5` | 10 | 15 |
| 4 | Mar 12 | `OVERHAUL` | 10 | 20 | `floor(20 / 30) * 5 = 0` | 15 | 25 |
| 5 | Apr 11 | `FILTER` | 5 | 30 | `floor(30 / 30) * 5 = 5` | 20 | 25 |
| 6 | May 1 | `TUNE_UP` | 5 | 20 | `floor(20 / 30) * 5 = 0` | 25 | 30 |
| 7 | May 31 | `ENGINE` | 10 | 30 | `floor(30 / 30) * 5 = 5` | 25 | 35 |
| 8 | Jun 20 | `FILTER` | 5 | 20 | `floor(20 / 30) * 5 = 0` | 35 | 40 |
| 9 | Jul 20 | `BRAKE` | 5 | 30 | `floor(30 / 30) * 5 = 5` | 35 | 40 |
| 10 | Aug 19 | `OVERHAUL` | 10 | 30 | `floor(30 / 30) * 5 = 5` | 35 | 45 |
| 11 | Sep 8 | `FILTER` | 5 | 20 | `floor(20 / 30) * 5 = 0` | 45 | 50 |
| 12 | Oct 8 | `REBUILD` | 10 | 30 | `floor(30 / 30) * 5 = 5` | 45 | 55 |

### Step-by-step interpretation

1. The first major service starts the generator at **10**.
2. Short 20-day gaps do **not** trigger decay because decay uses whole 30-day intervals.
3. Each 30-day gap removes **5** points before the next event is applied.
4. Repeated medium and major maintenance steadily offsets decay and pushes the score upward.
5. After event 11, the generator reaches the default eligibility threshold exactly: **50**.
6. After event 12, the score rises to **55**, so the asset is above the threshold.

### Collateral eligibility determination

Using the default rule:

```text
is_eligible = current_score >= 50
```

Immediately after the final `REBUILD` event:

```text
current_score = 55
eligibility_threshold = 50
55 >= 50 => eligible
```

If no new maintenance is recorded for the next 60 days, decay is applied again:

```text
elapsed_time = 60 days
decay_intervals = floor(60 / 30) = 2
total_decay = 2 * 5 = 10
new_score = 55 - 10 = 45
45 >= 50 => not eligible
```

This illustrates the intended behavior of the model:

- maintenance events increase the score,
- inactivity causes the score to decay in discrete steps, and
- collateral eligibility depends on the score **at the time of the check**, not just on historical maintenance volume.

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
