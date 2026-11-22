//! Fuzz target for the timestamping attestation encoding (`attestation_message`).
//!
//! Authorities sign `domain || record_id (8 bytes BE) || record_hash || timestamp
//! (8 bytes BE)`. A length or domain-separation mistake in that encoding would let
//! a signature be replayed across record types or across creation and renewal.
//!
//! The encoder is private to the contract, so this target does not call it.
//! It builds the same bytes independently, through the public contract API:
//!
//! * a creation signature over the independent encoding must be accepted, and
//!   the contract must verify it, which proves the two encodings agree;
//! * a renewal-domain signature must not be accepted as a creation attestation;
//! * a creation signature must not be accepted as a renewal;
//! * a signature bound to a different `record_id` must be rejected.
//!
//! # Usage
//!
//! ```bash
//! cd fuzz && cargo fuzz run attestation_message_fuzz -- -max_total_time=3600
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;

const TSA_DOMAIN: &[u8] = b"MAINSTAY-TSA-CREATE-V1";
const RENEWAL_DOMAIN: &[u8] = b"MAINSTAY-TSA-RENEW-V1";
const NOW: u64 = 1_800_000_000;
const TEN_YEARS: u64 = 10 * 365 * 24 * 60 * 60;

/// Independent encoder for the signed message, matching the documented layout.
fn encode(domain: &[u8], record_id: u64, hash: &[u8; 32], ts: u64) -> Vec<u8> {
    let mut m = domain.to_vec();
    m.extend_from_slice(&record_id.to_be_bytes());
    m.extend_from_slice(hash);
    m.extend_from_slice(&ts.to_be_bytes());
    m
}

fn take<const N: usize>(data: &[u8], pos: &mut usize) -> [u8; N] {
    let mut out = [0u8; N];
    let end = (*pos + N).min(data.len());
    if *pos < end {
        out[..end - *pos].copy_from_slice(&data[*pos..end]);
    }
    *pos += N;
    out
}

fuzz_target!(|data: &[u8]| {
    use ed25519_dalek::{Signer, SigningKey};
    use soroban_sdk::testutils::{Address as _, Ledger};
    use soroban_sdk::{Address, BytesN, Env};
    use timestamping::{Timestamping, TimestampingClient};

    let mut pos = 0usize;
    let record_id = u64::from_be_bytes(take::<8>(data, &mut pos));
    let hash = take::<32>(data, &mut pos);
    // Timestamps stay in the past, inside the authority window and clock skew.
    let ts = NOW - (u64::from_be_bytes(take::<8>(data, &mut pos)) % 1_000_000);
    let other_id = record_id.wrapping_add(1);

    let env = Env::default();
    env.mock_all_auths();
    env.cost_estimate().budget().reset_unlimited();
    env.ledger().with_mut(|l| l.timestamp = NOW);
    let id = env.register(Timestamping, ());
    let admin = Address::generate(&env);
    let client = TimestampingClient::new(&env, &id);
    client.initialize(&admin);

    let tsa = SigningKey::from_bytes(&[7u8; 32]);
    let tsa_pub = BytesN::from_array(&env, &tsa.verifying_key().to_bytes());
    client.add_authority(&admin, &tsa_pub, &(NOW + TEN_YEARS));

    let sign = |domain: &[u8], id: u64, ts: u64| -> BytesN<64> {
        BytesN::from_array(&env, &tsa.sign(&encode(domain, id, &hash, ts)).to_bytes())
    };
    let hash_b = BytesN::from_array(&env, &hash);

    // Creation: the independent encoding must be accepted and verifiable.
    let creation = sign(TSA_DOMAIN, record_id, ts);
    client.submit_timestamp(&record_id, &hash_b, &ts, &tsa_pub, &creation);
    assert!(
        client.verify_timestamp_signature(&record_id),
        "contract rejected a creation signature over the documented encoding"
    );

    // Domain separation: a renewal-domain signature is not a creation attestation.
    let renewal_as_creation = sign(RENEWAL_DOMAIN, other_id, ts);
    assert!(
        client
            .try_submit_timestamp(&other_id, &hash_b, &ts, &tsa_pub, &renewal_as_creation)
            .is_err(),
        "renewal-domain signature accepted as creation"
    );

    // Binding: a creation signature for one record_id must not create another.
    assert!(
        client
            .try_submit_timestamp(&other_id, &hash_b, &ts, &tsa_pub, &creation)
            .is_err(),
        "creation signature replayed under a different record_id"
    );

    // Domain separation: a creation signature is not a renewal. Renewal needs a
    // later timestamp, so use ts + 1, which is still inside the clock skew.
    let renew_ts = ts + 1;
    assert!(
        client
            .try_renew_timestamp(&record_id, &renew_ts, &tsa_pub, &sign(TSA_DOMAIN, record_id, renew_ts))
            .is_err(),
        "creation-domain signature accepted as renewal"
    );

    // A correct renewal signature is accepted and verifiable.
    client.renew_timestamp(
        &record_id,
        &renew_ts,
        &tsa_pub,
        &sign(RENEWAL_DOMAIN, record_id, renew_ts),
    );
    assert!(client.verify_timestamp_signature(&record_id));
});
