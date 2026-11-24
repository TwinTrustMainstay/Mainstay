# Metrics Collection and Time-Series Storage

This document describes how Mainstay metrics are defined, collected, stored,
retained, and queried. It covers the key metrics for response time, error
rate, and gas usage, plus the existing contract-health metrics.

## Pipeline

```
Stellar RPC ──► prometheus-exporter.sh ──► Prometheus (TSDB) ──► long-term store
   (reads)        /metrics on :9600          30-day raw          1-year daily tier
                                                  │
                                                  ▼
                                    metrics-query.sh / Grafana
```

- **Exporter** (`scripts/monitoring/prometheus-exporter.sh`) reads contract
  state and records RPC response time and error counts for each collection.
- **Cost tracker** (`scripts/finops/cost_tracker.py`) emits the gas metric
  `mainstay_operation_gas_stroops_total`. See [FinOps tracking](#gas-usage).
- **Prometheus** scrapes the exporter (`monitoring/prometheus/prometheus.yml`).
- **Recording rules** (`monitoring/prometheus/rules.yml`) derive error ratio,
  mean response time, and hourly gas rate.

## Key Metrics

The authoritative list is [`monitoring/metrics-catalog.yml`](../../monitoring/metrics-catalog.yml).
`scripts/check_metrics_catalog.sh` runs in CI and fails if a catalogued metric
is no longer emitted, or if an emitted metric is missing from the catalog.

| Concern        | Metric / rule                                  | Type    | Notes |
|----------------|-----------------------------------------------|---------|-------|
| Response time  | `mainstay:rpc_response_time_seconds_avg`      | rule    | `sum / requests` from the latest collection |
| Error rate     | `mainstay:rpc_error_ratio`                    | rule    | `errors / requests` from the latest collection |
| Gas usage      | `mainstay:operation_gas_stroops:rate1h`       | rule    | Per-operation gas spend per hour |
| Gas (raw)      | `mainstay_operation_gas_stroops_total`        | counter | Labels: `operation`, `department` |
| Throughput     | `mainstay_rpc_requests_last_scrape`           | gauge   | Read-only calls in the last collection |

Contract-health metrics (`mainstay_assets_total`, `mainstay_collateral_score`,
and others) are unchanged. The full list is in the catalog.

### Why per-scrape counters?

The exporter runs `stellar contract invoke` inside `$(...)` subshells, so
in-memory counters would be lost. Each call appends its exit code and duration
to a temporary log, and the totals are written as gauges at the end of each
collection. Error rate is therefore a ratio over the most recent scrape, not
an ever-growing counter. This makes the ratio directly meaningful for alerting.

## Retention Policy

| Tier    | Series                                   | Resolution | Kept for | Where |
|---------|------------------------------------------|------------|----------|-------|
| `raw`   | All catalogued metrics                   | Scrape interval (60s) | 30 days | Local Prometheus TSDB (`--storage.tsdb.retention.time=30d`) |
| `daily` | Per-asset and gas series (`retention: daily` in the catalog) | Daily aggregates | 1 year | Long-term store via `remote_write` |

Raw data is intentionally short-lived because the chain is the source of truth
for contract state. Retention applies to derived telemetry only. Changing a
tier means editing the `retention:` field in the catalog and updating this
table in the same PR.

To enable long-term storage, uncomment and configure `remote_write` in
`monitoring/prometheus/prometheus.yml`. The write filter there keeps only
`mainstay_*` series.

## Running Locally

```bash
# 1. Start the exporter (requires CONTRACT_* and STELLAR_NETWORK in .env)
./scripts/monitoring/prometheus-exporter.sh --port 9600

# 2. Start Prometheus with the Mainstay config and 30-day retention
prometheus --config.file=monitoring/prometheus/prometheus.yml \
           --storage.tsdb.retention.time=30d

# 3. Query metrics
./scripts/monitoring/metrics-query.sh 'mainstay_exporter_up'
./scripts/monitoring/metrics-query.sh 'mainstay:rpc_error_ratio'
./scripts/monitoring/metrics-query.sh --range --start 1767225600 --end 1767229200 \
    --step 60s 'mainstay:rpc_response_time_seconds_avg'
```

## Metric Query API

`scripts/monitoring/metrics-query.sh` wraps the Prometheus HTTP API.

| Option            | Meaning |
|-------------------|---------|
| (none)            | Instant query (`/api/v1/query`) |
| `--range`         | Range query (`/api/v1/query_range`) |
| `--start`, `--end`| Unix seconds or RFC 3339. Default: last hour |
| `--step`          | Resolution for range queries (default `60s`) |
| `--dry-run`       | Print the endpoint and parameters without calling it |

Only `mainstay_*` metrics and `mainstay:*` rules can be queried. This keeps the
wrapper from being a general-purpose Prometheus proxy. The base URL comes from
`PROMETHEUS_URL` (default `http://localhost:9090`).

## Gas Usage

Gas is recorded per operation and per department by the FinOps tooling
(see [`docs/monitoring/COST_TRACKING.md`](COST_TRACKING.md)). The cost tracker
writes `mainstay_operation_gas_stroops_total` to a Prometheus textfile, so
the gas series appear in the same store as the rest of the metrics.

## Related Documentation

- [Prometheus Metrics Exporter](PROMETHEUS_METRICS.md) — exporter setup and per-metric reference
- [Cost Tracking](COST_TRACKING.md) — gas and operating-cost tracking
- [Operations Runbook](../operations-runbook.md)
