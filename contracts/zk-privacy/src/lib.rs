//! Privacy-preserving verification for Mainstay.
//!
//! Verification today reveals asset data on-chain. This contract lets a
//! verifier check two facts without seeing the underlying values:
//!
//! 1. **Maintenance without the engineer.** A maintenance record stores a
//!    Pedersen commitment to the engineer's identity instead of their address.
//!    The submitter proves the commitment matches *one* of the registered
//!    roster commitments (an OR-proof). The verifier learns it came from an
//!    authorised engineer, not which one.
//! 2. **Collateral adequacy without the value.** The asset owner commits to a
//!    valuation and proves `threshold <= value < threshold + 2^32` with a
//!    bit-decomposition range proof. The valuation stays hidden.
//!
//! Soroban limits each transaction to 100M CPU instructions, and one
//! BLS12-381 scalar multiplication costs about 3.3M of that The range proof is
//! therefore verified across several transactions (`BITS_PER_TX` bits each),
//! followed by a finalize call that checks the sum. A roster membership proof
//! must fit in one transaction, so `MAX_ROSTER` is kept small.
//!
//! Proofs are built off-chain by the `prover` module. See `docs/zk-privacy.md`
//! for the guarantees and their limits.

#![no_std]

pub mod primitives;
pub mod prover;

#[cfg(test)]
mod test;

use primitives::{
    fr_add, fr_sub, neg_one, point_bytes, point_from_bytes, scalar_from_bytes, scalar_from_u64,
    zero, Bases, Transcript, RANGE_BITS,
};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, crypto::bls12_381::G1Affine,
    panic_with_error, symbol_short, vec, Address, BytesN, Env, Vec,
};

/// Maximum roster size. Each branch of a membership proof costs two scalar
/// multiplications on-chain. At this size one verification measures about
/// 82M CPU instructions, inside the per-transaction budget.
pub const MAX_ROSTER: u32 = 12;
/// Range-proof bits verified per transaction. Measured at about 58M CPU
/// instructions, which leaves room under the limit.
pub const BITS_PER_TX: u32 = 4;
/// Collateral attestations are valid for 7 days after finalization.
pub const ATTESTATION_TTL_SECS: u64 = 7 * 24 * 60 * 60;
/// Maximum maintenance record commitments kept per asset.
pub const MAX_MAINTENANCE_RECORDS: u32 = 50;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ContractError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    UnauthorizedAdmin = 3,
    /// The roster already holds `MAX_ROSTER` commitments.
    RosterFull = 4,
    /// The proof did not verify against the supplied statement.
    InvalidProof = 5,
    /// A membership proof was submitted while the roster is empty.
    EmptyRoster = 6,
    /// No collateral proof session is open for this asset.
    SessionNotFound = 7,
    /// Bit proofs must arrive in order, starting at the next unverified bit.
    BitsOutOfOrder = 8,
    /// `finalize_collateral_proof` was called before all bits were verified.
    IncompleteProof = 9,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchnorrProof {
    /// Commitment to the random nonces, `k_v·G + k_r·H`.
    pub a: BytesN<96>,
    pub z_v: BytesN<32>,
    pub z_r: BytesN<32>,
}

/// One branch of a membership OR-proof: its challenge, response and nonce commitment.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrBranch {
    pub a: BytesN<96>,
    pub e: BytesN<32>,
    pub z: BytesN<32>,
}

/// Proves that a record commitment matches one roster entry.
/// `branches` has one entry per roster member, in roster order.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MembershipProof {
    pub branches: Vec<OrBranch>,
}

/// Proves a bit commitment opens to 0 or 1. Only `e0` is sent, because `e1 = e - e0`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BitProof {
    pub a0: BytesN<96>,
    pub a1: BytesN<96>,
    pub e0: BytesN<32>,
    pub z0: BytesN<32>,
    pub z1: BytesN<32>,
}

/// Proves `value - threshold` is in `[0, 2^RANGE_BITS)`, given a commitment to `value`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RangeProof {
    pub bit_commitments: Vec<BytesN<96>>,
    pub bit_proofs: Vec<BitProof>,
}

/// A verified collateral attestation. `owner` is the account that opened the
/// proof session. Lenders must compare it with the asset registry owner, because
/// this contract does not check ownership itself.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollateralAttestation {
    pub owner: Address,
    pub commitment: BytesN<96>,
    /// The attested lower bound: `value >= threshold`.
    pub threshold: u64,
    pub verified_at: u64,
    pub expires_at: u64,
}

/// An in-progress range proof whose bits are being verified across transactions.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollateralSession {
    pub owner: Address,
    pub commitment: BytesN<96>,
    pub threshold: u64,
    pub bit_commitments: Vec<BytesN<96>>,
    /// Number of leading bits that have been verified.
    pub verified: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum DataKey {
    Admin,
    Bases,
    /// Roster commitments, in registration order.
    Roster,
    /// Negated roster commitments (`-Q_j`), used by membership verification.
    NegRoster,
    Maintenance(u64),
    Session(u64),
    Collateral(u64),
}

/// Stored form of the commitment bases, so each call skips hash-to-curve.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
struct StoredBases {
    g: BytesN<96>,
    h: BytesN<96>,
}

fn load_bases(env: &Env) -> Bases {
    let stored: StoredBases = env
        .storage()
        .instance()
        .get(&DataKey::Bases)
        .unwrap_or_else(|| panic_with_error!(env, ContractError::NotInitialized));
    Bases {
        g: point_from_bytes(&stored.g),
        h: point_from_bytes(&stored.h),
    }
}

fn load_roster(env: &Env, key: DataKey) -> Vec<BytesN<96>> {
    env.storage().instance().get(&key).unwrap_or(Vec::new(env))
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

/// Checks `z_v·G + z_r·H == A + e·C`, where `e` is the transcript challenge.
pub fn verify_opening_proof(env: &Env, bases: &Bases, c: &BytesN<96>, p: &SchnorrProof) -> bool {
    let bls = env.crypto().bls12_381();
    let c_pt = point_from_bytes(c);
    let a = point_from_bytes(&p.a);
    let e = Transcript::new(env, b"open")
        .absorb_point(&c_pt)
        .absorb_point(&a)
        .challenge();
    let lhs = bls.g1_msm(
        vec![env, bases.g.clone(), bases.h.clone()],
        vec![
            env,
            scalar_from_bytes(env, &p.z_v),
            scalar_from_bytes(env, &p.z_r),
        ],
    );
    let rhs = bls.g1_add(&a, &bls.g1_mul(&c_pt, &e));
    lhs.to_bytes() == rhs.to_bytes()
}

/// Checks the roster OR-proof against the negated roster `neg_roster` (`-Q_j`).
///
/// Branch `j` shows `R - Q_j = x·H` for some `x`, so the record and roster
/// commitments hide the same engineer value. The challenge absorbs the negated
/// roster points, which bind the roster just as the points themselves would.
pub fn verify_membership_proof(
    env: &Env,
    bases: &Bases,
    asset_id: u64,
    record: &BytesN<96>,
    neg_roster: &Vec<BytesN<96>>,
    proof: &MembershipProof,
) -> bool {
    let n = neg_roster.len();
    if n == 0 || proof.branches.len() != n {
        return false;
    }
    let bls = env.crypto().bls12_381();
    let record_pt = point_from_bytes(record);

    let mut t = Transcript::new(env, b"member");
    t.absorb_u64(asset_id).absorb_point(&record_pt);
    for q in neg_roster.iter() {
        t.absorb(&point_bytes(&q));
    }
    for br in proof.branches.iter() {
        t.absorb(&point_bytes(&br.a));
    }
    let big_e = t.challenge();

    let mut sum = zero(env);
    for j in 0..n {
        let br = proof.branches.get(j).unwrap();
        let neg_q = point_from_bytes(&neg_roster.get(j).unwrap());
        let e_j = scalar_from_bytes(env, &br.e);
        let z_j = scalar_from_bytes(env, &br.z);
        sum = fr_add(env, &sum, &e_j);

        // D_j = R - Q_j. Adding the stored negation costs no scalar multiplication.
        let diff = bls.g1_add(&record_pt, &neg_q);
        let lhs = bls.g1_mul(&bases.h, &z_j);
        let rhs = bls.g1_add(&point_from_bytes(&br.a), &bls.g1_mul(&diff, &e_j));
        if lhs.to_bytes() != rhs.to_bytes() {
            return false;
        }
    }
    sum.to_bytes() == big_e.to_bytes()
}

/// Checks one bit commitment: `B` opens to 0 (`B = s·H`) or to 1 (`B - G = s·H`).
#[allow(clippy::too_many_arguments)]
fn verify_bit(
    env: &Env,
    bases: &Bases,
    asset_id: u64,
    c: &G1Affine,
    threshold: u64,
    neg_g: &G1Affine,
    i: u32,
    b_pt: &G1Affine,
    bp: &BitProof,
) -> bool {
    let bls = env.crypto().bls12_381();
    let a0 = point_from_bytes(&bp.a0);
    let a1 = point_from_bytes(&bp.a1);

    let e = Transcript::new(env, b"range")
        .absorb_u64(asset_id)
        .absorb_point(c)
        .absorb_u64(threshold)
        .absorb_u32(i)
        .absorb_point(b_pt)
        .absorb_point(&a0)
        .absorb_point(&a1)
        .challenge();
    let e0 = scalar_from_bytes(env, &bp.e0);
    let e1 = fr_sub(env, &e, &e0);
    let z0 = scalar_from_bytes(env, &bp.z0);
    let z1 = scalar_from_bytes(env, &bp.z1);

    let x1 = bls.g1_add(b_pt, neg_g);
    let ok0 =
        bls.g1_mul(&bases.h, &z0).to_bytes() == bls.g1_add(&a0, &bls.g1_mul(b_pt, &e0)).to_bytes();
    let ok1 =
        bls.g1_mul(&bases.h, &z1).to_bytes() == bls.g1_add(&a1, &bls.g1_mul(&x1, &e1)).to_bytes();
    ok0 && ok1
}

/// Checks that the bit commitments sum to the commitment minus the threshold:
/// `Σ 2^i·B_i + t·G == C`.
fn verify_bit_sum(
    env: &Env,
    bases: &Bases,
    c: &G1Affine,
    threshold: u64,
    bit_commitments: &Vec<BytesN<96>>,
) -> bool {
    let bls = env.crypto().bls12_381();
    let mut points = Vec::new(env);
    let mut weights = Vec::new(env);
    for i in 0..RANGE_BITS {
        points.push_back(point_from_bytes(&bit_commitments.get(i).unwrap()));
        weights.push_back(scalar_from_u64(env, 1u64 << i));
    }
    let weighted = bls.g1_msm(points, weights);
    let rhs = bls.g1_add(
        &weighted,
        &bls.g1_mul(&bases.g, &scalar_from_u64(env, threshold)),
    );
    rhs.to_bytes() == c.to_bytes()
}

#[contract]
pub struct ZkPrivacy;

#[contractimpl]
impl ZkPrivacy {
    /// One-time setup. Derives the commitment bases and records the admin.
    pub fn initialize(env: Env, admin: Address) -> Result<(), ContractError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ContractError::AlreadyInitialized);
        }
        admin.require_auth();
        let bases = Bases::derive(&env);
        env.storage().instance().set(
            &DataKey::Bases,
            &StoredBases {
                g: bases.g.to_bytes(),
                h: bases.h.to_bytes(),
            },
        );
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::Roster, &Vec::<BytesN<96>>::new(&env));
        env.storage()
            .instance()
            .set(&DataKey::NegRoster, &Vec::<BytesN<96>>::new(&env));
        Ok(())
    }

    /// Adds an authorised engineer's commitment to the roster. Admin only.
    /// Returns the roster index. The negation is computed once here so that
    /// membership verification avoids the extra scalar multiplication.
    pub fn register_roster_commitment(
        env: Env,
        admin: Address,
        commitment: BytesN<96>,
    ) -> Result<u32, ContractError> {
        require_admin(&env, &admin)?;
        let mut roster = load_roster(&env, DataKey::Roster);
        if roster.len() >= MAX_ROSTER {
            return Err(ContractError::RosterFull);
        }
        let mut neg = load_roster(&env, DataKey::NegRoster);
        let bls = env.crypto().bls12_381();
        let neg_pt = bls.g1_mul(&point_from_bytes(&commitment), &neg_one(&env));
        neg.push_back(neg_pt.to_bytes());
        roster.push_back(commitment.clone());
        let index = roster.len() - 1;
        env.storage().instance().set(&DataKey::Roster, &roster);
        env.storage().instance().set(&DataKey::NegRoster, &neg);
        env.events()
            .publish((symbol_short!("ROSTER"),), (index, commitment));
        Ok(index)
    }

    pub fn get_roster(env: Env) -> Vec<BytesN<96>> {
        load_roster(&env, DataKey::Roster)
    }

    /// Checks a proof that `commitment = v·G + r·H` for some `v`, `r`.
    pub fn verify_opening(env: Env, commitment: BytesN<96>, proof: SchnorrProof) -> bool {
        let bases = load_bases(&env);
        verify_opening_proof(&env, &bases, &commitment, &proof)
    }

    /// Checks that `record` commits to the same value as one roster entry.
    pub fn verify_membership(
        env: Env,
        asset_id: u64,
        record: BytesN<96>,
        proof: MembershipProof,
    ) -> bool {
        let bases = load_bases(&env);
        let neg_roster = load_roster(&env, DataKey::NegRoster);
        verify_membership_proof(&env, &bases, asset_id, &record, &neg_roster, &proof)
    }

    /// Stores a verified maintenance record commitment for an asset. Anyone can
    /// submit: the proof shows the record came from an authorised engineer, but
    /// not that the asset itself was serviced. Gate calls on the lifecycle
    /// contract's own checks. Resubmitting a stored record is a no-op.
    pub fn submit_maintenance_proof(
        env: Env,
        asset_id: u64,
        record: BytesN<96>,
        proof: MembershipProof,
    ) -> Result<(), ContractError> {
        let neg_roster = load_roster(&env, DataKey::NegRoster);
        if neg_roster.is_empty() {
            return Err(ContractError::EmptyRoster);
        }
        let bases = load_bases(&env);
        if !verify_membership_proof(&env, &bases, asset_id, &record, &neg_roster, &proof) {
            return Err(ContractError::InvalidProof);
        }

        let key = DataKey::Maintenance(asset_id);
        let mut records: Vec<BytesN<96>> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));
        if records.iter().any(|r| r == record) {
            return Ok(());
        }
        if records.len() >= MAX_MAINTENANCE_RECORDS {
            records.pop_front();
        }
        records.push_back(record.clone());
        env.storage().persistent().set(&key, &records);
        env.events()
            .publish((symbol_short!("MAINT_ZK"), asset_id), record);
        Ok(())
    }

    pub fn get_maintenance_records(env: Env, asset_id: u64) -> Vec<BytesN<96>> {
        env.storage()
            .persistent()
            .get(&DataKey::Maintenance(asset_id))
            .unwrap_or(Vec::new(&env))
    }

    /// Step 1 of a collateral proof. The asset owner opens a session with the
    /// commitment and the bit commitments. Nothing is verified yet. Replaces any
    /// earlier session for the asset.
    pub fn begin_collateral_proof(
        env: Env,
        owner: Address,
        asset_id: u64,
        commitment: BytesN<96>,
        threshold: u64,
        bit_commitments: Vec<BytesN<96>>,
    ) -> Result<(), ContractError> {
        owner.require_auth();
        if bit_commitments.len() != RANGE_BITS {
            return Err(ContractError::InvalidProof);
        }
        let session = CollateralSession {
            owner,
            commitment,
            threshold,
            bit_commitments,
            verified: 0,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Session(asset_id), &session);
        Ok(())
    }

    /// Step 2 of a collateral proof. Verifies `bit_proofs.len()` bits starting at
    /// the next unverified bit. Each call may verify at most `BITS_PER_TX` bits.
    pub fn submit_collateral_bits(
        env: Env,
        asset_id: u64,
        bit_proofs: Vec<BitProof>,
    ) -> Result<u32, ContractError> {
        let key = DataKey::Session(asset_id);
        let mut session: CollateralSession = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(ContractError::SessionNotFound)?;
        session.owner.require_auth();

        let count = bit_proofs.len();
        let start = session.verified;
        if count == 0 || count > BITS_PER_TX || start + count > RANGE_BITS {
            return Err(ContractError::BitsOutOfOrder);
        }

        let bases = load_bases(&env);
        let c = point_from_bytes(&session.commitment);
        let neg_g = env.crypto().bls12_381().g1_mul(&bases.g, &neg_one(&env));
        for k in 0..count {
            let i = start + k;
            let b_pt = point_from_bytes(&session.bit_commitments.get(i).unwrap());
            let bp = bit_proofs.get(k).unwrap();
            if !verify_bit(
                &env,
                &bases,
                asset_id,
                &c,
                session.threshold,
                &neg_g,
                i,
                &b_pt,
                &bp,
            ) {
                return Err(ContractError::InvalidProof);
            }
        }

        session.verified = start + count;
        env.storage().persistent().set(&key, &session);
        Ok(session.verified)
    }

    /// Step 3 of a collateral proof. Once every bit is verified, checks the
    /// bit sum against the commitment and records the attestation.
    pub fn finalize_collateral_proof(env: Env, asset_id: u64) -> Result<(), ContractError> {
        let key = DataKey::Session(asset_id);
        let session: CollateralSession = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(ContractError::SessionNotFound)?;
        session.owner.require_auth();
        if session.verified != RANGE_BITS {
            return Err(ContractError::IncompleteProof);
        }

        let bases = load_bases(&env);
        let c = point_from_bytes(&session.commitment);
        if !verify_bit_sum(
            &env,
            &bases,
            &c,
            session.threshold,
            &session.bit_commitments,
        ) {
            return Err(ContractError::InvalidProof);
        }

        let now = env.ledger().timestamp();
        let attestation = CollateralAttestation {
            owner: session.owner.clone(),
            commitment: session.commitment.clone(),
            threshold: session.threshold,
            verified_at: now,
            expires_at: now + ATTESTATION_TTL_SECS,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Collateral(asset_id), &attestation);
        env.storage().persistent().remove(&key);
        env.events()
            .publish((symbol_short!("COLL_ZK"), asset_id), session.threshold);
        Ok(())
    }

    pub fn get_collateral_session(env: Env, asset_id: u64) -> Option<CollateralSession> {
        env.storage().persistent().get(&DataKey::Session(asset_id))
    }

    pub fn get_collateral_attestation(env: Env, asset_id: u64) -> Option<CollateralAttestation> {
        env.storage()
            .persistent()
            .get(&DataKey::Collateral(asset_id))
    }

    /// True if a live attestation shows `value >= min_threshold` for the asset.
    /// The caller must also check `owner` against the asset registry.
    pub fn is_collateral_adequate(env: Env, asset_id: u64, min_threshold: u64) -> bool {
        match Self::get_collateral_attestation(env.clone(), asset_id) {
            Some(a) => a.threshold >= min_threshold && env.ledger().timestamp() < a.expires_at,
            None => false,
        }
    }
}
