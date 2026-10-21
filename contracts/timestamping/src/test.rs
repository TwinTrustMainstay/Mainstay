extern crate std;

use crate::{
    ContractError, Timestamping, TimestampingClient, DEFAULT_RECORD_VALIDITY_SECS,
    MAX_CLOCK_SKEW_SECS, MAX_HISTORY_ENTRIES, RENEWAL_DOMAIN, TSA_DOMAIN,
};
use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::{symbol_short, Address, BytesN, Env};

const NOW: u64 = 1_800_000_000;
const TEN_YEARS: u64 = 10 * 365 * 24 * 60 * 60;
const ONE_YEAR: u64 = 365 * 24 * 60 * 60;

struct Ctx {
    env: Env,
    id: Address,
    admin: Address,
    tsa: SigningKey,
    tsa_pub: BytesN<32>,
}

fn setup() -> Ctx {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = NOW);
    let id = env.register(Timestamping, ());
    let admin = Address::generate(&env);
    let client = TimestampingClient::new(&env, &id);
    client.initialize(&admin);

    let tsa = SigningKey::from_bytes(&[7u8; 32]);
    let tsa_pub = BytesN::from_array(&env, &tsa.verifying_key().to_bytes());
    client.add_authority(&admin, &tsa_pub, &(NOW + TEN_YEARS));
    Ctx {
        env,
        id,
        admin,
        tsa,
        tsa_pub,
    }
}

/// Builds the creation-attestation message exactly as the contract does.
fn message(domain: &[u8], record_id: u64, hash: &[u8; 32], ts: u64) -> std::vec::Vec<u8> {
    let mut m = domain.to_vec();
    m.extend_from_slice(&record_id.to_be_bytes());
    m.extend_from_slice(hash);
    m.extend_from_slice(&ts.to_be_bytes());
    m
}

fn sign(
    tsa: &SigningKey,
    env: &Env,
    domain: &[u8],
    id: u64,
    hash: &[u8; 32],
    ts: u64,
) -> BytesN<64> {
    let sig = tsa.sign(&message(domain, id, hash, ts)).to_bytes();
    BytesN::from_array(env, &sig)
}

#[test]
fn creation_attestation_is_stored_and_verifiable() {
    let c = setup();
    let client = TimestampingClient::new(&c.env, &c.id);
    let hash = [1u8; 32];
    let hash_b = BytesN::from_array(&c.env, &hash);
    let sig = sign(&c.tsa, &c.env, TSA_DOMAIN, 42, &hash, NOW - 60);

    client.submit_timestamp(&42, &hash_b, &(NOW - 60), &c.tsa_pub, &sig);

    let rec = client.get_timestamped_record(&42);
    assert_eq!(rec.created_at, NOW - 60);
    assert_eq!(rec.last_timestamp, NOW - 60);
    assert_eq!(rec.renewal_count, 0);
    assert_eq!(rec.expires_at, NOW - 60 + DEFAULT_RECORD_VALIDITY_SECS);
    assert_eq!(rec.authority, c.tsa_pub);
    assert!(client.verify_timestamp_signature(&42));
    assert!(client.is_timestamp_valid(&42));
}

#[test]
fn rejects_unknown_authority_duplicate_and_future_timestamps() {
    let c = setup();
    let client = TimestampingClient::new(&c.env, &c.id);
    let hash = [2u8; 32];
    let hash_b = BytesN::from_array(&c.env, &hash);

    let rogue = SigningKey::from_bytes(&[9u8; 32]);
    let rogue_pub = BytesN::from_array(&c.env, &rogue.verifying_key().to_bytes());
    let sig = sign(&rogue, &c.env, TSA_DOMAIN, 1, &hash, NOW);
    assert_eq!(
        client.try_submit_timestamp(&1, &hash_b, &NOW, &rogue_pub, &sig),
        Err(Ok(ContractError::AuthorityNotFound))
    );

    let future = NOW + MAX_CLOCK_SKEW_SECS + 1;
    let sig_f = sign(&c.tsa, &c.env, TSA_DOMAIN, 1, &hash, future);
    assert_eq!(
        client.try_submit_timestamp(&1, &hash_b, &future, &c.tsa_pub, &sig_f),
        Err(Ok(ContractError::FutureTimestamp))
    );

    let sig_ok = sign(&c.tsa, &c.env, TSA_DOMAIN, 1, &hash, NOW);
    client.submit_timestamp(&1, &hash_b, &NOW, &c.tsa_pub, &sig_ok);
    assert_eq!(
        client.try_submit_timestamp(&1, &hash_b, &NOW, &c.tsa_pub, &sig_ok),
        Err(Ok(ContractError::RecordExists))
    );
}

#[test]
fn rejects_signature_from_another_key_or_wrong_domain() {
    let c = setup();
    let client = TimestampingClient::new(&c.env, &c.id);
    let hash = [3u8; 32];
    let hash_b = BytesN::from_array(&c.env, &hash);

    // Signed by a key that is not the authority's: the host traps.
    let other = SigningKey::from_bytes(&[8u8; 32]);
    let bad = sign(&other, &c.env, TSA_DOMAIN, 5, &hash, NOW);
    assert!(client
        .try_submit_timestamp(&5, &hash_b, &NOW, &c.tsa_pub, &bad)
        .is_err());

    // A renewal-domain signature is not accepted as a creation attestation.
    let renewal_sig = sign(&c.tsa, &c.env, RENEWAL_DOMAIN, 5, &hash, NOW);
    assert!(client
        .try_submit_timestamp(&5, &hash_b, &NOW, &c.tsa_pub, &renewal_sig)
        .is_err());
}

#[test]
fn authority_expiry_blocks_new_attestations() {
    let c = setup();
    let client = TimestampingClient::new(&c.env, &c.id);
    let hash = [4u8; 32];
    let hash_b = BytesN::from_array(&c.env, &hash);
    let short = BytesN::from_array(&c.env, &[11u8; 32]);
    let short_key = SigningKey::from_bytes(&[11u8; 32]);
    let until = NOW + 100;
    client.add_authority(&c.admin, &short, &until);

    let sig = sign(&short_key, &c.env, TSA_DOMAIN, 2, &hash, until + 1);
    c.env.ledger().with_mut(|l| l.timestamp = until + 1);
    assert_eq!(
        client.try_submit_timestamp(&2, &hash_b, &(until + 1), &short, &sig),
        Err(Ok(ContractError::AuthorityExpired))
    );
}

#[test]
fn renewal_extends_expiry_and_keeps_creation_time() {
    let c = setup();
    let client = TimestampingClient::new(&c.env, &c.id);
    let hash = [5u8; 32];
    let hash_b = BytesN::from_array(&c.env, &hash);
    let created = NOW;
    client.submit_timestamp(
        &7,
        &hash_b,
        &created,
        &c.tsa_pub,
        &sign(&c.tsa, &c.env, TSA_DOMAIN, 7, &hash, created),
    );

    // Renew after one year, with a newer attested time.
    let later = created + ONE_YEAR;
    c.env.ledger().with_mut(|l| l.timestamp = later);
    let renew_sig = sign(&c.tsa, &c.env, RENEWAL_DOMAIN, 7, &hash, later);
    client.renew_timestamp(&7, &later, &c.tsa_pub, &renew_sig);

    let rec = client.get_timestamped_record(&7);
    assert_eq!(rec.created_at, created);
    assert_eq!(rec.last_timestamp, later);
    assert_eq!(rec.renewal_count, 1);
    assert_eq!(rec.expires_at, later + DEFAULT_RECORD_VALIDITY_SECS);
    assert!(client.verify_timestamp_signature(&7));

    // A renewal that is not later than the last timestamp is rejected.
    let stale_sig = sign(&c.tsa, &c.env, RENEWAL_DOMAIN, 7, &hash, later);
    assert_eq!(
        client.try_renew_timestamp(&7, &later, &c.tsa_pub, &stale_sig),
        Err(Ok(ContractError::StaleRenewal))
    );
}

#[test]
fn expired_record_cannot_be_renewed_and_reports_invalid() {
    let c = setup();
    let client = TimestampingClient::new(&c.env, &c.id);
    let hash = [6u8; 32];
    let hash_b = BytesN::from_array(&c.env, &hash);
    client.submit_timestamp(
        &8,
        &hash_b,
        &NOW,
        &c.tsa_pub,
        &sign(&c.tsa, &c.env, TSA_DOMAIN, 8, &hash, NOW),
    );

    let after_expiry = NOW + DEFAULT_RECORD_VALIDITY_SECS;
    c.env.ledger().with_mut(|l| l.timestamp = after_expiry);
    assert!(!client.is_timestamp_valid(&8));
    let renew_sig = sign(&c.tsa, &c.env, RENEWAL_DOMAIN, 8, &hash, after_expiry);
    assert_eq!(
        client.try_renew_timestamp(&8, &after_expiry, &c.tsa_pub, &renew_sig),
        Err(Ok(ContractError::RecordExpired))
    );
}

#[test]
fn revocation_invalidates_timestamps_from_its_effective_time() {
    let c = setup();
    let client = TimestampingClient::new(&c.env, &c.id);
    let hash = [7u8; 32];
    let hash_b = BytesN::from_array(&c.env, &hash);
    let issued = NOW - 1000;
    client.submit_timestamp(
        &9,
        &hash_b,
        &issued,
        &c.tsa_pub,
        &sign(&c.tsa, &c.env, TSA_DOMAIN, 9, &hash, issued),
    );
    assert!(client.is_timestamp_valid(&9));

    // Revoke as of before the record's time, as if the key was compromised earlier.
    client.revoke_authority(&c.admin, &c.tsa_pub, &(NOW - 2000));
    assert!(!client.is_timestamp_valid(&9));

    // A revoked authority cannot issue anything new.
    let hash2 = [8u8; 32];
    let hash2_b = BytesN::from_array(&c.env, &hash2);
    let sig = sign(&c.tsa, &c.env, TSA_DOMAIN, 10, &hash2, NOW);
    assert_eq!(
        client.try_submit_timestamp(&10, &hash2_b, &NOW, &c.tsa_pub, &sig),
        Err(Ok(ContractError::AuthorityRevoked))
    );
}

#[test]
fn revocation_cannot_be_in_the_future_and_later_timestamps_survive() {
    let c = setup();
    let client = TimestampingClient::new(&c.env, &c.id);
    assert_eq!(
        client.try_revoke_authority(&c.admin, &c.tsa_pub, &(NOW + 1)),
        Err(Ok(ContractError::InvalidRevocationTime))
    );

    let hash = [9u8; 32];
    let hash_b = BytesN::from_array(&c.env, &hash);
    let earlier = NOW - 10;
    client.submit_timestamp(
        &11,
        &hash_b,
        &earlier,
        &c.tsa_pub,
        &sign(&c.tsa, &c.env, TSA_DOMAIN, 11, &hash, earlier),
    );
    // Revoking from now on leaves the earlier timestamp valid.
    client.revoke_authority(&c.admin, &c.tsa_pub, &NOW);
    assert!(client.is_timestamp_valid(&11));
}

#[test]
fn authority_history_records_every_change() {
    let c = setup();
    let client = TimestampingClient::new(&c.env, &c.id);
    client.renew_authority(&c.admin, &c.tsa_pub, &(NOW + TEN_YEARS * 2));
    client.revoke_authority(&c.admin, &c.tsa_pub, &NOW);

    let history = client.get_authority_history();
    assert_eq!(history.len(), 3);
    assert_eq!(history.get(0).unwrap().action, symbol_short!("added"));
    assert_eq!(history.get(1).unwrap().action, symbol_short!("renewed"));
    assert_eq!(history.get(2).unwrap().action, symbol_short!("revoked"));
    assert_eq!(client.get_authority(&c.tsa_pub).revoked_at, Some(NOW));

    // A revoked key cannot be re-registered.
    assert_eq!(
        client.try_add_authority(&c.admin, &c.tsa_pub, &(NOW + ONE_YEAR)),
        Err(Ok(ContractError::AuthorityAlreadyExists))
    );
}

#[test]
fn validity_bounds_and_admin_only_changes() {
    let c = setup();
    let client = TimestampingClient::new(&c.env, &c.id);
    assert_eq!(
        client.try_set_record_validity(&c.admin, &1),
        Err(Ok(ContractError::InvalidValidity))
    );
    client.set_record_validity(&c.admin, &ONE_YEAR);
    assert_eq!(client.get_record_validity(), ONE_YEAR);

    let stranger = Address::generate(&c.env);
    assert_eq!(
        client.try_set_record_validity(&stranger, &ONE_YEAR),
        Err(Ok(ContractError::UnauthorizedAdmin))
    );
    assert_eq!(
        client.try_add_authority(&stranger, &c.tsa_pub, &(NOW + ONE_YEAR)),
        Err(Ok(ContractError::UnauthorizedAdmin))
    );
}

#[test]
fn history_is_bounded() {
    let c = setup();
    let client = TimestampingClient::new(&c.env, &c.id);
    // The setup already logged one event. Fill the rest with renewals.
    for _ in 1..MAX_HISTORY_ENTRIES {
        client.renew_authority(&c.admin, &c.tsa_pub, &(NOW + TEN_YEARS));
    }
    assert_eq!(
        client.try_renew_authority(&c.admin, &c.tsa_pub, &(NOW + TEN_YEARS)),
        Err(Ok(ContractError::HistoryFull))
    );
}
