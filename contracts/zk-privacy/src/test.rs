use crate::prover::{
    blinding_from_seed, commit_value, prove_adequacy, prove_membership, prove_opening,
};
use crate::{
    BitProof, ContractError, MembershipProof, RangeProof, ZkPrivacy, ZkPrivacyClient,
    ATTESTATION_TTL_SECS, BITS_PER_TX, MAX_ROSTER,
};
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::{Address, BytesN, Env, Vec};

fn setup() -> (Env, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    // Provers run host crypto too, so lift the test budget. Cost tests reset it per call.
    env.cost_estimate().budget().reset_unlimited();
    let id = env.register(ZkPrivacy, ());
    let admin = Address::generate(&env);
    ZkPrivacyClient::new(&env, &id).initialize(&admin);
    (env, id, admin)
}

fn seed(env: &Env, byte: u8) -> BytesN<32> {
    BytesN::from_array(env, &[byte; 32])
}

/// Runs the multi-transaction collateral flow: begin, one call per bit chunk, finalize.
fn submit_collateral(
    env: &Env,
    client: &ZkPrivacyClient,
    owner: &Address,
    asset_id: u64,
    commitment: &BytesN<96>,
    threshold: u64,
    proof: &RangeProof,
) -> Result<(), ContractError> {
    client.begin_collateral_proof(
        owner,
        &asset_id,
        commitment,
        &threshold,
        &proof.bit_commitments,
    );
    let total = proof.bit_proofs.len();
    let mut start = 0;
    while start < total {
        let mut chunk: Vec<BitProof> = Vec::new(env);
        for k in start..(start + BITS_PER_TX).min(total) {
            chunk.push_back(proof.bit_proofs.get(k).unwrap());
        }
        client
            .try_submit_collateral_bits(&asset_id, &chunk)
            .map_err(|_| ContractError::InvalidProof)?
            .map_err(|_| ContractError::InvalidProof)?;
        start += BITS_PER_TX;
    }
    client
        .try_finalize_collateral_proof(&asset_id)
        .map_err(|_| ContractError::InvalidProof)?
        .map_err(|_| ContractError::InvalidProof)
}

#[test]
fn opening_proof_verifies_and_binds_to_commitment() {
    let (env, id, _admin) = setup();
    let client = ZkPrivacyClient::new(&env, &id);
    let r = blinding_from_seed(&env, &seed(&env, 1), b"open");
    let (c, proof) = prove_opening(&env, 777, &r, &seed(&env, 2));

    assert!(client.verify_opening(&c, &proof));

    // The same proof does not verify for a different value under the same blinding.
    let other = commit_value(&env, 778, &r);
    assert!(!client.verify_opening(&other, &proof));
}

#[test]
fn opening_proof_rejects_tampered_response() {
    let (env, id, _admin) = setup();
    let client = ZkPrivacyClient::new(&env, &id);
    let r = blinding_from_seed(&env, &seed(&env, 1), b"open");
    let (c, mut proof) = prove_opening(&env, 42, &r, &seed(&env, 3));

    let mut arr = proof.z_v.to_array();
    arr[31] ^= 1;
    proof.z_v = BytesN::from_array(&env, &arr);
    assert!(!client.verify_opening(&c, &proof));
}

#[test]
fn maintenance_proof_hides_engineer_and_stores_record() {
    let (env, id, admin) = setup();
    let client = ZkPrivacyClient::new(&env, &id);

    // Three engineers on the roster. Each commitment hides the engineer's scalar.
    let r11 = blinding_from_seed(&env, &seed(&env, 10), b"eng-11");
    let r22 = blinding_from_seed(&env, &seed(&env, 10), b"eng-22");
    let r33 = blinding_from_seed(&env, &seed(&env, 10), b"eng-33");
    client.register_roster_commitment(&admin, &commit_value(&env, 11, &r11));
    client.register_roster_commitment(&admin, &commit_value(&env, 22, &r22));
    client.register_roster_commitment(&admin, &commit_value(&env, 33, &r33));
    let roster = client.get_roster();

    // Engineer 22 (roster index 1) submits a maintenance record.
    let rec_blind = blinding_from_seed(&env, &seed(&env, 4), b"rec-1");
    let record = commit_value(&env, 22, &rec_blind);
    let proof = prove_membership(
        &env,
        9,
        &record,
        &roster,
        1,
        &rec_blind,
        &r22,
        &seed(&env, 5),
    );

    assert!(client.verify_membership(&9, &record, &proof));
    client.submit_maintenance_proof(&9, &record, &proof);
    assert_eq!(client.get_maintenance_records(&9).len(), 1);

    // Resubmitting the same record does not duplicate it.
    client.submit_maintenance_proof(&9, &record, &proof);
    assert_eq!(client.get_maintenance_records(&9).len(), 1);
}

#[test]
fn membership_proof_fails_for_unregistered_engineer_or_wrong_asset() {
    let (env, id, admin) = setup();
    let client = ZkPrivacyClient::new(&env, &id);

    let r11 = blinding_from_seed(&env, &seed(&env, 10), b"eng-11");
    let r22 = blinding_from_seed(&env, &seed(&env, 10), b"eng-22");
    client.register_roster_commitment(&admin, &commit_value(&env, 11, &r11));
    client.register_roster_commitment(&admin, &commit_value(&env, 22, &r22));
    let roster = client.get_roster();

    // Engineer 99 is not on the roster. Claiming "index 0" with a made-up
    // blinding must fail.
    let rec_blind = blinding_from_seed(&env, &seed(&env, 4), b"rec-x");
    let stranger = commit_value(&env, 99, &rec_blind);
    let bogus = prove_membership(
        &env,
        9,
        &stranger,
        &roster,
        0,
        &rec_blind,
        &r11,
        &seed(&env, 6),
    );
    assert!(!client.verify_membership(&9, &stranger, &bogus));
    assert_eq!(
        client.try_submit_maintenance_proof(&9, &stranger, &bogus),
        Err(Ok(ContractError::InvalidProof))
    );

    // A valid proof is bound to its asset id.
    let rec_blind2 = blinding_from_seed(&env, &seed(&env, 7), b"rec-2");
    let record = commit_value(&env, 22, &rec_blind2);
    let proof = prove_membership(
        &env,
        9,
        &record,
        &roster,
        1,
        &rec_blind2,
        &r22,
        &seed(&env, 8),
    );
    assert!(client.verify_membership(&9, &record, &proof));
    assert!(!client.verify_membership(&10, &record, &proof));
}

#[test]
fn maintenance_submission_requires_a_roster() {
    let (env, id, _admin) = setup();
    let client = ZkPrivacyClient::new(&env, &id);
    let record = commit_value(&env, 1, &blinding_from_seed(&env, &seed(&env, 1), b"x"));
    let proof = MembershipProof {
        branches: Vec::new(&env),
    };
    assert_eq!(
        client.try_submit_maintenance_proof(&1, &record, &proof),
        Err(Ok(ContractError::EmptyRoster))
    );
}

#[test]
fn collateral_proof_verifies_in_chunks_and_records_owner() {
    let (env, id, _admin) = setup();
    let client = ZkPrivacyClient::new(&env, &id);
    let owner = Address::generate(&env);
    let r = blinding_from_seed(&env, &seed(&env, 1), b"collateral");
    let (c, proof) = prove_adequacy(&env, 7, 1500, &r, 1000, &seed(&env, 2)).unwrap();

    submit_collateral(&env, &client, &owner, 7, &c, 1000, &proof).unwrap();

    let att = client.get_collateral_attestation(&7).unwrap();
    assert_eq!(att.owner, owner);
    assert_eq!(att.threshold, 1000);
    assert!(client.get_collateral_session(&7).is_none());
    assert!(client.is_collateral_adequate(&7, &1000));
    assert!(client.is_collateral_adequate(&7, &900));
    assert!(!client.is_collateral_adequate(&7, &1001));
    assert!(!client.is_collateral_adequate(&8, &1000));
}

#[test]
fn collateral_proof_is_bound_to_threshold_and_asset() {
    let (env, id, _admin) = setup();
    let client = ZkPrivacyClient::new(&env, &id);
    let owner = Address::generate(&env);
    let r = blinding_from_seed(&env, &seed(&env, 1), b"collateral");
    let (c, proof) = prove_adequacy(&env, 7, 1500, &r, 1000, &seed(&env, 2)).unwrap();

    // Claiming a higher threshold than the prover used must fail.
    assert_eq!(
        submit_collateral(&env, &client, &owner, 7, &c, 1200, &proof),
        Err(ContractError::InvalidProof)
    );
    // Replaying the same proof under a different asset id must fail.
    assert_eq!(
        submit_collateral(&env, &client, &owner, 8, &c, 1000, &proof),
        Err(ContractError::InvalidProof)
    );
}

#[test]
fn collateral_prover_refuses_values_below_threshold_or_out_of_range() {
    let env = Env::default();
    let r = blinding_from_seed(&env, &seed(&env, 1), b"collateral");
    assert!(prove_adequacy(&env, 1, 999, &r, 1000, &seed(&env, 2)).is_none());
    let too_big = 1000 + (1u64 << crate::primitives::RANGE_BITS);
    assert!(prove_adequacy(&env, 1, too_big, &r, 1000, &seed(&env, 2)).is_none());
}

#[test]
fn collateral_bits_reject_tampered_and_misordered_proofs() {
    let (env, id, _admin) = setup();
    let client = ZkPrivacyClient::new(&env, &id);
    let owner = Address::generate(&env);
    let r = blinding_from_seed(&env, &seed(&env, 1), b"collateral");
    let (c, proof) = prove_adequacy(&env, 7, 1500, &r, 1000, &seed(&env, 2)).unwrap();

    client.begin_collateral_proof(&owner, &7, &c, &1000, &proof.bit_commitments);

    // Chunks apply at the next unverified position, so bit 1 offered first is
    // checked as bit 0 and fails.
    let mut chunk = Vec::new(&env);
    chunk.push_back(proof.bit_proofs.get(1).unwrap());
    assert_eq!(
        client.try_submit_collateral_bits(&7, &chunk),
        Err(Ok(ContractError::InvalidProof))
    );

    // An empty chunk is out of bounds.
    let empty: Vec<BitProof> = Vec::new(&env);
    assert_eq!(
        client.try_submit_collateral_bits(&7, &empty),
        Err(Ok(ContractError::BitsOutOfOrder))
    );

    // A tampered first bit fails verification.
    let mut bp = proof.bit_proofs.get(0).unwrap();
    let mut arr = bp.z0.to_array();
    arr[31] ^= 1;
    bp.z0 = BytesN::from_array(&env, &arr);
    let mut first = Vec::new(&env);
    first.push_back(bp);
    assert_eq!(
        client.try_submit_collateral_bits(&7, &first),
        Err(Ok(ContractError::InvalidProof))
    );
}

#[test]
fn collateral_finalize_requires_all_bits() {
    let (env, id, _admin) = setup();
    let client = ZkPrivacyClient::new(&env, &id);
    let owner = Address::generate(&env);
    let r = blinding_from_seed(&env, &seed(&env, 1), b"collateral");
    let (c, proof) = prove_adequacy(&env, 7, 1500, &r, 1000, &seed(&env, 2)).unwrap();

    client.begin_collateral_proof(&owner, &7, &c, &1000, &proof.bit_commitments);
    let mut chunk = Vec::new(&env);
    for k in 0..BITS_PER_TX {
        chunk.push_back(proof.bit_proofs.get(k).unwrap());
    }
    client.submit_collateral_bits(&7, &chunk);
    assert_eq!(
        client.try_finalize_collateral_proof(&7),
        Err(Ok(ContractError::IncompleteProof))
    );
    assert_eq!(
        client.try_finalize_collateral_proof(&99),
        Err(Ok(ContractError::SessionNotFound))
    );
}

#[test]
fn collateral_attestation_expires() {
    let (env, id, _admin) = setup();
    let client = ZkPrivacyClient::new(&env, &id);
    let owner = Address::generate(&env);
    let r = blinding_from_seed(&env, &seed(&env, 1), b"collateral");
    let (c, proof) = prove_adequacy(&env, 7, 1500, &r, 1000, &seed(&env, 2)).unwrap();
    submit_collateral(&env, &client, &owner, 7, &c, 1000, &proof).unwrap();

    env.ledger()
        .with_mut(|l| l.timestamp += ATTESTATION_TTL_SECS + 1);
    assert!(!client.is_collateral_adequate(&7, &1000));
}

#[test]
fn roster_is_admin_only_and_bounded() {
    let (env, id, admin) = setup();
    let client = ZkPrivacyClient::new(&env, &id);
    let c = commit_value(&env, 1, &blinding_from_seed(&env, &seed(&env, 1), b"r"));

    let stranger = Address::generate(&env);
    assert_eq!(
        client.try_register_roster_commitment(&stranger, &c),
        Err(Ok(ContractError::UnauthorizedAdmin))
    );

    for _ in 0..MAX_ROSTER {
        client.register_roster_commitment(&admin, &c);
    }
    assert_eq!(
        client.try_register_roster_commitment(&admin, &c),
        Err(Ok(ContractError::RosterFull))
    );
}

#[test]
fn membership_verification_cpu_cost_fits_transaction_budget() {
    let (env, id, admin) = setup();
    let client = ZkPrivacyClient::new(&env, &id);
    let mut roster_blinds = Vec::new(&env);
    for i in 0..MAX_ROSTER {
        let r = blinding_from_seed(&env, &seed(&env, 10), &[b'e', i as u8]);
        roster_blinds.push_back(r.clone());
        client.register_roster_commitment(&admin, &commit_value(&env, 100 + i as u64, &r));
    }
    let roster = client.get_roster();
    let last = MAX_ROSTER - 1;
    let rec_blind = blinding_from_seed(&env, &seed(&env, 4), b"rec");
    let record = commit_value(&env, 100 + last as u64, &rec_blind);
    let proof = prove_membership(
        &env,
        1,
        &record,
        &roster,
        last,
        &rec_blind,
        &roster_blinds.get(last).unwrap(),
        &seed(&env, 5),
    );

    env.cost_estimate().budget().reset_unlimited();
    assert!(client.verify_membership(&1, &record, &proof));
    let cpu = env.cost_estimate().budget().cpu_instruction_cost();
    assert!(
        cpu < 100_000_000,
        "membership verify uses {cpu} CPU instructions"
    );
}

#[test]
fn collateral_chunk_cpu_cost_fits_transaction_budget() {
    let (env, id, _admin) = setup();
    let client = ZkPrivacyClient::new(&env, &id);
    let owner = Address::generate(&env);
    let r = blinding_from_seed(&env, &seed(&env, 1), b"collateral");
    let (c, proof) = prove_adequacy(&env, 7, 1500, &r, 1000, &seed(&env, 2)).unwrap();
    client.begin_collateral_proof(&owner, &7, &c, &1000, &proof.bit_commitments);

    let mut chunk = Vec::new(&env);
    for k in 0..BITS_PER_TX {
        chunk.push_back(proof.bit_proofs.get(k).unwrap());
    }
    env.cost_estimate().budget().reset_unlimited();
    client.submit_collateral_bits(&7, &chunk);
    let cpu = env.cost_estimate().budget().cpu_instruction_cost();
    assert!(
        cpu < 100_000_000,
        "collateral chunk uses {cpu} CPU instructions"
    );
}
