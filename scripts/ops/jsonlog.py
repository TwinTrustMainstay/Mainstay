"""Structured JSON-lines logging, log search and retention (#1729).

Every line is one JSON object with the required fields `ts`, `level`,
`service` and `event`, plus optional `message` and free-form fields. Lines
are written to a file that a log shipper (Fluent Bit, see
monitoring/logging/) forwards to Loki. Keys that look like secrets are
redacted before anything is written.

Usage (from the repository root):

    python3 -m scripts.ops.jsonlog search logs/*.jsonl --level WARNING --service canary
    python3 -m scripts.ops.jsonlog search - --since 2026-10-15T00:00:00Z --contains timeout < app.jsonl
    python3 -m scripts.ops.jsonlog prune logs --env mainnet --dry-run

Set MAINSTAY_LOG_FILE to make the ops CLIs (config, snapshot, canary) emit
their events into a shared JSONL file.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import sys
import time
from pathlib import Path
from typing import Any, Iterable, Iterator

from scripts.ops import config

LOG_FILE_VAR = "MAINSTAY_LOG_FILE"
REQUIRED_FIELDS = ("ts", "level", "service", "event")
LEVELS = {"DEBUG": 10, "INFO": 20, "WARNING": 30, "ERROR": 40}
REDACTED = "[REDACTED]"
_SECRET_KEY = re.compile(r"secret|passw|token|private|api[_-]?key|credential", re.IGNORECASE)
_SECRET_VALUE = re.compile(r"\bS[A-Z2-7]{55}\b")  # Stellar secret seed format
LOG_GLOB = "*.jsonl"


class LogError(Exception):
    """Raised for malformed log records or invalid search arguments."""


def redact(value: Any, key: str = "") -> Any:
    """Recursively redact values whose key looks secret, and Stellar seeds anywhere."""
    if _SECRET_KEY.search(key):
        return REDACTED
    if isinstance(value, dict):
        return {k: redact(v, str(k)) for k, v in value.items()}
    if isinstance(value, list):
        return [redact(v, key) for v in value]
    if isinstance(value, str) and _SECRET_VALUE.search(value):
        return _SECRET_VALUE.sub(REDACTED, value)
    return value


def format_record(
    level: str,
    service: str,
    event: str,
    message: str = "",
    /,
    *,
    now: dt.datetime | None = None,
    **fields: Any,
) -> str:
    """Render one record as a single JSON line (no trailing newline)."""
    level = level.upper()
    if level not in LEVELS:
        raise LogError(f"unknown level '{level}'")
    if not service or not event:
        raise LogError("service and event are required")
    base: dict[str, Any] = {
        "ts": (now or dt.datetime.now(dt.timezone.utc)).isoformat(timespec="milliseconds"),
        "level": level,
        "service": service,
        "event": event,
    }
    if message:
        base["message"] = redact(message)
    # Extra fields may add context but never override the required fields.
    extra = {k: v for k, v in redact(fields).items() if k not in base}
    return json.dumps({**extra, **base}, sort_keys=True, separators=(",", ":"), default=str)


def log_event(level: str, service: str, event: str, message: str = "", **fields: Any) -> None:
    """Append a record to $MAINSTAY_LOG_FILE if it is set. A no-op otherwise."""
    path = os.environ.get(LOG_FILE_VAR)
    if not path:
        return
    line = format_record(level, service, event, message, **fields)
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    with target.open("a", encoding="utf-8") as fh:
        fh.write(line + "\n")


def parse_line(line: str) -> dict[str, Any]:
    """Parse and validate one line. Raises LogError if it is not a valid record."""
    try:
        record = json.loads(line)
    except json.JSONDecodeError as exc:
        raise LogError(f"not JSON: {exc.msg}") from exc
    if not isinstance(record, dict):
        raise LogError("record is not a JSON object")
    missing = [f for f in REQUIRED_FIELDS if f not in record]
    if missing:
        raise LogError(f"missing fields: {', '.join(missing)}")
    if record["level"] not in LEVELS:
        raise LogError(f"unknown level '{record['level']}'")
    return record


def read_records(paths: Iterable[str | Path]) -> Iterator[tuple[dict[str, Any] | None, str]]:
    """Yield (record, raw_line) for each line. Malformed lines yield (None, raw_line).

    The path "-" reads standard input.
    """
    for raw_path in paths:
        handle = sys.stdin if str(raw_path) == "-" else Path(raw_path).open(encoding="utf-8", errors="replace")
        try:
            for line in handle:
                line = line.rstrip("\n")
                if not line.strip():
                    continue
                try:
                    yield parse_line(line), line
                except LogError:
                    yield None, line
        finally:
            if handle is not sys.stdin:
                handle.close()


def _parse_bound(value: str | dt.datetime | None, name: str) -> dt.datetime | None:
    if value is None or isinstance(value, dt.datetime):
        return value
    try:
        parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as exc:
        raise LogError(f"{name} must be an ISO-8601 timestamp: {value}") from exc
    return parsed if parsed.tzinfo else parsed.replace(tzinfo=dt.timezone.utc)


def matches(
    record: dict[str, Any],
    *,
    level: str | None = None,
    service: str | None = None,
    event: str | None = None,
    since: dt.datetime | None = None,
    until: dt.datetime | None = None,
    contains: str | None = None,
) -> bool:
    if level and LEVELS[record["level"]] < LEVELS[level.upper()]:
        return False
    if service and record["service"] != service:
        return False
    if event and record["event"] != event:
        return False
    if since or until:
        ts = _parse_bound(record["ts"], "ts")
        if since and ts < since:
            return False
        if until and ts > until:
            return False
    if contains and contains not in json.dumps(record, sort_keys=True):
        return False
    return True


def search(
    paths: Iterable[str | Path],
    *,
    limit: int | None = None,
    **filters: Any,
) -> tuple[list[dict[str, Any]], int]:
    """Return (matching records, malformed line count)."""
    since = _parse_bound(filters.pop("since", None), "--since")
    until = _parse_bound(filters.pop("until", None), "--until")
    results: list[dict[str, Any]] = []
    malformed = 0
    for record, _raw in read_records(paths):
        if record is None:
            malformed += 1
            continue
        if matches(record, since=since, until=until, **filters):
            results.append(record)
            if limit and len(results) >= limit:
                break
    return results, malformed


def apply_retention(directory: Path, retention_days: int, now: dt.datetime | None = None, *, dry_run: bool = False) -> list[Path]:
    """Delete .jsonl files whose modification time is older than the retention window."""
    if retention_days < 1:
        raise LogError("retention_days must be at least 1")
    cutoff = (now or dt.datetime.now(dt.timezone.utc)).timestamp() - retention_days * 86400
    expired = [p for p in sorted(directory.glob(LOG_GLOB)) if p.stat().st_mtime < cutoff]
    if not dry_run:
        for path in expired:
            path.unlink()
    return expired


def _cmd_search(args: argparse.Namespace) -> int:
    records, malformed = search(
        args.paths,
        limit=args.limit,
        level=args.level,
        service=args.service,
        event=args.event,
        since=args.since,
        until=args.until,
        contains=args.contains,
    )
    for record in records:
        print(json.dumps(record, sort_keys=True))
    if malformed:
        print(f"skipped {malformed} malformed line(s)", file=sys.stderr)
    return 0


def _cmd_prune(args: argparse.Namespace) -> int:
    days = args.days if args.days is not None else config.get_environment(args.env, args.config).logging.retention_days
    expired = apply_retention(args.directory, days, dry_run=args.dry_run)
    verb = "would delete" if args.dry_run else "deleted"
    for path in expired:
        print(f"{verb}: {path}")
    print(f"{len(expired)} file(s) past {days}-day retention")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="python3 -m scripts.ops.jsonlog")
    sub = parser.add_subparsers(dest="command", required=True)

    p_search = sub.add_parser("search", help="filter structured log records")
    p_search.add_argument("paths", nargs="+", help="JSONL files, or '-' for stdin")
    p_search.add_argument("--level", choices=sorted(LEVELS), help="minimum level")
    p_search.add_argument("--service")
    p_search.add_argument("--event")
    p_search.add_argument("--since", type=lambda v: _parse_bound(v, "--since"))
    p_search.add_argument("--until", type=lambda v: _parse_bound(v, "--until"))
    p_search.add_argument("--contains", help="substring match against the whole record")
    p_search.add_argument("--limit", type=int, default=None)
    p_search.set_defaults(func=_cmd_search)

    p_prune = sub.add_parser("prune", help="delete log files past the retention window")
    p_prune.add_argument("directory", type=Path)
    p_prune.add_argument("--env", default=None, help="take retention from this environment")
    p_prune.add_argument("--days", type=int, default=None, help="override retention in days")
    p_prune.add_argument("--config", type=Path, default=None)
    p_prune.add_argument("--dry-run", action="store_true")
    p_prune.set_defaults(func=_cmd_prune)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        return args.func(args)
    except (LogError, config.ConfigError) as exc:
        print(f"log error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
