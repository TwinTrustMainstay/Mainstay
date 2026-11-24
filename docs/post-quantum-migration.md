# Post-Quantum Migration Path (Issue #1766)

## Quantum-Vulnerable Operations

Every signature check in this contract goes through Soroban's `Address::require_auth`,
which is backed by Ed25519 (via Stellar account/contract auth) or, for custom
accounts, whatever signature scheme the signing account's `__check_auth`
implements. Ed25519 is broken by a sufficiently large quantum computer running
Shor's algorithm. Concretely, in `asset-registry`:

- Admin actions (`set_lifecycle_contract`, pause/unpause, ownership transfer
  approval, upgrade proposals) are all gated by `require_admin`, which compares
  against an `Address` authenticated via Ed25519.
- Asset owner actions (metadata updates, transfers, co-owner proposal voting)
  are gated the same way, via the owner/co-owner `Address::require_auth()`.

There is no symmetric encryption or hashing in the authorization path that
needs migrating — only the signature scheme behind `Address::require_auth`.

## Design: Migration Path to Post-Quantum Algorithms

Target algorithms (both NIST-standardized):

- **CRYSTALS-Dilithium** (NIST FIPS 204) — lattice-based, the general-purpose
  recommendation.
- **SPHINCS+** (NIST FIPS 205) — hash-based, more conservative, larger
  signatures. Offered as a fallback for signers that want an algorithm family
  not based on lattice hardness assumptions.

### Current status

`CryptoAlgorithm` (`Legacy | Crystals | Sphincs`) and `QuantumMigrationConfig`
(`active_algorithm`, `dual_algorithm_enabled`, `legacy_sunset_at`) exist as of
this change, exposed via `set_quantum_config` / `get_quantum_config` /
`is_legacy_signature_accepted`. **This is the governance/configuration layer
only.** Soroban's host environment does not currently expose native
CRYSTALS-Dilithium or SPHINCS+ verification as a built-in host function the
way it does for Ed25519. Actually verifying a PQ signature on-chain requires
either:

1. A future Soroban host function for PQ verification (tracked upstream,
   not yet available), or
2. An in-contract WASM implementation of the verification algorithm using an
   audited Rust crate (e.g. `pqcrypto-dilithium`, `pqcrypto-sphincsplus`),
   compiled to `wasm32-unknown-unknown` and checked against the project's
   WASM size/budget constraints.

Implementing (2) without a security audit of the resulting contract would be
irresponsible for a contract moving real collateral — that work is
deliberately **not** included here and should be its own audited change once
an algorithm and crate are selected.

## Dual-Algorithm Support During Transition

`QuantumMigrationConfig.dual_algorithm_enabled` lets `active_algorithm` be set
to `Crystals` or `Sphincs` while Legacy (Ed25519) signatures continue to be
accepted — `is_legacy_signature_accepted` returns `true` in that mode. This
lets the migration proceed per-signer instead of as a single flag day.

## Phasing Out Legacy Algorithms

`QuantumMigrationConfig.legacy_sunset_at` is a ledger timestamp after which
`is_legacy_signature_accepted` returns `false`. `set_quantum_config` rejects
setting a sunset deadline while Legacy is still the active algorithm with
dual-algorithm support disabled, since that combination would lock out every
existing signer with no migration path.

## Migration Timeline

| Phase | What | Status |
|---|---|---|
| 0 | Governance layer: `CryptoAlgorithm`, `QuantumMigrationConfig`, admin-gated setters/getters | Done (this change) |
| 1 | Select and audit a PQ signature crate; prototype on-chain verification against the WASM size budget | Not started |
| 2 | Wire real PQ verification into `require_auth`-equivalent checks for admin and owner actions, gated by `active_algorithm` | Not started |
| 3 | Enable `dual_algorithm_enabled`; signers migrate at their own pace | Not started |
| 4 | Admin sets `legacy_sunset_at`; signers who haven't migrated are notified out-of-band | Not started |
| 5 | After `legacy_sunset_at` passes, remove Legacy verification code entirely | Not started |

Phases 1–2 are the actual cryptographic engineering work and are explicitly
out of scope for this issue; they require a dedicated audited change.
