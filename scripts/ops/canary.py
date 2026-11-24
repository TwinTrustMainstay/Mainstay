"""Canary rollout for contract upgrades with health gates and rollback (#1728).

A canary is a second contract instance deployed from the new WASM. A traffic
router (client or API layer, outside this repository) sends `percent` of
requests to the canary instance by reading the state file. This module
ramps that percentage through the stages in environments.toml, checks health
metrics at each stage, rolls back to 0% on degradation, and stops before 100%
until an approval-gated run resumes it.

Usage (from the repository root):

    python3 -m scripts.ops.canary start --env staging \\
        --stable-id <C...> --canary-id <C...>
    python3 -m scripts.ops.canary advance --env staging --metrics-url https://…/metrics.json
    python3 -m scripts.ops.canary advance --env staging --metrics-url … --approve-final
    python3 -m scripts.ops.canary status
    python3 -m scripts.ops.canary rollback --reason "manual"

Metrics endpoint: a JSON document with `total`, `failures`, `p95_latency_ms`
counted over the observation window for the canary instance.

Exit codes for `advance`: 0 promoted or held, 1 rolled back, 2 awaiting approval.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import sys
import time
import urllib.request
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Callable

from scripts.ops import config
from scripts.ops.jsonlog import log_event

REPO_ROOT = config.REPO_ROOT
DEFAULT_STATE_PATH = REPO_ROOT / "deploy" / "canary-state.json"
MAX_INCONCLUSIVE_OBSERVATIONS = 3
FINAL_STAGE = 100

STATUS_IDLE = "idle"
STATUS_OBSERVING = "observing"
STATUS_AWAITING_APPROVAL = "awaiting_approval"
STATUS_PROMOTED = "promoted"
STATUS_ROLLED_BACK = "rolled_back"


class CanaryError(Exception):
    """Raised for invalid rollout state or transitions."""


@dataclass(frozen=True)
class Metrics:
    total: int
    failures: int
    p95_latency_ms: float

    @property
    def error_rate(self) -> float:
        return self.failures / self.total if self.total else 0.0


@dataclass(frozen=True)
class Verdict:
    status: str  # "healthy" | "degraded" | "inconclusive"
    reasons: tuple[str, ...]


def evaluate(metrics: Metrics, thresholds: config.Canary) -> Verdict:
    """Judge one observation window against the environment's thresholds.

    Too few samples is `inconclusive` rather than healthy: a stage is never
    promoted on no evidence.
    """
    if metrics.total < thresholds.min_samples:
        return Verdict("inconclusive", (f"{metrics.total} samples < min_samples {thresholds.min_samples}",))
    reasons = []
    if metrics.error_rate > thresholds.max_error_rate:
        reasons.append(f"error rate {metrics.error_rate:.4f} > max {thresholds.max_error_rate}")
    if metrics.p95_latency_ms > thresholds.max_p95_latency_ms:
        reasons.append(f"p95 {metrics.p95_latency_ms}ms > max {thresholds.max_p95_latency_ms}ms")
    if reasons:
        return Verdict("degraded", tuple(reasons))
    return Verdict("healthy", ())


@dataclass
class CanaryState:
    environment: str
    stable_contract_id: str
    canary_contract_id: str
    percent: int = 0
    status: str = STATUS_IDLE
    history: list[dict[str, Any]] = field(default_factory=list)

    def record(self, event: str, **details: Any) -> None:
        self.history.append({"ts": _now(), "event": event, "percent": self.percent, **details})


def _now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def save_state(state: CanaryState, path: Path) -> None:
    """Write state atomically so a router never reads a half-written file."""
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(asdict(state), indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(tmp, path)


def load_state(path: Path) -> CanaryState:
    if not path.exists():
        raise CanaryError(f"no canary state at {path}; run 'start' first")
    return CanaryState(**json.loads(path.read_text(encoding="utf-8")))


def next_stage(stages: tuple[int, ...], current: int) -> int | None:
    for stage in stages:
        if stage > current:
            return stage
    return None


def run_rollout(
    state: CanaryState,
    canary: config.Canary,
    *,
    observe: Callable[[int], Metrics],
    save: Callable[[CanaryState], None],
    approve_final: bool,
    sleep: Callable[[float], None] = time.sleep,
) -> CanaryState:
    """Advance the rollout by one step per stage until it holds, promotes or rolls back.

    Stops before the final (100%) stage unless `approve_final` is set. Each
    stage is observed for `observe_secs`. An inconclusive observation is retried
    up to MAX_INCONCLUSIVE_OBSERVATIONS times, then treated as a failure.
    """
    if state.status == STATUS_PROMOTED:
        raise CanaryError("rollout already promoted to 100%")
    if state.status == STATUS_ROLLED_BACK:
        raise CanaryError("rollout was rolled back; start a new canary for this build")

    # A run interrupted mid-observation resumes by re-observing that stage,
    # so a stage is never skipped without its health check.
    cursor = state.percent - 1 if state.status == STATUS_OBSERVING else state.percent
    while (stage := next_stage(canary.stages, cursor)) is not None:
        if stage == FINAL_STAGE and not approve_final:
            state.status = STATUS_AWAITING_APPROVAL
            state.record("awaiting_approval", next_percent=stage)
            save(state)
            return state

        state.percent = stage
        state.status = STATUS_OBSERVING
        state.record("stage_started")
        save(state)
        cursor = stage

        verdict = Verdict("inconclusive", ())
        for attempt in range(1, MAX_INCONCLUSIVE_OBSERVATIONS + 1):
            if canary.observe_secs:
                sleep(canary.observe_secs)
            verdict = evaluate(observe(stage), canary)
            state.record("observed", attempt=attempt, verdict=verdict.status, reasons=list(verdict.reasons))
            save(state)
            if verdict.status != "inconclusive":
                break

        if verdict.status == "inconclusive":
            reason = f"inconclusive after {MAX_INCONCLUSIVE_OBSERVATIONS} observations: {'; '.join(verdict.reasons)}"
            return rollback(state, reason=reason, save=save)
        if verdict.status == "degraded":
            return rollback(state, reason="; ".join(verdict.reasons), save=save)

        state.record("stage_passed")
        save(state)

    state.status = STATUS_PROMOTED
    state.record("promoted")
    save(state)
    return state


def rollback(state: CanaryState, *, reason: str, save: Callable[[CanaryState], None]) -> CanaryState:
    state.percent = 0
    state.status = STATUS_ROLLED_BACK
    state.record("rolled_back", reason=reason)
    save(state)
    log_event("WARNING", "canary", "rolled_back", f"canary rolled back: {reason}",
              environment=state.environment, canary_contract_id=state.canary_contract_id)
    return state


def fetch_metrics(url: str, timeout: float = 10.0) -> Metrics:
    with urllib.request.urlopen(url, timeout=timeout) as resp:  # noqa: S310 - operator-supplied URL
        body = json.loads(resp.read().decode("utf-8"))
    try:
        return Metrics(
            total=int(body["total"]),
            failures=int(body["failures"]),
            p95_latency_ms=float(body["p95_latency_ms"]),
        )
    except (KeyError, TypeError, ValueError) as exc:
        raise CanaryError(f"metrics payload from {url} is missing or malformed: {exc}") from exc


def _cmd_start(args: argparse.Namespace) -> int:
    env = config.get_environment(args.env, args.config)
    if args.state.exists() and load_state(args.state).status in (STATUS_OBSERVING, STATUS_AWAITING_APPROVAL):
        raise CanaryError("a rollout is already in progress; roll it back or finish it first")
    state = CanaryState(
        environment=env.name,
        stable_contract_id=args.stable_id,
        canary_contract_id=args.canary_id,
    )
    state.record("started", stages=list(env.canary.stages))
    save_state(state, args.state)
    print(f"started canary for {env.name}: stages {list(env.canary.stages)}")
    return 0


def _cmd_advance(args: argparse.Namespace) -> int:
    env = config.get_environment(args.env, args.config)
    state = load_state(args.state)
    if state.environment != env.name:
        raise CanaryError(f"state is for '{state.environment}', not '{env.name}'")
    if not args.metrics_url:
        raise CanaryError("--metrics-url is required to observe the canary")
    metrics_url = args.metrics_url
    state = run_rollout(
        state,
        env.canary,
        observe=lambda _pct: fetch_metrics(metrics_url),
        save=lambda s: save_state(s, args.state),
        approve_final=args.approve_final,
    )
    print(f"status={state.status} percent={state.percent}")
    return {STATUS_ROLLED_BACK: 1, STATUS_AWAITING_APPROVAL: 2}.get(state.status, 0)


def _cmd_status(args: argparse.Namespace) -> int:
    print(json.dumps(asdict(load_state(args.state)), indent=2, sort_keys=True))
    return 0


def _cmd_rollback(args: argparse.Namespace) -> int:
    state = load_state(args.state)
    if state.status == STATUS_PROMOTED:
        raise CanaryError("refusing to roll back a promoted rollout from here; redeploy the previous WASM")
    rollback(state, reason=args.reason, save=lambda s: save_state(s, args.state))
    print(f"rolled back to 0% ({args.reason})")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="python3 -m scripts.ops.canary")
    parser.add_argument("--state", type=Path, default=DEFAULT_STATE_PATH, help="canary state file read by the traffic router")
    parser.add_argument("--config", type=Path, default=None, help="environments file (default: environments.toml)")
    sub = parser.add_subparsers(dest="command", required=True)

    p_start = sub.add_parser("start", help="begin a canary rollout")
    p_start.add_argument("--env", default=None)
    p_start.add_argument("--stable-id", required=True)
    p_start.add_argument("--canary-id", required=True)
    p_start.set_defaults(func=_cmd_start)

    p_advance = sub.add_parser("advance", help="observe and advance to the next stage")
    p_advance.add_argument("--env", default=None)
    p_advance.add_argument("--metrics-url", default=os.environ.get("CANARY_METRICS_URL"))
    p_advance.add_argument("--approve-final", action="store_true", help="allow promotion to 100%% (approval-gated)")
    p_advance.set_defaults(func=_cmd_advance)

    p_status = sub.add_parser("status", help="print the current rollout state")
    p_status.set_defaults(func=_cmd_status)

    p_rollback = sub.add_parser("rollback", help="send all traffic back to the stable contract")
    p_rollback.add_argument("--reason", required=True)
    p_rollback.set_defaults(func=_cmd_rollback)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        code = args.func(args)
    except (CanaryError, config.ConfigError) as exc:
        print(f"canary error: {exc}", file=sys.stderr)
        log_event("ERROR", "canary", args.command, str(exc))
        return 1
    log_event("INFO", "canary", args.command, exit_code=code)
    return code


if __name__ == "__main__":
    sys.exit(main())
