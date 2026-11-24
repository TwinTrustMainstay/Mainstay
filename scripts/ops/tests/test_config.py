"""Tests for scripts/ops/config.py (#1726)."""

import datetime as dt
import json
import tempfile
import tomllib
import textwrap
import unittest
from pathlib import Path

from scripts.ops import config
from scripts.ops.config import ConfigError

REPO_CONFIG = config.DEFAULT_CONFIG_PATH

VALID_ENV_TOML = textwrap.dedent(
    """
    [{name}]
    rpc_url = "{rpc}"
    network_passphrase = "Test SDF Network ; September 2015"

    [{name}.limits]
    max_fee_stroops = 10000000
    max_batch_size = 25

    [{name}.timeouts]
    rpc_timeout_secs = 30
    tx_timeout_secs = 120

    [{name}.rates]
    max_requests_per_minute = 120
    max_submissions_per_hour = 20

    [{name}.logging]
    level = "INFO"
    retention_days = 30

    [{name}.canary]
    stages = [5, 25, 50, 100]
    max_error_rate = 0.02
    max_p95_latency_ms = 3000
    min_samples = 20
    observe_secs = 300
    """
)


def write_config(directory: str, body: str) -> Path:
    path = Path(directory) / "environments.toml"
    path.write_text(body, encoding="utf-8")
    return path


def env_toml(name: str, rpc: str = "https://rpc.example.org") -> str:
    return VALID_ENV_TOML.format(name=name, rpc=rpc)


class RepoConfigTests(unittest.TestCase):
    def test_repository_environments_file_is_fully_valid(self):
        environments = config.load_environments(REPO_CONFIG)
        for required in ("dev", "staging", "testnet", "mainnet"):
            self.assertIn(required, environments)

    def test_mainnet_is_more_conservative_than_testnet(self):
        environments = config.load_environments(REPO_CONFIG)
        self.assertGreater(
            environments["mainnet"].logging.retention_days,
            environments["testnet"].logging.retention_days,
        )
        self.assertLess(
            environments["mainnet"].canary.max_error_rate,
            environments["testnet"].canary.max_error_rate,
        )
        self.assertEqual(environments["mainnet"].canary.stages[-1], 100)


class SchemaTests(unittest.TestCase):
    def test_loads_valid_environment_and_converts_stages_to_tuple(self):
        with tempfile.TemporaryDirectory() as tmp:
            envs = config.load_environments(write_config(tmp, env_toml("staging")))
        self.assertEqual(envs["staging"].canary.stages, (5, 25, 50, 100))
        self.assertEqual(envs["staging"].limits.max_batch_size, 25)

    def test_missing_section_is_reported(self):
        body = env_toml("staging").replace("[staging.timeouts]", "[staging.other]")
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(ConfigError, "missing section: \\[staging.timeouts\\]"):
                config.load_environments(write_config(tmp, body))

    def test_unknown_key_is_rejected_to_catch_typos(self):
        body = env_toml("staging").replace("max_batch_size = 25", "max_batch_siz = 25")
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(ConfigError, "unknown keys: max_batch_siz"):
                config.load_environments(write_config(tmp, body))

    def test_all_problems_are_reported_together(self):
        body = env_toml("staging", rpc="http://remote.example.org").replace(
            "max_batch_size = 25", "max_batch_size = 0"
        )
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(ConfigError) as ctx:
                config.load_environments(write_config(tmp, body))
        message = str(ctx.exception)
        self.assertIn("rpc_url must use https", message)
        self.assertIn("max_batch_size must be a positive integer", message)

    def test_invalid_toml_is_a_config_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(ConfigError, "invalid TOML"):
                config.load_environments(write_config(tmp, "[staging\n"))

    def test_missing_file_is_a_config_error(self):
        with self.assertRaisesRegex(ConfigError, "config file not found"):
            config.load_environments(Path("/nonexistent/environments.toml"))


class ValidationRuleTests(unittest.TestCase):
    def _env(self, name: str, rpc: str = "https://rpc.example.org", **overrides) -> config.Environment:
        # Build without aggregate validation so tests can inspect invalid values.
        body = env_toml(name, rpc=rpc)
        for old, new in overrides.items():
            body = body.replace(old, new)
        return config._build_environment(name, tomllib.loads(body)[name])

    def test_rate_limit_cannot_be_disabled_outside_dev(self):
        env = self._env("staging", **{"max_submissions_per_hour = 20": "max_submissions_per_hour = 0"})
        self.assertIn(
            "staging.rates.max_submissions_per_hour must be > 0",
            config.validate_environment(env)[0],
        )

    def test_rate_limit_may_be_disabled_in_dev(self):
        env = self._env("dev", **{"max_submissions_per_hour = 20": "max_submissions_per_hour = 0"})
        self.assertEqual(config.validate_environment(env), [])

    def test_mainnet_requires_long_log_retention(self):
        env = self._env("mainnet", rpc="https://mainnet.example.org")
        errors = config.validate_environment(env)
        self.assertTrue(any("mainnet.logging.retention_days must be >= 90" in e for e in errors))

    def test_stages_must_be_increasing_and_end_at_100(self):
        bad_order = self._env("staging", **{"stages = [5, 25, 50, 100]": "stages = [25, 5, 100]"})
        self.assertTrue(any("strictly increasing" in e for e in config.validate_environment(bad_order)))
        not_full = self._env("staging", **{"stages = [5, 25, 50, 100]": "stages = [5, 25, 50]"})
        self.assertTrue(any("must end at 100" in e for e in config.validate_environment(not_full)))

    def test_localhost_http_only_allowed_for_local_environments(self):
        staging = self._env("staging", rpc="http://localhost:8000/soroban/rpc")
        self.assertTrue(any("rpc_url must use https" in e for e in config.validate_environment(staging)))
        dev = self._env("dev", rpc="http://localhost:8000/soroban/rpc")
        self.assertEqual(config.validate_environment(dev), [])

    def test_unknown_log_level_is_rejected(self):
        env = self._env("staging", **{'level = "INFO"': 'level = "TRACE"'})
        self.assertTrue(any("logging.level must be one of" in e for e in config.validate_environment(env)))


class ResolutionTests(unittest.TestCase):
    def test_environment_resolved_from_env_var_when_not_given(self):
        import os

        with tempfile.TemporaryDirectory() as tmp:
            path = write_config(tmp, env_toml("staging") + env_toml("testnet"))
            old = os.environ.get(config.ENV_NAME_VAR)
            os.environ[config.ENV_NAME_VAR] = "staging"
            try:
                self.assertEqual(config.get_environment(None, path).name, "staging")
            finally:
                if old is None:
                    del os.environ[config.ENV_NAME_VAR]
                else:
                    os.environ[config.ENV_NAME_VAR] = old

    def test_unknown_environment_lists_known_names(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = write_config(tmp, env_toml("staging"))
            with self.assertRaisesRegex(ConfigError, "unknown environment 'nope'.*staging"):
                config.get_environment("nope", path)


class DiffAndDigestTests(unittest.TestCase):
    def setUp(self):
        self.envs = config.load_environments(REPO_CONFIG)

    def test_diff_ignores_identity_fields_and_reports_real_changes(self):
        changes = config.diff_environments(self.envs["staging"], self.envs["mainnet"])
        self.assertNotIn("rpc_url", changes)
        self.assertNotIn("name", changes)
        self.assertIn("logging.retention_days", changes)
        self.assertEqual(changes["logging.retention_days"], (14, 365))

    def test_identical_environments_have_no_diff(self):
        self.assertEqual(config.diff_environments(self.envs["testnet"], self.envs["testnet"]), {})

    def test_digest_is_stable_and_changes_with_config(self):
        env = self.envs["testnet"]
        self.assertEqual(config.config_digest(env), config.config_digest(env))
        changed = config.Environment(**{**env.__dict__, "limits": config.Limits(1, 1)})
        self.assertNotEqual(config.config_digest(env), config.config_digest(changed))


class AuditTrailTests(unittest.TestCase):
    def test_audit_records_are_appended_as_jsonl(self):
        env = config.load_environments(REPO_CONFIG)["testnet"]
        when = dt.datetime(2026, 10, 16, 12, 0, tzinfo=dt.timezone.utc)
        with tempfile.TemporaryDirectory() as tmp:
            audit = Path(tmp) / "audit" / "config-audit.jsonl"
            config.append_audit(audit, action="show", env=env, actor="alice", now=when)
            config.append_audit(audit, action="show", env=env, actor="bob", now=when)
            records = config.read_audit(audit)
            raw_lines = audit.read_text(encoding="utf-8").splitlines()
        self.assertEqual([r["actor"] for r in records], ["alice", "bob"])
        self.assertEqual(records[0]["sha256"], config.config_digest(env))
        self.assertEqual(records[0]["environment"], "testnet")
        self.assertEqual(len(raw_lines), 2)
        self.assertEqual(json.loads(raw_lines[0])["action"], "show")

    def test_read_audit_on_missing_file_is_empty(self):
        self.assertEqual(config.read_audit(Path("/nonexistent/audit.jsonl")), [])


class CliTests(unittest.TestCase):
    def test_validate_all_succeeds_on_repo_config(self):
        self.assertEqual(config.main(["validate"]), 0)

    def test_validate_reports_failure_with_exit_code_one(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = write_config(tmp, env_toml("staging", rpc="http://remote.example.org"))
            self.assertEqual(config.main(["--file", str(path), "validate"]), 1)

    def test_diff_writes_audit_record_when_requested(self):
        with tempfile.TemporaryDirectory() as tmp:
            audit = Path(tmp) / "audit.jsonl"
            code = config.main(
                ["diff", "--from", "staging", "--to", "mainnet", "--audit-file", str(audit), "--actor", "ci"]
            )
            records = config.read_audit(audit)
        self.assertEqual(code, 0)
        self.assertEqual(records[0]["action"], "diff:staging->mainnet")
        self.assertEqual(records[0]["actor"], "ci")


if __name__ == "__main__":
    unittest.main()
