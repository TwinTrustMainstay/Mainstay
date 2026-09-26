# Real-time operations dashboard

Static reports should be treated as historical exports, not the operational
view. `scripts/stream-events.sh` provides a small, deployment-neutral event
feed for a dashboard or alerting service. It polls Soroban `getEvents`, emits
one JSON event per line, and advances the RPC pagination cursor so consumers
can update asset health, maintenance, transfer, and critical-maintenance views
without repeatedly rebuilding a report.

```bash
export STELLAR_RPC_URL=https://soroban-testnet.stellar.org
export CONTRACT_LIFECYCLE=<lifecycle-contract-id>
POLL_SECONDS=3 ./scripts/stream-events.sh | jq --unbuffered \
  'select(.topics[0].sym == "MAINT" or .topics[0].sym == "CRIT_MNT")'
```

Run the stream as a supervised process and persist the last emitted cursor in
the dashboard service. The script is intentionally read-only: it never holds a
signing key and cannot mutate contract state. Use the existing
`docs/EVENT_INDEXER.md` event schemas to project events into dashboard rows and
alerts.
