//! Off-chain proof generation.
//!
//! Each prover takes a 32-byte `seed` from a CSPRNG. The seed must stay secret:
//! all nonces are derived from it by hashing the seed together with the
//! statement being proven. Binding the derivation to the statement matters,
//! because reusing a nonce across two different challenges reveals the witness.
//! Re-proving the same statement with the same seed gives the same proof.
//!
//! Only public outputs (commitments and proofs) should leave the prover.
//! The seed, values, and blindings must not be sent on-chain.

use soroban_sdk::{
    crypto::bls12_381::{Fr, G1Affine},
    BytesN, Env, Vec,
};

use crate::primitives::{
    fr_add, fr_inv, fr_mul, fr_sub, neg_one, point_from_bytes, scalar_from_u64, zero, Bases,
    Transcript, RANGE_BITS,
};
use crate::{BitProof, MembershipProof, OrBranch, RangeProof, SchnorrProof, MAX_ROSTER};

/// Derives a scalar from the seed, bound to the statement transcript `stmt`,
/// a per-use `label`, and an index.
fn draw(seed: &BytesN<32>, stmt: &Transcript, label: &[u8], idx: u32) -> Fr {
    let mut t = stmt.clone();
    t.absorb(&seed.to_array()).absorb(label).absorb_u32(idx);
    t.challenge()
}

/// Derives a blinding factor from the seed. Use a distinct `label` for each
/// commitment that needs its own blinding.
pub fn blinding_from_seed(env: &Env, seed: &BytesN<32>, label: &[u8]) -> Fr {
    let stmt = Transcript::new(env, b"blinding");
    draw(seed, &stmt, label, 0)
}

/// Commits to `value` with `blinding`, returning the 96-byte encoding.
pub fn commit_value(env: &Env, value: u64, blinding: &Fr) -> BytesN<96> {
    Bases::derive(env)
        .commit(env, &scalar_from_u64(env, value), blinding)
        .to_bytes()
}

/// Proves knowledge of the opening `(value, blinding)` of
/// `C = value·G + blinding·H`. Returns the commitment and the proof.
pub fn prove_opening(
    env: &Env,
    value: u64,
    blinding: &Fr,
    seed: &BytesN<32>,
) -> (BytesN<96>, SchnorrProof) {
    let bases = Bases::derive(env);
    let v = scalar_from_u64(env, value);
    let c = bases.commit(env, &v, blinding);

    let mut stmt = Transcript::new(env, b"open-stmt");
    stmt.absorb_point(&c);
    let kv = draw(seed, &stmt, b"kv", 0);
    let kr = draw(seed, &stmt, b"kr", 0);
    let a = bases.commit(env, &kv, &kr);

    let e = Transcript::new(env, b"open")
        .absorb_point(&c)
        .absorb_point(&a)
        .challenge();
    let z_v = fr_add(env, &kv, &fr_mul(env, &e, &v));
    let z_r = fr_add(env, &kr, &fr_mul(env, &e, blinding));

    (
        c.to_bytes(),
        SchnorrProof {
            a: a.to_bytes(),
            z_v: z_v.to_bytes(),
            z_r: z_r.to_bytes(),
        },
    )
}

/// Proves that `record` (committed with `record_blinding`) hides the same value
/// as roster entry `index` (committed with `roster_blinding`). Both commitments
/// must use the same committed engineer value. The verifier learns that the
/// record matches *some* roster entry, not which one.
///
/// `roster` must match the roster stored on-chain, in order.
#[allow(clippy::too_many_arguments)]
pub fn prove_membership(
    env: &Env,
    asset_id: u64,
    record: &BytesN<96>,
    roster: &Vec<BytesN<96>>,
    index: u32,
    record_blinding: &Fr,
    roster_blinding: &Fr,
    seed: &BytesN<32>,
) -> MembershipProof {
    let n = roster.len();
    assert!(n > 0 && n <= MAX_ROSTER, "roster size out of range");
    assert!(index < n, "roster index out of range");

    let bases = Bases::derive(env);
    let bls = env.crypto().bls12_381();
    let record_pt = point_from_bytes(record);
    let neg = neg_one(env);

    // Public statement: asset, record, and the full roster. Nonces are bound to it.
    let mut stmt = Transcript::new(env, b"member-stmt");
    stmt.absorb_u64(asset_id).absorb_point(&record_pt);
    for q in roster.iter() {
        stmt.absorb(&q.to_array());
    }

    // Negated roster points, matching what the contract stores (`-Q_j`).
    let mut neg_qs: Vec<G1Affine> = Vec::new(env);
    for j in 0..n {
        neg_qs.push_back(bls.g1_mul(&point_from_bytes(&roster.get(j).unwrap()), &neg));
    }

    // Per-branch nonce commitments A_j and challenge/response slots. The real
    // branch's slots are filled in after the challenge is known.
    let mut a_pts: Vec<G1Affine> = Vec::new(env);
    let mut es: Vec<Fr> = Vec::new(env);
    let mut zs: Vec<Fr> = Vec::new(env);
    let mut k_real = zero(env);

    for j in 0..n {
        if j == index {
            k_real = draw(seed, &stmt, b"mem-k", j);
            a_pts.push_back(bls.g1_mul(&bases.h, &k_real));
            es.push_back(zero(env));
            zs.push_back(zero(env));
            continue;
        }
        // Simulated branch: choose e_j and z_j, then set A_j = z_j·H - e_j·D_j.
        let diff = bls.g1_add(&record_pt, &neg_qs.get(j).unwrap());
        let e_j = draw(seed, &stmt, b"mem-e", j);
        let z_j = draw(seed, &stmt, b"mem-z", j);
        let a_j = bls.g1_add(
            &bls.g1_mul(&bases.h, &z_j),
            &bls.g1_mul(&diff, &fr_mul(env, &e_j, &neg)),
        );
        a_pts.push_back(a_j);
        es.push_back(e_j);
        zs.push_back(z_j);
    }

    // Fiat-Shamir challenge. Must match the verifier's transcript exactly
    // (label "member", same absorption order), so it is rebuilt here rather than
    // reusing `stmt`, which carries a different label for nonce derivation.
    let mut t = Transcript::new(env, b"member");
    t.absorb_u64(asset_id).absorb_point(&record_pt);
    for q in neg_qs.iter() {
        t.absorb_point(&q);
    }
    for j in 0..n {
        t.absorb_point(&a_pts.get(j).unwrap());
    }
    let big_e = t.challenge();

    // The real challenge is whatever makes the branch challenges sum to E.
    let mut sum_others = zero(env);
    for j in 0..n {
        if j != index {
            sum_others = fr_add(env, &sum_others, &es.get(j).unwrap());
        }
    }
    let e_real = fr_sub(env, &big_e, &sum_others);
    let d_real = fr_sub(env, record_blinding, roster_blinding);
    let z_real = fr_add(env, &k_real, &fr_mul(env, &e_real, &d_real));
    es.set(index, e_real);
    zs.set(index, z_real);

    let mut branches = Vec::new(env);
    for j in 0..n {
        branches.push_back(OrBranch {
            a: a_pts.get(j).unwrap().to_bytes(),
            e: es.get(j).unwrap().to_bytes(),
            z: zs.get(j).unwrap().to_bytes(),
        });
    }
    MembershipProof { branches }
}

/// Proves `threshold <= value < threshold + 2^RANGE_BITS` for the commitment
/// `C = value·G + blinding·H`. Returns the commitment and the proof, or `None`
/// if `value < threshold` or the surplus does not fit in `RANGE_BITS` bits.
pub fn prove_adequacy(
    env: &Env,
    asset_id: u64,
    value: u64,
    blinding: &Fr,
    threshold: u64,
    seed: &BytesN<32>,
) -> Option<(BytesN<96>, RangeProof)> {
    if value < threshold {
        return None;
    }
    let surplus = value - threshold;
    if RANGE_BITS < 64 && surplus >= (1u64 << RANGE_BITS) {
        return None;
    }

    let bases = Bases::derive(env);
    let bls = env.crypto().bls12_381();
    let c = bases.commit(env, &scalar_from_u64(env, value), blinding);
    let neg_g = bls.g1_mul(&bases.g, &neg_one(env));

    let mut stmt = Transcript::new(env, b"range-stmt");
    stmt.absorb_u64(asset_id)
        .absorb_point(&c)
        .absorb_u64(threshold);

    // Blindings s_i for bits 0..RANGE_BITS-1. The last one is fixed so that
    // Σ 2^i·s_i = blinding, which the on-chain sum check requires.
    let mut s: Vec<Fr> = Vec::new(env);
    let mut weighted_sum = zero(env);
    for i in 0..(RANGE_BITS - 1) {
        let s_i = draw(seed, &stmt, b"range-s", i);
        let w = scalar_from_u64(env, 1u64 << i);
        weighted_sum = fr_add(env, &weighted_sum, &fr_mul(env, &w, &s_i));
        s.push_back(s_i);
    }
    let last = RANGE_BITS - 1;
    let w_last = scalar_from_u64(env, 1u64 << last);
    let s_last = fr_mul(
        env,
        &fr_sub(env, blinding, &weighted_sum),
        &fr_inv(env, &w_last),
    );
    s.push_back(s_last);

    let mut bit_commitments = Vec::new(env);
    let mut bit_proofs = Vec::new(env);
    for i in 0..RANGE_BITS {
        let bit = (surplus >> i) & 1;
        let s_i = s.get(i).unwrap();
        let big_b = bases.commit(env, &scalar_from_u64(env, bit), &s_i);
        bit_commitments.push_back(big_b.to_bytes());

        // Branch 0 witness: B = s·H (bit is 0). Branch 1 witness: B - G = s·H.
        let x0 = big_b.clone();
        let x1 = bls.g1_add(&big_b, &neg_g);

        let mut bit_stmt = stmt.clone();
        bit_stmt.absorb_u32(i).absorb_point(&big_b);
        let k = draw(seed, &bit_stmt, b"bit-k", i);
        let e_sim = draw(seed, &bit_stmt, b"bit-e", i);
        let z_sim = draw(seed, &bit_stmt, b"bit-z", i);

        // Real branch gets a fresh nonce. The simulated branch picks (e, z) and derives A.
        let x_sim = if bit == 0 { &x1 } else { &x0 };
        let a_real = bls.g1_mul(&bases.h, &k);
        let a_sim = bls.g1_add(
            &bls.g1_mul(&bases.h, &z_sim),
            &bls.g1_mul(x_sim, &fr_mul(env, &e_sim, &neg_one(env))),
        );
        let (a0, a1) = if bit == 0 {
            (a_real, a_sim)
        } else {
            (a_sim, a_real)
        };

        let e_total = Transcript::new(env, b"range")
            .absorb_u64(asset_id)
            .absorb_point(&c)
            .absorb_u64(threshold)
            .absorb_u32(i)
            .absorb_point(&big_b)
            .absorb_point(&a0)
            .absorb_point(&a1)
            .challenge();
        let e_real = fr_sub(env, &e_total, &e_sim);
        let z_real = fr_add(env, &k, &fr_mul(env, &e_real, &s_i));

        let (e0, z0, z1) = if bit == 0 {
            (e_real, z_real, z_sim)
        } else {
            (e_sim, z_sim, z_real)
        };
        bit_proofs.push_back(BitProof {
            a0: a0.to_bytes(),
            a1: a1.to_bytes(),
            e0: e0.to_bytes(),
            z0: z0.to_bytes(),
            z1: z1.to_bytes(),
        });
    }

    Some((
        c.to_bytes(),
        RangeProof {
            bit_commitments,
            bit_proofs,
        },
    ))
}
