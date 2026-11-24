# Test Additions for Issue #839

## Summary

Added two comprehensive tests to verify that `score_increment` configuration updates retroactively affect the collateral scores of existing assets when `compute_decay` is called.

## Tests Added

### 1. `test_score_increment_update_affects_existing_asset_score`

**Location:** `/workspaces/Mainstay/contracts/lifecycle/src/lib.rs` (after line 6830)

**Purpose:** Verify that a single maintenance record's score is recalculated using a new `score_increment` value when queried after a config update.

**Test Flow:**
1. Submit one maintenance record with default `score_increment` (5)
   - Verify initial score is 5
2. Update `score_increment` to 10 via `update_score_increment`
3. Re-query the same asset's score without submitting new maintenance
4. Assert the new score is 10 (reflecting the updated increment on the existing record)

**Key Assertion:**
```rust
assert_eq!(updated_score, 10,
    "Score should reflect new increment (10) for existing maintenance record after config update"
);
```

---

### 2. `test_score_increment_update_weights_old_records_correctly`

**Location:** `/workspaces/Mainstay/contracts/lifecycle/src/lib.rs` (after line 6868)

**Purpose:** Verify that multiple old maintenance records are all weighted with the new `score_increment` during `compute_decay` when the configuration is updated.

**Test Flow:**
1. Submit 3 maintenance records with default `score_increment` (5)
   - Expected initial score: 5 + 5 + 5 = 15
   - Verify score is 15
2. Update `score_increment` to 8 via `update_score_increment`
3. Re-query the same asset's score without submitting new maintenance
4. Assert the new score is 24 (reflecting 3 records × 8 new increment)

**Key Assertion:**
```rust
assert_eq!(score_after_update, 24,
    "Score after updating increment to 8 should be 24 (3 * 8), reflecting new weight on old records"
);
```

---

## Coverage

These tests verify that:
- ✅ Config updates to `score_increment` are immediately reflected in score calculations
- ✅ Existing maintenance records are re-weighted using the new increment
- ✅ The `compute_decay` function respects the current config state, not a cached state
- ✅ Multiple records are all affected uniformly by the increment change

## How to Run

```bash
# Run both new tests
./scripts/test.sh -k "test_score_increment_update"

# Run individual tests
./scripts/test.sh -k "test_score_increment_update_affects_existing_asset_score"
./scripts/test.sh -k "test_score_increment_update_weights_old_records_correctly"

# Run all lifecycle tests
./scripts/test.sh -p lifecycle
```

## Related Issue

These tests address Issue #839, which identified a gap in test coverage:
- **Problem:** Changing `score_increment` via `update_score_increment` affects all future `compute_decay` calls, but there was no test verifying that existing assets' scores change correctly after a config update.
- **Solution:** The two new tests ensure that `compute_decay` properly applies the updated increment to both new and old maintenance records.
