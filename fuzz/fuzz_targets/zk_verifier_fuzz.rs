//! Fuzz target for the zk-privacy verifiers (`verify_opening_proof` and
//! `verify_membership_proof`).
//!
//! The verifiers parse untrusted curve points and scalars from raw bytes. This
//! target carves fixed-width fields out of the fuzzer input and checks that no
//! input makes a verifier panic. Malformed points, scalars at or above the group
//! order, mismatched roster and branch lengths, and truncated encodings are all
//! reachable from the byte stream.
//!
//! # Usage
//!
//! ```bash
//! cd fuzz && cargo fuzz run zk_verifier_fuzz -- -max_total_time=3600
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;

/// Reads a fixed-width field from `data` at `*pos`, zero-padding when the input
/// runs out. Truncated input therefore still reaches the verifiers.
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
    use soroban_sdk::{BytesN, Env, Vec};
    use zk_privacy::primitives::Bases;
    use zk_privacy::{verify_membership_proof, verify_opening_proof};
    use zk_privacy::{MembershipProof, OrBranch, SchnorrProof};

    // The verifiers run host crypto, so lift the budget as the unit tests do.
    let env = Env::default();
    env.cost_estimate().budget().reset_unlimited();
    let bases = Bases::derive(&env);

    let mut pos = 0usize;

    // Opening proof: malformed commitment and nonce points, and scalars that may
    // be at or above the group order (they must be reduced, not rejected by panic).
    let commitment = BytesN::from_array(&env, &take::<96>(data, &mut pos));
    let schnorr = SchnorrProof {
        a: BytesN::from_array(&env, &take::<96>(data, &mut pos)),
        z_v: BytesN::from_array(&env, &take::<32>(data, &mut pos)),
        z_r: BytesN::from_array(&env, &take::<32>(data, &mut pos)),
    };
    let _ = verify_opening_proof(&env, &bases, &commitment, &schnorr);

    // Membership proof: roster size and branch count are taken from the input so
    // that mismatched lengths, which must return false, are exercised too.
    let roster_len = (take::<1>(data, &mut pos)[0] % 8) as usize;
    let branch_len = (take::<1>(data, &mut pos)[0] % 8) as usize;
    let asset_id = u64::from_be_bytes(take::<8>(data, &mut pos));
    let record = BytesN::from_array(&env, &take::<96>(data, &mut pos));

    let mut neg_roster = Vec::new(&env);
    for _ in 0..roster_len {
        neg_roster.push_back(BytesN::from_array(&env, &take::<96>(data, &mut pos)));
    }

    let mut branches = Vec::new(&env);
    for _ in 0..branch_len {
        branches.push_back(OrBranch {
            a: BytesN::from_array(&env, &take::<96>(data, &mut pos)),
            e: BytesN::from_array(&env, &take::<32>(data, &mut pos)),
            z: BytesN::from_array(&env, &take::<32>(data, &mut pos)),
        });
    }
    let proof = MembershipProof { branches };
    let _ = verify_membership_proof(&env, &bases, asset_id, &record, &neg_roster, &proof);

    // Truncated encodings: a slice shorter than the wire width must not convert
    // into a fixed-width point or scalar, so no verifier call can be built from it.
    let width = (take::<1>(data, &mut pos)[0] as usize) % 96;
    let short = &data[..width.min(data.len())];
    if short.len() < 96 {
        let converted: Result<[u8; 96], _> = short.try_into();
        assert!(converted.is_err(), "truncated point encoding was accepted");
    }
});
