# Cost Tracking and FinOps

Mainstay runs on Stellar, and every contract call costs a network fee. This
document covers how those fees are recorded per operation and department, how
to see where the spend goes, how to forecast it, and how budget alerts work.

Tooling: [`scripts/finops/cost_tracker.py`](../../scripts/finops/cost_tracker.py)
(Python standard library only, so it runs in CI and on operator machines
without installs). Tests: [`scripts/finops/test_cost_tracker.py`](../../scripts/finops/test_cost_tracker.py).

## Units

All amounts are in **stroops**. 1 XLM = 10,000,000 stroops. Reports also show
XLM for readability.

## Recording Fees

Each contract operation's fee is appended to a JSONL ledger with its operation
name and the department responsible for it. The default ledger is
`costs/ledger.jsonl` (override with `--ledger` or `COST_LEDGER`). The `costs/`
directory is git-ignored, since the ledger holds operational data.

```bash
python3 scripts/finops/cost_tracker.py record \
    --operation submit_maintenance \
    --department field-ops \
    --fee-stroops 123456 \
    --timestamp 2026-10-16T12:00:00Z    # optional; defaults to now
```

The fee value comes from the transaction result. Use the `feeCharged` field
returned by Soroban RPC `getTransaction` for a submitted transaction, or the
fee reported by simulation for a dry run.

Each ledger line looks like this:

```json
{"ts": "2026-10-16T12:00:00Z", "operation": "submit_maintenance", "department": "field-ops", "fee_stroops": 123456}
```

Malformed lines are reported on stderr and skipped, so a single bad entry does
not stop reporting.

## Reports

```bash
python3 scripts/finops/cost_tracker.py report \
    --budgets scripts/finops/budgets.example.json \
    --as-of 2026-10-16
```

The report contains:

- **Totals**: all-time spend in stroops and XLM.
- **By operation** and **by department**: call count, total, and average fee.
- **Optimisation candidates**: operations to review, with reasons (below).
- **Forecast**: projected spend for the next 30 days (below).
- **Budget status**: month-to-date spend per department against its budget.

Add `--json` for machine-readable output.

### Optimisation candidates

An operation is flagged when either of these holds:

| Rule | Threshold | Suggested action |
|------|-----------|------------------|
| Dominant spend | Takes 25% or more of total spend | Batch calls, or reduce call frequency |
| Expensive per call | Average fee is at least 2x the overall average | Review its resource footprint (storage reads/writes, footprint size) |

These are prompts for review, not automatic changes. An operation can be
flagged and still be the right cost for what it does.

### Forecast

The forecast fits a linear trend to daily spend over the last 30 days. Days
with no spend count as zero, so quiet periods pull the trend down instead of
being skipped. The trend is then projected forward 30 days, with projected
daily values clamped at zero. It needs at least 2 days of records. Otherwise it
reports insufficient data.

A linear trend is deliberately simple. It is useful for spotting a sustained
rise in cost, not for predicting spikes.

### Budgets and alerts

Budgets are monthly limits per department, in a JSON file:

```json
{
  "departments": {
    "field-ops": { "monthly_stroops": 50000000 }
  }
}
```

Spend is counted from the first day of the report's month up to the `--as-of`
date. Each department gets one of these levels:

| Level    | Condition                       |
|----------|---------------------------------|
| `OK`     | Under 80% of budget             |
| `WARN`   | 80% to under 100% of budget     |
| `BREACH` | 100% of budget or more          |

`report` exits with status **2** when any department is in `BREACH`, so it can
gate a scheduled job or CI step. Exit status 0 means no breach.

## Prometheus Export

```bash
python3 scripts/finops/cost_tracker.py export-prom \
    --out /var/lib/node_exporter/textfile/mainstay-gas.prom
```

This writes the cumulative counter `mainstay_operation_gas_stroops_total` with
`operation` and `department` labels, in Prometheus text format. Point a
node_exporter textfile collector at the directory and the series enter the same
store as the rest of the metrics. Recording rules and queries are in
[METRICS_COLLECTION.md](METRICS_COLLECTION.md#gas-usage).

The file is written atomically (temp file, then rename), so a scrape never sees
a partial file.

## Testing

```bash
python3 -m unittest discover -s scripts/finops -p 'test_*.py' -v
```

These tests run in the CI docs job. They cover recording and validation,
malformed ledger lines, summaries, optimisation flags, the forecast, budget
levels (including that last month's spend is excluded), Prometheus escaping,
and the CLI exit code on breach.

## Related Documentation

- [Metrics Collection and Time-Series Storage](METRICS_COLLECTION.md)
- [Operations Runbook](../operations-runbook.md)
