"""Tests for scripts/ops/canary.py (#1728)."""

import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.ops import canary, config
from scripts.ops.canary import CanaryError, CanaryState, Metrics
from scripts.ops.tests.test_config import env_toml

THRESHOLDS = config.Canary(
    stages=(5, 25, 50, 100),
    max_error_rate=0.02,
    max_p95_latency_ms=3000,
    min_samples=20,
    observe_secs=0,
)

HEALTHY = Metrics(total=1000, failures=5, p95_latency_ms=800)
DEGRADED = Metrics(total=1000, failures=200, p95_latency_ms=800)
SLOW = Metrics(total=1000, failures=0, p95_latency_ms=9000)
THIN = Metrics(total=3, failures=0, p95_latency_ms=100)


def new_state() -> CanaryState:
    return CanaryState(environment="staging", stable_contract_id="CSTABLE", canary_contract_id="CCANARY")


class Recorder:
    """Collects every persisted snapshot of the state, in order."""

    def __init__(self):
        self.saved: list[tuple[int, str]] = []

    def __call__(self, state: CanaryState) -> None:
        self.saved.append((state.percent, state.status))


def observe_with(sequence):
    values = iter(sequence)
    return lambda _pct: next(values)


class EvaluationTests(unittest.TestCase):
    def test_healthy_window_passes(self):
        self.assertEqual(canary.evaluate(HEALTHY, THRESHOLDS).status, "healthy")

    def test_error_rate_above_budget_degrades(self):
        verdict = canary.evaluate(DEGRADED, THRESHOLDS)
        self.assertEqual(verdict.status, "degraded")
        self.assertIn("error rate", verdict.reasons[0])

    def test_latency_above_budget_degrades(self):
        verdict = canary.evaluate(SLOW, THRESHOLDS)
        self.assertEqual(verdict.status, "degraded")
        self.assertIn("p95", verdict.reasons[0])

    def test_too_few_samples_is_inconclusive_not_healthy(self):
        self.assertEqual(canary.evaluate(THIN, THRESHOLDS).status, "inconclusive")

    def test_zero_traffic_has_zero_error_rate_but_is_inconclusive(self):
        empty = Metrics(total=0, failures=0, p95_latency_ms=0)
        self.assertEqual(empty.error_rate, 0.0)
        self.assertEqual(canary.evaluate(empty, THRESHOLDS).status, "inconclusive")


class RolloutTests(unittest.TestCase):
    def test_full_ramp_with_approval_promotes_through_every_stage(self):
        state = new_state()
        rec = Recorder()
        canary.run_rollout(state, THRESHOLDS, observe=lambda _p: HEALTHY, save=rec, approve_final=True)
        self.assertEqual(state.status, canary.STATUS_PROMOTED)
        self.assertEqual(state.percent, 100)
        stages_started = [s.get("event") for s in state.history].count("stage_started")
        self.assertEqual(stages_started, 4)

    def test_rollout_stops_before_100_without_approval(self):
        state = new_state()
        canary.run_rollout(state, THRESHOLDS, observe=lambda _p: HEALTHY, save=Recorder(), approve_final=False)
        self.assertEqual(state.status, canary.STATUS_AWAITING_APPROVAL)
        self.assertEqual(state.percent, 50)

    def test_approval_resumes_from_the_held_stage(self):
        state = new_state()
        canary.run_rollout(state, THRESHOLDS, observe=lambda _p: HEALTHY, save=Recorder(), approve_final=False)
        canary.run_rollout(state, THRESHOLDS, observe=lambda _p: HEALTHY, save=Recorder(), approve_final=True)
        self.assertEqual(state.status, canary.STATUS_PROMOTED)
        self.assertEqual(state.percent, 100)

    def test_degraded_stage_rolls_back_to_zero_percent(self):
        state = new_state()
        observations = iter([HEALTHY, DEGRADED])
        canary.run_rollout(
            state, THRESHOLDS, observe=lambda _p: next(observations), save=Recorder(), approve_final=True
        )
        self.assertEqual(state.status, canary.STATUS_ROLLED_BACK)
        self.assertEqual(state.percent, 0)
        self.assertEqual(state.history[-1]["event"], "rolled_back")
        self.assertIn("error rate", state.history[-1]["reason"])

    def test_inconclusive_observations_are_retried_then_treated_as_failure(self):
        state = new_state()
        calls = []

        def observe(pct):
            calls.append(pct)
            return THIN

        canary.run_rollout(state, THRESHOLDS, observe=observe, save=Recorder(), approve_final=True)
        self.assertEqual(state.status, canary.STATUS_ROLLED_BACK)
        self.assertEqual(len(calls), canary.MAX_INCONCLUSIVE_OBSERVATIONS)
        self.assertIn("inconclusive", state.history[-1]["reason"])

    def test_inconclusive_then_healthy_still_promotes_the_stage(self):
        state = new_state()
        observations = iter([THIN, HEALTHY] + [HEALTHY] * 10)
        canary.run_rollout(
            state, THRESHOLDS, observe=lambda _p: next(observations), save=Recorder(), approve_final=True
        )
        self.assertEqual(state.status, canary.STATUS_PROMOTED)

    def test_every_stage_waits_for_observe_window(self):
        thresholds = config.Canary((5, 100), 0.02, 3000, 1, 300)
        sleeps = []
        canary.run_rollout(
            new_state(), thresholds, observe=lambda _p: HEALTHY, save=Recorder(),
            approve_final=True, sleep=sleeps.append,
        )
        self.assertEqual(sleeps, [300, 300])

    def test_interrupted_observation_resumes_by_reobserving_that_stage(self):
        state = new_state()
        state.percent = 25
        state.status = canary.STATUS_OBSERVING
        seen = []

        def observe(pct):
            seen.append(pct)
            return HEALTHY

        canary.run_rollout(state, THRESHOLDS, observe=observe, save=Recorder(), approve_final=True)
        self.assertEqual(seen[0], 25)
        self.assertEqual(state.status, canary.STATUS_PROMOTED)

    def test_state_is_persisted_at_every_transition(self):
        state = new_state()
        rec = Recorder()
        canary.run_rollout(state, THRESHOLDS, observe=lambda _p: HEALTHY, save=rec, approve_final=True)
        self.assertGreater(len(rec.saved), 8)
        self.assertEqual(rec.saved[-1], (100, canary.STATUS_PROMOTED))

    def test_promoted_or_rolled_back_rollouts_cannot_be_resumed(self):
        promoted = new_state()
        promoted.status = canary.STATUS_PROMOTED
        with self.assertRaisesRegex(CanaryError, "already promoted"):
            canary.run_rollout(promoted, THRESHOLDS, observe=lambda _p: HEALTHY, save=Recorder(), approve_final=True)
        rolled = new_state()
        rolled.status = canary.STATUS_ROLLED_BACK
        with self.assertRaisesRegex(CanaryError, "rolled back"):
            canary.run_rollout(rolled, THRESHOLDS, observe=lambda _p: HEALTHY, save=Recorder(), approve_final=True)

    def test_manual_rollback_clears_traffic(self):
        state = new_state()
        state.percent = 50
        canary.rollback(state, reason="operator", save=Recorder())
        self.assertEqual((state.percent, state.status), (0, canary.STATUS_ROLLED_BACK))


class StatePersistenceTests(unittest.TestCase):
    def test_state_round_trips_through_disk(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "deploy" / "canary-state.json"
            state = new_state()
            state.record("started")
            canary.save_state(state, path)
            loaded = canary.load_state(path)
        self.assertEqual(loaded, state)
        self.assertFalse(path.with_suffix(".json.tmp").exists())

    def test_missing_state_gives_actionable_error(self):
        with self.assertRaisesRegex(CanaryError, "run 'start' first"):
            canary.load_state(Path("/nonexistent/canary-state.json"))


class CliTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.state_path = Path(self._tmp.name) / "canary-state.json"
        # Zero observe window: the repository config waits minutes per stage.
        body = (env_toml("staging") + env_toml("testnet")).replace("observe_secs = 300", "observe_secs = 0")
        self.config_path = Path(self._tmp.name) / "environments.toml"
        self.config_path.write_text(body, encoding="utf-8")

    def tearDown(self):
        self._tmp.cleanup()

    def _run(self, *args):
        return canary.main(["--state", str(self.state_path), "--config", str(self.config_path), *args])

    def test_start_then_status_reports_stages(self):
        self.assertEqual(self._run("start", "--env", "staging", "--stable-id", "CS", "--canary-id", "CC"), 0)
        body = json.loads(self.state_path.read_text(encoding="utf-8"))
        self.assertEqual(body["status"], canary.STATUS_IDLE)
        self.assertEqual(body["canary_contract_id"], "CC")

    def test_advance_exit_codes_follow_rollout_outcome(self):
        self._run("start", "--env", "staging", "--stable-id", "CS", "--canary-id", "CC")
        healthy = Metrics(total=1000, failures=0, p95_latency_ms=100)
        with mock.patch.object(canary, "fetch_metrics", return_value=healthy):
            code_pending = self._run("advance", "--env", "staging", "--metrics-url", "http://m")
            self.assertEqual(code_pending, 2)
            self.assertEqual(
                json.loads(self.state_path.read_text())["status"], canary.STATUS_AWAITING_APPROVAL
            )
            code_done = self._run("advance", "--env", "staging", "--metrics-url", "http://m", "--approve-final")
        self.assertEqual(code_done, 0)
        self.assertEqual(json.loads(self.state_path.read_text())["status"], canary.STATUS_PROMOTED)

    def test_advance_exits_one_on_rollback(self):
        self._run("start", "--env", "staging", "--stable-id", "CS", "--canary-id", "CC")
        with mock.patch.object(canary, "fetch_metrics", return_value=DEGRADED):
            self.assertEqual(self._run("advance", "--env", "staging", "--metrics-url", "http://m"), 1)

    def test_advance_requires_metrics_source(self):
        self._run("start", "--env", "staging", "--stable-id", "CS", "--canary-id", "CC")
        with mock.patch.dict("os.environ", {}, clear=True):
            self.assertEqual(self._run("advance", "--env", "staging"), 1)

    def test_manual_rollback_refuses_promoted_rollouts(self):
        self._run("start", "--env", "staging", "--stable-id", "CS", "--canary-id", "CC")
        promoted = json.loads(self.state_path.read_text())
        promoted["status"] = canary.STATUS_PROMOTED
        self.state_path.write_text(json.dumps(promoted))
        self.assertEqual(self._run("rollback", "--reason", "incident"), 1)

    def test_start_refuses_to_clobber_an_active_rollout(self):
        self._run("start", "--env", "staging", "--stable-id", "CS", "--canary-id", "CC")
        active = json.loads(self.state_path.read_text())
        active["status"] = canary.STATUS_OBSERVING
        self.state_path.write_text(json.dumps(active))
        self.assertEqual(self._run("start", "--env", "staging", "--stable-id", "CS", "--canary-id", "CC2"), 1)

    def test_advance_refuses_state_for_a_different_environment(self):
        self._run("start", "--env", "staging", "--stable-id", "CS", "--canary-id", "CC")
        self.assertEqual(self._run("advance", "--env", "testnet", "--metrics-url", "http://m"), 1)


class MetricsParsingTests(unittest.TestCase):
    def test_malformed_metrics_payload_is_rejected(self):
        response = mock.MagicMock()
        response.__enter__.return_value.read.return_value = b'{"total": 5}'
        with mock.patch("urllib.request.urlopen", return_value=response):
            with self.assertRaisesRegex(CanaryError, "missing or malformed"):
                canary.fetch_metrics("http://m")


if __name__ == "__main__":
    unittest.main()
