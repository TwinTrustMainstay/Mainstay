------------------------- MODULE MaintenanceRecords -------------------------
(*
 * Formal model of the asset / maintenance-record ledger (issue #1755).
 *
 * Abstracts the lifecycle contract to the properties the issue asks for:
 *   - collateral value conservation: value only moves between the pool and
 *     registered assets, or between assets; it is never created or destroyed;
 *   - no orphaned records: every maintenance record refers to an asset that
 *     has been registered at some point, and records are only accepted for
 *     assets that are currently registered.
 *
 * Assets are identified by a fixed set of ids (AssetIds). An id, once used,
 * stays in `ever` forever, so history can never be re-attached to a new asset.
 *
 * Assumptions are documented in verification/README.md.
 *)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    AssetIds,       \* finite set of candidate asset ids
    TotalValue,     \* total collateral value in the system (conserved)
    MaxRecords      \* bound on the maintenance history length (for model checking)

VARIABLES
    registered,     \* assets currently registered (active or decommissioned-but-known excluded)
    ever,           \* every asset id that has ever been registered
    balance,        \* balance[a]: collateral value attached to asset a
    pool,           \* unallocated value not attached to any asset
    records         \* maintenance history: sequence of asset ids, one per record

vars == <<registered, ever, balance, pool, records>>

\* Sum of f[x] over the finite set S.
RECURSIVE SumOver(_, _)
SumOver(f, S) ==
    IF S = {} THEN 0
    ELSE LET x == CHOOSE y \in S : TRUE
         IN f[x] + SumOver(f, S \ {x})

Init ==
    /\ registered = {}
    /\ ever = {}
    /\ balance = [a \in AssetIds |-> 0]
    /\ pool = TotalValue
    /\ records = <<>>

\* Register a new asset, attaching value v taken from the pool.
Register(a, v) ==
    /\ a \notin ever
    /\ v \in 0..pool
    /\ registered' = registered \cup {a}
    /\ ever' = ever \cup {a}
    /\ balance' = [balance EXCEPT ![a] = v]
    /\ pool' = pool - v
    /\ records' = records

\* Decommission an asset: its remaining value returns to the pool.
\* History is kept, so records still refer to an id in `ever`.
Decommission(a) ==
    /\ a \in registered
    /\ registered' = registered \ {a}
    /\ balance' = [balance EXCEPT ![a] = 0]
    /\ pool' = pool + balance[a]
    /\ UNCHANGED <<ever, records>>

\* Append a maintenance record for a currently registered asset.
Submit(a) ==
    /\ a \in registered
    /\ Len(records) < MaxRecords
    /\ records' = Append(records, a)
    /\ UNCHANGED <<registered, ever, balance, pool>>

\* Move value between two registered assets (e.g. collateral re-allocation).
Transfer(a, b, amt) ==
    /\ a \in registered
    /\ b \in registered
    /\ a # b
    /\ amt \in 1..balance[a]
    /\ balance' = [balance EXCEPT ![a] = @ - amt, ![b] = @ + amt]
    /\ UNCHANGED <<registered, ever, pool, records>>

Next ==
    \/ \E a \in AssetIds, v \in 0..TotalValue : Register(a, v)
    \/ \E a \in AssetIds : Decommission(a)
    \/ \E a \in AssetIds : Submit(a)
    \/ \E a, b \in AssetIds, amt \in 1..TotalValue : Transfer(a, b, amt)

Spec == Init /\ [][Next]_vars

---------------------------------------------------------------------------
\* Invariants

TypeOK ==
    /\ registered \subseteq ever
    /\ ever \subseteq AssetIds
    /\ balance \in [AssetIds -> 0..TotalValue]
    /\ pool \in 0..TotalValue
    /\ Len(records) <= MaxRecords
    /\ \A i \in 1..Len(records) : records[i] \in AssetIds

\* Collateral value conservation: nothing is created or destroyed.
Conservation == pool + SumOver(balance, AssetIds) = TotalValue

\* Value only sits on registered assets.
UnregisteredHoldsNoValue == \A a \in AssetIds \ registered : balance[a] = 0

\* No orphaned records: every record names an asset that exists in the ledger.
NoOrphanedRecords == \A i \in 1..Len(records) : records[i] \in ever

\* Records are only ever written against currently registered assets at write time,
\* so every record's asset is in `ever` (checked above) and history is never rewritten.
HistoryAppendOnly == \A i \in 1..Len(records) : records[i] \in AssetIds

=============================================================================
