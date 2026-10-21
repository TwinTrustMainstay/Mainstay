//! Trusted timestamping for maintenance records (issue #1750).
//!
//! A registered timestamp authority (TSA) signs `sha256(record) || issued_at || expires_at`
//! with its Ed25519 key. Anyone may relay the signed token to the contract. The signature,
//! not the relayer, is what authenticates the timestamp, so the contract never needs to reach
//! an external timestamping service. Renewal is submitting a newer token for the same record.

use soroban_sdk::{panic_with_error, symbol_short, Address, Bytes, BytesN, Env, Vec};

use crate::errors::ContractError;
use crate::storage::history_key;
use crate::types::{DataKey, MaintenanceRecord, TimestampAuthority, TrustedTimestamp};
use shared::extend_persistent_ttl;

/// Domain-separation prefix so a TSA signature cannot be replayed as another signature type.
const TS_DOMAIN: &[u8] = b"MAINSTAY/TSA/v1";

fn load_authorities(env: &Env) -> Vec<TimestampAuthority> {
    env.storage()
        .persistent()
        .get(&DataKey::TimestampAuthorities)
        .unwrap_or_else(|| Vec::new(env))
}

fn authority_at(env: &Env, authority_id: u32) -> TimestampAuthority {
    load_authorities(env)
        .get(authority_id)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::TimestampAuthorityNotFound))
}

fn find_record(env: &Env, asset_id: u64, record_timestamp: u64) -> Option<MaintenanceRecord> {
    let history: Vec<MaintenanceRecord> = env
        .storage()
        .persistent()
        .get(&history_key(asset_id))
        .unwrap_or_else(|| Vec::new(env));
    history.iter().find(|record| record.timestamp == record_timestamp)
}

/// Message the authority signs: domain || sha256(record) || issued_at || expires_at.
fn token_message(env: &Env, record: &MaintenanceRecord, issued_at: u64, expires_at: u64) -> Bytes {
    let mut message = Bytes::from_slice(env, TS_DOMAIN);
    message.append(&crate::hash_maintenance_record(env, record));
    message.append(&Bytes::from_slice(env, &issued_at.to_be_bytes()));
    message.append(&Bytes::from_slice(env, &expires_at.to_be_bytes()));
    message
}

/// Register a new timestamp authority. Returns its `authority_id`.
pub(crate) fn register_authority(env: &Env, admin: Address, public_key: BytesN<32>) -> u32 {
    crate::ensure_not_paused(env);
    crate::require_admin(env, &admin);

    let mut authorities = load_authorities(env);
    let authority_id = authorities.len();
    authorities.push_back(TimestampAuthority {
        id: authority_id,
        public_key,
        registered_at: env.ledger().timestamp(),
        revoked_at: None,
    });
    env.storage()
        .persistent()
        .set(&DataKey::TimestampAuthorities, &authorities);
    extend_persistent_ttl(env, &DataKey::TimestampAuthorities);
    env.events()
        .publish((symbol_short!("TSA_REG"),), authority_id);
    authority_id
}

/// Revoke a timestamp authority. Tokens it issued before revocation stay valid.
pub(crate) fn revoke_authority(env: &Env, admin: Address, authority_id: u32) {
    crate::ensure_not_paused(env);
    crate::require_admin(env, &admin);

    let mut authorities = load_authorities(env);
    let mut authority = authorities
        .get(authority_id)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::TimestampAuthorityNotFound));
    if authority.revoked_at.is_some() {
        panic_with_error!(env, ContractError::TimestampAuthorityInactive);
    }
    authority.revoked_at = Some(env.ledger().timestamp());
    authorities.set(authority_id, authority);
    env.storage()
        .persistent()
        .set(&DataKey::TimestampAuthorities, &authorities);
    extend_persistent_ttl(env, &DataKey::TimestampAuthorities);
    env.events()
        .publish((symbol_short!("TSA_REV"),), authority_id);
}

/// Store a timestamp token for a maintenance record after verifying the authority's signature.
///
/// The first token for a record is an issue (`TS_ISSUE`). A later token with a larger
/// `issued_at` is a renewal (`TS_RENEW`). Any token whose `issued_at` is not later than the
/// stored one is rejected, so an older token cannot roll the timestamp back.
pub(crate) fn submit_timestamp(
    env: &Env,
    asset_id: u64,
    record_timestamp: u64,
    authority_id: u32,
    issued_at: u64,
    expires_at: u64,
    signature: BytesN<64>,
) {
    crate::ensure_not_paused(env);
    let now = env.ledger().timestamp();
    if expires_at <= issued_at || issued_at > now {
        panic_with_error!(env, ContractError::InvalidTimestampWindow);
    }

    let authority = authority_at(env, authority_id);
    let active_at_issue = issued_at >= authority.registered_at
        && authority.revoked_at.is_none_or(|revoked| issued_at < revoked);
    if !active_at_issue {
        panic_with_error!(env, ContractError::TimestampAuthorityInactive);
    }

    let record = find_record(env, asset_id, record_timestamp)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::RecordNotFound));
    let message = token_message(env, &record, issued_at, expires_at);
    env.crypto()
        .ed25519_verify(&authority.public_key, &message, &signature);

    let key = DataKey::TrustedTimestamp(asset_id, record_timestamp);
    let existing: Option<TrustedTimestamp> = env.storage().persistent().get(&key);
    if let Some(ref stored) = existing {
        if issued_at <= stored.issued_at {
            panic_with_error!(env, ContractError::TimestampNotNewer);
        }
    }

    let token = TrustedTimestamp {
        asset_id,
        record_timestamp,
        authority_id,
        issued_at,
        expires_at,
        signature,
    };
    env.storage().persistent().set(&key, &token);
    extend_persistent_ttl(env, &key);
    let topic = if existing.is_some() {
        symbol_short!("TS_RENEW")
    } else {
        symbol_short!("TS_ISSUE")
    };
    env.events()
        .publish((topic, asset_id), (record_timestamp, issued_at, expires_at));
}

pub(crate) fn get_timestamped_record(
    env: &Env,
    asset_id: u64,
    record_timestamp: u64,
) -> Option<TrustedTimestamp> {
    env.storage()
        .persistent()
        .get(&DataKey::TrustedTimestamp(asset_id, record_timestamp))
}

/// Returns `true` when the stored token is unexpired and its signature still verifies against
/// its authority's key. Returns `false` when there is no token, the token has expired, or the
/// record is no longer in history. A stored signature that fails to verify traps, as with
/// `verify_maintenance_signature`.
pub(crate) fn verify_timestamp(env: &Env, asset_id: u64, record_timestamp: u64) -> bool {
    let Some(token) = get_timestamped_record(env, asset_id, record_timestamp) else {
        return false;
    };
    if token.expires_at <= env.ledger().timestamp() {
        return false;
    }
    let Some(record) = find_record(env, asset_id, record_timestamp) else {
        return false;
    };
    let authority = authority_at(env, token.authority_id);
    let message = token_message(env, &record, token.issued_at, token.expires_at);
    env.crypto()
        .ed25519_verify(&authority.public_key, &message, &token.signature);
    true
}

pub(crate) fn get_timestamp_authorities(env: &Env) -> Vec<TimestampAuthority> {
    load_authorities(env)
}
