"""Tests for scripts/ops/jsonlog.py (#1729)."""

import contextlib
import datetime as dt
import io
import json
import os
import re
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.ops import config, jsonlog
from scripts.ops.jsonlog import LogError

REPO_ROOT = config.REPO_ROOT
T0 = dt.datetime(2026, 10, 16, 12, 0, 0, 123000, tzinfo=dt.timezone.utc)
SEED = "S" + "A" * 55  # shape-valid Stellar secret seed (56 chars, base32 alphabet)


def record_line(level, service, event, when, **fields) -> str:
    return jsonlog.format_record(level, service, event, now=when, **fields)


class FormatTests(unittest.TestCase):
    def test_required_fields_and_millisecond_timestamp(self):
        line = jsonlog.format_record("info", "snapshot", "create", "done", now=T0, counts={"assets": 2})
        record = json.loads(line)
        self.assertEqual(record["level"], "INFO")
        self.assertEqual(record["service"], "snapshot")
        self.assertEqual(record["event"], "create")
        self.assertEqual(record["message"], "done")
        self.assertEqual(record["ts"], "2026-10-16T12:00:00.123+00:00")
        self.assertEqual(record["counts"], {"assets": 2})

    def test_one_record_is_exactly_one_line(self):
        line = jsonlog.format_record("INFO", "svc", "evt", message="a\nb", now=T0)
        self.assertNotIn("\n", line)

    def test_extra_fields_cannot_override_required_fields(self):
        line = jsonlog.format_record("INFO", "svc", "evt", now=T0, level="DEBUG", service="other")
        record = json.loads(line)
        self.assertEqual(record["level"], "INFO")
        self.assertEqual(record["service"], "svc")

    def test_unknown_level_and_missing_identity_are_rejected(self):
        with self.assertRaisesRegex(LogError, "unknown level"):
            jsonlog.format_record("TRACE", "svc", "evt", now=T0)
        with self.assertRaisesRegex(LogError, "required"):
            jsonlog.format_record("INFO", "", "evt", now=T0)

    def test_secret_looking_keys_are_redacted(self):
        line = jsonlog.format_record(
            "INFO", "svc", "evt", now=T0, admin_secret="hunter2", api_token="abc", nested={"password": "x", "ok": 1}
        )
        record = json.loads(line)
        self.assertEqual(record["admin_secret"], jsonlog.REDACTED)
        self.assertEqual(record["api_token"], jsonlog.REDACTED)
        self.assertEqual(record["nested"], {"password": jsonlog.REDACTED, "ok": 1})

    def test_stellar_seeds_in_values_are_redacted(self):
        line = jsonlog.format_record("INFO", "svc", "evt", message=f"key was {SEED}", now=T0)
        self.assertNotIn(SEED, line)
        self.assertIn(jsonlog.REDACTED, line)


class ParseTests(unittest.TestCase):
    def test_valid_record_parses(self):
        record = jsonlog.parse_line(record_line("WARNING", "canary", "rolled_back", T0))
        self.assertEqual(record["level"], "WARNING")

    def test_invalid_records_are_rejected_with_reason(self):
        with self.assertRaisesRegex(LogError, "not JSON"):
            jsonlog.parse_line("plain text")
        with self.assertRaisesRegex(LogError, "not a JSON object"):
            jsonlog.parse_line("[1, 2]")
        with self.assertRaisesRegex(LogError, "missing fields: service, event"):
            jsonlog.parse_line('{"ts": "x", "level": "INFO"}')
        with self.assertRaisesRegex(LogError, "unknown level"):
            jsonlog.parse_line('{"ts": "x", "level": "NOPE", "service": "s", "event": "e"}')


class SearchTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self._tmp.name)
        self.file = self.dir / "ops.jsonl"
        lines = [
            record_line("DEBUG", "config", "show", T0),
            record_line("INFO", "snapshot", "create", T0 + dt.timedelta(hours=1)),
            record_line("WARNING", "canary", "rolled_back", T0 + dt.timedelta(hours=2), reason="p95 too high"),
            record_line("ERROR", "canary", "advance", T0 + dt.timedelta(hours=3), message="rpc timeout"),
            "this line is not json",
            "",
            '{"ts": "2026-10-16T13:00:00+00:00", "level": "INFO"}',
        ]
        self.file.write_text("\n".join(lines) + "\n", encoding="utf-8")

    def tearDown(self):
        self._tmp.cleanup()

    def test_level_is_a_minimum_threshold(self):
        records, _ = jsonlog.search([self.file], level="WARNING")
        self.assertEqual({r["level"] for r in records}, {"WARNING", "ERROR"})

    def test_service_and_event_filters(self):
        records, _ = jsonlog.search([self.file], service="canary", event="advance")
        self.assertEqual([r["event"] for r in records], ["advance"])

    def test_time_window_filters(self):
        since = jsonlog._parse_bound("2026-10-16T12:30:00Z", "since")
        until = jsonlog._parse_bound("2026-10-16T14:30:00Z", "until")
        records, _ = jsonlog.search([self.file], since=since, until=until)
        self.assertEqual([r["service"] for r in records], ["snapshot", "canary"])

    def test_substring_search_matches_message_and_fields(self):
        records, _ = jsonlog.search([self.file], contains="p95 too high")
        self.assertEqual(len(records), 1)
        records, _ = jsonlog.search([self.file], contains="rpc timeout")
        self.assertEqual(len(records), 1)

    def test_malformed_lines_are_counted_not_returned(self):
        records, malformed = jsonlog.search([self.file])
        self.assertEqual(len(records), 4)
        self.assertEqual(malformed, 2)

    def test_limit_stops_early(self):
        records, _ = jsonlog.search([self.file], limit=2)
        self.assertEqual(len(records), 2)

    def test_bad_time_bound_is_reported(self):
        with self.assertRaisesRegex(LogError, "must be an ISO-8601"):
            jsonlog.search([self.file], since=jsonlog._parse_bound("yesterday-ish", "--since"))

    def test_cli_prints_matches_and_reports_skipped_lines(self):
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            code = jsonlog.main(["search", str(self.file), "--service", "canary"])
        self.assertEqual(code, 0)
        printed = [json.loads(line) for line in stdout.getvalue().splitlines()]
        self.assertEqual([r["event"] for r in printed], ["rolled_back", "advance"])
        self.assertIn("skipped 2 malformed", stderr.getvalue())

    def test_search_reads_stdin_when_path_is_dash(self):
        content = record_line("INFO", "svc", "evt", T0) + "\n"
        with mock.patch("sys.stdin", new=io.StringIO(content)):
            records, malformed = jsonlog.search(["-"])
        self.assertEqual((len(records), malformed), (1, 0))


class RetentionTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self._tmp.name)
        now = dt.datetime(2026, 10, 16, tzinfo=dt.timezone.utc).timestamp()
        self.now = dt.datetime(2026, 10, 16, tzinfo=dt.timezone.utc)
        for name, age_days in [("old.jsonl", 40), ("recent.jsonl", 3), ("notes.txt", 400)]:
            path = self.dir / name
            path.write_text("{}\n", encoding="utf-8")
            os.utime(path, (now - age_days * 86400, now - age_days * 86400))

    def tearDown(self):
        self._tmp.cleanup()

    def test_files_older_than_window_are_removed_and_only_jsonl_is_touched(self):
        removed = jsonlog.apply_retention(self.dir, retention_days=30, now=self.now)
        self.assertEqual([p.name for p in removed], ["old.jsonl"])
        self.assertFalse((self.dir / "old.jsonl").exists())
        self.assertTrue((self.dir / "recent.jsonl").exists())
        self.assertTrue((self.dir / "notes.txt").exists())

    def test_dry_run_reports_without_deleting(self):
        removed = jsonlog.apply_retention(self.dir, retention_days=30, now=self.now, dry_run=True)
        self.assertEqual(len(removed), 1)
        self.assertTrue((self.dir / "old.jsonl").exists())

    def test_prune_cli_takes_retention_from_environment(self):
        # mainnet keeps 365 days, so a 40-day-old file survives.
        self.assertEqual(jsonlog.main(["prune", str(self.dir), "--env", "mainnet", "--dry-run"]), 0)
        self.assertTrue((self.dir / "old.jsonl").exists())

    def test_retention_must_be_positive(self):
        with self.assertRaises(LogError):
            jsonlog.apply_retention(self.dir, retention_days=0, now=self.now)


class EmitterIntegrationTests(unittest.TestCase):
    def test_log_event_is_a_noop_without_log_file(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            jsonlog.log_event("INFO", "svc", "evt")  # must not raise or write

    def test_log_event_appends_to_configured_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / "logs" / "ops.jsonl"
            with mock.patch.dict(os.environ, {jsonlog.LOG_FILE_VAR: str(target)}):
                jsonlog.log_event("WARNING", "canary", "rolled_back", "p95 high", percent=25)
                jsonlog.log_event("INFO", "snapshot", "create")
            records, malformed = jsonlog.search([target])
        self.assertEqual(malformed, 0)
        self.assertEqual([r["service"] for r in records], ["canary", "snapshot"])
        self.assertEqual(records[0]["percent"], 25)

    def test_canary_rollback_emits_warning_record(self):
        from scripts.ops import canary

        state = canary.CanaryState(environment="staging", stable_contract_id="S", canary_contract_id="C", percent=25)
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / "ops.jsonl"
            with mock.patch.dict(os.environ, {jsonlog.LOG_FILE_VAR: str(target)}):
                canary.rollback(state, reason="error rate too high", save=lambda s: None)
            records, _ = jsonlog.search([target], level="WARNING")
        self.assertEqual(records[0]["event"], "rolled_back")
        self.assertEqual(records[0]["canary_contract_id"], "C")


class MonitoringConfigTests(unittest.TestCase):
    """Keeps the Loki retention in step with the mainnet retention policy."""

    def test_loki_retention_matches_mainnet_policy(self):
        text = (REPO_ROOT / "monitoring" / "logging" / "loki-config.yml").read_text(encoding="utf-8")
        match = re.search(r"^\s*retention_period:\s*(\d+)h\s*$", text, re.MULTILINE)
        self.assertIsNotNone(match, "loki-config.yml must set limits_config.retention_period in hours")
        mainnet_days = config.load_environments()["mainnet"].logging.retention_days
        self.assertEqual(int(match.group(1)), mainnet_days * 24)

    def test_fluent_bit_reads_the_directory_the_tools_write_to(self):
        conf = (REPO_ROOT / "monitoring" / "logging" / "fluent-bit.conf").read_text(encoding="utf-8")
        self.assertIn("Path              /var/log/mainstay/*.jsonl", conf)
        self.assertIn("Parser            mainstay_json", conf)
        parsers = (REPO_ROOT / "monitoring" / "logging" / "parsers.conf").read_text(encoding="utf-8")
        self.assertIn("Name          mainstay_json", parsers)
        self.assertIn("Time_Format   %Y-%m-%dT%H:%M:%S.%L%z", parsers)


if __name__ == "__main__":
    unittest.main()
