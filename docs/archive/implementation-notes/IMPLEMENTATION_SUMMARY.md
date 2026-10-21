# Implementation Summary: Issue #839 - Score Increment Configuration Tests

## Overview

Added two comprehensive unit tests to verify that updating `score_increment` via `update_score_increment` retroactively affects the collateral scores of existing assets when queried via `get_collateral_score`.

## Problem Statement

**Issue #839:** Changing `score_increment` via `update_score_increment` affects all future `compute_decay` calls, but there was no test verifying that the score for an existing asset changes correctly after a config update. This gap in test coverage could allow regressions where existing records are not properly re-weighted with new increment values.

## Solution

Added two focused unit tests that explicitly verify the behavior:

### Test 1: `test_score_increment_update_affects_existing_asset_score` (Line 6838)

**Purpose:** Verify that a single existing maintenance record's score is recalculated using the new `score_increment` when `get_collateral_score` is called after a config update.

**Test Sequence:**
```
1. Submit 1 maintenance record with default score_increment=5
   → Expected score: 5
   
2. Call update_score_increment(10) to change config
   
3. Call get_collateral_score() again WITHOUT new maintenance
   → Expected score: 10 (same record, new increment)
```

**Key Assertion:**
```rust
assert_eq!(updated_score, 10,
    "Score should reflect new increment (10) for existing maintenance record after config update"
);
```

### Test 2: `test_score_increment_update_weights_old_records_correctly` (Line 6875)

**Purpose:** Verify that multiple old maintenance records are all re-weighted uniformly with the new `score_increment` during `compute_decay`.

**Test Sequence:**
```
1. Submit 3 maintenance records with default score_increment=5
   → Expected score: 5 + 5 + 5 = 15
   
2. Call update_score_increment(8) to change config
   
3. Call get_collateral_score() again WITHOUT new maintenance
   → Expected score: 8 + 8 + 8 = 24 (all 3 records, new increment)
```

**Key Assertion:**
```rust
assert_eq!(score_after_update, 24,
    "Score after updating increment to 8 should be 24 (3 * 8), reflecting new weight on old records"
);
```

## Technical Details

### How Score Calculation Works

The lifecycle contract calculates collateral scores via `compute_decay()`, which:
1. Loads all maintenance records for an asset
2. Loads the current config (including `score_increment`)
3. For each record, applies the **current** `score_increment` from config (not the increment that was active when the record was submitted)
4. Applies recency weighting based on record age
5. Returns the weighted sum (capped at 100)

This is by design—the score reflects the current policy, not historical policies. The new tests verify this behavior is working correctly.

### Test Setup

Both tests use the standard test infrastructure:
- `setup()` helper to create lifecycle, asset registry, and engineer registry contracts
- `register_asset()` helper to create a test asset
- `register_engineer()` helper to create a verified engineer
- `mock_all_auths()` to bypass signature verification

Both tests use `max_history=0` (defaults to 200) since history size is not the focus.

## Files Modified

- **`/workspaces/Mainstay/contracts/lifecycle/src/lib.rs`**
  - Added 2 new test functions after `test_score_increment_affects_scoring()`
  - Lines 6832-6911 (80 lines total)

## Testing

To run the new tests:

```bash
# Run both new tests
./scripts/test.sh -k "test_score_increment_update"

# Run first test only
./scripts/test.sh -k "test_score_increment_update_affects_existing_asset_score"

# Run second test only
./scripts/test.sh -k "test_score_increment_update_weights_old_records_correctly"

# Run all lifecycle tests
./scripts/test.sh -p lifecycle
```

## Coverage

These tests verify:
- ✅ Single record re-scoring after config update
- ✅ Multiple records all uniformly re-weighted
- ✅ `compute_decay` respects current config state
- ✅ No caching of increment values
- ✅ Score calculations remain consistent across config changes

## Regression Prevention

These tests prevent regressions where:
- A config update to `score_increment` is not properly propagated to existing assets
- Some records are re-weighted but others are not
- Old records retain cached increment values instead of using the current config
- Score calculations become inconsistent after admin configuration updates
