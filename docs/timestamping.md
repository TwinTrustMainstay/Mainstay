# Maintenance Record Timestamping (`contracts/timestamping`)

On-chain timestamps only reflect when a transaction was submitted, and whoever
controls the submitting account can choose that moment. The timestamping
contract accepts a creation time only when a **registered timestamp authority
(TSA)** has signed it. Records can then be shown to have existed at a given time,
and that proof stays valid across renewals and authority rotation.

## Trust model

- The admin registers TSA **Ed25519** public keys with a validity window
  (`add_authority`). Keys can be extended (`renew_authority`) and revoked
  (`revoke_authority`). Every change is recorded in `get_authority_history`.
- A TSA is trusted to sign the time it is given. The contract verifies the signature
  and does not check the clock.
- Ed25519 is used because Soroban verifies it natively. RFC 3161 tokens use CMS/ASN.1
  with RSA or ECDSA, which is too expensive to verify on-chain.

## Integrating an external TSA

The contract cannot call out to a TSA, so an off-chain **relayer** does the work:

1. Obtain a timestamp for the record. If the TSA issues RFC 3161 tokens, verify the
   token off-chain and run a signing proxy that produces the Ed25519 attestation
   below. The authority key is the proxy's public key, which the admin registers.
2. Submit the attestation with `submit_timestamp`.
3. Before expiry, obtain a newer attestation and call `renew_timestamp`.

Attestation messages are exact byte strings (all integers big-endian):

| Operation | Signed message |
|-----------|----------------|
| Creation | `"MAINSTAY-TSA-CREATE-V1" ‖ record_id (u64) ‖ record_hash (32 B) ‖ timestamp (u64)` |
| Renewal  | `"MAINSTAY-TSA-RENEW-V1"  ‖ record_id (u64) ‖ record_hash (32 B) ‖ timestamp (u64)` |

The separate domain tags stop a creation attestation from being replayed as a renewal.

## Guarantees

- **Creation time.** `get_timestamped_record` returns `created_at`, which the TSA
  signed. Anyone can check the signature with `verify_timestamp_signature`.
- **Clock bound.** A TSA attestation may not be more than `MAX_CLOCK_SKEW_SECS`
  (300 s) ahead of the ledger clock. Backdating is limited only by the TSA's
  honesty and the revocation policy below.
- **Expiry and renewal.** A timestamp is valid for `record_validity` (default 5 years,
  configurable from 1 to 20 years). After that, `is_timestamp_valid` is false until a
  renewal extends it. A renewal must have a time later than `last_timestamp`.
- **Authority rotation.** An authority's validity window (`valid_until`) limits the
  times it may attest to. Renewals are rejected once the window ends.
- **Revocation.** `revoke_authority` takes an `effective_from` time, which may be in the
  past. Timestamps at or after it become invalid, and the authority can issue nothing
  new. Use a backdated `effective_from` when a key is known to have been compromised
  from some earlier point.

## Limits

- The TSA's honesty and key security are the root of trust. A compromised key can
  sign any time up to its revocation point, so revoke promptly and choose
  `effective_from` carefully.
- `verify_timestamp_signature` traps on an invalid signature instead of returning
  `false`. This cannot happen for accepted records, since they were checked when stored.
- Authority history is capped at `MAX_HISTORY_ENTRIES` (200). When it is full,
  `add_authority`, `renew_authority` and `revoke_authority` fail with `HistoryFull`.
  Archive the history off-chain before it fills.

## Errors

See [error-reference.md](error-reference.md#timestamping).
