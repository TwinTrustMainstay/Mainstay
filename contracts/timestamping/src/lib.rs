//! Trusted timestamping for maintenance records.
//!
//! On-chain timestamps can be forged by whoever controls the submitting
//! account. This contract accepts a creation time only when a registered
//! timestamp authority (TSA) has signed it:
//!
//! * **Authorities.** The admin registers TSA Ed25519 public keys with a
//!   validity window. Authorities can be renewed and revoked. Every change is
//!   recorded in an append-only history.
//! * **Creation proof.** A TSA signs `TSA_DOMAIN || record_id || record_hash || timestamp`.
//!   `submit_timestamp` verifies the signature and stores the record with its
//!   signature, so `get_timestamped_record` proves when the record existed.
//! * **Expiry and renewal.** Each timestamp expires after the configured
//!   validity period. `renew_timestamp` extends it with a newer TSA signature
//!   over `RENEWAL_DOMAIN || ...`, so the chain of attestations stays verifiable.
//! * **Revocation.** If an authority is revoked, timestamps it issued at or
//!   after the revocation time are no longer valid.
//!
//! Integration with an external TSA runs off-chain. A relayer obtains the
//! authority's signed attestation (for example, by wrapping an RFC 3161 token
//! in a signing proxy), then submits it here. See `docs/timestamping.md`.

#![no_std]

#[cfg(test)]
mod test;

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Bytes, BytesN, Env,
    Symbol, Vec,
};

/// Domain tag for creation attestations.
pub const TSA_DOMAIN: &[u8] = b"MAINSTAY-TSA-CREATE-V1";
/// Domain tag for renewal attestations. Keeping it separate stops a creation signature from being reused as a renewal.
pub const RENEWAL_DOMAIN: &[u8] = b"MAINSTAY-TSA-RENEW-V1";
/// Default timestamp validity (5 years).
pub const DEFAULT_RECORD_VALIDITY_SECS: u64 = 5 * 365 * 24 * 60 * 60;
/// Shortest validity an admin may set (1 year).
pub const MIN_RECORD_VALIDITY_SECS: u64 = 365 * 24 * 60 * 60;
/// Longest validity an admin may set (20 years).
pub const MAX_RECORD_VALIDITY_SECS: u64 = 20 * 365 * 24 * 60 * 60;
/// A TSA attestation may be at most this far ahead of the ledger clock.
pub const MAX_CLOCK_SKEW_SECS: u64 = 300;
/// Authority history entries retained for audit.
pub const MAX_HISTORY_ENTRIES: u32 = 200;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ContractError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    UnauthorizedAdmin = 3,
    /// No authority is registered under this key.
    AuthorityNotFound = 4,
    /// An authority with this key is already registered (including revoked ones).
    AuthorityAlreadyExists = 5,
    /// The authority has been revoked and can no longer issue timestamps.
    AuthorityRevoked = 6,
    /// The attestation's time is past the authority's validity window.
    AuthorityExpired = 7,
    /// A timestamp is already recorded for this record id.
    RecordExists = 8,
    RecordNotFound = 9,
    /// The attested time is further ahead of the ledger clock than `MAX_CLOCK_SKEW_SECS`.
    FutureTimestamp = 10,
    /// A renewal's time is not later than the record's latest timestamp.
    StaleRenewal = 11,
    /// The record's validity has passed, so it can no longer be renewed.
    RecordExpired = 12,
    /// Validity period is outside the allowed bounds.
    InvalidValidity = 13,
    /// The authority.s validity end is not in the future.
    InvalidAuthorityValidity = 14,
    /// The authority history has reached `MAX_HISTORY_ENTRIES`.
    HistoryFull = 15,
    /// A revocation cannot take effect in the future.
    InvalidRevocationTime = 16,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Authority {
    pub key: BytesN<32>,
    pub added_at: u64,
    /// Attestations with a time after this are rejected.
    pub valid_until: u64,
    /// Set by revocation. Timestamps at or after this time are invalid, and no new
    /// attestations are accepted. The admin may backdate this to when a key was compromised.
    pub revoked_at: Option<u64>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorityEvent {
    pub key: BytesN<32>,
    /// `added`, `renewed`, or `revoked`.
    pub action: Symbol,
    pub at: u64,
    pub valid_until: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimestampedRecord {
    pub record_id: u64,
    pub record_hash: BytesN<32>,
    /// The creation time, attested by `authority`. This never changes.
    pub created_at: u64,
    pub authority: BytesN<32>,
    pub creation_signature: BytesN<64>,
    /// The latest attested time. Equals `created_at` until the record is renewed.
    pub last_timestamp: u64,
    pub renewal_count: u32,
    /// Signature of the latest renewal, if any.
    pub renewal_signature: Option<BytesN<64>>,
    pub expires_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum DataKey {
    Admin,
    RecordValidity,
    Authority(BytesN<32>),
    History,
    Record(u64),
}

fn require_admin(env: &Env, admin: &Address) -> Result<(), ContractError> {
    let stored: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(ContractError::NotInitialized)?;
    admin.require_auth();
    if *admin != stored {
        return Err(ContractError::UnauthorizedAdmin);
    }
    Ok(())
}

fn record_validity(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::RecordValidity)
        .unwrap_or(DEFAULT_RECORD_VALIDITY_SECS)
}

fn load_authority(env: &Env, key: &BytesN<32>) -> Result<Authority, ContractError> {
    env.storage()
        .persistent()
        .get(&DataKey::Authority(key.clone()))
        .ok_or(ContractError::AuthorityNotFound)
}

fn append_history(env: &Env, event: AuthorityEvent) -> Result<(), ContractError> {
    let mut history: Vec<AuthorityEvent> = env
        .storage()
        .persistent()
        .get(&DataKey::History)
        .unwrap_or(Vec::new(env));
    if history.len() >= MAX_HISTORY_ENTRIES {
        return Err(ContractError::HistoryFull);
    }
    history.push_back(event);
    env.storage().persistent().set(&DataKey::History, &history);
    Ok(())
}

/// Builds the signed message: `domain || record_id (8 bytes BE) || record_hash || timestamp (8 bytes BE)`.
fn attestation_message(
    env: &Env,
    domain: &[u8],
    record_id: u64,
    record_hash: &BytesN<32>,
    timestamp: u64,
) -> Bytes {
    let mut msg = Bytes::from_slice(env, domain);
    msg.append(&Bytes::from_slice(env, &record_id.to_be_bytes()));
    msg.append(&Bytes::from_array(env, &record_hash.to_array()));
    msg.append(&Bytes::from_slice(env, &timestamp.to_be_bytes()));
    msg
}

/// Checks that `authority` may attest to `timestamp` at ledger time `now`.
fn check_authority_window(
    authority: &Authority,
    timestamp: u64,
    now: u64,
) -> Result<(), ContractError> {
    if authority.revoked_at.is_some() {
        return Err(ContractError::AuthorityRevoked);
    }
    if timestamp > authority.valid_until {
        return Err(ContractError::AuthorityExpired);
    }
    if timestamp > now.saturating_add(MAX_CLOCK_SKEW_SECS) {
        return Err(ContractError::FutureTimestamp);
    }
    Ok(())
}

#[contract]
pub struct Timestamping;

#[contractimpl]
impl Timestamping {
    /// One-time setup. The admin manages authorities and the validity period.
    pub fn initialize(env: Env, admin: Address) -> Result<(), ContractError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ContractError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::RecordValidity, &DEFAULT_RECORD_VALIDITY_SECS);
        Ok(())
    }

    /// Sets how long a timestamp stays valid before it must be renewed. Admin only.
    pub fn set_record_validity(env: Env, admin: Address, secs: u64) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;
        if !(MIN_RECORD_VALIDITY_SECS..=MAX_RECORD_VALIDITY_SECS).contains(&secs) {
            return Err(ContractError::InvalidValidity);
        }
        env.storage()
            .instance()
            .set(&DataKey::RecordValidity, &secs);
        Ok(())
    }

    pub fn get_record_validity(env: Env) -> u64 {
        record_validity(&env)
    }

    /// Registers a TSA Ed25519 key valid until `valid_until`. Admin only.
    /// A revoked key cannot be re-registered.
    pub fn add_authority(
        env: Env,
        admin: Address,
        key: BytesN<32>,
        valid_until: u64,
    ) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;
        let now = env.ledger().timestamp();
        if valid_until <= now {
            return Err(ContractError::InvalidAuthorityValidity);
        }
        let key_id = DataKey::Authority(key.clone());
        if env.storage().persistent().has(&key_id) {
            return Err(ContractError::AuthorityAlreadyExists);
        }
        let authority = Authority {
            key: key.clone(),
            added_at: now,
            valid_until,
            revoked_at: None,
        };
        env.storage().persistent().set(&key_id, &authority);
        append_history(
            &env,
            AuthorityEvent {
                key: key.clone(),
                action: symbol_short!("added"),
                at: now,
                valid_until,
            },
        )?;
        env.events().publish((symbol_short!("TSA_ADD"),), key);
        Ok(())
    }

    /// Extends an active authority's validity. Admin only.
    pub fn renew_authority(
        env: Env,
        admin: Address,
        key: BytesN<32>,
        valid_until: u64,
    ) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;
        let now = env.ledger().timestamp();
        let mut authority = load_authority(&env, &key)?;
        if authority.revoked_at.is_some() {
            return Err(ContractError::AuthorityRevoked);
        }
        if valid_until <= now {
            return Err(ContractError::InvalidAuthorityValidity);
        }
        authority.valid_until = valid_until;
        env.storage()
            .persistent()
            .set(&DataKey::Authority(key.clone()), &authority);
        append_history(
            &env,
            AuthorityEvent {
                key: key.clone(),
                action: symbol_short!("renewed"),
                at: now,
                valid_until,
            },
        )?;
        env.events().publish((symbol_short!("TSA_REN"),), key);
        Ok(())
    }

    /// Revokes an authority from `effective_from`. Timestamps it issued at or after
    /// that time become invalid, and it can issue nothing new. `effective_from` may be
    /// in the past (for a key compromised earlier), but not the future. Admin only.
    pub fn revoke_authority(
        env: Env,
        admin: Address,
        key: BytesN<32>,
        effective_from: u64,
    ) -> Result<(), ContractError> {
        require_admin(&env, &admin)?;
        let now = env.ledger().timestamp();
        if effective_from > now {
            return Err(ContractError::InvalidRevocationTime);
        }
        let mut authority = load_authority(&env, &key)?;
        if authority.revoked_at.is_some() {
            return Err(ContractError::AuthorityRevoked);
        }
        authority.revoked_at = Some(effective_from);
        env.storage()
            .persistent()
            .set(&DataKey::Authority(key.clone()), &authority);
        append_history(
            &env,
            AuthorityEvent {
                key: key.clone(),
                action: symbol_short!("revoked"),
                at: now,
                valid_until: authority.valid_until,
            },
        )?;
        env.events().publish((symbol_short!("TSA_REV"),), key);
        Ok(())
    }

    pub fn get_authority(env: Env, key: BytesN<32>) -> Result<Authority, ContractError> {
        load_authority(&env, &key)
    }

    /// Full authority change history, oldest first.
    pub fn get_authority_history(env: Env) -> Vec<AuthorityEvent> {
        env.storage()
            .persistent()
            .get(&DataKey::History)
            .unwrap_or(Vec::new(&env))
    }

    /// Records a creation time for `record_id`, attested by `authority`.
    ///
    /// `signature` must be the authority's Ed25519 signature over the creation
    /// message. The host traps if the signature is invalid.
    pub fn submit_timestamp(
        env: Env,
        record_id: u64,
        record_hash: BytesN<32>,
        timestamp: u64,
        authority: BytesN<32>,
        signature: BytesN<64>,
    ) -> Result<(), ContractError> {
        let record_key = DataKey::Record(record_id);
        if env.storage().persistent().has(&record_key) {
            return Err(ContractError::RecordExists);
        }
        let auth = load_authority(&env, &authority)?;
        let now = env.ledger().timestamp();
        check_authority_window(&auth, timestamp, now)?;

        let msg = attestation_message(&env, TSA_DOMAIN, record_id, &record_hash, timestamp);
        env.crypto().ed25519_verify(&authority, &msg, &signature);

        let record = TimestampedRecord {
            record_id,
            record_hash,
            created_at: timestamp,
            authority,
            creation_signature: signature,
            last_timestamp: timestamp,
            renewal_count: 0,
            renewal_signature: None,
            expires_at: timestamp + record_validity(&env),
        };
        env.storage().persistent().set(&record_key, &record);
        env.events()
            .publish((symbol_short!("TS_NEW"), record_id), timestamp);
        Ok(())
    }

    /// Extends a record with a later TSA attestation. The record must not have
    /// expired. The new time must be later than `last_timestamp`.
    pub fn renew_timestamp(
        env: Env,
        record_id: u64,
        timestamp: u64,
        authority: BytesN<32>,
        signature: BytesN<64>,
    ) -> Result<(), ContractError> {
        let record_key = DataKey::Record(record_id);
        let mut record: TimestampedRecord = env
            .storage()
            .persistent()
            .get(&record_key)
            .ok_or(ContractError::RecordNotFound)?;
        let now = env.ledger().timestamp();
        if now >= record.expires_at {
            return Err(ContractError::RecordExpired);
        }
        if timestamp <= record.last_timestamp {
            return Err(ContractError::StaleRenewal);
        }
        let auth = load_authority(&env, &authority)?;
        check_authority_window(&auth, timestamp, now)?;

        let msg = attestation_message(
            &env,
            RENEWAL_DOMAIN,
            record_id,
            &record.record_hash,
            timestamp,
        );
        env.crypto().ed25519_verify(&authority, &msg, &signature);

        record.last_timestamp = timestamp;
        record.renewal_count += 1;
        record.renewal_signature = Some(signature);
        record.expires_at = timestamp + record_validity(&env);
        env.storage().persistent().set(&record_key, &record);
        env.events()
            .publish((symbol_short!("TS_REN"), record_id), timestamp);
        Ok(())
    }

    /// Returns the stored record, including the creation attestation.
    pub fn get_timestamped_record(
        env: Env,
        record_id: u64,
    ) -> Result<TimestampedRecord, ContractError> {
        env.storage()
            .persistent()
            .get(&DataKey::Record(record_id))
            .ok_or(ContractError::RecordNotFound)
    }

    /// Re-checks the stored creation signature, and the renewal signature if
    /// present, against the authority key. Returns `true` if they verify. The host
    /// traps if a signature does not verify, which cannot happen for records
    /// accepted by this contract.
    pub fn verify_timestamp_signature(env: Env, record_id: u64) -> Result<bool, ContractError> {
        let record: TimestampedRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Record(record_id))
            .ok_or(ContractError::RecordNotFound)?;
        let creation = attestation_message(
            &env,
            TSA_DOMAIN,
            record_id,
            &record.record_hash,
            record.created_at,
        );
        env.crypto()
            .ed25519_verify(&record.authority, &creation, &record.creation_signature);
        if let Some(sig) = record.renewal_signature.clone() {
            let renewal = attestation_message(
                &env,
                RENEWAL_DOMAIN,
                record_id,
                &record.record_hash,
                record.last_timestamp,
            );
            env.crypto()
                .ed25519_verify(&record.authority, &renewal, &sig);
        }
        Ok(true)
    }

    /// True if the record is inside its validity period and was not issued by an
    /// authority revoked before its latest timestamp.
    pub fn is_timestamp_valid(env: Env, record_id: u64) -> Result<bool, ContractError> {
        let record: TimestampedRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Record(record_id))
            .ok_or(ContractError::RecordNotFound)?;
        let now = env.ledger().timestamp();
        if now >= record.expires_at {
            return Ok(false);
        }
        let auth = load_authority(&env, &record.authority)?;
        Ok(match auth.revoked_at {
            Some(revoked_at) => record.last_timestamp < revoked_at,
            None => true,
        })
    }
}
