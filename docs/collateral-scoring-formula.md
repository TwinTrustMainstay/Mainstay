# Collateral Scoring Formula

This document provides the precise mathematical formula and supporting calculations used by the Mainstay Lifecycle contract to compute an asset's collateral score. It is intended for lenders, auditors, and integrators who need to reproduce or verify on-chain score calculations.

> For a narrative overview of the scoring model (thresholds, eligibility, best practices), see [docs/collateral-scoring.md](collateral-scoring.md).

---

## Core Formula

The Lifecycle contract computes the collateral score by evaluating **two independent models** and taking the **minimum** of their results:

```
score = min(history_score, config_score)

if history is non-empty AND score < 1:
    score = 1   // Score floor for maintained assets
```

### Model A: Recency-Weighted History Score

This model reads the full maintenance history and applies a recency weight to each record:

```
For each MaintenanceRecord with age_ledgers = current_ledger - record_ledger:
    recency_weight = max(0, MAX_AGE_LEDGERS - age_ledgers)
    contribution = score_increment × recency_weight / MAX_AGE_LEDGERS

history_score = min( Σ contributions, 100 )
```

Where:
- `MAX_AGE_LEDGERS` is a fixed constant (typically ~5,356,800 ledgers ≈ 365 days)
- `score_increment` is the base increment per event (default: `5`)

### Model B: Stored Score with Lazy Config Decay

This model uses a stored accumulator and applies decay based on elapsed wall-clock time:

```
elapsed = current_timestamp - last_update_timestamp
decay_intervals = floor(elapsed / decay_interval)
config_score = max(0, stored_accumulator - decay_intervals × decay_rate)
```

Where the stored accumulator is updated on each maintenance event:

```
weighted_increment = score_increment × (500 + engineer_reputation) / 1000
stored_accumulator = min(stored_accumulator + weighted_increment, 100)
```

---

## Task Type Weights

Each maintenance task type contributes a base point value used as `score_increment` in both models:

| Tier | Points | Task Types |
|---|---|---|
| **Minor** | 2 | `OIL_CHG`, `LUBE`, `INSPECT` |
| **Medium** | 5 | `FILTER`, `TUNE_UP`, `BRAKE` |
| **Major** | 10 | `ENGINE`, `OVERHAUL`, `REBUILD` |
| **Unknown** | 3 | Any task type not in the tables above |

### Weight Selection Logic

```
if task_type in ["OIL_CHG", "LUBE", "INSPECT"]:
    score_increment = 2
elif task_type in ["FILTER", "TUNE_UP", "BRAKE"]:
    score_increment = 5
elif task_type in ["ENGINE", "OVERHAUL", "REBUILD"]:
    score_increment = 10
else:
    score_increment = 3
```

---

## Time-Based Age Penalty (Decay)

The decay mechanism ensures that scores degrade over time without new maintenance, reflecting the reality that an asset's condition deteriorates with age.

### Default Decay Parameters

| Parameter | Default Value | Description |
|---|---|---|
| `decay_rate` | 5 | Points deducted per full decay interval |
| `decay_interval` | 2,592,000 seconds (30 days) | Length of one decay interval |
| Effective rate | 0.167 points/day | Average daily decay |

### Decay Formula

```
decay_intervals = floor((current_time - last_maintenance_time) / decay_interval)
total_decay = decay_intervals × decay_rate

score_after_decay = max(0, score_before_decay - total_decay)
score_final = (history_not_empty AND score_after_decay == 0) ? MIN_SCORE_WITH_HISTORY (1) : score_after_decay
```

The final line is the floor clamp: it only ever raises a fully-decayed score (0) up to 1, and only for
assets with maintenance history. See [Score Floor](#score-floor) below.

### Decay is lazy

Decay is **not** computed automatically. It is applied on-demand when:
- `get_collateral_score(asset_id)` is called
- `submit_maintenance(asset_id, ...)` is called (decay applied first, then new points added)

This means a score queried immediately after maintenance will match the stored value. Querying the same asset 90 days later will produce a lower score.

### Decay Clamping

Decay is clamped at `0`. A score never goes negative:
```
score = max(0, score - total_decay)
```

However, if the asset has at least one maintenance record, the score floor of `1` is applied before returning:
```
if history_not_empty AND score == 0:
    score = 1
```

---

## Engineer Reputation Weighting

The engineer submitting a maintenance record influences the score increment through their reputation score (0–1000).

### Reputation Formula

```
weighted_increment = score_increment × (500 + reputation_score) / 1000
```

### Effect by Reputation Tier

| Reputation | Multiplier | Effect |
|---|---|---|
| 0 (worst) | 0.50× | Half credit for all tasks |
| 250 | 0.75× | 75% credit |
| 500 (default/neutral) | 1.00× | Full task weight applied |
| 750 | 1.25× | 25% bonus |
| 1000 (best) | 1.50× | 50% bonus |

### Example: Major task (10 pts) by engineer reputation

| Reputation | `weighted_increment` | Score added |
|---|---|---|
| 0 | `10 × (500 + 0) / 1000` | **5.0** |
| 250 | `10 × (500 + 250) / 1000` | **7.5** |
| 500 | `10 × (500 + 500) / 1000` | **10.0** |
| 750 | `10 × (500 + 750) / 1000` | **12.5** |
| 1000 | `10 × (500 + 1000) / 1000` | **15.0** |

> Note: In the Soroban implementation, all arithmetic uses integer division. Results are truncated toward zero.

---

## Score Cap

The score is always capped at **100 points** regardless of the number or weight of maintenance events:

```
score = min(raw_score, 100)
```

### Capping Example

If the raw score would be 108 after a `REBUILD` (10 pts) from a 1000-reputation engineer (1.50× = 15 pts) applied to a current score of 95:

```
score_before = 95
weighted_increment = 10 × (500 + 1000) / 1000 = 15
raw_score = 95 + 15 = 110
final_score = min(110, 100) = 100  // Capped
```

---

## Score Floor

Assets with at least one verified maintenance record are guaranteed a minimum score of **1**, even if all contributions have fully decayed:

```
if maintenance_history.len() > 0 AND score == 0:
    score = 1
```

This distinguishes maintained assets from those with no history at all (score 0).

---

## Special Cases

### Deprecated or Decommissioned Assets

Assets with `deprecation_status != Active` return a score of `0` immediately. No decay, no history computation, no floor:

```
if asset.deprecation_status != Active:
    return 0
```

### Frozen Assets

If the `FROZEN` storage key is set for an asset (set by `decommission_asset`), the score is frozen at the value captured at decommission time:

```
if is_frozen(asset_id):
    return frozen_score
```

### No Maintenance History

An asset with zero maintenance records always returns `0`:
```
if history.is_empty():
    return 0
```

---

## Concrete Calculation Examples

### Example 1: Single Maintenance, Short Hold

**Setup:**
- Brand-new asset (no history)
- Engineer reputation: 500 (neutral)
- Task: `FILTER` (medium, weight = 5)

```
weighted_increment = 5 × (500 + 500) / 1000 = 5.0
new_score = min(0 + 5, 100) = 5
```

**After 15 days, queried again:**
```
elapsed = 15 × 86400 = 1,296,000 seconds
decay_intervals = floor(1,296,000 / 2,592,000) = 0
score = max(0, 5 - 0) = 5  // No decay yet
```

**After 45 days, queried again:**
```
elapsed = 45 × 86400 = 3,888,000 seconds
decay_intervals = floor(3,888,000 / 2,592,000) = 1
score = max(0, 5 - 5) = 0 → floor: 1 (has history)
```

### Example 2: Multi-Maintenance Build-up

**Setup:**
- Asset with 20 previous maintenance records, current stored score: 55
- Engineer reputation: 750
- Task: `ENGINE` (major, weight = 10)
- Last maintenance was 12 days ago

```
// Decay first
elapsed = 12 × 86400 = 1,036,800 seconds
decay_intervals = floor(1,036,800 / 2,592,000) = 0
score_after_decay = 55  // No decay

// Apply new maintenance
weighted_increment = 10 × (500 + 750) / 1000 = 12.5 → 12
new_score = min(55 + 12, 100) = 67
```

### Example 3: Full Year Maintenance Schedule

The table below walks through a realistic 12-month generator maintenance schedule:

**Defaults:** `decay_rate = 5`, `decay_interval = 30 days`, `score_increment = 5`, all engineers at reputation 500.

| Event | Date | Task | Weight | Days since prior | Decay applied | Score after decay | Score after event |
|------:|------|------|-------:|-----------------:|--------------:|------------------:|------------------:|
| 1 | Jan 1 | `ENGINE` | 10 | — | 0 | 0 | 10 |
| 2 | Jan 21 | `FILTER` | 5 | 20 | 0 | 10 | 15 |
| 3 | Feb 20 | `BRAKE` | 5 | 30 | 5 | 10 | 15 |
| 4 | Mar 12 | `OVERHAUL` | 10 | 20 | 0 | 15 | 25 |
| 5 | Apr 11 | `FILTER` | 5 | 30 | 5 | 20 | 25 |
| 6 | May 1 | `TUNE_UP` | 5 | 20 | 0 | 25 | 30 |
| 7 | May 31 | `ENGINE` | 10 | 30 | 5 | 25 | 35 |
| 8 | Jun 20 | `FILTER` | 5 | 20 | 0 | 35 | 40 |
| 9 | Jul 20 | `BRAKE` | 5 | 30 | 5 | 35 | 40 |
| 10 | Aug 19 | `OVERHAUL` | 10 | 30 | 5 | 35 | 45 |
| 11 | Sep 8 | `FILTER` | 5 | 20 | 0 | 45 | 50 |
| 12 | Oct 8 | `REBUILD` | 10 | 30 | 5 | 45 | 55 |

**Key observations:**
- Short gaps (< 30 days) do **not** trigger decay because decay uses whole 30-day intervals
- Each full 30-day gap removes exactly 5 points
- After event 11, the asset reaches the eligibility threshold of 50
- After event 12, the score is 55 (above threshold)

**60 days of inactivity after event 12:**
```
elapsed = 60 days
decay_intervals = floor(60 / 30) = 2
total_decay = 2 × 5 = 10
score = max(0, 55 - 10) = 45  → Below eligibility threshold
```

### Example 4: High-Reputation Engineer Impact

Compare two `OVERHAUL` submissions to the same asset (current score: 30, no decay accrued):

**Engineer A** (reputation 200):
```
weighted_increment = 10 × (500 + 200) / 1000 = 7
new_score = 30 + 7 = 37
```

**Engineer B** (reputation 950):
```
weighted_increment = 10 × (500 + 950) / 1000 = 14
new_score = 30 + 14 = 44
```

The higher-reputation engineer adds **2× more** score per maintenance event.

---

## Eligibility Threshold

### Default Configuration
| Parameter | Default | Range |
|---|---|---|
| `eligibility_threshold` | 50 | 0–100 |

An asset is **collateral-eligible** when:
```
get_collateral_score(asset_id) >= eligibility_threshold
```

### Threshold Examples

| Score | Eligible? | Notes |
|---|---|---|
| 0 | ❌ | No history or fully decayed |
| 1–49 | ❌ | Has history but below threshold |
| 50–99 | ✅ | Eligible for collateral |
| 100 | ✅ | Fully scored, maximum eligibility |

> **Threshold = 1 caveat:** because the [Score Floor](#score-floor) guarantees a minimum score of `1`
> for any asset with at least one maintenance record, configuring `eligibility_threshold = 1` makes
> **every maintained asset eligible**, including ones whose score has fully decayed to the floor. This
> threshold value does not express any quality or recency requirement — it only distinguishes
> "maintained" from "never maintained." Lenders who want to gate on maintenance quality must use a
> threshold above `1`.

---

## Configuration Parameters Reference

| Parameter | Storage Key | Default | Description |
|---|---|---|---|
| `score_increment` | Config | 5 | Base points per maintenance event |
| `decay_rate` | Config | 5 | Points deducted per decay interval |
| `decay_interval` | Config | 2,592,000 (30 days) | Seconds per decay interval |
| `max_history` | Config | 200 | Maximum maintenance records per asset |
| `eligibility_threshold` | Config | 50 | Minimum score for collateral eligibility |
| `TTL_THRESHOLD` | Constant | 518,400 ledgers (~30 days) | TTL extension threshold |
| `TTL_TARGET` | Constant | 518,400 ledgers (~30 days) | TTL extension target |
| `MAX_AGE_LEDGERS` | Constant | ~5,356,800 (~365 days) | Maximum history recency window |

All configurable parameters can be updated by the contract admin.

---

## Integration Checklist

When integrating collateral scoring into lending workflows:

- [ ] **Score is stale-aware**: Always call `get_collateral_score()` at decision time — never cache a previous score because lazy decay may have reduced it
- [ ] **Handle the floor**: A score of `1` means the asset has history but it has fully decayed; treat differently from `0` (no history)
- [ ] **Handle deprecation**: Deprecated assets return `0` regardless of history; check `deprecation_status` before dismissing
- [ ] **Reputation matters**: Two identical maintenance schedules can produce different scores if submitted by engineers with different reputation levels
- [ ] **Cross-contract cost**: `get_collateral_score` makes up to 2 cross-contract calls; budget gas accordingly

---

*This document is maintained alongside the Mainstay smart contract system. For implementation details, refer to the source code in `lifecycle/src/lib.rs`.*
