# Operations Infrastructure

Operational tooling for environment configuration, state snapshots, canary
rollouts, and centralized logging. All of it is stdlib-only Python under
`scripts/ops/`, tested by the `ops-tooling` CI job.

Run every command below from the repository root.

## Environment configuration (#1726)

`environments.toml` is the single source for per-environment settings. Each
environment defines the same schema:

| Section | Keys |
|---|---|
| top level | `rpc_url`, `network_passphrase` |
| `[env.limits]` | `max_fee_stroops`, `max_batch_size` |
| `[env.timeouts]` | `rpc_timeout_secs`, `tx_timeout_secs` |
| `[env.rates]` | `max_requests_per_minute`, `max_submissions_per_hour` |
| `[env.logging]` | `level`, `retention_days` |
| `[env.canary]` | `stages`, `max_error_rate`, `max_p95_latency_ms`, `min_samples`, `observe_secs` |

Environments: `dev`, `staging`, `testnet`, `mainnet`, `futurenet`, `standalone`.

Validation rejects unknown keys (to catch typos), enforces `https://` RPC
URLs except for localhost in `dev`/`standalone`, and requires
`max_submissions_per_hour > 0` outside dev (0 disables rate limiting, see the
[deployment runbook](deployment-runbook.md#45-post-deploy-configuration-max_submissions_per_hour)).
Mainnet additionally requires at least 90 days of log retention and a canary
error budget of at most 5%.

```bash
# Validate every environment (what CI runs)
python3 -m scripts.ops.config validate --env all

# Print the resolved config for one environment (default: $MAINSTAY_ENV or testnet)
python3 -m scripts.ops.config show --env mainnet

# Preview promotable differences before promoting staging to mainnet
python3 -m scripts.ops.config diff --from staging --to mainnet
```

Settings:

- `MAINSTAY_ENV` selects the environment when `--env` is omitted.
- `MAINSTAY_ENVIRONMENTS_FILE` loads an external config file instead of the
  repository copy, so secrets-free overrides can live outside git.

### Audit trail

Pass `--audit-file <path>` to `show` or `diff` to append a JSONL record:

```json
{"action": "diff:staging->mainnet", "actor": "alice", "environment": "mainnet", "sha256": "…", "ts": "2026-10-16T12:00:00+00:00"}
```

The `sha256` is a digest of the environment's canonical JSON, so an audit entry
can be matched to the exact configuration that was in effect. Identity fields
(`rpc_url`, `network_passphrase`) are excluded from promotion diffs.

## State snapshots and recovery (#1727)

Snapshots capture the export produced by `scripts/backup.sh` as one gzipped,
checksummed file, and recovery is planned from them. Live writes still go
through `scripts/restore.sh`.

```bash
# 1. Export live state (writes to backups/<timestamp>/ by default)
./scripts/backup.sh --output-dir backups/manual

# 2. Package it as a snapshot, pruning all but the newest 30
python3 -m scripts.ops.snapshot create --backup-dir backups/manual \
  --out-dir snapshots --network testnet --keep 30

# 3. Verify checksum, schema version, record counts and cross-references
python3 -m scripts.ops.snapshot verify snapshots/snapshot-<timestamp>.json.gz

# 4. Build the ordered recovery plan (dry run, nothing is written on-chain)
python3 -m scripts.ops.snapshot plan snapshots/snapshot-<timestamp>.json.gz --out plan.json

# 5. After restoring, export the new state and confirm it matches the snapshot
./scripts/backup.sh --output-dir backups/post-restore
python3 -m scripts.ops.snapshot compare snapshots/snapshot-<timestamp>.json.gz \
  --actual backups/post-restore
```

Safeguards:

- A snapshot is refused if its state has records that reference a missing
  asset (orphans) or a history entry that names a different asset.
- The asset registry assigns IDs sequentially, so a plan is refused when asset
  IDs are not contiguous from 1. Partial recovery is allowed with
  `--start-asset-id N`.
- A snapshot whose content no longer matches its SHA-256, or whose record
  counts differ from the manifest, is rejected when read.

The `state-snapshot` workflow runs the export daily. It is disabled until the
repository sets the `SNAPSHOTS_ENABLED` variable to `true`, and it runs in the
`snapshots` GitHub environment. Each snapshot is uploaded as an artifact kept
for 90 days.

## Canary rollouts (#1728)

Soroban contracts have no built-in traffic splitting. A canary is therefore a
second Lifecycle instance deployed from the new WASM, and a traffic router
(client or API layer, outside this repository) sends `percent` of requests to
it. The router reads `deploy/canary-state.json`, which the rollout tool keeps up
to date (written atomically).

Stages come from `[env.canary].stages` in `environments.toml`. Mainnet ramps
`1 → 5 → 25 → 50 → 100`. Each stage is observed for `observe_secs` against a
metrics endpoint (`CANARY_METRICS_URL`) that returns:

```json
{"total": 1200, "failures": 3, "p95_latency_ms": 640}
```

| Condition | Result |
|---|---|
| Error rate above `max_error_rate`, or p95 above `max_p95_latency_ms` | Rolled back to 0% |
| Fewer than `min_samples` requests | Retried up to 3 times, then rolled back |
| Stage healthy | Advances to the next stage |
| Next stage is 100% and no approval given | Held (`exit 2`) until approved |

An interrupted run resumes by re-observing the stage that was in progress, so
no stage is skipped without its health check.

```bash
python3 -m scripts.ops.canary start --env staging --stable-id <C…> --canary-id <C…>
python3 -m scripts.ops.canary advance --env staging           # ramps to the approval gate
python3 -m scripts.ops.canary advance --env staging --approve-final
python3 -m scripts.ops.canary status
python3 -m scripts.ops.canary rollback --reason "incident 123"  # refused once promoted
```

The `canary-deploy` workflow (manual dispatch) runs the ramp in the target
environment, then pauses for a reviewer in the `production-approval`
environment before moving to 100%. Configure that environment with required
reviewers in the repository settings. A promoted rollout cannot be rolled back
with this tool; redeploy the previous WASM instead.

## Centralized logging (#1729)

Ops tools write structured JSON lines, one record per line, with required
fields `ts` (millisecond UTC), `level`, `service` and `event`, and optional
`message` and context fields. Keys that look like secrets (`*secret*`,
`*token*`, `*password*`, `*api_key*`, and so on) are redacted, and Stellar secret
seeds are redacted from any value.

Set `MAINSTAY_LOG_FILE` to have the ops CLIs append to a shared file. Canary
rollbacks are logged at `WARNING` so they can be alerted on.

```bash
export MAINSTAY_LOG_FILE=/var/log/mainstay/ops.jsonl
python3 -m scripts.ops.canary advance --env mainnet --metrics-url "$CANARY_METRICS_URL"

# Search: minimum level, service, event, time window, substring
python3 -m scripts.ops.jsonlog search /var/log/mainstay/*.jsonl \
  --level WARNING --service canary --since 2026-10-15T00:00:00Z --contains "p95"

# Retention: delete files past the environment's retention_days (dry run first)
python3 -m scripts.ops.jsonlog prune /var/log/mainstay --env mainnet --dry-run
```

Malformed lines are skipped and counted in the output rather than failing the
search, so a single corrupt write does not hide the rest of the log.

Shipping: `monitoring/logging/fluent-bit.conf` tails `/var/log/mainstay/*.jsonl`,
parses each line, labels by `environment`, `service` and `level`, and forwards to
Loki. `monitoring/logging/loki-config.yml` sets 365-day retention (8760h),
matching `[mainnet.logging] retention_days`. A test fails if they drift apart.

Limitation: the shell scripts (`backup.sh`, `restore.sh`) still write plain text
to stdout. They are not yet in the JSON stream, so their output must be captured
separately until they emit through the same format.
