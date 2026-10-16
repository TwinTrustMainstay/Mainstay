//! Pedersen commitments and Fiat-Shamir transcripts over BLS12-381 G1.
//!
//! Commitments have the form `C = v·G + r·H`, where `G` and `H` are derived by
//! hashing two fixed labels to the curve. Nobody knows the discrete log of `H`
//! relative to `G`, so a commitment hides `v` (because `r` is random) and binds
//! the committer to `v` (because they cannot open it to a different value).

use soroban_sdk::{
    crypto::bls12_381::{Fr, G1Affine},
    vec, Bytes, BytesN, Env, Vec, U256,
};

/// Domain separation tag for hash-to-curve and Fiat-Shamir transcripts.
const DST: &[u8] = b"MAINSTAY-ZK-V1";

/// Width of the range proof. A collateral proof shows that `value - threshold`
/// fits in this many bits, so it proves `threshold <= value < threshold + 2^RANGE_BITS`.
pub const RANGE_BITS: u32 = 32;

/// Pedersen commitment bases.
#[derive(Clone)]
pub struct Bases {
    pub g: G1Affine,
    pub h: G1Affine,
}

impl Bases {
    /// Derives `G` and `H` from fixed labels.
    pub fn derive(env: &Env) -> Self {
        let bls = env.crypto().bls12_381();
        let dst = Bytes::from_slice(env, DST);
        let g = bls.hash_to_g1(&Bytes::from_slice(env, b"generator-G"), &dst);
        let h = bls.hash_to_g1(&Bytes::from_slice(env, b"generator-H"), &dst);
        Self { g, h }
    }

    /// Returns `value·G + blinding·H`.
    pub fn commit(&self, env: &Env, value: &Fr, blinding: &Fr) -> G1Affine {
        env.crypto().bls12_381().g1_msm(
            vec![env, self.g.clone(), self.h.clone()],
            vec![env, value.clone(), blinding.clone()],
        )
    }
}

pub fn scalar_from_u64(env: &Env, v: u64) -> Fr {
    Fr::from_u256(U256::from_u128(env, v as u128))
}

/// Maps 32 bytes to a scalar, reducing modulo the group order. Using the
/// reducing conversion means non-canonical encodings cannot be used to
/// produce a different scalar from the same bytes.
pub fn scalar_from_bytes(env: &Env, b: &BytesN<32>) -> Fr {
    Fr::from_u256(U256::from_be_bytes(
        env,
        &Bytes::from_array(env, &b.to_array()),
    ))
}

pub fn point_from_bytes(b: &BytesN<96>) -> G1Affine {
    G1Affine::from_bytes(b.clone())
}

pub fn zero(env: &Env) -> Fr {
    scalar_from_u64(env, 0)
}

pub fn one(env: &Env) -> Fr {
    scalar_from_u64(env, 1)
}

pub fn neg_one(env: &Env) -> Fr {
    env.crypto().bls12_381().fr_sub(&zero(env), &one(env))
}

pub fn fr_add(env: &Env, a: &Fr, b: &Fr) -> Fr {
    env.crypto().bls12_381().fr_add(a, b)
}

pub fn fr_sub(env: &Env, a: &Fr, b: &Fr) -> Fr {
    env.crypto().bls12_381().fr_sub(a, b)
}

pub fn fr_mul(env: &Env, a: &Fr, b: &Fr) -> Fr {
    env.crypto().bls12_381().fr_mul(a, b)
}

pub fn fr_inv(env: &Env, a: &Fr) -> Fr {
    env.crypto().bls12_381().fr_inv(a)
}

/// Fiat-Shamir transcript. Every value a verifier relies on must be absorbed
/// before `challenge` is called, so a proof can't be replayed for a different
/// statement.
#[derive(Clone)]
pub struct Transcript {
    env: Env,
    buf: Bytes,
}

impl Transcript {
    pub fn new(env: &Env, label: &[u8]) -> Self {
        let mut buf = Bytes::from_slice(env, DST);
        buf.append(&Bytes::from_slice(env, label));
        Self {
            env: env.clone(),
            buf,
        }
    }

    pub fn absorb(&mut self, data: &[u8]) -> &mut Self {
        self.buf.append(&Bytes::from_slice(&self.env, data));
        self
    }

    pub fn absorb_point(&mut self, p: &G1Affine) -> &mut Self {
        let bytes = p.to_bytes().to_array();
        self.absorb(&bytes)
    }

    pub fn absorb_u64(&mut self, v: u64) -> &mut Self {
        self.absorb(&v.to_be_bytes())
    }

    pub fn absorb_u32(&mut self, v: u32) -> &mut Self {
        self.absorb(&v.to_be_bytes())
    }

    pub fn challenge(&self) -> Fr {
        let digest = self.env.crypto().sha256(&self.buf).to_bytes();
        scalar_from_bytes(&self.env, &digest)
    }
}

/// Raw bytes of a point, for use with `absorb`.
pub fn point_bytes(p: &BytesN<96>) -> [u8; 96] {
    p.to_array()
}

/// Helper that builds a `Vec` of points from a slice (used by MSM calls).
pub fn points_vec(env: &Env, pts: &[G1Affine]) -> Vec<G1Affine> {
    let mut v = Vec::new(env);
    for p in pts {
        v.push_back(p.clone());
    }
    v
}

pub fn scalars_vec(env: &Env, s: &[Fr]) -> Vec<Fr> {
    let mut v = Vec::new(env);
    for x in s {
        v.push_back(x.clone());
    }
    v
}
