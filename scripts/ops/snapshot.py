"""Contract state snapshots, verification, and recovery planning (#1727).

A snapshot is a gzipped, checksummed JSON document built from the directory
that scripts/backup.sh produces. Snapshots are verified before use, and a
recovery plan is generated from them. Live writes stay with scripts/restore.sh.

Usage (from the repository root):

    python3 -m scripts.ops.snapshot create --backup-dir backups/20261016T000000Z \\
        --out-dir snapshots --network testnet --keep 30
    python3 -m scripts.ops.snapshot verify snapshots/snapshot-20261016T000000Z.json.gz
    python3 -m scripts.ops.snapshot plan snapshots/<file> --out plan.json
    python3 -m scripts.ops.snapshot compare snapshots/<file> --actual backups/<post-restore-export>

Snapshot layout (gzipped JSON):

    {"manifest": {...}, "state": {"assets": {...}, "engineers": ..., "maintenance": {...}}}
"""

from __future__ import annotations

import argparse
import datetime as dt
import gzip
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

from scripts.ops.jsonlog import log_event

SCHEMA_VERSION = 1
SNAPSHOT_PREFIX = "snapshot-"
SNAPSHOT_SUFFIX = ".json.gz"
TIMESTAMP_FORMAT = "%Y%m%dT%H%M%SZ"


class SnapshotError(Exception):
    """Raised when a snapshot cannot be created, read, or verified."""


def canonical_bytes(obj: Any) -> bytes:
    return json.dumps(obj, sort_keys=True, separators=(",", ":")).encode("utf-8")


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise SnapshotError(f"invalid JSON in {path}: {exc}") from exc


def _id_sort_key(path: Path) -> int:
    return int(path.stem.split("_")[0])


def state_from_backup_dir(backup_dir: Path) -> dict[str, Any]:
    """Assemble a state document from a scripts/backup.sh output directory."""
    if not backup_dir.is_dir():
        raise SnapshotError(f"backup directory not found: {backup_dir}")

    assets: dict[str, Any] = {}
    assets_dir = backup_dir / "assets"
    if assets_dir.is_dir():
        for path in sorted(assets_dir.glob("*.json"), key=_id_sort_key):
            assets[path.stem] = _load_json(path)

    maintenance: dict[str, dict[str, Any]] = {}
    maint_dir = backup_dir / "maintenance"
    if maint_dir.is_dir():
        for path in sorted(maint_dir.glob("*_history.json"), key=_id_sort_key):
            asset_id = path.stem.split("_")[0]
            maintenance.setdefault(asset_id, {})["history"] = _load_json(path)
        for path in sorted(maint_dir.glob("*_score.json"), key=_id_sort_key):
            asset_id = path.stem.split("_")[0]
            maintenance.setdefault(asset_id, {})["score"] = _load_json(path)

    engineers_path = backup_dir / "engineers.json"
    engineers = _load_json(engineers_path) if engineers_path.exists() else {}

    return {"assets": assets, "engineers": engineers, "maintenance": maintenance}


def _history_record_count(entry: dict[str, Any]) -> int:
    history = entry.get("history")
    return len(history) if isinstance(history, list) else 0


def compute_counts(state: dict[str, Any]) -> dict[str, int]:
    maintenance = state.get("maintenance", {})
    return {
        "assets": len(state.get("assets", {})),
        "maintenance_assets": sum(1 for entry in maintenance.values() if "history" in entry),
        "history_records": sum(_history_record_count(entry) for entry in maintenance.values()),
    }


def check_consistency(state: dict[str, Any]) -> list[str]:
    """Return problems found in a state document (empty when consistent)."""
    problems: list[str] = []
    assets = state.get("assets", {})
    maintenance = state.get("maintenance", {})

    for asset_id in assets:
        if not asset_id.isdigit() or int(asset_id) < 1:
            problems.append(f"invalid asset id '{asset_id}'")

    for asset_id in sorted(maintenance, key=lambda k: int(k) if k.isdigit() else 0):
        if asset_id not in assets:
            problems.append(f"maintenance for asset {asset_id} has no asset record (orphan)")
        for record in maintenance[asset_id].get("history") or []:
            if isinstance(record, dict) and "asset_id" in record and str(record["asset_id"]) != asset_id:
                problems.append(
                    f"history record under asset {asset_id} references asset {record['asset_id']}"
                )
    return problems


def recovery_blockers(state: dict[str, Any]) -> list[str]:
    """Problems that would make recovery unsafe.

    The asset registry assigns IDs sequentially, so restoring a snapshot only
    reproduces the original IDs when they are contiguous from 1.
    """
    ids = sorted(int(k) for k in state.get("assets", {}) if k.isdigit())
    if ids and ids != list(range(1, len(ids) + 1)):
        missing = sorted(set(range(1, ids[-1] + 1)) - set(ids))
        return [f"asset ids are not contiguous from 1 (missing: {missing[:10]})"]
    return []


def create_snapshot(
    state: dict[str, Any],
    out_dir: Path,
    *,
    network: str,
    now: dt.datetime | None = None,
) -> Path:
    """Write a gzipped, checksummed snapshot. Refuses inconsistent state."""
    problems = check_consistency(state)
    if problems:
        raise SnapshotError("refusing to snapshot inconsistent state:\n  - " + "\n  - ".join(problems))

    created = now or dt.datetime.now(dt.timezone.utc)
    manifest = {
        "schema_version": SCHEMA_VERSION,
        "created_at": created.isoformat(),
        "network": network,
        "counts": compute_counts(state),
        "state_sha256": _sha256(canonical_bytes(state)),
    }
    out_dir.mkdir(parents=True, exist_ok=True)
    path = out_dir / f"{SNAPSHOT_PREFIX}{created.strftime(TIMESTAMP_FORMAT)}{SNAPSHOT_SUFFIX}"
    if path.exists():
        raise SnapshotError(f"snapshot already exists: {path}")
    payload = canonical_bytes({"manifest": manifest, "state": state})
    with gzip.open(path, "wb", compresslevel=9) as fh:
        fh.write(payload)
    return path


def read_snapshot(path: Path) -> tuple[dict[str, Any], dict[str, Any]]:
    """Read a snapshot and check its checksum, schema version and counts."""
    try:
        with gzip.open(path, "rb") as fh:
            document = json.loads(fh.read().decode("utf-8"))
    except (OSError, EOFError) as exc:
        raise SnapshotError(f"cannot read snapshot {path}: {exc}") from exc
    except json.JSONDecodeError as exc:
        raise SnapshotError(f"snapshot {path} is not valid JSON: {exc}") from exc

    manifest = document.get("manifest")
    state = document.get("state")
    if not isinstance(manifest, dict) or not isinstance(state, dict):
        raise SnapshotError(f"snapshot {path} is missing manifest or state")
    if manifest.get("schema_version") != SCHEMA_VERSION:
        raise SnapshotError(
            f"unsupported schema_version {manifest.get('schema_version')} (expected {SCHEMA_VERSION})"
        )
    if _sha256(canonical_bytes(state)) != manifest.get("state_sha256"):
        raise SnapshotError(f"checksum mismatch: snapshot {path} is corrupt or was modified")
    if compute_counts(state) != manifest.get("counts"):
        raise SnapshotError(f"record counts do not match the manifest in {path}")
    return manifest, state


def verify_snapshot(path: Path) -> list[str]:
    """Full verification: integrity, then cross-record consistency."""
    _, state = read_snapshot(path)
    return check_consistency(state) + recovery_blockers(state)


def build_recovery_plan(state: dict[str, Any], start_asset_id: int = 1) -> list[dict[str, Any]]:
    """Ordered operations to restore a snapshot.

    Assets are registered in ascending ID order so that the registry reassigns
    the same IDs. Maintenance history is restored after its asset exists.
    """
    blockers = recovery_blockers(state)
    if blockers and start_asset_id <= 1:
        raise SnapshotError("cannot plan recovery: " + "; ".join(blockers))

    plan: list[dict[str, Any]] = []
    for asset_id in sorted((k for k in state.get("assets", {}) if k.isdigit()), key=int):
        if int(asset_id) >= start_asset_id:
            plan.append({"op": "register_asset", "asset_id": int(asset_id), "payload": state["assets"][asset_id]})
    for asset_id in sorted((k for k in state.get("maintenance", {}) if k.isdigit()), key=int):
        history = state["maintenance"][asset_id].get("history")
        if int(asset_id) >= start_asset_id and history:
            plan.append({"op": "restore_history", "asset_id": int(asset_id), "records": history})
    return plan


def compare_states(expected: dict[str, Any], actual: dict[str, Any]) -> list[str]:
    """Differences between a snapshot and a post-restore export. Empty when they match."""
    differences: list[str] = []
    exp_assets, act_assets = expected.get("assets", {}), actual.get("assets", {})
    for asset_id in sorted(set(exp_assets) | set(act_assets), key=lambda k: int(k) if k.isdigit() else 0):
        if asset_id not in act_assets:
            differences.append(f"asset {asset_id} missing after restore")
        elif asset_id not in exp_assets:
            differences.append(f"asset {asset_id} present after restore but not in snapshot")
        elif exp_assets[asset_id] != act_assets[asset_id]:
            differences.append(f"asset {asset_id} differs from snapshot")

    exp_maint = expected.get("maintenance", {})
    act_maint = actual.get("maintenance", {})
    for asset_id in sorted(set(exp_maint) | set(act_maint), key=lambda k: int(k) if k.isdigit() else 0):
        exp_hist = (exp_maint.get(asset_id) or {}).get("history")
        act_hist = (act_maint.get(asset_id) or {}).get("history")
        if exp_hist != act_hist:
            differences.append(f"maintenance history for asset {asset_id} differs from snapshot")
    return differences


def prune_snapshots(out_dir: Path, keep: int) -> list[Path]:
    """Delete the oldest snapshots, keeping the newest `keep`. Returns removed paths."""
    if keep < 1:
        raise SnapshotError("keep must be at least 1")
    snapshots = sorted(out_dir.glob(f"{SNAPSHOT_PREFIX}*{SNAPSHOT_SUFFIX}"))
    removed = snapshots[: max(0, len(snapshots) - keep)]
    for path in removed:
        path.unlink()
    return removed


def _cmd_create(args: argparse.Namespace) -> int:
    state = state_from_backup_dir(args.backup_dir)
    path = create_snapshot(state, args.out_dir, network=args.network)
    removed = prune_snapshots(args.out_dir, args.keep) if args.keep else []
    size = path.stat().st_size
    print(f"snapshot: {path} ({size} bytes, counts={compute_counts(state)})")
    for old in removed:
        print(f"pruned: {old}")
    return 0


def _cmd_verify(args: argparse.Namespace) -> int:
    problems = verify_snapshot(args.snapshot)
    if problems:
        for problem in problems:
            print(f"problem: {problem}", file=sys.stderr)
        return 1
    print(f"ok: {args.snapshot} verified")
    return 0


def _cmd_plan(args: argparse.Namespace) -> int:
    _, state = read_snapshot(args.snapshot)
    plan = build_recovery_plan(state, start_asset_id=args.start_asset_id)
    rendered = json.dumps(
        {"plan_sha256": _sha256(canonical_bytes(plan)), "operations": plan},
        indent=2, sort_keys=True,
    )
    if args.out:
        args.out.write_text(rendered + "\n", encoding="utf-8")
        print(f"plan: {len(plan)} operations written to {args.out}")
    else:
        print(rendered)
    return 0


def _cmd_compare(args: argparse.Namespace) -> int:
    _, expected = read_snapshot(args.snapshot)
    actual = state_from_backup_dir(args.actual)
    differences = compare_states(expected, actual)
    if differences:
        for line in differences:
            print(f"mismatch: {line}", file=sys.stderr)
        return 1
    print("ok: restored state matches snapshot")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="python3 -m scripts.ops.snapshot")
    sub = parser.add_subparsers(dest="command", required=True)

    p_create = sub.add_parser("create", help="create a snapshot from a backup directory")
    p_create.add_argument("--backup-dir", type=Path, required=True)
    p_create.add_argument("--out-dir", type=Path, default=Path("snapshots"))
    p_create.add_argument("--network", required=True)
    p_create.add_argument("--keep", type=int, default=30, help="retain this many snapshots (0 disables pruning)")
    p_create.set_defaults(func=_cmd_create)

    p_verify = sub.add_parser("verify", help="verify checksum, schema and consistency")
    p_verify.add_argument("snapshot", type=Path)
    p_verify.set_defaults(func=_cmd_verify)

    p_plan = sub.add_parser("plan", help="emit an ordered recovery plan")
    p_plan.add_argument("snapshot", type=Path)
    p_plan.add_argument("--start-asset-id", type=int, default=1)
    p_plan.add_argument("--out", type=Path, default=None)
    p_plan.set_defaults(func=_cmd_plan)

    p_compare = sub.add_parser("compare", help="compare a snapshot against a post-restore export")
    p_compare.add_argument("snapshot", type=Path)
    p_compare.add_argument("--actual", type=Path, required=True, help="backup.sh output directory of the restored state")
    p_compare.set_defaults(func=_cmd_compare)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        code = args.func(args)
    except SnapshotError as exc:
        print(f"snapshot error: {exc}", file=sys.stderr)
        log_event("ERROR", "snapshot", args.command, str(exc))
        return 1
    log_event("INFO", "snapshot", args.command, exit_code=code)
    return code


if __name__ == "__main__":
    sys.exit(main())
