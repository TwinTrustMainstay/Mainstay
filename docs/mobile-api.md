# Mobile API (`contracts/mobile-api`)

Mobile clients need to stay in sync with on-chain state without holding a
connection to the chain. The `mobile-api` contract provides the on-chain part of
that: a bounded change feed, cursor-based sync, topic subscriptions, rate limits,
and API version negotiation. Heavy payloads stay off-chain, referenced by hash.

## Flow

1. **Register** – the app calls `register_client(client, api_version)`. The
   response is the negotiated version: the lower of the app's version and
   `CURRENT_API_VERSION`. Versions below `MIN_API_VERSION` are rejected with
   `UnsupportedApiVersion`.
2. **Publish** – the platform (admin key or relayer) calls
   `publish_change(admin, kind, asset_id, payload_hash)`. Each change gets the
   next sequence number and emits a `CHANGE` event `(asset_id) -> seq`.
3. **Push** – an off-chain relay listens for `CHANGE` events and sends device
   push notifications. Contracts cannot push, so this relay is required.
4. **Sync** – on launch or reconnect, the app calls `sync(client, since_seq, limit)`
   with its stored cursor and applies each change. It saves `next_seq` as its new cursor.
5. **Notifications** – after `subscribe(client, topics)`, the app can call
   `pull_notifications` to get only the change kinds it cares about.

## Offline-first behaviour

- Each change has a monotonic `seq`. The client's cursor is the last `seq` it
  applied, so syncing is idempotent.
- The contract keeps the most recent `MAX_CHANGE_LOG_ENTRIES` (1000) changes.
  If a client's cursor falls before that window, `sync` returns
  `full_resync_required = true` with no changes. The client must load a full
  snapshot from the off-chain API and restart from `latest_seq`.

## Payload size

`ChangeRecord` holds only `seq`, `kind`, `asset_id`, a 32-byte `payload_hash`,
and `ledger_ts`. The full record is fetched off-chain and verified against the
hash, so the on-chain response size does not depend on record size. Pages are
capped at `MAX_PAGE_SIZE` (100).

## Rate limiting

Each registered client has a fixed window (default 60 requests per 60 seconds),
set by the admin with `set_rate_limit` within the bounds
`1..=10_000` requests and `1..=86_400` seconds. `sync`, `pull_notifications`,
`subscribe` and `unsubscribe` count against the window. Clients must
authenticate, so the limit cannot be bypassed by changing arguments.

## Versioning policy

- `CURRENT_API_VERSION` is incremented when the response shape changes.
- `MIN_API_VERSION` is raised only after every client that depends on the old
  version has been updated.
- Clients should call `get_version_info` once per launch to check compatibility.

## Errors

See [error-reference.md](error-reference.md#mobileapi).
