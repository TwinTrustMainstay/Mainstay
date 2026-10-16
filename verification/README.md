# Formal verification of the maintenance-record ledger

Issues: #1755 (TLA+ model), #1756 (executable verification and CI guard).

## What is proven

Model checking with TLC over the finite instance in `tla/MaintenanceRecords.cfg`
(2 asset ids, total value 2, history bound 3) checks these invariants on every
reachable state:

| Invariant | Meaning |
|---|---|
| `TypeOK` | Types and bounds of all state variables hold. |
| `Conservation` | `pool + Σ balance = TotalValue`: collateral value is never created or destroyed. |
| `UnregisteredHoldsNoValue` | Value only sits on registered assets. |
| `NoOrphanedRecords` | Every maintenance record refers to an asset id that has been registered. |
| `HistoryAppendOnly` | Record entries are never rewritten; history only grows (checked structurally). |

Result at the time of writing: **no error found, 228 distinct states, depth 8**.

Mutation checks confirm the invariants are not vacuous: a mutant where
decommission does not return value to the pool violates `Conservation`, and a
mutant that accepts records for unregistered assets violates `NoOrphanedRecords`.

## How to run

```sh
# Requires Java 11+ and tla2tools.jar (https://github.com/tlaplus/tlaplus/releases)
cd verification/tla
java -cp tla2tools.jar tlc2.TLC -config MaintenanceRecords.cfg MaintenanceRecords.tla
```

The executable explorer in `verification/src` mirrors the same transition
system and is run with `cargo test -p verification`. Its test asserts the same
distinct-state count as TLC, so any drift between the spec and the Rust model
fails CI.

## Proof assumptions

1. **Finite instance.** TLC proves the invariants for the bounded instance only.
   The bounded instance is small but covers every action, including re-use
   attempts on ids and transfers between two assets. It is a model check, not a
   deductive proof for unbounded ids or history length.
2. **Asset ids are never reused.** An id stays in `ever` after decommissioning, so
   records cannot be re-attached to a new asset. The contract enforces this by
   keeping history under the asset id.
3. **Value is an abstract non-negative integer.** Stroop-level arithmetic,
   overflow, and rounding are out of scope; the contract uses checked arithmetic.
4. **Authorization is abstracted away.** The model assumes each action has
   already passed the admin / owner / engineer checks enforced by the contract.
   Those checks are covered by the contract's own tests, not by this model.
5. **Timing and ledger TTL are abstracted away.** Expiry of persistent storage
   is not modelled, so history is assumed to persist for the model's lifetime.

## Maintaining the proof

When a contract change alters a state transition, update `tla/MaintenanceRecords.tla`
and `verification/src/model.rs` together, rerun TLC, and update the expected
state count in `verification/tests/model_check.rs` only after the spec and the
Rust model agree.
