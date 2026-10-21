# Real-time operations dashboard

> **Last updated:** 2026-09-25 · **Doc version:** 0.1.0

Static reports should be treated as historical exports, not the operational
view. `scripts/stream-events.sh` provides a small, deployment-neutral event
feed for a dashboard or alerting service. It polls Soroban `getEvents`, emits
one JSON event per line, and advances the RPC pagination cursor so consumers
can update asset health, maintenance, transfer, and critical-maintenance views
without repeatedly rebuilding a report.

```bash
source scripts/load-network-config.sh
export CONTRACT_LIFECYCLE=<lifecycle-contract-id>
POLL_SECONDS=3 ./scripts/stream-events.sh | jq --unbuffered \
  'select(.topics[0].sym == "MAINT" or .topics[0].sym == "CRIT_MNT")'
```

## Resuming from a persisted cursor

The streamer keeps its position in a state file so that a restart resumes from
the last processed ledger and cursor instead of replaying every event from
ledger 0. On each successful poll the script writes the last ledger and the
RPC pagination cursor to the state file, and on startup it reads them back and
continues from there. This prevents the dashboard from receiving duplicate
events after a restart or redeploy.

The state file path defaults to `scripts/.stream-events.state` and can be
overridden with the `STREAM_STATE_FILE` environment variable:

```bash
export STREAM_STATE_FILE=/var/lib/dashboard/stream-events.state
POLL_SECONDS=3 ./scripts/stream-events.sh | jq --unbuffered \
  'select(.topics[0].sym == "MAINT" or .topics[0].sym == "CRIT_MNT")'
```

Point the state file at a persistent volume so the position survives container
restarts. If the state file is missing or unreadable, the streamer starts from
the configured initial ledger. To intentionally replay history, stop the
streamer and remove the state file before restarting it.

## De-duplication

Events are de-duplicated by ID within the streamer, so an event that is
re-delivered by the RPC (for example after a retry or a cursor overlap) is
emitted only once. Consumers can therefore treat the feed as an
at-least-once source without additional deduplication logic.

## Retry and backoff

Transient RPC failures do not terminate the stream. When a `getEvents` call
fails, the streamer retries with exponential backoff, up to a maximum delay,
and resumes from the persisted cursor once the RPC recovers. The process only
exits on unrecoverable errors such as invalid configuration.

Run the stream as a supervised process. The script is intentionally read-only:
it never holds a signing key and cannot mutate contract state. Use the existing
`docs/EVENT_INDEXER.md` event schemas to project events into dashboard rows and
alerts.