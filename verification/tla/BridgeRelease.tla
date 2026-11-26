------------------------- MODULE BridgeRelease -------------------------
(*
 * Formal model of the bridge's attestation and release state machine
 * (issue #2039).
 *
 * Each cross-chain release is identified by a (source_chain, nonce) pair.
 * The bridge collects attestations from a fixed quorum of attestors and
 * executes a release at most once per id. After an attestation window
 * closes, a pending release can be expired instead of executed.
 *
 * The central safety property is NoDoubleRelease: for every id, the
 * ledger records at most one Executed transition.
 *)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Ids,            \* finite set of (source_chain, nonce) release ids
    Attestors,      \* finite set of attestor identities
    Threshold       \* number of distinct attestations required to execute

ASSUME Threshold \in 1..Cardinality(Attestors)

VARIABLES
    attestations,   \* attestations[id]: set of attestors that have signed id
    executed,       \* set of ids that have been released
    expired,        \* set of ids that have been expired
    windowClosed    \* set of ids whose attestation window is closed

vars == <<attestations, executed, expired, windowClosed>>

Init ==
    /\ attestations = [i \in Ids |-> {}]
    /\ executed = {}
    /\ expired = {}
    /\ windowClosed = {}

\* An attestor signs an id. Signatures accrue only while the window is open
\* and the release has not already been executed or expired.
Attest(id, a) ==
    /\ a \notin attestations[id]
    /\ id \notin windowClosed
    /\ id \notin executed
    /\ id \notin expired
    /\ attestations' = [attestations EXCEPT ![id] = @ \cup {a}]
    /\ UNCHANGED <<executed, expired, windowClosed>>

\* The attestation window closes. No new attestations after this point.
CloseWindow(id) ==
    /\ id \notin windowClosed
    /\ windowClosed' = windowClosed \cup {id}
    /\ UNCHANGED <<attestations, executed, expired>>

\* Execute the release: quorum reached and not already settled either way.
Execute(id) ==
    /\ Cardinality(attestations[id]) >= Threshold
    /\ id \notin executed
    /\ id \notin expired
    /\ executed' = executed \cup {id}
    /\ UNCHANGED <<attestations, expired, windowClosed>>

\* Expire a release after its window closes without reaching quorum.
Expire(id) ==
    /\ id \in windowClosed
    /\ id \notin executed
    /\ id \notin expired
    /\ Cardinality(attestations[id]) < Threshold
    /\ expired' = expired \cup {id}
    /\ UNCHANGED <<attestations, executed, windowClosed>>

Next ==
    \/ \E id \in Ids, a \in Attestors : Attest(id, a)
    \/ \E id \in Ids : CloseWindow(id)
    \/ \E id \in Ids : Execute(id)
    \/ \E id \in Ids : Expire(id)

Spec == Init /\ [][Next]_vars

---------------------------------------------------------------------------
\* Invariants

TypeOK ==
    /\ attestations \in [Ids -> SUBSET Attestors]
    /\ executed \subseteq Ids
    /\ expired \subseteq Ids
    /\ windowClosed \subseteq Ids

\* Core safety: no id is both executed and expired, and nothing executes twice
\* (set membership encodes at-most-once directly).
NoDoubleRelease == executed \cap expired = {}

\* A release only fires with sufficient attestations.
QuorumBeforeExecute ==
    \A id \in executed : Cardinality(attestations[id]) >= Threshold

\* Expiry implies the window closed without quorum.
ExpiryRequiresClosedWindow ==
    \A id \in expired : id \in windowClosed
                        /\ Cardinality(attestations[id]) < Threshold

=============================================================================
