"""Tests for scripts/finops/cost_tracker.py.

Run with:  python3 -m unittest discover -s scripts/finops -p 'test_*.py'
"""

import datetime as dt
import json
import os
import tempfile
import unittest

import cost_tracker as ct

UTC = dt.timezone.utc


def ts(day, hour=12):
    return dt.datetime(2026, 10, day, hour, 0, 0, tzinfo=UTC)


class CostTrackerTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.ledger = os.path.join(self._tmp.name, "ledger.jsonl")

    def tearDown(self):
        self._tmp.cleanup()

    def add(self, op, dept, fee, when):
        ct.record_fee(self.ledger, op, dept, fee, when)

    def records(self):
        return ct.load_ledger(self.ledger)

    def test_record_writes_jsonl_and_round_trips(self):
        entry = ct.record_fee(self.ledger, "submit_maintenance", "field-ops", 500, ts(1))
        self.assertEqual(entry["ts"], "2026-10-01T12:00:00Z")
        recs = self.records()
        self.assertEqual(len(recs), 1)
        self.assertEqual(recs[0]["fee_stroops"], 500)
        self.assertEqual(recs[0]["department"], "field-ops")

    def test_record_rejects_negative_fee_and_empty_operation(self):
        with self.assertRaises(ValueError):
            ct.record_fee(self.ledger, "op", "d", -1)
        with self.assertRaises(ValueError):
            ct.record_fee(self.ledger, "", "d", 1)

    def test_missing_department_defaults_to_unassigned(self):
        ct.record_fee(self.ledger, "op", "", 10, ts(1))
        self.assertEqual(self.records()[0]["department"], ct.UNASSIGNED)

    def test_malformed_lines_are_skipped(self):
        self.add("op", "d", 10, ts(1))
        with open(self.ledger, "a", encoding="utf-8") as fh:
            fh.write("not json\n")
            fh.write(json.dumps({"ts": "bad", "operation": "x", "fee_stroops": 1}) + "\n")
        self.assertEqual(len(self.records()), 1)

    def test_summary_by_operation_and_department(self):
        self.add("a", "x", 100, ts(1))
        self.add("a", "y", 300, ts(2))
        self.add("b", "x", 600, ts(3))
        recs = self.records()
        by_op = ct.summarise_by(recs, "operation")
        self.assertEqual(by_op["a"]["fee_stroops"], 400)
        self.assertEqual(by_op["a"]["count"], 2)
        self.assertEqual(by_op["a"]["avg_stroops"], 200)
        by_dept = ct.summarise_by(recs, "department")
        self.assertEqual(by_dept["x"]["fee_stroops"], 700)

    def test_optimisation_flags_dominant_and_expensive_operations(self):
        # "heavy" dominates spend and costs far more per call than the mean.
        for _ in range(2):
            self.add("heavy", "x", 10_000, ts(1))
        for _ in range(20):
            self.add("light", "x", 100, ts(1))
        recs = self.records()
        total = sum(r["fee_stroops"] for r in recs)
        findings = ct.optimisation_candidates(ct.summarise_by(recs, "operation"), total)
        flagged = {f["operation"]: f["reasons"] for f in findings}
        self.assertIn("heavy", flagged)
        self.assertNotIn("light", flagged)
        self.assertTrue(any("total spend" in r for r in flagged["heavy"]))

    def test_no_candidates_when_spend_is_even(self):
        # Five equal operations hold 20% each, below the 25% dominance threshold.
        for op in ("a", "b", "c", "d", "e"):
            self.add(op, "x", 100, ts(1))
        recs = self.records()
        total = sum(r["fee_stroops"] for r in recs)
        self.assertEqual(ct.optimisation_candidates(ct.summarise_by(recs, "operation"), total), [])

    def test_forecast_needs_two_days(self):
        self.add("a", "x", 100, ts(1))
        self.assertIsNone(ct.forecast(self.records(), dt.date(2026, 10, 16)))

    def test_forecast_projects_rising_trend_upward(self):
        # One extra stroop per day of spend, every day for the window.
        for i in range(ct.FORECAST_WINDOW_DAYS):
            day = dt.date(2026, 9, 17) + dt.timedelta(days=i)
            self.add("a", "x", 100 + 10 * i, dt.datetime.combine(day, dt.time(12), tzinfo=UTC))
        fc = ct.forecast(self.records(), dt.date(2026, 10, 16))
        self.assertGreater(fc["daily_slope_stroops"], 0)
        self.assertGreater(fc["projected_stroops"], 0)

    def test_forecast_flat_zero_spend(self):
        self.add("a", "x", 0, ts(1))
        self.add("a", "x", 0, ts(5))
        fc = ct.forecast(self.records(), dt.date(2026, 10, 16))
        self.assertEqual(fc["projected_stroops"], 0)

    def test_budget_levels(self):
        self.add("a", "ok", 500, ts(2))
        self.add("a", "warn", 850, ts(2))
        self.add("a", "breach", 1200, ts(2))
        # A record from last month must not count against this month's budget.
        self.add("a", "ok", 10_000, dt.datetime(2026, 9, 30, 12, tzinfo=UTC))
        budgets = {
            "ok": {"monthly_stroops": 1000},
            "warn": {"monthly_stroops": 1000},
            "breach": {"monthly_stroops": 1000},
        }
        alerts = {a["department"]: a for a in ct.budget_alerts(self.records(), budgets, dt.date(2026, 10, 16))}
        self.assertEqual(alerts["ok"]["level"], "OK")
        self.assertEqual(alerts["warn"]["level"], "WARN")
        self.assertEqual(alerts["breach"]["level"], "BREACH")

    def test_report_breach_is_reflected_in_totals(self):
        self.add("a", "d", 1_000_000, ts(3))
        report = ct.build_report(self.records(), {"d": {"monthly_stroops": 100}}, dt.date(2026, 10, 16))
        self.assertEqual(report["total_stroops"], 1_000_000)
        self.assertAlmostEqual(report["total_xlm"], 0.1)
        self.assertEqual(report["budget_alerts"][0]["level"], "BREACH")

    def test_prom_export_format_and_escaping(self):
        self.add('op"quoted', "field-ops", 42, ts(1))
        self.add("op_b", "field-ops", 8, ts(2))
        out = os.path.join(self._tmp.name, "gas.prom")
        self.assertEqual(ct.write_prom(self.records(), out), 2)
        text = open(out, encoding="utf-8").read()
        self.assertIn("# TYPE mainstay_operation_gas_stroops_total counter", text)
        self.assertIn('mainstay_operation_gas_stroops_total{operation="op\\"quoted",department="field-ops"} 42', text)
        self.assertIn('mainstay_operation_gas_stroops_total{operation="op_b",department="field-ops"} 8', text)

    def test_cli_record_then_report_as_json(self):
        rc = ct.main(["--ledger", self.ledger, "record", "--operation", "submit",
                      "--department", "lending", "--fee-stroops", "250",
                      "--timestamp", "2026-10-02T08:30:00Z"])
        self.assertEqual(rc, 0)
        self.assertEqual(ct.main(["--ledger", self.ledger, "report", "--as-of", "2026-10-16", "--json"]), 0)

    def test_cli_report_exit_code_signals_breach(self):
        self.add("a", "d", 5_000, ts(2))
        budgets = os.path.join(self._tmp.name, "budgets.json")
        with open(budgets, "w", encoding="utf-8") as fh:
            json.dump({"departments": {"d": {"monthly_stroops": 100}}}, fh)
        rc = ct.main(["--ledger", self.ledger, "report", "--budgets", budgets, "--as-of", "2026-10-16"])
        self.assertEqual(rc, 2)


if __name__ == "__main__":
    unittest.main()
