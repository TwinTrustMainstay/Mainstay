# Performance Benchmarks

Last updated: July 28, 2026

This document records performance baselines for collateral scoring, batch asset registration, and search queries. Benchmarks are implemented using [Criterion.rs](https://github.com/bheisler/criterion.rs) in the `benches/` directory.

## Running Benchmarks

```bash
cargo bench --manifest-path benches/Cargo.toml
```

Individual benchmarks:

```bash
cargo bench --manifest-path benches/Cargo.toml --bench collateral_bench
cargo bench --manifest-path benches/Cargo.toml --bench batch_registration_bench
cargo bench --manifest-path benches/Cargo.toml --bench search_bench
```

## Baseline Results

*Results below are placeholder values. Replace with actual benchmark output after first run.*

### 1. `get_collateral_score` — History Size Scaling

| History Size | Median Time | Notes |
|---|---|---|
| 10 records | *pending* | Linear scan over 10 records; negligible |
| 100 records | *pending* | Linear scan over 100 records; modest |
| 1000 records | *pending* | O(n) scan over full history; dominant cost |

**Observation:** `compute_decay` performs a linear pass over the full maintenance history per call. For assets with large histories, this becomes the bottleneck. Potential optimizations include maintaining a running accumulated score that only applies incremental decay on each call, avoiding the O(n) scan entirely.

### 2. `batch_register_assets` — Batch Size Scaling

| Batch Size | Median Time | Notes |
|---|---|---|
| 1 | *pending* | Single asset; baseline overhead |
| 10 | *pending* | Small fleet deployment |
| 25 | *pending* | Medium fleet |
| 50 (MAX_BATCH_SIZE) | *pending* | Max permitted; upper bound |

**Observation:** The batch path performs per-asset dedup checks and storage writes. Serial-number dedup requires a sha256 hash per asset (cost scales linearly). TTL extension per storage key also adds up.

### 3. Search Queries — Result Set Scaling

| Query | Dataset Size | Median Time | Notes |
|---|---|---|---|
| No filter (all types) | 1000 assets | *pending* | Full scan with 100-result cap |
| Single type filter | ~200 matched | *pending* | Type-indexed lookup; sub-linear |
| `get_assets_by_owner` | 50 owned | *pending* | Owner-indexed; direct read |

**Observation:** Filtered queries benefit from type/owner indexes. Unfiltered full scans are capped at 100 results by `SearchPage`, preventing unbounded return data.

## Optimization Opportunities

1. **Score caching**: Store pre-computed collateral score and only recompute on-demand with incremental decay. Currently every `get_collateral_score` scans the full maintenance history.

2. **Batch write amortization**: Group TTL extensions into a single batch write where the SDK supports it, rather than extending each key individually.

3. **Search result caching**: Index maintenance timestamps to avoid linear scans in `search_assets` with `SortOrder::ByMaintenanceDate`.

## Regressions

| Date | Change | Impact | Mitigation |
|---|---|---|---|
| — | — | — | — |

*Add entries when a code change causes a statistically significant regression (>10% slower) in any benchmark.*
