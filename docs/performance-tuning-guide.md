# Performance Tuning Guide

Last updated: October 16, 2026

This guide is for operators who run Mainstay contracts and the API gateway in
front of them. It covers:

1. What each operation costs and what limits it.
2. How to find the bottleneck when a path is slow.
3. Tuning levers: configuration, batching, pagination, caching, and TTL.
4. The metrics to watch.
5. How to record before/after measurements.

Measured timings are not in this guide. The baseline medians in
[performance-benchmarks.md](performance-benchmarks.md) are still marked
*pending*. Where this guide makes a quantitative claim, it comes from the
contract source (a limit or a loop bound), and the claim is arithmetic, not a
measurement.

## 1. Cost model

Soroban charges for CPU instructions, memory, ledger reads and writes, and
transaction size. Three properties of this codebase dominate those costs.

- **Work scales with stored history.** Several reads walk a per-asset list of
  maintenance records. `compute_decay`, which backs `get_collateral_score`,
  does a single linear pass over the full history on each call. The benchmark
  notes name this as the dominant cost for large histories.
- **Cross-contract calls are expensive.** Only the Lifecycle contract makes
  cross-contract calls on its own. `submit_maintenance` calls the Asset Registry
  to verify the asset, and the Engineer Registry to check the credential and
  reputation. Each call adds to the instruction budget. `search_assets` makes
  the same kind of call when it sorts by `ByCollateralScore`.
- **Every persistent key has a TTL.** Each write extends the TTL of the keys it
  touches. Storing more keys per asset means more TTL work per submission. See
  [ttl-strategy.md](ttl-strategy.md).

### Per-operation profile

The table lists the cost driver, the hard limit the contract enforces, and the
lever an operator can pull.

| Operation | Cost driver | Enforced limit | Lever |
|---|---|---|---|
| `submit_maintenance` | Asset and engineer lookups, history append, score update, TTL extension | Notes at most `max_notes_length` (default 256); rolling-hour submissions per engineer (default 20) | Keep notes short; tune per-engineer limits to fleet size |
| `batch_submit_maintenance` | Same as above, once per record | 50 records (`MAX_BATCH_SIZE`) | Batch up to the limit to amortize the per-call overhead |
| `get_collateral_score` | One cross-contract `get_asset` call, a linear pass over the asset's maintenance history (`compute_decay`), and two persistent writes with TTL extension | History is bounded by `max_history` (default 200) | Keep `max_history` at a value the lending model needs; compact archived history |
| `get_maintenance_history_page` | Reads one page | Page size capped at 100 | Page instead of reading the full history |
| `get_score_history` | Reads one page of score entries | Page size capped at 50 | Page instead of reading the full history |
| `get_transfer_history_paginated` | Reads one page of transfer records | Page size capped at 50 | Page instead of reading the full history |
| `get_assets_by_owner_paginated` / `get_assets_by_type_paginated` | Reads one page of IDs | Page size capped at 100 | Use the paginated variants, not the unpaginated ones |
| `batch_register_assets` | Per-asset serial-number dedup (SHA-256 each) and storage writes | 50 assets (`MAX_BATCH_SIZE`) | Batch up to the limit; keep serial numbers unique and short |
| `batch_get_assets` | One read per ID | 50 IDs | Hydrate lists in batches, not one call per asset |
| `get_asset_full_snapshots` | Asset, score, valuation, and maintenance summary per ID | 50 IDs | Use for detail views that need the whole picture in one call |
| `search_assets` | Filter over assets; cross-contract score lookup for `ByCollateralScore` | 100 results per page | Filter by type first; provide `lifecycle_contract` only when you sort by score |
| `batch_revoke_credentials` / `batch_revoke_engineer_authorizations` | One revoke per engineer | 50 entries | Batch up to the limit |

## 2. Finding the bottleneck

Work from the outside in. Most slow requests are one of three things: too many
calls, too much data per call, or a slow dependency.

1. **Count the calls.** Check the access log for the request path. If a client
   calls `get_asset` in a loop, switch it to `batch_get_assets`. For N assets
   the batch path needs `ceil(N / 50)` calls instead of N.
2. **Check the response size.** A history read without pagination grows with
   the asset's record count. Switch to the paginated variant and request the
   next page until a page comes back shorter than the page size.
3. **Check the contract path.** If a call is slow only when it involves
   scoring, the history scan in `compute_decay` is the likely cause. Compare
   the history length with the response time. If there is a clear trend, the
   scoring path is the bottleneck.
4. **Check the dependency.** RPC latency and failed simulations show up
   outside the contract. Follow the [monitoring guide](monitoring-guide.md) and
   the [multi-region runbook](multi-region-deployment-guide.md) to confirm the
   RPC endpoint is healthy before you tune the contract.
5. **Check the gateway.** The regional proxy caches successful `GET` and
   `HEAD` responses for five seconds. `X-Cache-Status` shows whether a request
   was a `HIT`, `MISS`, or `BYPASS`. A low hit rate on a read-heavy endpoint
   usually means the cache key includes varying parameters.

Use `X-Request-ID` to correlate a slow request across the proxy, the API, and
the contract call. See [api-performance.md](api-performance.md).

## 3. Tuning levers

### 3.1 Configuration

The Lifecycle contract's `Config` controls the size of the work each call can
do. The defaults are set in `contracts/lifecycle/src/lib.rs`.

| Setting | Default | Effect on performance |
|---|---|---|
| `max_history` | 200 | Upper bound on records per asset. Lower values bound the history scan in scoring, at the cost of retained history. Changes take effect through the timelocked `propose_update_config` flow. |
| `max_engineer_history` | 200 | Bounds per-engineer history. |
| `max_notes_length` | 256 | Bounds the size of each record's notes. |
| `max_submissions_per_hour` | 20 | Per-engineer throttle. Raise it for high-volume fleets; lower it to protect RPC capacity. |

Changes to `Config` are subject to the admin timelock. Plan them as a change
window, and record the before and after values in the change log.

### 3.2 Batching

Batching amortizes the fixed cost of an invocation across many items. The
batch endpoints that enforce a limit use 50 as the cap, listed in the table in
section 1. Oversized batches are rejected with `BatchTooLarge` (or
`BatchRevokeTooLarge` for revocations), so clients must split their input.
`batch_verify_engineers` does not set a cap in the contract source, so keep
its input size reasonable and measure before you raise it.

Use the largest batch the client can build without waiting. For a backlog of
1,000 records, the batch path needs 20 invocations of 50 instead of 1,000
single submissions.

### 3.3 Pagination

Read paths that return lists are paginated and capped. Clients should:

- Request the largest page the cap allows, to reduce round trips.
- Request the next page until a page comes back shorter than the page size.
  Lifecycle history reads take `offset` and `limit`. The owner and type
  paginators in the asset registry take a 0-based `page` and a `page_size`.
  Neither form returns a cursor.
- Avoid fetching a full history to answer a question about the latest record.
  The `get_maintenance_history_page` and `get_maintenance_history_opt` calls
  exist for this purpose.

### 3.4 Filtering before sorting

`search_assets` applies filters before the page is built. Filter on
`asset_type` or `manufacturer` first, and sort last. Sorting by
`ByCollateralScore` makes a cross-contract call per candidate, so narrow the
candidate set before you request that sort. The contract rejects the request
with `InvalidConfig` if `lifecycle_contract` is missing.

### 3.5 Caching

- **Gateway:** Successful `GET` and `HEAD` responses are cached for five seconds
  at the regional proxy. Writes are never cached. Keep cache keys free of
  per-request values, and do not cache responses that must reflect the latest
  ledger state.
- **Client:** Cache read-only lookups that change rarely, such as engineer
  profiles and asset metadata. Invalidate on the write events the API publishes.
- **Contract:** Do not rely on cached values for lending decisions. The
  [lender integration guide](lender-integration-guide.md) requires a direct
  contract read for any decision that depends on the latest state.

### 3.6 TTL and storage growth

Each write extends the TTL of the keys it touches to the target of 518,400
ledgers, about 30 days at 5 seconds per ledger. A key that is not written again
expires after that window unless something extends it. Keep the TTL extension
step on every path that writes persistent state, and avoid adding keys that are
written on every call without a reason. The
[storage growth test](../tests/test_storage_growth.rs) and the
[TTL extension test](../tests/test_ttl_extension.rs) show the expected
behavior.

Compact archived history with `compact_maintenance_history` for decommissioned
assets. Compaction reduces the number of records a scoring pass has to read.

### 3.7 Rate limits

The regional proxy enforces 100 requests per minute per IP and 1,000 requests
per day per API key. Requests above the limit receive `rate_limit_exceeded`.
Clients that batch and paginate use fewer requests for the same work, so they
are less likely to hit these limits. See [rate-limiting.md](rate-limiting.md).

## 4. Metrics to monitor

Monitor these signals together. A single metric rarely identifies the cause.

| Signal | Source | What it tells you |
|---|---|---|
| `mainstay_exporter_up` | Prometheus exporter | Whether the metrics exporter is reachable. |
| `mainstay_metrics_collection_seconds` | Prometheus exporter | How long one collection cycle takes. A rising value points to slow contract reads. |
| `mainstay_maintenance_records_total` | Prometheus exporter | Total record count. Use it to forecast history growth against `max_history`. |
| `mainstay_avg_collateral_score` | Prometheus exporter | Shifts here can indicate scoring or decay changes, not only performance. |
| `mainstay_days_since_last_service` | Prometheus exporter | Per-asset staleness. Not a performance metric, but it explains why scores decline. |
| `X-Cache-Status` distribution | Regional proxy logs | Cache hit rate for read endpoints. |
| Request latency by path | API access logs | Which endpoints are slow, and whether slowness correlates with traffic. |
| RPC latency and simulation failures | Monitoring stack | Whether the contract path or the network is the bottleneck. |

The full list of exporter metrics is in
[PROMETHEUS_METRICS.md](monitoring/PROMETHEUS_METRICS.md). The alerting rules
are in `scripts/monitoring/alerting-rules.yml`.

Set alerts on sustained changes rather than single samples. For latency, alert
on the 95th percentile over a window long enough to smooth out bursts.

## 5. Recording before and after measurements

A tuning change is complete only when it is measured. Record each change in the
[performance benchmarks](performance-benchmarks.md) regressions table, or in a
case-study entry like the one below.

### 5.1 Method

1. Pick one operation and one input size. Write the size down (for example,
   "1,000 assets, owner with 50 assets").
2. Run the benchmark on the unchanged code and record the median. Run it at
   least three times and keep the spread.
3. Apply one change. Run the same benchmark with the same inputs.
4. Record the median, the spread, the contract version, and the commit.
5. Keep the change only if the improvement is larger than the spread. A
   change that moves the median by less than the spread is noise.

The benchmark commands are in
[performance-benchmarks.md](performance-benchmarks.md#running-benchmarks).

### 5.2 Case study template

```markdown
### Case study: <short title>

- **Operation:** `<contract>.<function>`
- **Input size:** <records / assets / calls>
- **Change:** <what changed, and the commit>
- **Before:** median <value>, spread <value>, n = <runs>
- **After:** median <value>, spread <value>, n = <runs>
- **Result:** <improvement or no change, with the numbers>
- **Rollback:** <how to revert, if needed>
```

### 5.3 Worked example (analysis, not measured)

Consider a client that hydrates a fleet of 1,000 assets for a dashboard.

- **Before:** one `get_asset` call per asset. That is 1,000 invocations.
- **After:** `batch_get_assets` with 50 IDs per call. That is `ceil(1000 / 50)`
  = 20 invocations.

The invocation count falls by 98%. The per-invocation overhead falls by the
same proportion. The data returned is unchanged. This is a count-based
argument. Measure the latency in your environment before you report a speedup.

## 6. Known gaps

- The [performance benchmarks](performance-benchmarks.md) still have *pending*
  baselines. Until they are filled in, relative claims in this guide are not
  measurements.
- `compute_decay` is the largest known scaling cost. The proposed fix, a
  running score that applies only incremental decay, is not implemented.
- The API gateway that serves these endpoints is deployed outside this
  repository. Gateway-level limits must be confirmed in that service.
