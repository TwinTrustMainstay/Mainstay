# Engineer Performance Bonds

Tracking issue: #1735. Contract: `contracts/engineer-bond`.

## Purpose

Engineers who sign maintenance records put their own capital at risk. A bonded engineer has a
direct financial stake in the quality of their work. Bonds are denominated in any Stellar Asset
Contract token, typically native XLM.

## Economics

| Parameter | Value | Set by |
|-----------|-------|--------|
| Minimum bond | `min_bond` (in token base units) | Admin at `initialize` |
| Treasury | Address receiving slashed funds | Admin at `initialize` |
| Recovery streak | `RECOVERY_STREAK` = 3 good jobs | Contract constant |
| History cap | `MAX_HISTORY` = 50 events per engineer | Contract constant |

- **Bonding**: an engineer calls `post_bond`. Tokens move into the contract. The engineer is
  bonded once the balance is at or above `min_bond` and no dispute is open.
- **Good performance**: `record_performance(good = true)` extends the good-job streak.
- **Poor performance**: `record_performance(good = false)` resets the streak to zero.
- **Dispute**: `open_dispute` freezes withdrawals and makes the engineer not bonded.
  `resolve_dispute` lifts the freeze without penalty.
- **Slashing**: `slash` moves part of the bond to the treasury. It also resets the streak.
- **Recovery**: after any slash, the engineer must reach `RECOVERY_STREAK` consecutive good
  jobs before withdrawing. Good performance is what restores withdrawal rights.
- **Withdrawal**: `withdraw_bond` returns tokens. A remainder above zero must stay at or above
  `min_bond`, so the engineer either withdraws fully or keeps a valid bond.

## History

Every bond movement and performance outcome is appended to the engineer's history
(`get_history`): posts, slashes, withdrawals, good and poor jobs, and dispute open and resolve.
Entries are capped at 50 per engineer, and the oldest are dropped first.

## Operating guidance

- Slash only for documented, verifiable failures: a disputed record that was upheld, or a
  demonstrably poor maintenance outcome. Record the reason in the operations log.
- Use `open_dispute` while investigating. Do not slash during an open dispute unless the
  dispute is being upheld.
- Keep the treasury under multisig control, since it holds all slashed funds.
