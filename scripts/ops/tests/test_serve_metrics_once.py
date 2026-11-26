"""Tests for scripts/monitoring/serve-metrics-once.sh (issue #2022).

The script answers one metrics scrape. A failed collection (METRICS_FILE
unset, missing or unreadable) must exit non-zero and must not write any part of
an HTTP 200 response, so it can never look like a successful scrape.
"""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = (
    Path(__file__).resolve().parents[2] / "monitoring" / "serve-metrics-once.sh"
)
BASH = shutil.which("bash")
STATUS_LINE = b"HTTP/1.1 200 OK"


@unittest.skipIf(BASH is None, "bash is required to run the metrics script")
class ServeMetricsOnceTest(unittest.TestCase):
    def run_script(self, metrics_file=None):
        env = {k: v for k, v in os.environ.items() if k != "METRICS_FILE"}
        if metrics_file is not None:
            env["METRICS_FILE"] = str(metrics_file)
        return subprocess.run(
            [BASH, str(SCRIPT)],
            env=env,
            capture_output=True,
            timeout=30,
            check=False,
        )

    def test_script_enables_strict_mode(self):
        lines = SCRIPT.read_text(encoding="utf-8").splitlines()
        self.assertIn("set -euo pipefail", [line.strip() for line in lines])

    def test_serves_metrics_with_200_and_exact_body(self):
        body = b"# HELP up Exporter up\nmainstay_up 1\n\n"
        with tempfile.TemporaryDirectory() as tmp:
            metrics = Path(tmp) / "mainstay-metrics.prom"
            metrics.write_bytes(body)
            result = self.run_script(metrics)

        self.assertEqual(result.returncode, 0, result.stderr)
        header, sep, served = result.stdout.partition(b"\r\n\r\n")
        self.assertEqual(sep, b"\r\n\r\n")
        self.assertTrue(header.startswith(STATUS_LINE))
        self.assertIn(b"Content-Type: text/plain", header)
        # Byte-exact, including the trailing blank line.
        self.assertEqual(served, body)

    def test_missing_metrics_file_fails_without_200(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = self.run_script(Path(tmp) / "does-not-exist.prom")

        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn(STATUS_LINE, result.stdout)
        self.assertEqual(result.stdout, b"")

    def test_unreadable_metrics_path_fails_without_200(self):
        # A directory cannot be read with cat, which simulates a collection
        # that left something unusable at METRICS_FILE.
        with tempfile.TemporaryDirectory() as tmp:
            result = self.run_script(Path(tmp))

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, b"")

    def test_unset_metrics_file_fails_with_clear_message(self):
        result = self.run_script()

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, b"")
        self.assertIn(b"METRICS_FILE is not set", result.stderr)


if __name__ == "__main__":
    unittest.main()
