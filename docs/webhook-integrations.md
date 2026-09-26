# Real-time lifecycle integrations

Soroban contracts cannot make outbound HTTP requests. Mainstay exposes a
durable subscription list and emits lifecycle events so an off-chain relay can
deliver webhooks without clients polling contract state.

Register a subscriber with `subscribe_to_updates(asset_id, subscriber)` and
remove it with `unsubscribe_from_updates`. The subscriber address must
authorize both calls. A relay should:

1. Subscribe to `SUB_UPD` and `UNSUB_UPD` events to maintain its delivery
   registry.
2. Stream lifecycle events such as `maint`, `ESG_IMP`, `DECOMM`, and `XFER`
   from the Stellar RPC event API.
3. Deliver the event payload to each registered endpoint, retrying failures
   with exponential backoff and recording the ledger/event identifier for
   idempotency.

`get_update_subscribers(asset_id)` is available for reconciliation after a
relay restart. The contract remains the source of truth; webhook delivery is
at-least-once and should be treated as an integration convenience rather than
an authorization mechanism.
