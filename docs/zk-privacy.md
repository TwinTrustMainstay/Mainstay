# Privacy-Preserving Verification (`contracts/zk-privacy`)

Verification currently reveals asset data on-chain. This contract lets
verifiers check two facts without learning the underlying values:

| Fact | What is hidden | What the verifier learns |
|------|----------------|--------------------------|
| A maintenance record was made by an authorised engineer | Which engineer | The engineer is one of the registered roster |
| A collateral valuation meets a lender's threshold | The valuation | `threshold <= value < threshold + 2^32` |

Proofs are generated off-chain with the `prover` module and verified on-chain
by the contract.

## Construction

- **Commitments.** `C = v·G + r·H` over BLS12-381 G1, with `G` and `H` derived
  by hashing fixed labels to the curve (nothing-up-my-sleeve). Nobody knows the
  discrete log of `H` relative to `G`. A commitment is perfectly hiding, since
  `r` is uniform, and computationally binding under the discrete-log assumption.
- **Opening proof.** A Schnorr proof of knowledge of `(v, r)` for `C`.
- **Maintenance membership.** A one-of-N OR-proof (Cramer–Damgård–Schoenmakers).
  For roster entry `j`, branch `j` proves `R - Q_j = x·H`, which holds exactly when
  the record `R` and roster commitment `Q_j` hide the same engineer value. Only
  the real branch carries a witness. The other branches are simulated, so the
  verifier cannot tell which branch is real.
- **Collateral range proof.** Let `d = value - threshold`. The prover commits to
  each of the 32 bits of `d` (`B_i = b_i·G + s_i·H`) and proves each `B_i` opens to
  0 or 1 with an OR-proof. The verifier then checks
  `Σ 2^i·B_i + threshold·G == C`. The final blinding is fixed to make this hold,
  so `value - threshold` is forced into `[0, 2^32)`.
- **Fiat–Shamir.** Every challenge is a SHA-256 hash of a transcript that includes
  the asset id, the commitments, the threshold, and the nonce commitments, so a proof
  cannot be replayed for another asset or threshold.

## Guarantees

- **Engineer privacy (maintenance).** Given a verified record, the verifier learns
  only that it matches one roster entry. The anonymity set is the roster
  (at most 12 engineers in this version). Records are unlinkable to one another
  only insofar as commitments use fresh blindings.
- **Valuation privacy (collateral).** The verifier learns only the threshold and the
  bounded window `[threshold, threshold + 2^32)`. The exact value stays hidden.
- **Soundness.** A prover who does not know the opening, or who uses a value outside
  the statement, cannot produce a verifying proof, except with negligible
  probability under the discrete-log assumption and the random-oracle model.
- **Nonce safety.** The prover derives every nonce from a secret 32-byte seed and the
  statement. Reusing a seed on the same statement gives the same proof. On a different
  statement, the nonces differ, so the witness is not exposed.

## Assumptions and limits

- **Seed secrecy.** The prover's seed must be 32 bytes from a CSPRNG and must stay
  off-chain. Anyone who learns the seed and a proof's statement can recover the
  witness.
- **Anonymity set is small.** `MAX_ROSTER = 12`. Each membership branch costs two
  scalar multiplications on-chain. Measured verification cost at 12 is about 82M CPU
  instructions, which is near the 100M per-transaction limit. A larger anonymity
  set would need per-scope rosters (for example, one per asset type) or a
  succinct proof system.
- **Range window is bounded.** The collateral proof covers 2^32 base units above the
  threshold. Pick the unit scale so valuations fit. A valuation at or above
  `threshold + 2^32` cannot be proven.
- **Range proof is multi-transaction.** A 32-bit proof does not fit in one transaction
  (about 58M CPU per 4-bit chunk, 100M limit). The owner calls
  `begin_collateral_proof`, then `submit_collateral_bits` eight times (four bits each),
  then `finalize_collateral_proof`.
- **Ownership is not checked here.** `begin_collateral_proof` requires the owner's
  auth, and the attestation records `owner`. This contract cannot check the asset
  registry. Lenders must compare `CollateralAttestation.owner` with the registry
  owner before relying on an attestation.
- **Value binding.** A collateral proof shows that the committed value meets the
  threshold. It does not show that the committed value is the asset's real
  valuation. That needs a signed valuation from an oracle or appraiser, checked
  before the owner commits.
- **Maintenance submission is permissionless.** The proof shows the record came from
  an authorised engineer, but not that the asset was serviced. Gate lifecycle
  decisions on the lifecycle contract's own checks.
- **Roster management.** The admin registers engineer commitments. It learns the
  engineer scalar when it builds each commitment, so the admin must be trusted to
  register only real engineers and to keep blindings private.
- **Attestation expiry.** Collateral attestations last
  `ATTESTATION_TTL_SECS` (7 days) after finalization. Lenders must re-verify after
  that.

## Operational flow

1. Admin computes `C_j = commit(H(engineer_address), r_j)` for each authorised
   engineer, gives each engineer `r_j` privately, and calls
   `register_roster_commitment`.
2. Engineer records maintenance on asset `a`. They commit to the same engineer scalar
   with a fresh blinding `r_rec`, build `prove_membership(...)`, and call
   `submit_maintenance_proof(a, R, proof)`.
3. Owner of asset `a` commits to the valuation `v` with blinding `r`, builds
   `prove_adequacy(a, v, r, threshold, seed)`, then calls
   `begin_collateral_proof`, `submit_collateral_bits` (x8), and
   `finalize_collateral_proof`.
4. Lender calls `is_collateral_adequate(a, min_threshold)` and checks the owner
   against the asset registry.

## Errors

See [error-reference.md](error-reference.md#zkprivacy).
