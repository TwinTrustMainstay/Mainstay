# Governance

This document describes the on-chain governance contract (`contracts/governance`). It covers
proposal kinds, the proposal lifecycle, vote weighting, quorum and approval math, and the
parameter store. The source of truth is `contracts/governance/src/lib.rs`.

## Concepts

| Term | Meaning |
| --- | --- |
| Voting shares | Units held by holders. Minted by the admin with `mint_shares`. Transferable with `transfer_shares`. |
| Voting weight | A voter's share balance at the moment they vote. |
| Total supply snapshot | Total shares outstanding when the proposal was created. Quorum is measured against this value. |
| Treasury | A budget ledger (not a token balance). Funded by the admin with `fund_treasury`. Funding grants debit it. |
| Parameter store | Key/value map of platform parameters. Written by executed `Parameter` proposals. |

## Configuration

Set once in `initialize` through `GovernanceConfig`:

| Field | Type | Rule | Meaning |
| --- | --- | --- | --- |
| `voting_period_secs` | `u64` | any | Seconds a proposal stays open for voting. |
| `quorum_bps` | `u32` | `1..=10_000` | Minimum turnout as a share of the total supply snapshot. |
| `approval_threshold_bps` | `u32` | `1..=10_000` | Minimum share of votes cast that must be in favour. |
| `min_proposal_shares` | `u64` | any | Minimum shares a holder must own to create a proposal. |

`initialize` rejects any quorum or threshold outside `1..=10_000` with `InvalidConfig`, and
rejects a second call with `AlreadyInitialized`.

## Proposal kinds

```
ProposalKind::Parameter(name: Symbol, value: u64)
ProposalKind::Funding(recipient: Address, amount: u64)
```

- **Parameter**: on execution, writes `value` to the parameter store under `name`.
- **Funding**: on execution, debits `amount` from the treasury. `amount` must be greater
  than zero at creation (`InvalidProposal` otherwise). If the treasury balance is lower than
  `amount` at execution, execution fails with `InsufficientBudget`. The `recipient` is recorded
  on the proposal. The contract does not move tokens.

## Proposal lifecycle

```
create_proposal ──► Active ──(voting period ends)──► finalize_proposal ──► Passed | Rejected | NoQuorum
                                                                                │
                                                                   (Passed only) execute_proposal ──► Executed
```

1. **Create.** `create_proposal(proposer, title, kind)` requires the proposer to hold at least
   `min_proposal_shares`, and a non-zero balance. The title must be non-empty and at most
   `MAX_TITLE_LEN` (128) bytes. The proposal records `voting_ends_at = now + voting_period_secs`
   and `total_shares_snapshot` at creation.
2. **Vote.** `vote(voter, proposal_id, support)` is open while the status is `Active` and
   `now < voting_ends_at`. Each holder votes once per proposal. Weight equals the holder's
   current balance. Voting locks the holder's balance until `voting_ends_at`, so the same
   shares cannot be moved and voted again. Each proposal accepts at most
   `MAX_VOTES_PER_PROPOSAL` (500) votes.
3. **Finalize.** `finalize_proposal` is callable by anyone once `now >= voting_ends_at`. It
   records the outcome. Calling it twice returns `AlreadyFinalized`.
4. **Execute.** `execute_proposal` is callable by anyone, once, and only for `Passed`
   proposals. It applies the proposal and sets the status to `Executed`.

Status values:

| Status | Set by | Meaning |
| --- | --- | --- |
| `Active` | create | Voting open, or ended but not yet finalized. |
| `Passed` | finalize | Quorum met and approval threshold met. |
| `Rejected` | finalize | Quorum met but approval threshold not met. |
| `NoQuorum` | finalize | Turnout below quorum, or zero votes cast. |
| `Executed` | execute | Passed proposal applied. |

## Vote weighting and outcome math

Let `votes_for` and `votes_against` be the weighted sums of votes, `votes_cast = votes_for +
votes_against`, and `supply` the total supply snapshot. The outcome is computed by `outcome()`
using 128-bit arithmetic (no overflow):

1. **Quorum.** Quorum is met when all of these hold:
   - `votes_cast > 0`
   - `votes_cast * 10_000 >= supply * quorum_bps`

   If quorum is not met, the result is `NoQuorum`.
2. **Approval.** Otherwise, the proposal passes when
   `votes_for * 10_000 >= votes_cast * approval_threshold_bps`. Otherwise it is `Rejected`.

Worked example: `supply = 1_000`, `quorum_bps = 4_000` (40%), `approval_threshold_bps = 6_000`
(60%).

| votes_for | votes_against | votes_cast | Quorum (≥ 400) | For share | Result |
| --- | --- | --- | --- | --- | --- |
| 300 | 100 | 400 | met | 75% | `Passed` |
| 240 | 200 | 440 | met | 54.5% | `Rejected` |
| 200 | 100 | 300 | not met | n/a | `NoQuorum` |

Quorum uses the snapshot taken at creation, so shares minted after creation do not raise the
quorum for that proposal. The approval threshold uses votes cast, not supply, so abstaining
holders do not count against approval but do count toward quorum only if they vote.

## Parameter schema

- A parameter is identified by a `Symbol` and holds a `u64`.
- Parameters are read with `get_parameter(name)`, which returns `None` until a passed
  proposal sets the value.
- **Keys are not validated by the contract.** Any `Symbol` is accepted when a parameter
  proposal is created. The set of governable keys is therefore defined by off-chain
  convention, not by the contract. Consumers should treat unknown keys as ignorable.
- Parameter proposals have no range checks on `value`. Consumers must validate values
  before use.

## Funding proposals and the treasury

- `fund_treasury(admin, amount)` is admin only and rejects `amount = 0`.
- `get_treasury_balance()` returns the ledger balance.
- A funding grant is validated twice: at creation (`amount > 0`) and at execution (balance
  check). A grant that is valid when created can still fail execution if the treasury has
  been drawn down by other grants in the meantime.

## Error reference

| Error | Code | Raised when |
| --- | --- | --- |
| `NotInitialized` | 1 | Contract used before `initialize`. |
| `AlreadyInitialized` | 2 | `initialize` called twice. |
| `UnauthorizedAdmin` | 3 | Admin-only call by a non-admin. |
| `InvalidConfig` | 4 | Quorum or threshold outside `1..=10_000`. |
| `InvalidAmount` | 5 | Zero amount to `fund_treasury`, or zero-share transfer. |
| `InsufficientShares` | 6 | Proposer or voter lacks the required shares. |
| `SharesLocked` | 7 | Transfer of shares locked by an active vote. |
| `ProposalNotFound` | 8 | Unknown proposal id. |
| `InvalidProposal` | 9 | Empty or over-long title, or zero-amount funding grant. |
| `VotingClosed` | 10 | Voting attempted outside the open window. |
| `AlreadyVoted` | 11 | Holder voted twice on the same proposal. |
| `VotingStillOpen` | 12 | Finalize attempted before the period ends. |
| `AlreadyFinalized` | 13 | Finalize called on a non-active proposal. |
| `ProposalNotPassed` | 14 | Execute called on a proposal that did not pass. |
| `AlreadyExecuted` | 15 | Execute called twice. |
| `InsufficientBudget` | 16 | Funding grant exceeds the treasury balance at execution. |
| `TooManyVotes` | 17 | Proposal reached `MAX_VOTES_PER_PROPOSAL`. |

## Related documents

- [Access control](access-control.md)
- [Architecture](architecture.md)
- [Insurance pool](insurance-pool.md)
