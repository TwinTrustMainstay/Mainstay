#!/usr/bin/env python3
"""Mainstay FinOps: gas cost tracking, forecasting, and budget alerts.

Records the fee paid for each contract operation in an append-only JSONL
ledger, then reports where the spend goes, flags likely optimisation targets,
forecasts the next 30 days, and checks department budgets.

Ledger line format (one JSON object per line):
    {"ts": "2026-10-16T12:00:00Z", "operation": "submit_maintenance",
     "department": "field-ops", "fee_stroops": 123456}

Commands:
    record      Append one fee record to the ledger
    report      Summarise spend, optimisation candidates, forecast, and budgets
    export-prom Write mainstay_operation_gas_stroops_total as a Prometheus textfile

Examples:
    python3 scripts/finops/cost_tracker.py record --operation submit_maintenance \\
        --department field-ops --fee-stroops 123456
    python3 scripts/finops/cost_tracker.py report --budgets finops/budgets.json
    python3 scripts/finops/cost_tracker.py export-prom --out /var/lib/node_exporter/textfile/mainstay-gas.prom

The ledger path defaults to $COST_LEDGER or costs/ledger.jsonl.
Standard library only, so it runs in CI and on operator machines without installs.
"""

import argparse
import datetime as dt
import json
import os
import statistics
import sys
from collections import defaultdict

STROOPS_PER_XLM = 10_000_000
UNASSIGNED = "unassigned"

# Optimisation heuristics. See docs/monitoring/COST_TRACKING.md for rationale.
DOMINANT_SHARE = 0.25      # an operation taking >= 25% of total spend is a target
EXPENSIVE_FACTOR = 2.0     # average fee >= 2x the overall mean fee is a target

# Budget alert thresholds (fraction of the monthly budget).
BUDGET_WARN = 0.80
BUDGET_BREACH = 1.00

FORECAST_WINDOW_DAYS = 30
FORECAST_HORIZON_DAYS = 30


def default_ledger():
    return os.environ.get("COST_LEDGER", os.path.join("costs", "ledger.jsonl"))


def parse_ts(value):
    return dt.datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=dt.timezone.utc)


def format_ts(value):
    return value.strftime("%Y-%m-%dT%H:%M:%SZ")


def load_ledger(path):
    """Read all records. Malformed lines are reported and skipped, not fatal."""
    records = []
    if not os.path.exists(path):
        return records
    with open(path, encoding="utf-8") as fh:
        for lineno, line in enumerate(fh, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
                rec["_ts"] = parse_ts(rec["ts"])
                rec["fee_stroops"] = int(rec["fee_stroops"])
                rec.setdefault("department", UNASSIGNED)
                records.append(rec)
            except (ValueError, KeyError, TypeError) as exc:
                print(f"warning: skipping {path}:{lineno}: {exc}", file=sys.stderr)
    return records


def record_fee(path, operation, department, fee_stroops, when=None):
    if fee_stroops < 0:
        raise ValueError("fee_stroops must be non-negative")
    if not operation:
        raise ValueError("operation is required")
    when = when or dt.datetime.now(dt.timezone.utc)
    entry = {
        "ts": format_ts(when),
        "operation": operation,
        "department": department or UNASSIGNED,
        "fee_stroops": int(fee_stroops),
    }
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "a", encoding="utf-8") as fh:
        fh.write(json.dumps(entry, sort_keys=True) + "\n")
    return entry


def summarise_by(records, key):
    totals = defaultdict(lambda: {"count": 0, "fee_stroops": 0})
    for rec in records:
        bucket = totals[rec[key]]
        bucket["count"] += 1
        bucket["fee_stroops"] += rec["fee_stroops"]
    for bucket in totals.values():
        bucket["avg_stroops"] = bucket["fee_stroops"] / bucket["count"]
    return dict(totals)


def optimisation_candidates(by_operation, total_stroops):
    """Flag operations that dominate spend or cost much more per call than average."""
    if not by_operation or total_stroops == 0:
        return []
    overall_avg = total_stroops / sum(b["count"] for b in by_operation.values())
    findings = []
    for op, bucket in sorted(by_operation.items(), key=lambda kv: -kv[1]["fee_stroops"]):
        share = bucket["fee_stroops"] / total_stroops
        reasons = []
        if share >= DOMINANT_SHARE:
            reasons.append(f"{share:.0%} of total spend; review batching or call frequency")
        if overall_avg and bucket["avg_stroops"] >= EXPENSIVE_FACTOR * overall_avg:
            reasons.append(
                f"average fee {bucket['avg_stroops'] / overall_avg:.1f}x the overall average; "
                "review the operation's footprint"
            )
        if reasons:
            findings.append({"operation": op, "reasons": reasons})
    return findings


def daily_totals(records):
    days = defaultdict(int)
    for rec in records:
        days[rec["_ts"].date()] += rec["fee_stroops"]
    return days


def forecast(records, today):
    """Linear trend over the last FORECAST_WINDOW_DAYS, projected forward.

    Days with no spend count as zero, so quiet periods lower the trend rather
    than being skipped. Returns None when there are too few days of data.
    """
    days = daily_totals(records)
    if len(days) < 2:
        return None
    start = today - dt.timedelta(days=FORECAST_WINDOW_DAYS - 1)
    xs, ys = [], []
    for i in range(FORECAST_WINDOW_DAYS):
        day = start + dt.timedelta(days=i)
        xs.append(i)
        ys.append(days.get(day, 0))
    if all(y == 0 for y in ys):
        return {"projected_stroops": 0, "daily_slope_stroops": 0.0}
    x_mean = statistics.fmean(xs)
    y_mean = statistics.fmean(ys)
    denom = sum((x - x_mean) ** 2 for x in xs)
    slope = sum((x - x_mean) * (y - y_mean) for x, y in zip(xs, ys)) / denom
    intercept = y_mean - slope * x_mean
    projected = 0.0
    for step in range(1, FORECAST_HORIZON_DAYS + 1):
        projected += max(0.0, intercept + slope * (FORECAST_WINDOW_DAYS - 1 + step))
    return {"projected_stroops": round(projected), "daily_slope_stroops": slope}


def budget_alerts(records, budgets, today):
    """Compare month-to-date spend per department with its monthly budget."""
    alerts = []
    month_start = today.replace(day=1)
    spent = defaultdict(int)
    for rec in records:
        if month_start <= rec["_ts"].date() <= today:
            spent[rec["department"]] += rec["fee_stroops"]
    for dept, cfg in sorted(budgets.items()):
        limit = int(cfg["monthly_stroops"])
        used = spent.get(dept, 0)
        fraction = used / limit if limit else 0.0
        if fraction >= BUDGET_BREACH:
            level = "BREACH"
        elif fraction >= BUDGET_WARN:
            level = "WARN"
        else:
            level = "OK"
        alerts.append({
            "department": dept,
            "level": level,
            "spent_stroops": used,
            "budget_stroops": limit,
            "used_fraction": fraction,
        })
    return alerts


def load_budgets(path):
    if not path:
        return {}
    with open(path, encoding="utf-8") as fh:
        data = json.load(fh)
    return data.get("departments", data)


def build_report(records, budgets, today):
    total = sum(r["fee_stroops"] for r in records)
    by_op = summarise_by(records, "operation")
    by_dept = summarise_by(records, "department")
    return {
        "as_of": format_ts(dt.datetime.combine(today, dt.time(23, 59, 59), tzinfo=dt.timezone.utc)),
        "records": len(records),
        "total_stroops": total,
        "total_xlm": total / STROOPS_PER_XLM,
        "by_operation": by_op,
        "by_department": by_dept,
        "optimisation_candidates": optimisation_candidates(by_op, total),
        "forecast": forecast(records, today),
        "budget_alerts": budget_alerts(records, budgets, today),
    }


def print_report(report):
    print(f"Records: {report['records']}   As of: {report['as_of']}")
    print(f"Total gas spend: {report['total_stroops']:,} stroops ({report['total_xlm']:.4f} XLM)")

    print("\nBy operation:")
    for op, b in sorted(report["by_operation"].items(), key=lambda kv: -kv[1]["fee_stroops"]):
        print(f"  {op:<32} calls={b['count']:<6} total={b['fee_stroops']:>14,}  avg={b['avg_stroops']:,.0f}")

    print("\nBy department:")
    for dept, b in sorted(report["by_department"].items(), key=lambda kv: -kv[1]["fee_stroops"]):
        print(f"  {dept:<32} calls={b['count']:<6} total={b['fee_stroops']:>14,}")

    print("\nOptimisation candidates:")
    if not report["optimisation_candidates"]:
        print("  none")
    for item in report["optimisation_candidates"]:
        print(f"  {item['operation']}:")
        for reason in item["reasons"]:
            print(f"    - {reason}")

    print(f"\nForecast ({FORECAST_HORIZON_DAYS} days):")
    fc = report["forecast"]
    if fc is None:
        print("  insufficient data (need at least 2 days of records)")
    else:
        print(f"  projected spend: {fc['projected_stroops']:,} stroops")
        print(f"  daily trend:     {fc['daily_slope_stroops']:+,.1f} stroops/day")

    print("\nBudgets (month to date):")
    if not report["budget_alerts"]:
        print("  no budgets configured")
    for alert in report["budget_alerts"]:
        print(f"  [{alert['level']:<6}] {alert['department']:<20} "
              f"{alert['spent_stroops']:,} / {alert['budget_stroops']:,} "
              f"({alert['used_fraction']:.0%})")


def write_prom(records, out_path):
    """Write the cumulative gas counter in Prometheus text exposition format."""
    totals = defaultdict(int)
    for rec in records:
        totals[(rec["operation"], rec["department"])] += rec["fee_stroops"]

    lines = [
        "# HELP mainstay_operation_gas_stroops_total Cumulative gas (fee) spend in stroops.",
        "# TYPE mainstay_operation_gas_stroops_total counter",
    ]
    for (op, dept), value in sorted(totals.items()):
        lines.append(
            f'mainstay_operation_gas_stroops_total{{operation="{_escape(op)}",'
            f'department="{_escape(dept)}"}} {value}'
        )
    tmp = out_path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        fh.write("\n".join(lines) + "\n")
    os.replace(tmp, out_path)
    return len(totals)


def _escape(value):
    return str(value).replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")


def build_parser():
    parser = argparse.ArgumentParser(description="Mainstay FinOps cost tracker")
    parser.add_argument("--ledger", default=default_ledger(), help="ledger path (JSONL)")
    sub = parser.add_subparsers(dest="command", required=True)

    rec = sub.add_parser("record", help="append a fee record")
    rec.add_argument("--operation", required=True)
    rec.add_argument("--department", default=UNASSIGNED)
    rec.add_argument("--fee-stroops", type=int, required=True)
    rec.add_argument("--timestamp", help="ISO 8601 UTC, e.g. 2026-10-16T12:00:00Z (default: now)")

    rep = sub.add_parser("report", help="print spend summary, forecast, and budget alerts")
    rep.add_argument("--budgets", help="JSON file: {\"departments\": {\"name\": {\"monthly_stroops\": N}}}")
    rep.add_argument("--as-of", help="YYYY-MM-DD to evaluate against (default: today UTC)")
    rep.add_argument("--json", action="store_true", help="emit the report as JSON")

    prom = sub.add_parser("export-prom", help="write a Prometheus textfile")
    prom.add_argument("--out", required=True)
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)

    if args.command == "record":
        when = parse_ts(args.timestamp) if args.timestamp else None
        entry = record_fee(args.ledger, args.operation, args.department, args.fee_stroops, when)
        print(json.dumps(entry, sort_keys=True))
        return 0

    records = load_ledger(args.ledger)

    if args.command == "report":
        today = dt.date.fromisoformat(args.as_of) if args.as_of else dt.datetime.now(dt.timezone.utc).date()
        report = build_report(records, load_budgets(args.budgets), today)
        if args.json:
            print(json.dumps(report, indent=2, default=str))
        else:
            print_report(report)
        breached = any(a["level"] == "BREACH" for a in report["budget_alerts"])
        return 2 if breached else 0

    if args.command == "export-prom":
        count = write_prom(records, args.out)
        print(f"wrote {count} series to {args.out}")
        return 0

    return 1


if __name__ == "__main__":
    sys.exit(main())
