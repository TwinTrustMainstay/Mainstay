# Mainstay Rust SDK

`mainstay-sdk` is the official typed Rust facade for the Mainstay Asset
Registry Soroban contract. It wraps the generated contract client and keeps
application code independent of generated-client details.

```rust
use mainstay_sdk::{MainstayClient, SearchFilter};

let client = MainstayClient::new(&env, &asset_registry_id);
let page = client.search_assets(&SearchFilter {
    asset_type: None,
    manufacturer: None,
    min_age_months: None,
    max_age_months: None,
    sort: None,
    lifecycle_contract: None,
});
println!("{} matching assets", page.total);
```

The crate is included in the workspace, so it can be tested with
`cargo test -p mainstay-sdk`.

## Resilient Cross-Contract Calls

`mainstay_sdk::resilience` adds client-side service-mesh controls for calls to
Mainstay contracts. Soroban transactions are atomic and a contract cannot retry
its own invocation, so these controls run in the submitting client.

| Control | Type | Behaviour |
|---------|------|-----------|
| Retry with backoff | `RetryPolicy`, `retry` | Exponential delay, capped at `max_delay_ms`, retries only errors the caller marks retryable |
| Circuit breaker | `CircuitBreaker` | Opens after N consecutive failures, admits one probe after the cooldown |
| Rate limit | `TokenBucket` | Burst up to `capacity`, sustained `refill_per_second` |
| Tracing | `SpanRecord`, `TraceContext` | One span per attempt, nested calls join the caller's trace |

`ServiceMesh` combines these per registered service. Time and sleeping are
injected, so behaviour is deterministic in tests:

```rust
use mainstay_sdk::resilience::{RetryPolicy, ServiceConfig, ServiceMesh, CallError};

let mut mesh = ServiceMesh::new();
mesh.register("lifecycle", ServiceConfig {
    retry: RetryPolicy::new(3, 100, 2_000)?,   // up to 3 attempts, 100ms -> 2s backoff
    failure_threshold: 5,                      // open the circuit after 5 consecutive failures
    cooldown_ms: 30_000,                       // probe again after 30s
    rate_capacity: 20,                         // burst of 20 calls
    rate_per_second: 10,                       // sustained 10 calls/s
}, now_ms())?;

let result = mesh.call(
    "lifecycle",
    None,                  // or Some(parent_trace) for a nested call
    &|| now_ms(),
    &mut |ms| std::thread::sleep(std::time::Duration::from_millis(ms)),
    |_attempt| submit_transaction(),          // returns Result<T, E>
    |err| err.is_transient(),                 // which errors to retry
);
match result {
    Err(CallError::CircuitOpen { .. }) => { /* back off, do not submit */ }
    Err(CallError::RateLimited { .. }) => { /* queue or drop */ }
    _ => {}
}
let spans = mesh.take_spans();               // export to your tracing backend
```

Do not retry non-idempotent calls on ambiguous failures (for example, a
timeout after a transaction was submitted). Mark only errors you know did not
change state as retryable.
