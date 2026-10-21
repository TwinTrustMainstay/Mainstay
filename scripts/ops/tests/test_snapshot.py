"""Tests for scripts/ops/snapshot.py (#1727)."""

import datetime as dt
import gzip
import json
import tempfile
import unittest
from pathlib import Path

from scripts.ops import snapshot
from scripts.ops.snapshot import SnapshotError

NOW = dt.datetime(2026, 10, 16, 0, 0, tzinfo=dt.timezone.utc)


def write_backup(root: Path, assets=(1, 2), histories=None, scores=None, engineers=None) -> Path:
    """Build a directory with the same layout scripts/backup.sh produces."""
    (root / "assets").mkdir(parents=True)
    (root / "maintenance").mkdir()
    for asset_id in assets:
        (root / "assets" / f"{asset_id}.json").write_text(
            json.dumps({"asset_type": "GENSET", "serial": f"SN-{asset_id}"}), encoding="utf-8"
        )
    for asset_id, records in (histories or {}).items():
        (root / "maintenance" / f"{asset_id}_history.json").write_text(json.dumps(records), encoding="utf-8")
    for asset_id, score in (scores or {}).items():
        (root / "maintenance" / f"{asset_id}_score.json").write_text(json.dumps(score), encoding="utf-8")
    if engineers is not None:
        (root / "engineers.json").write_text(json.dumps(engineers), encoding="utf-8")
    return root


def sample_state() -> dict:
    return {
        "assets": {
            "1": {"asset_type": "GENSET", "serial": "SN-1"},
            "2": {"asset_type": "GENSET", "serial": "SN-2"},
        },
        "engineers": {},
        "maintenance": {
            "1": {"history": [{"asset_id": 1, "task": "INSPECT"}, {"asset_id": 1, "task": "OIL"}], "score": 42},
            "2": {"history": [{"asset_id": 2, "task": "INSPECT"}]},
        },
    }


class BackupImportTests(unittest.TestCase):
    def test_state_is_assembled_from_backup_layout(self):
        with tempfile.TemporaryDirectory() as tmp:
            backup = write_backup(
                Path(tmp) / "b",
                assets=(1, 2, 10),
                histories={1: [{"task": "INSPECT"}]},
                scores={1: 42},
                engineers={"issuers": []},
            )
            state = snapshot.state_from_backup_dir(backup)
        self.assertEqual(list(state["assets"]), ["1", "2", "10"])
        self.assertEqual(state["maintenance"]["1"], {"history": [{"task": "INSPECT"}], "score": 42})
        self.assertEqual(state["engineers"], {"issuers": []})

    def test_placeholder_engineers_file_and_missing_dirs_are_tolerated(self):
        with tempfile.TemporaryDirectory() as tmp:
            backup = Path(tmp) / "empty"
            backup.mkdir()
            state = snapshot.state_from_backup_dir(backup)
        self.assertEqual(state, {"assets": {}, "engineers": {}, "maintenance": {}})

    def test_missing_backup_directory_is_an_error(self):
        with self.assertRaisesRegex(SnapshotError, "backup directory not found"):
            snapshot.state_from_backup_dir(Path("/nonexistent/backup"))


class RoundTripTests(unittest.TestCase):
    def test_create_then_read_round_trip_preserves_state_and_counts(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = snapshot.create_snapshot(sample_state(), Path(tmp), network="testnet", now=NOW)
            manifest, state = snapshot.read_snapshot(path)
        self.assertEqual(state, sample_state())
        self.assertEqual(manifest["network"], "testnet")
        self.assertEqual(manifest["counts"], {"assets": 2, "maintenance_assets": 2, "history_records": 3})
        self.assertEqual(path.name, "snapshot-20261016T000000Z.json.gz")

    def test_snapshot_is_gzip_compressed(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = snapshot.create_snapshot(sample_state(), Path(tmp), network="testnet", now=NOW)
            raw = path.read_bytes()
        self.assertEqual(raw[:2], b"\x1f\x8b")
        self.assertIn(b"INSPECT", gzip.decompress(raw))

    def test_snapshot_of_same_timestamp_is_not_overwritten(self):
        with tempfile.TemporaryDirectory() as tmp:
            snapshot.create_snapshot(sample_state(), Path(tmp), network="testnet", now=NOW)
            with self.assertRaisesRegex(SnapshotError, "already exists"):
                snapshot.create_snapshot(sample_state(), Path(tmp), network="testnet", now=NOW)

    def test_inconsistent_state_is_refused_at_creation(self):
        bad = sample_state()
        bad["maintenance"]["9"] = {"history": [{"task": "X"}]}
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(SnapshotError, "orphan"):
                snapshot.create_snapshot(bad, Path(tmp), network="testnet", now=NOW)


class IntegrityTests(unittest.TestCase):
    def _snapshot_with(self, tmp: str, mutate) -> Path:
        path = snapshot.create_snapshot(sample_state(), Path(tmp), network="testnet", now=NOW)
        document = json.loads(gzip.decompress(path.read_bytes()))
        mutate(document)
        with gzip.open(path, "wb") as fh:
            fh.write(json.dumps(document).encode("utf-8"))
        return path

    def test_tampered_state_fails_checksum(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self._snapshot_with(tmp, lambda d: d["state"]["assets"]["1"].update(serial="FORGED"))
            with self.assertRaisesRegex(SnapshotError, "checksum mismatch"):
                snapshot.read_snapshot(path)

    def test_count_mismatch_with_manifest_is_detected(self):
        def mutate(doc):
            doc["manifest"]["counts"]["assets"] = 99
        with tempfile.TemporaryDirectory() as tmp:
            path = self._snapshot_with(tmp, mutate)
            with self.assertRaisesRegex(SnapshotError, "record counts do not match"):
                snapshot.read_snapshot(path)

    def test_unsupported_schema_version_is_rejected(self):
        def mutate(doc):
            doc["manifest"]["schema_version"] = 99
        with tempfile.TemporaryDirectory() as tmp:
            path = self._snapshot_with(tmp, mutate)
            with self.assertRaisesRegex(SnapshotError, "unsupported schema_version"):
                snapshot.read_snapshot(path)

    def test_truncated_file_is_reported_as_unreadable(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = snapshot.create_snapshot(sample_state(), Path(tmp), network="testnet", now=NOW)
            path.write_bytes(path.read_bytes()[:20])
            with self.assertRaises(SnapshotError):
                snapshot.read_snapshot(path)


class ConsistencyTests(unittest.TestCase):
    def test_history_referencing_another_asset_is_flagged(self):
        state = sample_state()
        state["maintenance"]["2"]["history"].append({"asset_id": 1, "task": "STRAY"})
        self.assertTrue(any("references asset 1" in p for p in snapshot.check_consistency(state)))

    def test_non_numeric_asset_id_is_flagged(self):
        state = sample_state()
        state["assets"]["abc"] = {}
        self.assertTrue(any("invalid asset id 'abc'" in p for p in snapshot.check_consistency(state)))

    def test_gapped_ids_block_recovery_from_the_start(self):
        state = sample_state()
        state["assets"]["5"] = {"serial": "SN-5"}
        self.assertIn("not contiguous", snapshot.recovery_blockers(state)[0])
        with self.assertRaisesRegex(SnapshotError, "cannot plan recovery"):
            snapshot.build_recovery_plan(state)
        # Partial recovery from a later ID is allowed.
        self.assertTrue(snapshot.build_recovery_plan(state, start_asset_id=5))


class RecoveryPlanTests(unittest.TestCase):
    def test_plan_registers_all_assets_before_their_history(self):
        plan = snapshot.build_recovery_plan(sample_state())
        ops = [(step["op"], step["asset_id"]) for step in plan]
        self.assertEqual(
            ops,
            [("register_asset", 1), ("register_asset", 2), ("restore_history", 1), ("restore_history", 2)],
        )

    def test_start_asset_id_skips_earlier_assets(self):
        plan = snapshot.build_recovery_plan(sample_state(), start_asset_id=2)
        self.assertEqual({step["asset_id"] for step in plan}, {2})

    def test_plan_cli_writes_checksummed_operations(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = snapshot.create_snapshot(sample_state(), Path(tmp), network="testnet", now=NOW)
            out = Path(tmp) / "plan.json"
            self.assertEqual(snapshot.main(["plan", str(path), "--out", str(out)]), 0)
            body = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual(len(body["operations"]), 4)
        self.assertEqual(len(body["plan_sha256"]), 64)


class ComparisonTests(unittest.TestCase):
    def test_identical_state_has_no_differences(self):
        self.assertEqual(snapshot.compare_states(sample_state(), sample_state()), [])

    def test_missing_and_changed_records_are_reported(self):
        actual = sample_state()
        del actual["assets"]["2"]
        actual["assets"]["1"]["serial"] = "CHANGED"
        actual["maintenance"]["1"]["history"] = []
        problems = snapshot.compare_states(sample_state(), actual)
        self.assertIn("asset 2 missing after restore", problems)
        self.assertIn("asset 1 differs from snapshot", problems)
        self.assertIn("maintenance history for asset 1 differs from snapshot", problems)

    def test_compare_cli_detects_post_restore_drift(self):
        with tempfile.TemporaryDirectory() as tmp:
            snap = snapshot.create_snapshot(sample_state(), Path(tmp) / "snaps", network="testnet", now=NOW)
            matching = write_backup(Path(tmp) / "match", assets=(1, 2), histories={1: sample_state()["maintenance"]["1"]["history"], 2: sample_state()["maintenance"]["2"]["history"]})
            self.assertEqual(snapshot.main(["compare", str(snap), "--actual", str(matching)]), 0)
            drifted = write_backup(Path(tmp) / "drift", assets=(1,), histories={1: []})
            self.assertEqual(snapshot.main(["compare", str(snap), "--actual", str(drifted)]), 1)


class RetentionTests(unittest.TestCase):
    def test_prune_keeps_only_the_newest_snapshots(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            for day in range(1, 6):
                when = dt.datetime(2026, 10, day, tzinfo=dt.timezone.utc)
                snapshot.create_snapshot(sample_state(), out, network="testnet", now=when)
            removed = snapshot.prune_snapshots(out, keep=2)
            remaining = sorted(p.name for p in out.glob("snapshot-*.json.gz"))
        self.assertEqual(len(removed), 3)
        self.assertEqual(remaining, ["snapshot-20261004T000000Z.json.gz", "snapshot-20261005T000000Z.json.gz"])

    def test_prune_rejects_keep_below_one(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(SnapshotError):
                snapshot.prune_snapshots(Path(tmp), keep=0)


class CliTests(unittest.TestCase):
    def test_create_and_verify_end_to_end(self):
        with tempfile.TemporaryDirectory() as tmp:
            backup = write_backup(
                Path(tmp) / "backup",
                assets=(1, 2),
                histories={1: [{"asset_id": 1, "task": "INSPECT"}]},
                scores={1: 40},
            )
            out = Path(tmp) / "snapshots"
            self.assertEqual(
                snapshot.main(["create", "--backup-dir", str(backup), "--out-dir", str(out), "--network", "testnet"]),
                0,
            )
            (created,) = out.glob("snapshot-*.json.gz")
            self.assertEqual(snapshot.main(["verify", str(created)]), 0)

    def test_verify_fails_for_corrupt_snapshot(self):
        with tempfile.TemporaryDirectory() as tmp:
            bogus = Path(tmp) / "snapshot-bogus.json.gz"
            bogus.write_bytes(b"not gzip")
            self.assertEqual(snapshot.main(["verify", str(bogus)]), 1)


if __name__ == "__main__":
    unittest.main()
