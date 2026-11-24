"""Environment-specific configuration for Mainstay operations tooling (#1726).

Loads ``environments.toml``, validates every environment against the schema,
diffs environments for safe promotion, and records config changes in an
append-only JSONL audit trail.

Usage (from the repository root):

    python3 -m scripts.ops.config validate [--env NAME | --env all]
    python3 -m scripts.ops.config show [--env NAME] [--audit-file PATH]
    python3 -m scripts.ops.config diff --from staging --to mainnet

Environment variables:

    MAINSTAY_ENV               Environment used when --env is omitted (default: testnet)
    MAINSTAY_ENVIRONMENTS_FILE Path to an external config file (default: environments.toml)
    GITHUB_ACTOR               Actor recorded in the audit trail (default: local user)
"""

from __future__ import annotations

import argparse
import datetime as dt
import getpass
import hashlib
import json
import os
import sys
import tomllib
from dataclasses import asdict, dataclass, fields
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CONFIG_PATH = REPO_ROOT / "environments.toml"
CONFIG_PATH_VAR = "MAINSTAY_ENVIRONMENTS_FILE"
ENV_NAME_VAR = "MAINSTAY_ENV"
DEFAULT_ENV = "testnet"

# Environments where a local (non-TLS) RPC endpoint is acceptable.
LOCAL_ENVS = frozenset({"dev", "standalone"})
# Environments where submission rate limiting must never be disabled.
RATE_LIMITED_ENVS = frozenset({"staging", "testnet", "futurenet", "mainnet"})
LOG_LEVELS = ("DEBUG", "INFO", "WARNING", "ERROR")
MAINNET_MIN_RETENTION_DAYS = 90
# Fields that legitimately differ between environments and are not promoted.
ENVIRONMENT_IDENTITY_FIELDS = frozenset({"name", "rpc_url", "network_passphrase"})


class ConfigError(Exception):
    """Raised when configuration is missing, malformed, or fails validation."""


@dataclass(frozen=True)
class Limits:
    max_fee_stroops: int
    max_batch_size: int


@dataclass(frozen=True)
class Timeouts:
    rpc_timeout_secs: int
    tx_timeout_secs: int


@dataclass(frozen=True)
class Rates:
    max_requests_per_minute: int
    max_submissions_per_hour: int


@dataclass(frozen=True)
class Logging:
    level: str
    retention_days: int


@dataclass(frozen=True)
class Canary:
    stages: tuple[int, ...]
    max_error_rate: float
    max_p95_latency_ms: int
    min_samples: int
    observe_secs: int


SECTION_TYPES: dict[str, type] = {
    "limits": Limits,
    "timeouts": Timeouts,
    "rates": Rates,
    "logging": Logging,
    "canary": Canary,
}


@dataclass(frozen=True)
class Environment:
    name: str
    rpc_url: str
    network_passphrase: str
    limits: Limits
    timeouts: Timeouts
    rates: Rates
    logging: Logging
    canary: Canary


def _build_section(env_name: str, section: str, cls: type, table: Any) -> Any:
    if not isinstance(table, dict):
        raise ConfigError(f"[{env_name}.{section}] must be a table")
    expected = {f.name for f in fields(cls)}
    problems = []
    if missing := sorted(expected - table.keys()):
        problems.append(f"missing keys: {', '.join(missing)}")
    if unknown := sorted(table.keys() - expected):
        problems.append(f"unknown keys: {', '.join(unknown)}")
    if problems:
        raise ConfigError(f"[{env_name}.{section}] " + "; ".join(problems))
    values = {k: tuple(v) if isinstance(v, list) else v for k, v in table.items()}
    return cls(**values)


def _build_environment(name: str, table: Any) -> Environment:
    if not isinstance(table, dict):
        raise ConfigError(f"[{name}] must be a table")
    for key in ("rpc_url", "network_passphrase"):
        if not isinstance(table.get(key), str) or not table.get(key):
            raise ConfigError(f"[{name}] missing string key: {key}")
    sections = {}
    for section, cls in SECTION_TYPES.items():
        if section not in table:
            raise ConfigError(f"[{name}] missing section: [{name}.{section}]")
        sections[section] = _build_section(name, section, cls, table[section])
    extra = sorted(set(table) - {"rpc_url", "network_passphrase"} - set(SECTION_TYPES))
    if extra:
        raise ConfigError(f"[{name}] unknown keys: {', '.join(extra)}")
    return Environment(
        name=name,
        rpc_url=table["rpc_url"],
        network_passphrase=table["network_passphrase"],
        **sections,
    )


def validate_environment(env: Environment) -> list[str]:
    """Return a list of human-readable validation errors (empty when valid)."""
    errors: list[str] = []

    def positive(section: str, key: str, value: int) -> None:
        if not isinstance(value, int) or value <= 0:
            errors.append(f"{env.name}.{section}.{key} must be a positive integer")

    positive("limits", "max_fee_stroops", env.limits.max_fee_stroops)
    positive("limits", "max_batch_size", env.limits.max_batch_size)
    positive("timeouts", "rpc_timeout_secs", env.timeouts.rpc_timeout_secs)
    positive("timeouts", "tx_timeout_secs", env.timeouts.tx_timeout_secs)
    positive("rates", "max_requests_per_minute", env.rates.max_requests_per_minute)
    positive("logging", "retention_days", env.logging.retention_days)

    if env.rates.max_submissions_per_hour < 0:
        errors.append(f"{env.name}.rates.max_submissions_per_hour must be >= 0")
    if env.name in RATE_LIMITED_ENVS and env.rates.max_submissions_per_hour == 0:
        errors.append(
            f"{env.name}.rates.max_submissions_per_hour must be > 0: 0 disables "
            "submission rate limiting, which is only allowed in dev"
        )

    if env.logging.level not in LOG_LEVELS:
        errors.append(f"{env.name}.logging.level must be one of {', '.join(LOG_LEVELS)}")
    if env.name == "mainnet" and env.logging.retention_days < MAINNET_MIN_RETENTION_DAYS:
        errors.append(
            f"mainnet.logging.retention_days must be >= {MAINNET_MIN_RETENTION_DAYS}"
        )

    is_https = env.rpc_url.startswith("https://")
    is_local_http = env.rpc_url.startswith("http://localhost")
    if not (is_https or (is_local_http and env.name in LOCAL_ENVS)):
        errors.append(
            f"{env.name}.rpc_url must use https:// (plain http is only allowed "
            "for localhost in dev/standalone)"
        )

    stages = env.canary.stages
    if not stages:
        errors.append(f"{env.name}.canary.stages must not be empty")
    else:
        if any(not isinstance(s, int) or not 1 <= s <= 100 for s in stages):
            errors.append(f"{env.name}.canary.stages must be integers in 1..=100")
        if list(stages) != sorted(set(stages)):
            errors.append(f"{env.name}.canary.stages must be strictly increasing")
        if stages[-1] != 100:
            errors.append(f"{env.name}.canary.stages must end at 100")
    if not 0 <= env.canary.max_error_rate <= 1:
        errors.append(f"{env.name}.canary.max_error_rate must be between 0 and 1")
    if env.name == "mainnet" and env.canary.max_error_rate > 0.05:
        errors.append("mainnet.canary.max_error_rate must be <= 0.05")
    if env.canary.min_samples < 1:
        errors.append(f"{env.name}.canary.min_samples must be >= 1")
    if env.canary.observe_secs < 0:
        errors.append(f"{env.name}.canary.observe_secs must be >= 0")

    return errors


def load_environments(path: Path | None = None) -> dict[str, Environment]:
    """Load and fully validate every environment. Raises ConfigError on any problem."""
    path = Path(path or os.environ.get(CONFIG_PATH_VAR) or DEFAULT_CONFIG_PATH)
    try:
        raw = tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise ConfigError(f"config file not found: {path}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise ConfigError(f"invalid TOML in {path}: {exc}") from exc

    environments: dict[str, Environment] = {}
    errors: list[str] = []
    for name, table in raw.items():
        try:
            env = _build_environment(name, table)
        except ConfigError as exc:
            errors.append(str(exc))
            continue
        errors.extend(validate_environment(env))
        environments[name] = env

    if errors:
        raise ConfigError("invalid configuration:\n  - " + "\n  - ".join(errors))
    return environments


def resolve_environment_name(name: str | None = None) -> str:
    return name or os.environ.get(ENV_NAME_VAR) or DEFAULT_ENV


def get_environment(name: str | None = None, path: Path | None = None) -> Environment:
    resolved = resolve_environment_name(name)
    environments = load_environments(path)
    if resolved not in environments:
        known = ", ".join(sorted(environments))
        raise ConfigError(f"unknown environment '{resolved}' (known: {known})")
    return environments[resolved]


def config_digest(env: Environment) -> str:
    """SHA-256 over the canonical JSON form of one environment's configuration."""
    canonical = json.dumps(asdict(env), sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def diff_environments(source: Environment, target: Environment) -> dict[str, tuple[Any, Any]]:
    """Return promotable fields that differ, as {"section.key": (source, target)}.

    Identity fields (name, rpc_url, network_passphrase) are excluded because they
    are expected to differ between environments.
    """
    src, dst = asdict(source), asdict(target)
    changes: dict[str, tuple[Any, Any]] = {}
    for key in src:
        if key in ENVIRONMENT_IDENTITY_FIELDS:
            continue
        if isinstance(src[key], dict):
            for sub in src[key]:
                if src[key][sub] != dst[key][sub]:
                    changes[f"{key}.{sub}"] = (src[key][sub], dst[key][sub])
        elif src[key] != dst[key]:
            changes[key] = (src[key], dst[key])
    return changes


def append_audit(
    audit_path: Path,
    *,
    action: str,
    env: Environment,
    actor: str | None = None,
    now: dt.datetime | None = None,
) -> dict[str, str]:
    """Append one audit record (JSONL). Returns the record that was written."""
    record = {
        "ts": (now or dt.datetime.now(dt.timezone.utc)).isoformat(),
        "action": action,
        "environment": env.name,
        "sha256": config_digest(env),
        "actor": actor or os.environ.get("GITHUB_ACTOR") or getpass.getuser(),
    }
    audit_path.parent.mkdir(parents=True, exist_ok=True)
    with audit_path.open("a", encoding="utf-8") as fh:
        fh.write(json.dumps(record, sort_keys=True) + "\n")
    return record


def read_audit(audit_path: Path) -> list[dict[str, str]]:
    if not audit_path.exists():
        return []
    return [json.loads(line) for line in audit_path.read_text(encoding="utf-8").splitlines() if line]


def _cmd_validate(args: argparse.Namespace) -> int:
    environments = load_environments(args.file)
    names = sorted(environments) if args.env in (None, "all") else [args.env]
    for name in names:
        if name not in environments:
            raise ConfigError(f"unknown environment '{name}'")
        print(f"ok  {name:<12} sha256={config_digest(environments[name])[:16]}")
    return 0


def _cmd_show(args: argparse.Namespace) -> int:
    env = get_environment(args.env, args.file)
    print(json.dumps(asdict(env), indent=2, sort_keys=True))
    if args.audit_file:
        record = append_audit(args.audit_file, action="show", env=env, actor=args.actor)
        print(f"audit: {json.dumps(record, sort_keys=True)}", file=sys.stderr)
    return 0


def _cmd_diff(args: argparse.Namespace) -> int:
    environments = load_environments(args.file)
    for name in (args.source, args.target):
        if name not in environments:
            raise ConfigError(f"unknown environment '{name}'")
    changes = diff_environments(environments[args.source], environments[args.target])
    if not changes:
        print(f"no promotable differences between {args.source} and {args.target}")
        return 0
    print(f"promotable differences {args.source} -> {args.target}:")
    for key, (src_val, dst_val) in sorted(changes.items()):
        print(f"  {key}: {src_val!r} -> {dst_val!r}")
    if args.audit_file:
        record = append_audit(
            args.audit_file, action=f"diff:{args.source}->{args.target}",
            env=environments[args.target], actor=args.actor,
        )
        print(f"audit: {json.dumps(record, sort_keys=True)}", file=sys.stderr)
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="python3 -m scripts.ops.config", description=__doc__.split("\n")[0])
    parser.add_argument("--file", type=Path, default=None, help="config file (default: $MAINSTAY_ENVIRONMENTS_FILE or environments.toml)")
    sub = parser.add_subparsers(dest="command", required=True)

    p_validate = sub.add_parser("validate", help="validate all environments")
    p_validate.add_argument("--env", default="all", help="environment name or 'all'")
    p_validate.set_defaults(func=_cmd_validate)

    p_show = sub.add_parser("show", help="print one environment's resolved config")
    p_show.add_argument("--env", default=None, help="environment name (default: $MAINSTAY_ENV or testnet)")
    p_show.add_argument("--audit-file", type=Path, default=None, help="append an audit record to this JSONL file")
    p_show.add_argument("--actor", default=None)
    p_show.set_defaults(func=_cmd_show)

    p_diff = sub.add_parser("diff", help="show promotable differences between two environments")
    p_diff.add_argument("--from", dest="source", required=True)
    p_diff.add_argument("--to", dest="target", required=True)
    p_diff.add_argument("--audit-file", type=Path, default=None)
    p_diff.add_argument("--actor", default=None)
    p_diff.set_defaults(func=_cmd_diff)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        return args.func(args)
    except ConfigError as exc:
        print(f"config error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
