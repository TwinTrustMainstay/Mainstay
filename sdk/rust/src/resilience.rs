//! Client-side resilience for cross-contract calls.
//!
//! A Soroban transaction is atomic and a contract cannot retry its own
//! invocation, so these controls run in the client that submits calls. They
//! provide the service-mesh behaviours described in issue #1733:
//!
//! - retry with exponential backoff: [`RetryPolicy`] and [`retry`]
//! - a circuit breaker per service: [`CircuitBreaker`]
//! - a token-bucket rate limit per service: [`TokenBucket`]
//! - a trace span for every attempt: [`SpanRecord`]
//!
//! [`ServiceMesh`] combines them. Time and sleeping are injected by the caller,
//! so behaviour is deterministic in tests and the real clock is supplied only
//! at the edge.

use std::collections::HashMap;

/// Exponential backoff policy.
///
/// `max_attempts` counts the first try, so `1` means no retries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    max_attempts: u32,
    base_delay_ms: u64,
    max_delay_ms: u64,
}

impl RetryPolicy {
    /// Creates a policy, rejecting values that cannot produce a valid schedule.
    pub fn new(
        max_attempts: u32,
        base_delay_ms: u64,
        max_delay_ms: u64,
    ) -> Result<Self, &'static str> {
        if max_attempts == 0 {
            return Err("max_attempts must be at least 1");
        }
        if base_delay_ms == 0 {
            return Err("base_delay_ms must be greater than 0");
        }
        if max_delay_ms < base_delay_ms {
            return Err("max_delay_ms must be at least base_delay_ms");
        }
        Ok(Self {
            max_attempts,
            base_delay_ms,
            max_delay_ms,
        })
    }

    /// Total attempts allowed, including the first.
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// Delay before retry `retry_index` (0 is the first retry).
    ///
    /// The delay doubles on each retry and is capped at `max_delay_ms`.
    /// Large indexes saturate rather than overflow.
    pub fn backoff_ms(&self, retry_index: u32) -> u64 {
        let factor = 1u64.checked_shl(retry_index).unwrap_or(u64::MAX);
        self.base_delay_ms
            .saturating_mul(factor)
            .min(self.max_delay_ms)
    }
}

/// Runs `op` until it succeeds, fails with a non-retryable error, or uses up
/// its attempts. `op` receives the 1-based attempt number. `sleep` is called
/// with the backoff delay in milliseconds before each retry.
pub fn retry<T, E>(
    policy: RetryPolicy,
    mut op: impl FnMut(u32) -> Result<T, E>,
    is_retryable: impl Fn(&E) -> bool,
    mut sleep: impl FnMut(u64),
) -> Result<T, E> {
    let mut attempt = 1;
    loop {
        match op(attempt) {
            Ok(value) => return Ok(value),
            Err(err) => {
                if attempt >= policy.max_attempts() || !is_retryable(&err) {
                    return Err(err);
                }
                sleep(policy.backoff_ms(attempt - 1));
                attempt += 1;
            }
        }
    }
}

/// The state of a [`CircuitBreaker`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CircuitState {
    /// Calls flow normally. Failures are counted.
    Closed,
    /// Calls are rejected until the cooldown has elapsed.
    Open,
    /// The cooldown has elapsed. A single probe call is admitted.
    HalfOpen,
}

/// Stops calling a failing service so it has room to recover.
///
/// After `failure_threshold` consecutive failures the breaker opens. It stays
/// open for `cooldown_ms`, then admits one probe call. A successful probe
/// closes the breaker, and a failed probe opens it again.
#[derive(Clone, Debug)]
pub struct CircuitBreaker {
    failure_threshold: u32,
    cooldown_ms: u64,
    consecutive_failures: u32,
    state: CircuitState,
    opened_at_ms: u64,
    probe_in_flight: bool,
}

impl CircuitBreaker {
    /// Creates a closed breaker.
    pub fn new(failure_threshold: u32, cooldown_ms: u64) -> Result<Self, &'static str> {
        if failure_threshold == 0 {
            return Err("failure_threshold must be at least 1");
        }
        Ok(Self {
            failure_threshold,
            cooldown_ms,
            consecutive_failures: 0,
            state: CircuitState::Closed,
            opened_at_ms: 0,
            probe_in_flight: false,
        })
    }

    /// The current state.
    pub fn state(&self) -> CircuitState {
        self.state
    }

    /// Returns `true` if a call may be made now.
    ///
    /// Calling this moves an open breaker to half-open once the cooldown has
    /// elapsed, and that call is the probe. While a probe is in flight, other
    /// calls are rejected.
    pub fn allow(&mut self, now_ms: u64) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if now_ms.saturating_sub(self.opened_at_ms) >= self.cooldown_ms {
                    self.state = CircuitState::HalfOpen;
                    self.probe_in_flight = true;
                    true
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => {
                if self.probe_in_flight {
                    false
                } else {
                    self.probe_in_flight = true;
                    true
                }
            }
        }
    }

    /// Records a successful call and closes the breaker.
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.probe_in_flight = false;
        self.state = CircuitState::Closed;
    }

    /// Records a failed call at `now_ms`, opening the breaker if the failure
    /// threshold is reached or a half-open probe failed.
    pub fn record_failure(&mut self, now_ms: u64) {
        self.probe_in_flight = false;
        match self.state {
            CircuitState::HalfOpen => self.trip(now_ms),
            CircuitState::Closed => {
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                if self.consecutive_failures >= self.failure_threshold {
                    self.trip(now_ms);
                }
            }
            CircuitState::Open => {}
        }
    }

    fn trip(&mut self, now_ms: u64) {
        self.state = CircuitState::Open;
        self.opened_at_ms = now_ms;
        self.consecutive_failures = 0;
    }
}

/// A token-bucket rate limiter.
///
/// Holds up to `capacity` tokens and refills at `refill_per_second`. Each call
/// spends one token, so bursts up to `capacity` are allowed and the sustained
/// rate is capped.
#[derive(Clone, Debug)]
pub struct TokenBucket {
    capacity: f64,
    refill_per_second: f64,
    tokens: f64,
    last_ms: u64,
}

impl TokenBucket {
    /// Creates a full bucket as of `now_ms`.
    pub fn new(capacity: u32, refill_per_second: u32, now_ms: u64) -> Result<Self, &'static str> {
        if capacity == 0 {
            return Err("rate capacity must be at least 1");
        }
        if refill_per_second == 0 {
            return Err("rate refill_per_second must be at least 1");
        }
        Ok(Self {
            capacity: f64::from(capacity),
            refill_per_second: f64::from(refill_per_second),
            tokens: f64::from(capacity),
            last_ms: now_ms,
        })
    }

    /// Takes one token if available. Returns `false` when the bucket is empty.
    pub fn try_acquire(&mut self, now_ms: u64) -> bool {
        let elapsed = now_ms.saturating_sub(self.last_ms) as f64;
        self.tokens = (self.tokens + elapsed * self.refill_per_second / 1000.0).min(self.capacity);
        self.last_ms = now_ms.max(self.last_ms);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Settings for one service registered with a [`ServiceMesh`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceConfig {
    pub retry: RetryPolicy,
    pub failure_threshold: u32,
    pub cooldown_ms: u64,
    pub rate_capacity: u32,
    pub rate_per_second: u32,
}

/// Identifies a span so that a nested call can join its caller's trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceContext {
    pub trace_id: u64,
    pub span_id: u64,
}

/// How a span ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpanOutcome {
    Success,
    Failure,
    /// Not sent. The circuit was open.
    CircuitOpen,
    /// Not sent. The rate limit was exceeded.
    RateLimited,
}

/// One attempt, or one rejected call, recorded for distributed tracing.
///
/// Rejected calls have `attempt == 0`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpanRecord {
    pub trace_id: u64,
    pub span_id: u64,
    pub parent_span_id: Option<u64>,
    pub service: String,
    pub attempt: u32,
    pub outcome: SpanOutcome,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// Why a call through the mesh did not succeed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError<E> {
    UnknownService {
        service: String,
    },
    RateLimited {
        service: String,
    },
    CircuitOpen {
        service: String,
    },
    /// The service ran and returned an error, after any retries.
    Failed(E),
}

#[derive(Default)]
struct IdGen(u64);

impl IdGen {
    fn next_id(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
}

struct ServiceState {
    breaker: CircuitBreaker,
    bucket: TokenBucket,
}

/// Routes calls to services with retry, circuit breaking, rate limiting, and tracing.
#[derive(Default)]
pub struct ServiceMesh {
    configs: HashMap<String, ServiceConfig>,
    states: HashMap<String, ServiceState>,
    spans: Vec<SpanRecord>,
    ids: IdGen,
}

impl ServiceMesh {
    /// Creates an empty mesh.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a service. `now_ms` starts its rate limiter.
    pub fn register(
        &mut self,
        service: &str,
        config: ServiceConfig,
        now_ms: u64,
    ) -> Result<(), &'static str> {
        if self.configs.contains_key(service) {
            return Err("service is already registered");
        }
        let state = ServiceState {
            breaker: CircuitBreaker::new(config.failure_threshold, config.cooldown_ms)?,
            bucket: TokenBucket::new(config.rate_capacity, config.rate_per_second, now_ms)?,
        };
        self.configs.insert(service.to_string(), config);
        self.states.insert(service.to_string(), state);
        Ok(())
    }

    /// The circuit state for a registered service.
    pub fn circuit_state(&self, service: &str) -> Option<CircuitState> {
        self.states.get(service).map(|s| s.breaker.state())
    }

    /// Returns and clears the spans recorded so far.
    pub fn take_spans(&mut self) -> Vec<SpanRecord> {
        std::mem::take(&mut self.spans)
    }

    /// Calls `op` on `service`.
    ///
    /// The rate limit and circuit are checked once up front. Each attempt is
    /// then recorded in the breaker, and a retry happens only if `is_retryable`
    /// accepts the error. If the circuit opens during a retry, the call stops.
    ///
    /// `clock` returns the current time in milliseconds. `sleep` is given each
    /// backoff delay. `parent` joins this call to a caller's trace. Without it,
    /// the call starts a new trace.
    pub fn call<T, E>(
        &mut self,
        service: &str,
        parent: Option<TraceContext>,
        clock: &dyn Fn() -> u64,
        sleep: &mut dyn FnMut(u64),
        mut op: impl FnMut(u32) -> Result<T, E>,
        is_retryable: impl Fn(&E) -> bool,
    ) -> Result<T, CallError<E>> {
        let Self {
            configs,
            states,
            spans,
            ids,
        } = self;

        let config = *configs
            .get(service)
            .ok_or_else(|| CallError::UnknownService {
                service: service.to_string(),
            })?;
        let state = states
            .get_mut(service)
            .ok_or_else(|| CallError::UnknownService {
                service: service.to_string(),
            })?;

        let trace = match parent {
            Some(ctx) => Trace {
                trace_id: ctx.trace_id,
                parent_span_id: Some(ctx.span_id),
            },
            None => Trace {
                trace_id: ids.next_id(),
                parent_span_id: None,
            },
        };

        let now = clock();
        if !state.bucket.try_acquire(now) {
            spans.push(trace.span(ids, service, 0, SpanOutcome::RateLimited, now, now));
            return Err(CallError::RateLimited {
                service: service.to_string(),
            });
        }
        if !state.breaker.allow(now) {
            spans.push(trace.span(ids, service, 0, SpanOutcome::CircuitOpen, now, now));
            return Err(CallError::CircuitOpen {
                service: service.to_string(),
            });
        }

        let mut attempt: u32 = 1;
        loop {
            let start = clock();
            match op(attempt) {
                Ok(value) => {
                    let end = clock();
                    state.breaker.record_success();
                    spans.push(trace.span(ids, service, attempt, SpanOutcome::Success, start, end));
                    return Ok(value);
                }
                Err(err) => {
                    let end = clock();
                    state.breaker.record_failure(end);
                    spans.push(trace.span(ids, service, attempt, SpanOutcome::Failure, start, end));

                    if attempt >= config.retry.max_attempts() || !is_retryable(&err) {
                        return Err(CallError::Failed(err));
                    }

                    sleep(config.retry.backoff_ms(attempt - 1));
                    let resume = clock();
                    if !state.breaker.allow(resume) {
                        spans.push(trace.span(
                            ids,
                            service,
                            0,
                            SpanOutcome::CircuitOpen,
                            resume,
                            resume,
                        ));
                        return Err(CallError::CircuitOpen {
                            service: service.to_string(),
                        });
                    }
                    attempt += 1;
                }
            }
        }
    }
}

/// The trace a call belongs to, used to stamp its spans.
struct Trace {
    trace_id: u64,
    parent_span_id: Option<u64>,
}

impl Trace {
    fn span(
        &self,
        ids: &mut IdGen,
        service: &str,
        attempt: u32,
        outcome: SpanOutcome,
        start_ms: u64,
        end_ms: u64,
    ) -> SpanRecord {
        SpanRecord {
            trace_id: self.trace_id,
            span_id: ids.next_id(),
            parent_span_id: self.parent_span_id,
            service: service.to_string(),
            attempt,
            outcome,
            start_ms,
            end_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn policy() -> RetryPolicy {
        RetryPolicy::new(4, 100, 1_000).unwrap()
    }

    fn config() -> ServiceConfig {
        ServiceConfig {
            retry: policy(),
            failure_threshold: 5,
            cooldown_ms: 500,
            rate_capacity: 10,
            rate_per_second: 10,
        }
    }

    #[test]
    fn retry_policy_rejects_invalid_values() {
        assert!(RetryPolicy::new(0, 100, 1_000).is_err());
        assert!(RetryPolicy::new(3, 0, 1_000).is_err());
        assert!(RetryPolicy::new(3, 500, 100).is_err());
        assert!(RetryPolicy::new(1, 100, 100).is_ok());
    }

    #[test]
    fn backoff_doubles_and_is_capped() {
        let p = policy();
        assert_eq!(p.backoff_ms(0), 100);
        assert_eq!(p.backoff_ms(1), 200);
        assert_eq!(p.backoff_ms(2), 400);
        assert_eq!(p.backoff_ms(3), 800);
        assert_eq!(p.backoff_ms(4), 1_000);
        assert_eq!(p.backoff_ms(200), 1_000);
    }

    #[test]
    fn retry_succeeds_after_transient_failures_with_exponential_sleeps() {
        let sleeps = RefCell::new(Vec::new());
        let result: Result<u32, &str> = retry(
            policy(),
            |attempt| {
                if attempt < 3 {
                    Err("busy")
                } else {
                    Ok(attempt)
                }
            },
            |_| true,
            |ms| sleeps.borrow_mut().push(ms),
        );
        assert_eq!(result, Ok(3));
        assert_eq!(*sleeps.borrow(), vec![100, 200]);
    }

    #[test]
    fn retry_stops_on_non_retryable_error() {
        let mut calls = 0;
        let result: Result<(), &str> = retry(
            policy(),
            |_| {
                calls += 1;
                Err("invalid input")
            },
            |e| *e != "invalid input",
            |_| {},
        );
        assert_eq!(result, Err("invalid input"));
        assert_eq!(calls, 1);
    }

    #[test]
    fn retry_returns_last_error_when_attempts_run_out() {
        let mut calls = 0;
        let result: Result<(), u32> = retry(
            RetryPolicy::new(3, 10, 10).unwrap(),
            |attempt| {
                calls += 1;
                Err(attempt)
            },
            |_| true,
            |_| {},
        );
        assert_eq!(result, Err(3));
        assert_eq!(calls, 3);
    }

    #[test]
    fn breaker_opens_after_threshold_and_rejects_during_cooldown() {
        let mut b = CircuitBreaker::new(2, 500).unwrap();
        assert!(b.allow(0));
        b.record_failure(0);
        assert_eq!(b.state(), CircuitState::Closed);
        b.record_failure(10);
        assert_eq!(b.state(), CircuitState::Open);
        assert!(!b.allow(100));
        assert!(!b.allow(509));
    }

    #[test]
    fn breaker_admits_single_probe_after_cooldown() {
        let mut b = CircuitBreaker::new(1, 500).unwrap();
        b.record_failure(0);
        assert!(b.allow(500));
        assert_eq!(b.state(), CircuitState::HalfOpen);
        assert!(!b.allow(501), "second call during probe must be rejected");
    }

    #[test]
    fn successful_probe_closes_breaker() {
        let mut b = CircuitBreaker::new(1, 500).unwrap();
        b.record_failure(0);
        assert!(b.allow(600));
        b.record_success();
        assert_eq!(b.state(), CircuitState::Closed);
        assert!(b.allow(601));
    }

    #[test]
    fn failed_probe_reopens_breaker_for_a_new_cooldown() {
        let mut b = CircuitBreaker::new(1, 500).unwrap();
        b.record_failure(0);
        assert!(b.allow(500));
        b.record_failure(500);
        assert_eq!(b.state(), CircuitState::Open);
        assert!(!b.allow(999));
        assert!(b.allow(1_000));
    }

    #[test]
    fn success_resets_consecutive_failure_count() {
        let mut b = CircuitBreaker::new(2, 500).unwrap();
        b.record_failure(0);
        b.record_success();
        b.record_failure(10);
        assert_eq!(
            b.state(),
            CircuitState::Closed,
            "one failure after a success must not open"
        );
    }

    #[test]
    fn token_bucket_allows_burst_then_refills() {
        let mut bucket = TokenBucket::new(2, 1, 0).unwrap(); // 1 token per second
        assert!(bucket.try_acquire(0));
        assert!(bucket.try_acquire(0));
        assert!(!bucket.try_acquire(0), "burst capacity exhausted");
        assert!(!bucket.try_acquire(500), "half a token is not enough");
        assert!(bucket.try_acquire(1_000));
        assert!(!bucket.try_acquire(1_000));
    }

    #[test]
    fn token_bucket_never_exceeds_capacity() {
        let mut bucket = TokenBucket::new(2, 1_000, 0).unwrap();
        // A long idle period must not bank more than the capacity.
        assert!(bucket.try_acquire(60_000));
        assert!(bucket.try_acquire(60_000));
        assert!(!bucket.try_acquire(60_000));
    }

    #[test]
    fn mesh_rejects_unknown_service() {
        let mut mesh = ServiceMesh::new();
        let clock = || 0;
        let result: Result<(), CallError<()>> =
            mesh.call("missing", None, &clock, &mut |_| {}, |_| Ok(()), |_| false);
        assert_eq!(
            result,
            Err(CallError::UnknownService {
                service: "missing".into()
            })
        );
    }

    #[test]
    fn mesh_retries_and_records_one_span_per_attempt_in_one_trace() {
        let mut mesh = ServiceMesh::new();
        mesh.register("lifecycle", config(), 0).unwrap();
        let now = Cell::new(0u64);
        let clock = || now.get();
        let result: Result<u32, CallError<&str>> = mesh.call(
            "lifecycle",
            None,
            &clock,
            &mut |ms| now.set(now.get() + ms),
            |attempt| {
                if attempt < 3 {
                    Err("busy")
                } else {
                    Ok(attempt)
                }
            },
            |_| true,
        );
        assert_eq!(result, Ok(3));

        let spans = mesh.take_spans();
        assert_eq!(spans.len(), 3);
        let trace = spans[0].trace_id;
        assert!(spans
            .iter()
            .all(|s| s.trace_id == trace && s.service == "lifecycle"));
        assert_eq!(
            spans.iter().map(|s| s.attempt).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(spans[0].outcome, SpanOutcome::Failure);
        assert_eq!(spans[2].outcome, SpanOutcome::Success);
        assert_eq!(spans[0].parent_span_id, None);
    }

    #[test]
    fn mesh_joins_parent_trace_for_nested_calls() {
        let mut mesh = ServiceMesh::new();
        mesh.register("asset", config(), 0).unwrap();
        let parent = TraceContext {
            trace_id: 42,
            span_id: 7,
        };
        let clock = || 0;
        let _: Result<(), CallError<()>> = mesh.call(
            "asset",
            Some(parent),
            &clock,
            &mut |_| {},
            |_| Ok(()),
            |_| false,
        );
        let spans = mesh.take_spans();
        assert_eq!(spans[0].trace_id, 42);
        assert_eq!(spans[0].parent_span_id, Some(7));
    }

    #[test]
    fn mesh_opens_circuit_and_rejects_without_calling_service() {
        let mut mesh = ServiceMesh::new();
        let clock = || 0;
        let mut calls = 0;

        // One attempt per call, so two failing calls reach the threshold of 2.
        let no_retry = ServiceConfig {
            retry: RetryPolicy::new(1, 10, 10).unwrap(),
            failure_threshold: 2,
            ..config()
        };
        mesh.register("engineer-no-retry", no_retry, 0).unwrap();
        for _ in 0..2 {
            let _: Result<(), CallError<&str>> = mesh.call(
                "engineer-no-retry",
                None,
                &clock,
                &mut |_| {},
                |_| Err("down"),
                |_| true,
            );
        }
        assert_eq!(
            mesh.circuit_state("engineer-no-retry"),
            Some(CircuitState::Open)
        );

        let result: Result<(), CallError<&str>> = mesh.call(
            "engineer-no-retry",
            None,
            &clock,
            &mut |_| {},
            |_| {
                calls += 1;
                Ok(())
            },
            |_| true,
        );
        assert_eq!(
            result,
            Err(CallError::CircuitOpen {
                service: "engineer-no-retry".into()
            })
        );
        assert_eq!(calls, 0, "an open circuit must not reach the service");
        assert!(mesh
            .take_spans()
            .iter()
            .any(|s| s.outcome == SpanOutcome::CircuitOpen));
    }

    #[test]
    fn mesh_rate_limits_bursts_per_service() {
        let mut mesh = ServiceMesh::new();
        let limited = ServiceConfig {
            rate_capacity: 1,
            rate_per_second: 1,
            ..config()
        };
        mesh.register("lending", limited, 0).unwrap();
        let clock = || 0;
        let first: Result<(), CallError<()>> =
            mesh.call("lending", None, &clock, &mut |_| {}, |_| Ok(()), |_| false);
        let second: Result<(), CallError<()>> =
            mesh.call("lending", None, &clock, &mut |_| {}, |_| Ok(()), |_| false);
        assert!(first.is_ok());
        assert_eq!(
            second,
            Err(CallError::RateLimited {
                service: "lending".into()
            })
        );
        assert!(mesh
            .take_spans()
            .iter()
            .any(|s| s.outcome == SpanOutcome::RateLimited));
    }

    #[test]
    fn mesh_rejects_duplicate_registration() {
        let mut mesh = ServiceMesh::new();
        mesh.register("svc", config(), 0).unwrap();
        assert!(mesh.register("svc", config(), 0).is_err());
    }
}
