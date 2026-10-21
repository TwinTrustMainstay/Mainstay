//! Mobile-facing sync and notification API.
//!
//! Mobile clients cannot hold a long-lived connection to the chain, so this
//! contract exposes a bounded, cursor-based change feed that a client can
//! poll after coming back online. Its design goals map to the mobile tasks:
//!
//! * **Small payloads** – a change is a fixed-size record (kind, asset id,
//!   32-byte payload hash, timestamp). Clients fetch full details from the
//!   off-chain API only when they need them.
//! * **Push notifications** – on-chain code cannot push. Every published change
//!   emits a `CHANGE` event for an off-chain relay (which pushes to devices),
//!   and clients can pull only the topics they subscribed to via
//!   `pull_notifications`.
//! * **Offline-first sync** – each change has a monotonic sequence number.
//!   A client stores its last cursor and calls `sync` to catch up. If it has
//!   been away longer than the retained log, `full_resync_required` tells it
//!   to fetch a full snapshot instead.
//! * **Rate limiting** – every read/subscribe call is counted in a fixed window
//!   per registered client. Clients must authenticate, so limits cannot be
//!   bypassed by changing the call arguments.
//! * **API versioning** – clients register the version they speak. The server
//!   answers with the negotiated version (the lower of the two) and rejects
//!   versions below `MIN_API_VERSION`.

#![no_std]

use shared::error::SharedContractError;
use shared::{extend_persistent_ttl, TTL_TARGET, TTL_THRESHOLD};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, Symbol, Vec,
};

/// Highest API version this server speaks.
pub const CURRENT_API_VERSION: u32 = 2;
/// Oldest API version still accepted from clients.
pub const MIN_API_VERSION: u32 = 1;
/// Number of change records retained on-chain. Older records are pruned.
pub const MAX_CHANGE_LOG_ENTRIES: u64 = 1_000;
/// Largest page a single `sync` / `pull_notifications` call may return.
pub const MAX_PAGE_SIZE: u32 = 100;
/// Upper bound on sequence numbers examined by one `pull_notifications` call.
/// Filtering can skip many records, so this keeps a call's cost bounded.
pub const MAX_SCAN_PER_CALL: u64 = 200;
/// Maximum number of topics a client may subscribe to.
pub const MAX_SUBSCRIPTION_TOPICS: u32 = 16;
pub const DEFAULT_RATE_LIMIT_REQUESTS: u32 = 60;
pub const DEFAULT_RATE_LIMIT_WINDOW_SECS: u64 = 60;
pub const MAX_RATE_LIMIT_REQUESTS: u32 = 10_000;
pub const MAX_RATE_LIMIT_WINDOW_SECS: u64 = 86_400;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ContractError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    UnauthorizedAdmin = 3,
    /// The caller has not registered a client with `register_client`.
    ClientNotRegistered = 4,
    /// The client's API version is older than `MIN_API_VERSION`.
    UnsupportedApiVersion = 5,
    /// The client exceeded its request budget for the current window.
    RateLimitExceeded = 6,
    /// `limit` was zero or above `MAX_PAGE_SIZE`.
    InvalidLimit = 7,
    /// Subscription list was empty or longer than `MAX_SUBSCRIPTION_TOPICS`.
    InvalidTopics = 8,
    /// Rate-limit settings were outside their allowed bounds.
    InvalidRateLimitConfig = 9,
    /// The requested change is outside the retained log.
    ChangeNotFound = 10,
    /// `pull_notifications` was called without a subscription.
    NoSubscription = 11,
}

impl From<SharedContractError> for ContractError {
    fn from(e: SharedContractError) -> Self {
        match e {
            SharedContractError::NotInitialized => ContractError::NotInitialized,
            SharedContractError::AlreadyInitialized => ContractError::AlreadyInitialized,
            SharedContractError::UnauthorizedAdmin => ContractError::UnauthorizedAdmin,
            _ => ContractError::UnauthorizedAdmin,
        }
    }
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeRecord {
    pub seq: u64,
    /// Kind of change, e.g. `maint`, `asset`, `lien`. Used for subscription filtering.
    pub kind: Symbol,
    pub asset_id: u64,
    /// Hash of the full off-chain payload; keeps this record small.
    pub payload_hash: BytesN<32>,
    pub ledger_ts: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientInfo {
    pub address: Address,
    /// Negotiated API version (the lower of client and server versions).
    pub api_version: u32,
    pub registered_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiVersionInfo {
    pub current: u32,
    pub minimum: u32,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateLimitConfig {
    pub max_requests: u32,
    pub window_secs: u64,
}

/// A page of changes plus the cursor the client should send next time.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncPage {
    pub changes: Vec<ChangeRecord>,
    /// Pass this as `since_seq` on the next call.
    pub next_seq: u64,
    pub latest_seq: u64,
    /// True when the client's cursor is older than the retained log. The client
    /// must reload a full snapshot and restart from `latest_seq`.
    pub full_resync_required: bool,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateWindow {
    pub window_start: u64,
    pub count: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum DataKey {
    Admin,
    RateLimit,
    LatestSeq,
    FirstSeq,
    Change(u64),
    Client(Address),
    Subscription(Address),
    Window(Address),
}

fn bump_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD, TTL_TARGET);
}

fn latest_seq(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::LatestSeq)
        .unwrap_or(0)
}

fn first_seq(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::FirstSeq)
        .unwrap_or(1)
}

fn rate_limit_config(env: &Env) -> RateLimitConfig {
    env.storage()
        .instance()
        .get(&DataKey::RateLimit)
        .unwrap_or(RateLimitConfig {
            max_requests: DEFAULT_RATE_LIMIT_REQUESTS,
            window_secs: DEFAULT_RATE_LIMIT_WINDOW_SECS,
        })
}

fn require_admin(env: &Env, admin: &Address) -> Result<(), ContractError> {
    let stored: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(ContractError::NotInitialized)?;
    shared::require_admin(admin, &stored).map_err(ContractError::from)
}

fn require_registered(env: &Env, client: &Address) -> Result<ClientInfo, ContractError> {
    let key = DataKey::Client(client.clone());
    let info: ClientInfo = env
        .storage()
        .persistent()
        .get(&key)
        .ok_or(ContractError::ClientNotRegistered)?;
    extend_persistent_ttl(env, key);
    Ok(info)
}

/// Counts one request against the client's fixed window. Fails once the
/// window's budget is spent; the window resets after `window_secs`.
fn consume_rate_limit(env: &Env, client: &Address) -> Result<(), ContractError> {
    let cfg = rate_limit_config(env);
    let now = env.ledger().timestamp();
    let key = DataKey::Window(client.clone());
    let mut window: RateWindow = env.storage().persistent().get(&key).unwrap_or(RateWindow {
        window_start: now,
        count: 0,
    });

    if now.saturating_sub(window.window_start) >= cfg.window_secs {
        window = RateWindow {
            window_start: now,
            count: 0,
        };
    }
    if window.count >= cfg.max_requests {
        return Err(ContractError::RateLimitExceeded);
    }

    window.count += 1;
    env.storage().persistent().set(&key, &window);
    extend_persistent_ttl(env, key);
    Ok(())
}

fn matches_topics(change: &ChangeRecord, topics: &Option<Vec<Symbol>>) -> bool {
    match topics {
        None => true,
        Some(list) => list.iter().any(|t| t == change.kind),
    }
}

/// Reads a page of changes after `since_seq`.
///
/// `topics` filters the results when present. Scanning stops at `limit`
/// matches or `MAX_SCAN_PER_CALL` examined sequence numbers, whichever comes
/// first, and `next_seq` marks how far the scan got.
fn read_page(
    env: &Env,
    since_seq: u64,
    limit: u32,
    topics: Option<Vec<Symbol>>,
    max_scan: u64,
) -> Result<SyncPage, ContractError> {
    let latest = latest_seq(env);
    let first = first_seq(env);
    let mut changes = Vec::new(env);

    // The client's cursor points before the retained log, so changes were lost.
    if since_seq.saturating_add(1) < first {
        return Ok(SyncPage {
            changes,
            next_seq: latest,
            latest_seq: latest,
            full_resync_required: true,
        });
    }

    let mut cursor = since_seq;
    let mut seq = since_seq.saturating_add(1);
    let mut scanned = 0u64;
    while seq <= latest && changes.len() < limit && scanned < max_scan {
        let change: ChangeRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Change(seq))
            .ok_or(ContractError::ChangeNotFound)?;
        if matches_topics(&change, &topics) {
            changes.push_back(change);
        }
        cursor = seq;
        seq += 1;
        scanned += 1;
    }

    Ok(SyncPage {
        changes,
        next_seq: cursor,
        latest_seq: latest,
        full_resync_required: false,
    })
}

fn validate_limit(limit: u32) -> Result<(), ContractError> {
    if limit == 0 || limit > MAX_PAGE_SIZE {
        return Err(ContractError::InvalidLimit);
    }
    Ok(())
}

#[contract]
pub struct MobileApi;

#[contractimpl]
impl MobileApi {
    /// One-time setup. The admin is the only account allowed to publish changes
    /// and to change rate-limit settings.
    pub fn initialize(env: Env, admin: Address) -> Result<(), ContractError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ContractError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::LatestSeq, &0u64);
        env.storage().instance().set(&DataKey::FirstSeq, &1u64);
        env.storage().instance().set(
            &DataKey::RateLimit,
            &RateLimitConfig {
                max_requests: DEFAULT_RATE_LIMIT_REQUESTS,
                window_secs: DEFAULT_RATE_LIMIT_WINDOW_SECS,
            },
        );
        bump_instance(&env);
        Ok(())
    }

    /// Versions this server supports.
    pub fn get_version_info(_env: Env) -> ApiVersionInfo {
        ApiVersionInfo {
            current: CURRENT_API_VERSION,
            minimum: MIN_API_VERSION,
        }
    }

    /// Registers (or re-registers) a client and returns the negotiated API version.
    ///
    /// Re-registering with a new version updates the stored version. Clients
    /// must register before calling `sync`, `pull_notifications` or `subscribe`.
    pub fn register_client(
        env: Env,
        client: Address,
        api_version: u32,
    ) -> Result<u32, ContractError> {
        client.require_auth();
        if api_version < MIN_API_VERSION {
            return Err(ContractError::UnsupportedApiVersion);
        }
        let negotiated = api_version.min(CURRENT_API_VERSION);
        let key = DataKey::Client(client.clone());
        let info = ClientInfo {
            address: client,
            api_version: negotiated,
            registered_at: env.ledger().timestamp(),
        };
        env.storage().persistent().set(&key, &info);
        extend_persistent_ttl(&env, key);
        Ok(negotiated)
    }

    pub fn get_client(env: Env, client: Address) -> Option<ClientInfo> {
        env.storage().persistent().get(&DataKey::Client(client))
    }

    /// Sets the per-client request budget. Admin only.
    pub fn set_rate_limit(
        env: Env,
        admin: Address,
        max_requests: u32,
        window_secs: u64,
    ) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;
        if max_requests == 0
            || max_requests > MAX_RATE_LIMIT_REQUESTS
            || window_secs == 0
            || window_secs > MAX_RATE_LIMIT_WINDOW_SECS
        {
            return Err(ContractError::InvalidRateLimitConfig);
        }
        env.storage().instance().set(
            &DataKey::RateLimit,
            &RateLimitConfig {
                max_requests,
                window_secs,
            },
        );
        Ok(())
    }

    pub fn get_rate_limit(env: Env) -> RateLimitConfig {
        rate_limit_config(&env)
    }

    /// Publishes a change to the feed. Admin only.
    ///
    /// Returns the new sequence number and emits a `CHANGE` event carrying
    /// `(asset_id, seq)` for the off-chain push relay.
    pub fn publish_change(
        env: Env,
        admin: Address,
        kind: Symbol,
        asset_id: u64,
        payload_hash: BytesN<32>,
    ) -> Result<u64, ContractError> {
        require_admin(&env, &admin)?;

        let seq = latest_seq(&env) + 1;
        let change = ChangeRecord {
            seq,
            kind,
            asset_id,
            payload_hash,
            ledger_ts: env.ledger().timestamp(),
        };
        let key = DataKey::Change(seq);
        env.storage().persistent().set(&key, &change);
        extend_persistent_ttl(&env, key);
        env.storage().instance().set(&DataKey::LatestSeq, &seq);

        // Prune the oldest record once the log is full. Clients whose cursor
        // falls behind the pruned range get `full_resync_required`.
        let mut first = first_seq(&env);
        if seq + 1 - first > MAX_CHANGE_LOG_ENTRIES {
            env.storage().persistent().remove(&DataKey::Change(first));
            first += 1;
            env.storage().instance().set(&DataKey::FirstSeq, &first);
        }

        env.events()
            .publish((soroban_sdk::symbol_short!("CHANGE"), asset_id), seq);
        bump_instance(&env);
        Ok(seq)
    }

    pub fn get_latest_seq(env: Env) -> u64 {
        latest_seq(&env)
    }

    /// Returns one retained change record.
    pub fn get_change(env: Env, seq: u64) -> Result<ChangeRecord, ContractError> {
        if seq < first_seq(&env) || seq > latest_seq(&env) {
            return Err(ContractError::ChangeNotFound);
        }
        env.storage()
            .persistent()
            .get(&DataKey::Change(seq))
            .ok_or(ContractError::ChangeNotFound)
    }

    /// Catch-up sync for offline-first clients. Returns every change after
    /// `since_seq`, up to `limit`.
    pub fn sync(
        env: Env,
        client: Address,
        since_seq: u64,
        limit: u32,
    ) -> Result<SyncPage, ContractError> {
        client.require_auth();
        require_registered(&env, &client)?;
        consume_rate_limit(&env, &client)?;
        validate_limit(limit)?;
        read_page(&env, since_seq, limit, None, MAX_SCAN_PER_CALL)
    }

    /// Subscribes the client to a set of change kinds. Replaces any earlier subscription.
    pub fn subscribe(env: Env, client: Address, topics: Vec<Symbol>) -> Result<(), ContractError> {
        client.require_auth();
        require_registered(&env, &client)?;
        consume_rate_limit(&env, &client)?;
        if topics.is_empty() || topics.len() > MAX_SUBSCRIPTION_TOPICS {
            return Err(ContractError::InvalidTopics);
        }
        let key = DataKey::Subscription(client);
        env.storage().persistent().set(&key, &topics);
        extend_persistent_ttl(&env, key);
        Ok(())
    }

    pub fn unsubscribe(env: Env, client: Address) -> Result<(), ContractError> {
        client.require_auth();
        require_registered(&env, &client)?;
        env.storage()
            .persistent()
            .remove(&DataKey::Subscription(client));
        Ok(())
    }

    pub fn get_subscription(env: Env, client: Address) -> Option<Vec<Symbol>> {
        env.storage()
            .persistent()
            .get(&DataKey::Subscription(client))
    }

    /// Pulls only the changes matching the client's subscription, after `since_seq`.
    ///
    /// Scans at most `MAX_SCAN_PER_CALL` sequence numbers per call. When the
    /// page is empty but `next_seq < latest_seq`, call again with `next_seq`.
    pub fn pull_notifications(
        env: Env,
        client: Address,
        since_seq: u64,
        limit: u32,
    ) -> Result<SyncPage, ContractError> {
        client.require_auth();
        require_registered(&env, &client)?;
        consume_rate_limit(&env, &client)?;
        validate_limit(limit)?;
        let topics: Vec<Symbol> = env
            .storage()
            .persistent()
            .get(&DataKey::Subscription(client))
            .ok_or(ContractError::NoSubscription)?;
        read_page(&env, since_seq, limit, Some(topics), MAX_SCAN_PER_CALL)
    }
}

#[cfg(test)]
mod test;
