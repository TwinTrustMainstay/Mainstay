//! Backend helpers for mobile clients (#1745).
//!
//! The Mainstay repo has no HTTP server. These helpers hold the logic a mobile
//! API needs, and they have no dependencies and no transport:
//!
//! - **API versioning**: negotiate a client's requested major version.
//! - **Rate limiting**: per-client token bucket for mobile traffic.
//! - **Offline-first sync**: a change log with cursor-based delta pages, and
//!   compaction so a client that was offline gets only the latest state per entity.
//! - **Push notifications**: a bounded per-device queue with de-duplication and ack.
//! - **Payload minimization**: field projection so clients ask only for the fields they render.
//!
//! Wire these into an HTTP server (for example the one described in
//! `terraform/modules/api-server`) to serve mobile clients.

use std::collections::{BTreeMap, HashMap, VecDeque};

/// Errors returned by the mobile API helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobileApiError {
    /// The version string is not of the form `v<major>` or `<major>`.
    InvalidVersion(String),
    /// The requested major version is not supported by this server.
    UnsupportedVersion { requested: u32, supported: Vec<u32> },
    /// The client has exhausted its request budget. Retry after the given seconds.
    RateLimited { retry_after_secs: u64 },
}

/// Parse `v2` or `2` into a major version number.
pub fn parse_version(raw: &str) -> Result<u32, MobileApiError> {
    raw.strip_prefix('v')
        .unwrap_or(raw)
        .parse::<u32>()
        .map_err(|_| MobileApiError::InvalidVersion(raw.to_string()))
}

/// Resolve a client's requested API version against the server's supported majors.
///
/// Returns the requested major if it is supported, or an error that lists what is.
pub fn negotiate_version(requested: &str, supported: &[u32]) -> Result<u32, MobileApiError> {
    let major = parse_version(requested)?;
    if supported.contains(&major) {
        Ok(major)
    } else {
        Err(MobileApiError::UnsupportedVersion {
            requested: major,
            supported: supported.to_vec(),
        })
    }
}

/// Token-bucket rate limiter keyed by client ID.
///
/// Time is passed in explicitly (seconds) so the limiter is deterministic and easy
/// to test. Token math uses milli-tokens to avoid floating point.
#[derive(Debug)]
pub struct RateLimiter {
    capacity_milli: u64,
    refill_milli_per_sec: u64,
    buckets: HashMap<String, Bucket>,
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens_milli: u64,
    last_secs: u64,
}

impl RateLimiter {
    /// Create a limiter that allows bursts of `capacity` requests and refills at
    /// `refill_per_sec` requests per second.
    ///
    /// # Panics
    /// Panics if `capacity` or `refill_per_sec` is zero.
    pub fn new(capacity: u32, refill_per_sec: u32) -> Self {
        assert!(capacity > 0, "capacity must be > 0");
        assert!(refill_per_sec > 0, "refill_per_sec must be > 0");
        Self {
            capacity_milli: capacity as u64 * 1000,
            refill_milli_per_sec: refill_per_sec as u64 * 1000,
            buckets: HashMap::new(),
        }
    }

    /// Consume one request token for `client` at time `now_secs`.
    ///
    /// # Errors
    /// Returns [`MobileApiError::RateLimited`] with the seconds until the next token if the bucket is empty.
    pub fn check(&mut self, client: &str, now_secs: u64) -> Result<(), MobileApiError> {
        let cap = self.capacity_milli;
        let refill = self.refill_milli_per_sec;
        let bucket = self.buckets.entry(client.to_string()).or_insert(Bucket {
            tokens_milli: cap,
            last_secs: now_secs,
        });

        let elapsed = now_secs.saturating_sub(bucket.last_secs);
        bucket.tokens_milli = cap.min(
            bucket
                .tokens_milli
                .saturating_add(elapsed.saturating_mul(refill)),
        );
        bucket.last_secs = now_secs;

        if bucket.tokens_milli >= 1000 {
            bucket.tokens_milli -= 1000;
            Ok(())
        } else {
            let missing = 1000 - bucket.tokens_milli;
            Err(MobileApiError::RateLimited {
                retry_after_secs: missing.div_ceil(refill),
            })
        }
    }
}

/// One change to an entity, ordered by `seq`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub seq: u64,
    pub entity_id: u64,
    /// `true` if the entity was removed. `payload` is empty in that case.
    pub deleted: bool,
    pub payload: String,
}

/// One page of changes returned to a syncing client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPage {
    pub changes: Vec<Change>,
    /// Pass this back as `since` on the next request. Equals `since` when nothing new.
    pub next_cursor: u64,
    pub has_more: bool,
}

/// Append-only change log that backs delta sync.
#[derive(Debug, Default)]
pub struct ChangeLog {
    next_seq: u64,
    changes: Vec<Change>,
}

impl ChangeLog {
    /// Record an upsert (`Some(payload)`) or a delete (`None`). Returns the new sequence number.
    pub fn record(&mut self, entity_id: u64, payload: Option<String>) -> u64 {
        self.next_seq += 1;
        self.changes.push(Change {
            seq: self.next_seq,
            entity_id,
            deleted: payload.is_none(),
            payload: payload.unwrap_or_default(),
        });
        self.next_seq
    }

    /// Return up to `limit` changes with `seq > since`, in order.
    pub fn delta(&self, since: u64, limit: usize) -> SyncPage {
        let start = self.changes.partition_point(|c| c.seq <= since);
        let end = (start + limit).min(self.changes.len());
        let changes = self.changes[start..end].to_vec();
        let next_cursor = changes.last().map_or(since, |c| c.seq);
        SyncPage {
            changes,
            next_cursor,
            has_more: end < self.changes.len(),
        }
    }
}

/// Keep only the latest change per entity, in sequence order.
///
/// A client that has been offline for a while only needs the final state of each
/// entity, not every intermediate edit. Compacting cuts the payload size.
pub fn compact(changes: &[Change]) -> Vec<Change> {
    let mut latest: BTreeMap<u64, &Change> = BTreeMap::new();
    for change in changes {
        latest.insert(change.entity_id, change);
    }
    let mut out: Vec<Change> = latest.into_values().cloned().collect();
    out.sort_by_key(|c| c.seq);
    out
}

/// A push notification addressed to one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushNotification {
    pub id: u64,
    pub kind: String,
    pub body: String,
}

/// Per-device push queue. Bounded: when a device's queue is full, the oldest entry is dropped.
#[derive(Debug)]
pub struct PushQueue {
    max_per_device: usize,
    queues: HashMap<String, VecDeque<PushNotification>>,
}

impl PushQueue {
    /// # Panics
    /// Panics if `max_per_device` is zero.
    pub fn new(max_per_device: usize) -> Self {
        assert!(max_per_device > 0, "max_per_device must be > 0");
        Self {
            max_per_device,
            queues: HashMap::new(),
        }
    }

    /// Queue a notification for `device`. Returns `false` if a notification with the same ID is already queued.
    pub fn enqueue(&mut self, device: &str, notification: PushNotification) -> bool {
        let queue = self.queues.entry(device.to_string()).or_default();
        if queue.iter().any(|n| n.id == notification.id) {
            return false;
        }
        if queue.len() == self.max_per_device {
            queue.pop_front();
        }
        queue.push_back(notification);
        true
    }

    /// Notifications still waiting for `device`, oldest first.
    pub fn pending(&self, device: &str) -> Vec<PushNotification> {
        self.queues
            .get(device)
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Acknowledge delivered notifications. Returns how many were removed.
    pub fn ack(&mut self, device: &str, ids: &[u64]) -> usize {
        let Some(queue) = self.queues.get_mut(device) else {
            return 0;
        };
        let before = queue.len();
        queue.retain(|n| !ids.contains(&n.id));
        before - queue.len()
    }
}

/// Split a comma-separated field list such as `"id,status"` into field names.
/// Blank entries are dropped.
pub fn parse_fields(raw: &str) -> Vec<&str> {
    raw.split(',')
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .collect()
}

/// Return only the requested fields of a record. An empty `fields` list returns the whole record.
/// Unknown field names are ignored.
pub fn select_fields(
    record: &BTreeMap<String, String>,
    fields: &[&str],
) -> BTreeMap<String, String> {
    if fields.is_empty() {
        return record.clone();
    }
    fields
        .iter()
        .filter_map(|f| record.get(*f).map(|v| (f.to_string(), v.clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> BTreeMap<String, String> {
        [("id", "7"), ("status", "Active"), ("notes", "long text")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn version_parses_with_or_without_prefix() {
        assert_eq!(parse_version("v2"), Ok(2));
        assert_eq!(parse_version("3"), Ok(3));
        assert!(matches!(
            parse_version("vx"),
            Err(MobileApiError::InvalidVersion(_))
        ));
    }

    #[test]
    fn negotiation_accepts_supported_and_lists_supported_on_failure() {
        assert_eq!(negotiate_version("v1", &[1, 2]), Ok(1));
        assert_eq!(
            negotiate_version("v9", &[1, 2]),
            Err(MobileApiError::UnsupportedVersion {
                requested: 9,
                supported: vec![1, 2]
            })
        );
    }

    #[test]
    fn rate_limiter_allows_burst_then_limits() {
        let mut rl = RateLimiter::new(2, 1);
        assert!(rl.check("app", 100).is_ok());
        assert!(rl.check("app", 100).is_ok());
        assert_eq!(
            rl.check("app", 100),
            Err(MobileApiError::RateLimited {
                retry_after_secs: 1
            })
        );
    }

    #[test]
    fn rate_limiter_refills_over_time_and_isolates_clients() {
        let mut rl = RateLimiter::new(1, 1);
        assert!(rl.check("a", 0).is_ok());
        assert!(rl.check("a", 0).is_err());
        assert!(rl.check("a", 1).is_ok());
        // Another client is unaffected
        assert!(rl.check("b", 1).is_ok());
    }

    #[test]
    fn delta_returns_pages_with_cursor() {
        let mut log = ChangeLog::default();
        for id in 1..=5 {
            log.record(id, Some(format!("v{id}")));
        }
        let first = log.delta(0, 2);
        assert_eq!(first.changes.len(), 2);
        assert!(first.has_more);
        assert_eq!(first.next_cursor, 2);

        let second = log.delta(first.next_cursor, 10);
        assert_eq!(
            second.changes.iter().map(|c| c.seq).collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
        assert!(!second.has_more);

        let caught_up = log.delta(5, 10);
        assert!(caught_up.changes.is_empty());
        assert_eq!(caught_up.next_cursor, 5);
    }

    #[test]
    fn deletes_are_recorded_as_tombstones() {
        let mut log = ChangeLog::default();
        log.record(9, None);
        let page = log.delta(0, 10);
        assert!(page.changes[0].deleted);
        assert!(page.changes[0].payload.is_empty());
    }

    #[test]
    fn compact_keeps_latest_per_entity_in_order() {
        let mut log = ChangeLog::default();
        log.record(1, Some("a".into()));
        log.record(2, Some("b".into()));
        log.record(1, Some("a2".into()));
        log.record(2, None);
        let page = log.delta(0, 100);
        let compacted = compact(&page.changes);
        assert_eq!(compacted.len(), 2);
        assert_eq!(compacted[0].entity_id, 1);
        assert_eq!(compacted[0].payload, "a2");
        assert!(compacted[1].deleted);
    }

    #[test]
    fn push_queue_dedupes_bounds_and_acks() {
        let mut q = PushQueue::new(2);
        let n = |id| PushNotification {
            id,
            kind: "alert".into(),
            body: format!("n{id}"),
        };
        assert!(q.enqueue("dev", n(1)));
        assert!(!q.enqueue("dev", n(1)), "duplicate id rejected");
        assert!(q.enqueue("dev", n(2)));
        assert!(q.enqueue("dev", n(3)), "oldest dropped when full");
        let ids: Vec<u64> = q.pending("dev").iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![2, 3]);
        assert_eq!(q.ack("dev", &[2]), 1);
        assert_eq!(q.pending("dev").len(), 1);
        assert_eq!(q.ack("unknown", &[1]), 0);
    }

    #[test]
    fn projection_returns_requested_fields_only() {
        let rec = record();
        let fields = parse_fields("id, status ,,missing");
        let selected = select_fields(&rec, &fields);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected.get("status").map(String::as_str), Some("Active"));
        assert!(!selected.contains_key("notes"));
        assert_eq!(select_fields(&rec, &[]), rec);
    }
}
